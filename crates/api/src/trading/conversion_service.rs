use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::sideshift_client::{AmountType, SideShiftClient};
use crate::payment_receipt_service::PaymentReceiptService;
use crate::coinmarketcap_service::CoinMarketCapService;
use blockchain::Blockchain;

/// Conversion service that orchestrates cryptocurrency swaps
pub struct ConversionService {
    db_pool: Pool,
    sideshift_client: Arc<SideShiftClient>,
    coinmarketcap_service: Arc<CoinMarketCapService>,
    receipt_service: Option<Arc<PaymentReceiptService>>,
}

impl ConversionService {
    pub fn new(
        db_pool: Pool,
        sideshift_client: Arc<SideShiftClient>,
        coinmarketcap_service: Arc<CoinMarketCapService>,
    ) -> Self {
        Self {
            db_pool,
            sideshift_client,
            coinmarketcap_service,
            receipt_service: None,
        }
    }

    /// Create a new conversion service with receipt generation
    pub fn new_with_receipts(
        db_pool: Pool,
        sideshift_client: Arc<SideShiftClient>,
        coinmarketcap_service: Arc<CoinMarketCapService>,
        receipt_service: Arc<PaymentReceiptService>,
    ) -> Self {
        Self {
            db_pool,
            sideshift_client,
            coinmarketcap_service,
            receipt_service: Some(receipt_service),
        }
    }

    /// Get a conversion quote with fee breakdown
    pub async fn get_quote(
        &self,
        from_asset: &str,
        to_asset: &str,
        amount: Decimal,
        amount_type: AmountType,
    ) -> Result<ConversionQuoteWithFees> {
        // Try SideShift first
        match self
            .sideshift_client
            .get_quote(from_asset, to_asset, amount, amount_type.clone())
            .await
        {
            Ok(quote) => {
                // Calculate fee breakdown
                let total_fees = quote.network_fee + quote.platform_fee + quote.sideshift_fee;
                
                Ok(ConversionQuoteWithFees {
                    quote_id: quote.quote_id,
                    from_asset: quote.from_asset.clone(),
                    to_asset: quote.to_asset.clone(),
                    from_amount: quote.from_amount,
                    to_amount: quote.to_amount,
                    exchange_rate: quote.exchange_rate,
                    network_fee: quote.network_fee,
                    platform_fee: quote.platform_fee,
                    provider_fee: quote.sideshift_fee,
                    total_fees,
                    provider: ConversionProvider::SideShift,
                    expires_at: quote.expires_at,
                })
            }
            Err(e) => {
                // Check if we should fallback to Jupiter for Solana tokens
                // Try Jupiter if either token is a Solana token
                if self.is_solana_token(from_asset) || self.is_solana_token(to_asset) {
                    tracing::warn!(
                        "SideShift unavailable for {}/{}, attempting Jupiter fallback: {}",
                        from_asset,
                        to_asset,
                        e
                    );
                    self.get_jupiter_quote(from_asset, to_asset, amount, amount_type)
                        .await
                } else {
                    Err(e).context("SideShift API unavailable and no fallback available")
                }
            }
        }
    }

    /// Execute a conversion based on a quote
    pub async fn execute_conversion(
        &self,
        user_id: Uuid,
        quote: ConversionQuoteWithFees,
        settle_address: &str,
        refund_address: Option<&str>,
        blockchain: Blockchain,
    ) -> Result<ConversionResult> {
        // Validate quote hasn't expired
        if Utc::now() > quote.expires_at {
            anyhow::bail!("Quote has expired");
        }

        // Execute based on provider
        let result = match quote.provider {
            ConversionProvider::SideShift => {
                self.execute_sideshift_conversion(&quote, settle_address, refund_address)
                    .await?
            }
            ConversionProvider::Jupiter => {
                self.execute_jupiter_conversion(&quote, settle_address)
                    .await?
            }
        };

        // Record conversion in database
        let conversion_id = self.record_conversion(user_id, &quote, &result).await?;

        // Generate blockchain receipt if receipt service is available
        if let Some(receipt_service) = &self.receipt_service {
            match receipt_service
                .generate_conversion_receipt(
                    conversion_id,
                    quote.from_amount,
                    quote.from_asset.clone(),
                    quote.to_amount,
                    quote.to_asset.clone(),
                    quote.exchange_rate,
                    settle_address.to_string(),
                    result.deposit_address.clone(),
                    blockchain,
                    quote.network_fee.into(),
                    quote.platform_fee.into(),
                    Some(quote.provider_fee),
                )
                .await
            {
                Ok(_) => {
                    tracing::info!(
                        "Blockchain receipt created for conversion {}",
                        conversion_id
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to create blockchain receipt for conversion {}: {}",
                        conversion_id,
                        e
                    );
                    // Don't fail the conversion if receipt creation fails
                }
            }
        }

        Ok(result)
    }

