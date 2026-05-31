use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use crate::error::{ApiError, ApiResult};

const SIDESHIFT_API_BASE: &str = "https://sideshift.ai/api/v2";

/// SideShift API client for cryptocurrency conversions and staking
pub struct SideShiftClient {
    client: Client,
    affiliate_id: Option<String>,
    circuit_breaker: Arc<blockchain::circuit_breaker::CircuitBreaker>,
}

impl SideShiftClient {
    pub fn new(affiliate_id: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        let circuit_breaker_config = blockchain::circuit_breaker::CircuitBreakerConfig {
            failure_threshold: 5,
            success_threshold: 2,
            timeout: Duration::from_secs(60),
        };

        let circuit_breaker = Arc::new(blockchain::circuit_breaker::CircuitBreaker::new(
            "sideshift_api".to_string(),
            circuit_breaker_config,
        ));

        Self {
            client,
            affiliate_id,
            circuit_breaker,
        }
    }

    /// Get a conversion quote from SideShift
    pub async fn get_quote(
        &self,
        from_asset: &str,
        to_asset: &str,
        amount: Decimal,
        amount_type: AmountType,
    ) -> ApiResult<ConversionQuote> {
        // Check circuit breaker
        if !self.circuit_breaker.is_request_allowed().await {
            warn!("SideShift API circuit breaker is open");
            return Err(ApiError::CircuitBreakerOpen(
                "SideShift API is temporarily unavailable. Please try again later.".to_string()
            ));
        }

        let url = format!("{}/quotes", SIDESHIFT_API_BASE);

        let request_body = QuoteRequest {
            deposit_coin: from_asset.to_lowercase(),
            settle_coin: to_asset.to_lowercase(),
            deposit_amount: if amount_type == AmountType::From {
                Some(amount.to_string())
            } else {
                None
            },
            settle_amount: if amount_type == AmountType::To {
                Some(amount.to_string())
            } else {
                None
            },
            affiliate_id: self.affiliate_id.clone(),
        };

        let response = self
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                // Note: We need to use a blocking approach here since record_failure is async
                ApiError::SideShiftApiError(format!("Failed to send quote request: {}", e))
            })?;

        if !response.status().is_success() {
            self.circuit_breaker.record_failure().await;
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(ApiError::SideShiftApiError(format!(
                "SideShift API returned error status {}: {}",
                status,
                error_text
            )));
        }

        let quote_response: QuoteResponse = response
            .json()
            .await
            .map_err(|e| {
                ApiError::SideShiftApiError(format!("Failed to parse quote response: {}", e))
            })?;

        self.circuit_breaker.record_success().await;

        Ok(self.convert_quote_response(quote_response))
    }

    /// Create a fixed conversion order
    pub async fn create_order(
        &self,
        quote_id: &str,
        settle_address: &str,
        refund_address: Option<&str>,
    ) -> Result<ConversionOrder> {
        let url = format!("{}/shifts/fixed", SIDESHIFT_API_BASE);

        let request_body = OrderRequest {
            quote_id: quote_id.to_string(),
            settle_address: settle_address.to_string(),
            refund_address: refund_address.map(|s| s.to_string()),
            affiliate_id: self.affiliate_id.clone(),
        };

        let response = self
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .context("Failed to send order request to SideShift API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "SideShift API returned error status {}: {}",
                status,
                error_text
            );
        }

        let order_response: OrderResponse = response
            .json()
            .await
            .context("Failed to parse SideShift order response")?;

        Ok(self.convert_order_response(order_response))
    }

    /// Get order status by order ID
    pub async fn get_order_status(&self, order_id: &str) -> Result<OrderStatus> {
        let url = format!("{}/shifts/{}", SIDESHIFT_API_BASE, order_id);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send order status request to SideShift API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "SideShift API returned error status {}: {}",
                status,
                error_text
            );
        }

        let status_response: OrderStatusResponse = response
            .json()
            .await
            .context("Failed to parse SideShift order status response")?;

        Ok(OrderStatus {
            order_id: status_response.id,
            status: status_response.status,
            deposit_address: status_response.deposit_address,
            deposit_amount: Decimal::from_str_exact(&status_response.deposit_amount)
                .unwrap_or(Decimal::ZERO),
            settle_amount: Decimal::from_str_exact(&status_response.settle_amount)
                .unwrap_or(Decimal::ZERO),
            expires_at: status_response.expires_at,
            created_at: status_response.created_at,
        })
    }

    /// Get list of supported coins
    pub async fn get_supported_coins(&self) -> Result<Vec<SupportedCoin>> {
        let url = format!("{}/coins", SIDESHIFT_API_BASE);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send coins request to SideShift API")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "SideShift API returned error status: {}",
                response.status()
            );
        }

        let coins: Vec<CoinResponse> = response
            .json()
            .await
            .context("Failed to parse SideShift coins response")?;

        Ok(coins
            .into_iter()
            .map(|coin| SupportedCoin {
                coin: coin.coin,
                name: coin.name,
                networks: coin.networks,
                has_staking: coin.has_staking.unwrap_or(false),
            })
            .collect())
    }

    /// Get staking information for a coin
    pub async fn get_staking_info(&self, coin: &str) -> Result<StakingInfo> {
        let url = format!("{}/staking/{}", SIDESHIFT_API_BASE, coin.to_lowercase());

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send staking info request to SideShift API")?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            anyhow::bail!(
                "SideShift API returned error status {}: {}",
                status,
                error_text
            );
        }

        let staking_response: StakingInfoResponse = response
            .json()
            .await
            .context("Failed to parse SideShift staking info response")?;

        Ok(StakingInfo {
            coin: staking_response.coin,
            apy: Decimal::from_str_exact(&staking_response.apy).unwrap_or(Decimal::ZERO),
            minimum_amount: Decimal::from_str_exact(&staking_response.minimum_amount)
                .unwrap_or(Decimal::ZERO),
            lock_period_days: staking_response.lock_period_days,
            compound_frequency: staking_response.compound_frequency,
        })
    }

    fn convert_quote_response(&self, response: QuoteResponse) -> ConversionQuote {
        ConversionQuote {
            quote_id: response.id,
            from_asset: response.deposit_coin.to_uppercase(),
            to_asset: response.settle_coin.to_uppercase(),
            from_amount: Decimal::from_str_exact(&response.deposit_amount)
                .unwrap_or(Decimal::ZERO),
            to_amount: Decimal::from_str_exact(&response.settle_amount).unwrap_or(Decimal::ZERO),
            exchange_rate: Decimal::from_str_exact(&response.rate).unwrap_or(Decimal::ZERO),
            network_fee: Decimal::ZERO, // SideShift includes fees in the rate
            platform_fee: Decimal::ZERO,
            sideshift_fee: Decimal::ZERO,
            expires_at: response.expires_at,
        }
    }

    fn convert_order_response(&self, response: OrderResponse) -> ConversionOrder {
        ConversionOrder {
            order_id: response.id,
            deposit_address: response.deposit_address,
            deposit_coin: response.deposit_coin.to_uppercase(),
            settle_coin: response.settle_coin.to_uppercase(),
            deposit_amount: Decimal::from_str_exact(&response.deposit_amount)
                .unwrap_or(Decimal::ZERO),
            settle_amount: Decimal::from_str_exact(&response.settle_amount)
                .unwrap_or(Decimal::ZERO),
            expires_at: response.expires_at,
            created_at: response.created_at,
        }
    }
}