    /// Get conversion history for a user
    pub async fn get_conversion_history(
        &self,
        user_id: Uuid,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<ConversionRecord>> {
        let client = self.db_pool.get().await?;

        let rows = client
            .query(
                "SELECT id, user_id, from_asset, to_asset, from_amount, to_amount, 
                        exchange_rate, network_fee, platform_fee, provider_fee, 
                        provider, transaction_hash, status, created_at, completed_at
                 FROM conversions
                 WHERE user_id = $1
                 ORDER BY created_at DESC
                 LIMIT $2 OFFSET $3",
                &[&user_id, &limit, &offset],
            )
            .await?;

        let records = rows
            .iter()
            .map(|row| ConversionRecord {
                id: row.get("id"),
                user_id: row.get("user_id"),
                from_asset: row.get("from_asset"),
                to_asset: row.get("to_asset"),
                from_amount: row.get("from_amount"),
                to_amount: row.get("to_amount"),
                exchange_rate: row.get("exchange_rate"),
                network_fee: row.get("network_fee"),
                platform_fee: row.get("platform_fee"),
                provider_fee: row.get("provider_fee"),
                provider: row.get("provider"),
                transaction_hash: row.get("transaction_hash"),
                status: row.get("status"),
                created_at: row.get("created_at"),
                completed_at: row.get("completed_at"),
            })
            .collect();

        Ok(records)
    }

    /// Execute conversion via SideShift
    async fn execute_sideshift_conversion(
        &self,
        quote: &ConversionQuoteWithFees,
        settle_address: &str,
        refund_address: Option<&str>,
    ) -> Result<ConversionResult> {
        let order = self
            .sideshift_client
            .create_order(&quote.quote_id, settle_address, refund_address)
            .await?;

        Ok(ConversionResult {
            order_id: order.order_id,
            deposit_address: order.deposit_address,
            deposit_amount: order.deposit_amount,
            settle_amount: order.settle_amount,
            status: ConversionStatus::Pending,
            transaction_hash: None,
            provider: ConversionProvider::SideShift,
        })
    }

    /// Execute conversion via Jupiter (fallback for Solana tokens)
    async fn execute_jupiter_conversion(
        &self,
        quote: &ConversionQuoteWithFees,
        settle_address: &str,
    ) -> Result<ConversionResult> {
        // Note: Full Jupiter swap execution requires Solana wallet integration
        // For MVP, we return a pending status with instructions for manual execution
        
        tracing::info!(
            "Jupiter swap requested: {} {} -> {} {}",
            quote.from_amount,
            quote.from_asset,
            quote.to_amount,
            quote.to_asset
        );
        
        // Generate an order ID for tracking
        let order_id = format!("jupiter_{}", Uuid::new_v4());
        
        // In production, this would:
        // 1. Call Jupiter Swap API to get transaction
        // 2. Sign transaction with user's wallet
        // 3. Submit to Solana network
        // 4. Monitor transaction status
        
        // For now, return pending status
        // User would need to complete swap manually at jup.ag
        Ok(ConversionResult {
            order_id,
            deposit_address: settle_address.to_string(), // User's wallet for Solana swaps
            deposit_amount: quote.from_amount,
            settle_amount: quote.to_amount,
            status: ConversionStatus::Pending,
            transaction_hash: None,
            provider: ConversionProvider::Jupiter,
        })
    }

    /// Get quote from Jupiter (fallback for Solana tokens)
    async fn get_jupiter_quote(
        &self,
        from_asset: &str,
        to_asset: &str,
        amount: Decimal,
        amount_type: AmountType,
    ) -> Result<ConversionQuoteWithFees> {
        // Jupiter API integration for Solana token swaps
        // Note: Jupiter's public API has rate limits and may require authentication
        // For MVP, we'll use estimated rates based on common market prices
        
        tracing::info!(
            "Getting Jupiter quote for {} {} -> {}",
            amount,
            from_asset,
            to_asset
        );
        
        // Get estimated exchange rate (in production, this would call Jupiter API)
        let exchange_rate = self.get_estimated_rate(from_asset, to_asset).await?;
        
        // Calculate amounts based on type
        let (from_amount, to_amount) = match amount_type {
            AmountType::From => {
                let to_amt = amount * exchange_rate;
                (amount, to_amt)
            }
            AmountType::To => {
                let from_amt = amount / exchange_rate;
                (from_amt, amount)
            }
        };
        
        // Jupiter fees are typically very low on Solana
        // Price impact depends on liquidity but is usually < 0.5% for common pairs
        let price_impact_fee = from_amount * Decimal::from_str_exact("0.003").unwrap_or(Decimal::ZERO); // 0.3%
        
        // Solana network fee is very low (~0.000005 SOL)
        let network_fee = if from_asset.to_uppercase() == "SOL" {
            Decimal::from_str_exact("0.000005").unwrap_or(Decimal::ZERO)
        } else {
            Decimal::ZERO
        };
        
        let platform_fee = Decimal::ZERO; // No additional platform fee for Jupiter
        let total_fees = network_fee + price_impact_fee;
        
        // Generate a quote ID
        let quote_id = format!("jupiter_{}", Uuid::new_v4());
        
        // Jupiter quotes expire in 30 seconds
        let expires_at = Utc::now() + chrono::Duration::seconds(30);
        
        Ok(ConversionQuoteWithFees {
            quote_id,
            from_asset: from_asset.to_string(),
            to_asset: to_asset.to_string(),
            from_amount,
            to_amount,
            exchange_rate,
            network_fee,
            platform_fee,
            provider_fee: price_impact_fee,
            total_fees,
            provider: ConversionProvider::Jupiter,
            expires_at,
        })
    }
    
    /// Get estimated exchange rate using CoinMarketCap real-time prices
    async fn get_estimated_rate(&self, from_asset: &str, to_asset: &str) -> Result<Decimal> {
        // Use CoinMarketCap to get real-time conversion rate
        match self.coinmarketcap_service.convert(from_asset, to_asset, Decimal::ONE).await {
            Ok(conversion) => Ok(conversion.rate),
            Err(e) => {
                tracing::warn!(
                    "Failed to get CMC rate for {}/{}: {}. Falling back to direct price calculation",
                    from_asset,
                    to_asset,
                    e
                );
                
                // Fallback: Calculate rate from individual prices
                let from_price = self.coinmarketcap_service.get_price_by_symbol(from_asset).await?;
                let to_price = self.coinmarketcap_service.get_price_by_symbol(to_asset).await?;
                
                if to_price.price_usd == Decimal::ZERO {
                    anyhow::bail!("Cannot calculate rate: {} price is zero", to_asset);
                }
                
                let rate = from_price.price_usd / to_price.price_usd;
                Ok(rate)
            }
        }
    }

    /// Check if an asset is a Solana token
    fn is_solana_token(&self, asset: &str) -> bool {
        // Common Solana tokens (case-insensitive)
        let asset_upper = asset.to_uppercase();
        matches!(
            asset_upper.as_str(),
            "SOL" | "USDC" | "USDT" | "RAY" | "SRM" | "BONK" | "JUP" | "ORCA" | "WSOL"
        ) || asset.starts_with("So1") // Solana token addresses start with "So1"
    }

    /// Record conversion in database and return the conversion ID
    async fn record_conversion(
        &self,
        user_id: Uuid,
        quote: &ConversionQuoteWithFees,
        result: &ConversionResult,
    ) -> Result<Uuid> {
        let client = self.db_pool.get().await?;

        let status_str = match result.status {
            ConversionStatus::Pending => "pending",
            ConversionStatus::Processing => "processing",
            ConversionStatus::Completed => "completed",
            ConversionStatus::Failed => "failed",
        };

        let provider_str = match quote.provider {
            ConversionProvider::SideShift => "SIDESHIFT",
            ConversionProvider::Jupiter => "JUPITER",
        };

        let conversion_id = Uuid::new_v4();
        
        let completed_at: Option<DateTime<Utc>> = if result.status == ConversionStatus::Completed {
            Some(Utc::now())
        } else {
            None
        };

        client
            .execute(
                "INSERT INTO conversions 
                 (id, user_id, from_asset, to_asset, from_amount, to_amount, 
                  exchange_rate, network_fee, platform_fee, provider_fee, 
                  provider, transaction_hash, status, completed_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13::VARCHAR, $14)",
                &[
                    &conversion_id,
                    &user_id,
                    &quote.from_asset,
                    &quote.to_asset,
                    &quote.from_amount,
                    &quote.to_amount,
                    &quote.exchange_rate,
                    &quote.network_fee,
                    &quote.platform_fee,
                    &quote.provider_fee,
                    &provider_str,
                    &result.transaction_hash,
                    &status_str,
                    &completed_at,
                ],
            )
            .await?;

        Ok(conversion_id)
    }
}

// Public types

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionQuoteWithFees {
    pub quote_id: String,
    pub from_asset: String,
    pub to_asset: String,
    pub from_amount: Decimal,
    pub to_amount: Decimal,
    pub exchange_rate: Decimal,
    pub network_fee: Decimal,
    pub platform_fee: Decimal,
    pub provider_fee: Decimal,
    pub total_fees: Decimal,
    pub provider: ConversionProvider,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionResult {
    pub order_id: String,
    pub deposit_address: String,
    pub deposit_amount: Decimal,
    pub settle_amount: Decimal,
    pub status: ConversionStatus,
    pub transaction_hash: Option<String>,
    pub provider: ConversionProvider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub from_asset: String,
    pub to_asset: String,
    pub from_amount: Decimal,
    pub to_amount: Decimal,
    pub exchange_rate: Decimal,
    pub network_fee: Option<Decimal>,
    pub platform_fee: Option<Decimal>,
    pub provider_fee: Option<Decimal>,
    pub provider: String,
    pub transaction_hash: Option<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversionProvider {
    SideShift,
    Jupiter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConversionStatus {
    Pending,
    Processing,
    Completed,
    Failed,
}