// Request/Response types

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AmountType {
    From,
    To,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QuoteRequest {
    deposit_coin: String,
    settle_coin: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    deposit_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settle_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    affiliate_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QuoteResponse {
    id: String,
    deposit_coin: String,
    settle_coin: String,
    deposit_amount: String,
    settle_amount: String,
    rate: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OrderRequest {
    quote_id: String,
    settle_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    refund_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    affiliate_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrderResponse {
    id: String,
    deposit_address: String,
    deposit_coin: String,
    settle_coin: String,
    deposit_amount: String,
    settle_amount: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrderStatusResponse {
    id: String,
    status: String,
    deposit_address: String,
    deposit_amount: String,
    settle_amount: String,
    expires_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoinResponse {
    coin: String,
    name: String,
    networks: Vec<String>,
    #[serde(default)]
    has_staking: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StakingInfoResponse {
    coin: String,
    apy: String,
    minimum_amount: String,
    lock_period_days: u32,
    compound_frequency: String,
}

// Public types

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionQuote {
    pub quote_id: String,
    pub from_asset: String,
    pub to_asset: String,
    pub from_amount: Decimal,
    pub to_amount: Decimal,
    pub exchange_rate: Decimal,
    pub network_fee: Decimal,
    pub platform_fee: Decimal,
    pub sideshift_fee: Decimal,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionOrder {
    pub order_id: String,
    pub deposit_address: String,
    pub deposit_coin: String,
    pub settle_coin: String,
    pub deposit_amount: Decimal,
    pub settle_amount: Decimal,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderStatus {
    pub order_id: String,
    pub status: String,
    pub deposit_address: String,
    pub deposit_amount: Decimal,
    pub settle_amount: Decimal,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportedCoin {
    pub coin: String,
    pub name: String,
    pub networks: Vec<String>,
    pub has_staking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingInfo {
    pub coin: String,
    pub apy: Decimal,
    pub minimum_amount: Decimal,
    pub lock_period_days: u32,
    pub compound_frequency: String,
}
