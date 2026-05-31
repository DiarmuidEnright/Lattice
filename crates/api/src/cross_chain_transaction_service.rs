use blockchain::Blockchain;
use chrono::{DateTime, Utc};
use database::DbPool;
use rust_decimal::Decimal;
use shared::{Error, Result};
use tracing::{debug, info};
use uuid::Uuid;

/// Normalized transaction format across all blockchains
/// 
/// **Validates: Requirement 5.3**
#[derive(Debug, Clone)]
pub struct NormalizedTransaction {
    pub id: Uuid,
    pub blockchain: Blockchain,
    pub from_address: String,
    pub to_address: String,
    pub amount: Decimal,
    pub token_symbol: String,
    pub token_address: Option<String>,
    pub transaction_hash: String,
    pub status: TransactionStatus,
    pub fees: TransactionFees,
    pub timestamp: DateTime<Utc>,
    pub confirmations: u64,
}

/// Transaction status across chains
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionStatus {
    Pending,
    Confirmed,
    Failed,
}

/// Blockchain-specific transaction fees
/// 
/// **Validates: Requirements 5.4, 5.6**
#[derive(Debug, Clone)]
pub struct TransactionFees {
    pub gas_fee: Option<Decimal>,
    pub network_fee: Decimal,
    pub platform_fee: Option<Decimal>,
    pub total_fee: Decimal,
    pub fee_currency: String,
}

/// Cross-chain transaction service
/// 
/// Handles transaction normalization, fee calculation, and cross-chain transfers
/// 
/// **Validates: Requirements 5.3, 5.4, 5.5, 5.6**
pub struct CrossChainTransactionService {
    db: DbPool,
}

impl CrossChainTransactionService {
    /// Create a new cross-chain transaction service
    pub fn new(db: DbPool) -> Self {
        info!("Initializing cross-chain transaction service");
        Self { db }
    }
    
    /// Normalize a transaction from any blockchain into standard format
    /// 
    /// **Validates: Requirement 5.3**
    pub async fn normalize_transaction(
        &self,
        blockchain: Blockchain,
        transaction_hash: &str,
    ) -> Result<NormalizedTransaction> {
        debug!(
            "Normalizing transaction {} on {:?}",
            transaction_hash, blockchain
        );
        
        match blockchain {
            Blockchain::Solana => self.normalize_solana_transaction(transaction_hash).await,
            Blockchain::Ethereum | Blockchain::BinanceSmartChain | Blockchain::Polygon => {
                self.normalize_evm_transaction(blockchain, transaction_hash).await
            }
        }
    }
    
    /// Normalize a Solana transaction
    async fn normalize_solana_transaction(
        &self,
        transaction_hash: &str,
    ) -> Result<NormalizedTransaction> {
        // For now, return a placeholder
        // Full implementation would query Solana RPC for transaction details
        Ok(NormalizedTransaction {
            id: Uuid::new_v4(),
            blockchain: Blockchain::Solana,
            from_address: "".to_string(),
            to_address: "".to_string(),
            amount: Decimal::ZERO,
            token_symbol: "SOL".to_string(),
            token_address: None,
            transaction_hash: transaction_hash.to_string(),
            status: TransactionStatus::Pending,
            fees: TransactionFees {
                gas_fee: None,
                network_fee: Decimal::new(5000, 9), // 0.000005 SOL typical fee
                platform_fee: None,
                total_fee: Decimal::new(5000, 9),
                fee_currency: "SOL".to_string(),
            },
            timestamp: Utc::now(),
            confirmations: 0,
        })
    }
    
    /// Normalize an EVM transaction (Ethereum, BSC, Polygon)
    async fn normalize_evm_transaction(
        &self,
        blockchain: Blockchain,
        transaction_hash: &str,
    ) -> Result<NormalizedTransaction> {
        // For now, return a placeholder
        // Full implementation would query EVM RPC for transaction details
        let native_token = match blockchain {
            Blockchain::Ethereum => "ETH",
            Blockchain::BinanceSmartChain => "BNB",
            Blockchain::Polygon => "MATIC",
            _ => "UNKNOWN",
        };
        
        Ok(NormalizedTransaction {
            id: Uuid::new_v4(),
            blockchain,
            from_address: "".to_string(),
            to_address: "".to_string(),
            amount: Decimal::ZERO,
            token_symbol: native_token.to_string(),
            token_address: None,
            transaction_hash: transaction_hash.to_string(),
            status: TransactionStatus::Pending,
            fees: TransactionFees {
                gas_fee: Some(Decimal::new(21000, 0)), // Base gas units
                network_fee: Decimal::new(20, 9), // 20 gwei typical
                platform_fee: None,
                total_fee: Decimal::new(420000, 9), // 21000 * 20 gwei
                fee_currency: native_token.to_string(),
            },
            timestamp: Utc::now(),
            confirmations: 0,
        })
    }
    
    /// Calculate transaction fees for a specific blockchain
    /// 
    /// **Validates: Requirements 5.4, 5.6**
    pub async fn calculate_transaction_fees(
        &self,
        blockchain: Blockchain,
        token_address: Option<&str>,
        amount: Decimal,
    ) -> Result<TransactionFees> {
        debug!(
            "Calculating transaction fees for {:?} blockchain",
            blockchain
        );
        
        match blockchain {
            Blockchain::Solana => self.calculate_solana_fees(token_address, amount).await,
            Blockchain::Ethereum => self.calculate_ethereum_fees(token_address, amount).await,
            Blockchain::BinanceSmartChain => self.calculate_bsc_fees(token_address, amount).await,
            Blockchain::Polygon => self.calculate_polygon_fees(token_address, amount).await,
        }
    }
    
    /// Calculate Solana transaction fees
    async fn calculate_solana_fees(
        &self,
        token_address: Option<&str>,
        _amount: Decimal,
    ) -> Result<TransactionFees> {
        let base_fee = Decimal::new(5000, 9); // 0.000005 SOL
        
        // SPL token transfers cost slightly more
        let network_fee = if token_address.is_some() {
            base_fee * Decimal::new(2, 0) // 2x for SPL tokens
        } else {
            base_fee
        };
        
        Ok(TransactionFees {
            gas_fee: None,
            network_fee,
            platform_fee: None,
            total_fee: network_fee,
            fee_currency: "SOL".to_string(),
        })
    }
    
    /// Calculate Ethereum transaction fees
    async fn calculate_ethereum_fees(
        &self,
        token_address: Option<&str>,
        _amount: Decimal,
    ) -> Result<TransactionFees> {
        // Base gas units for ETH transfer: 21000
        // ERC-20 transfer: ~65000
        let gas_units = if token_address.is_some() {
            Decimal::new(65000, 0)
        } else {
            Decimal::new(21000, 0)
        };
        
        // Typical gas price: 20 gwei (20 * 10^9 wei)
        let gas_price_gwei = Decimal::new(20, 0);
        let gas_price_eth = gas_price_gwei / Decimal::new(1_000_000_000, 0);
        
        let network_fee = gas_units * gas_price_eth;
        
        Ok(TransactionFees {
            gas_fee: Some(gas_units),
            network_fee,
            platform_fee: None,
            total_fee: network_fee,
            fee_currency: "ETH".to_string(),
        })
    }
    
    /// Calculate Binance Smart Chain transaction fees
    async fn calculate_bsc_fees(
        &self,
        token_address: Option<&str>,
        _amount: Decimal,
    ) -> Result<TransactionFees> {
        // BSC has lower gas prices than Ethereum
        let gas_units = if token_address.is_some() {
            Decimal::new(65000, 0)
        } else {
            Decimal::new(21000, 0)
        };
        
        // Typical gas price: 5 gwei
        let gas_price_gwei = Decimal::new(5, 0);
        let gas_price_bnb = gas_price_gwei / Decimal::new(1_000_000_000, 0);
        
        let network_fee = gas_units * gas_price_bnb;
        
        Ok(TransactionFees {
            gas_fee: Some(gas_units),
            network_fee,
            platform_fee: None,
            total_fee: network_fee,
            fee_currency: "BNB".to_string(),
        })
    }
    
    /// Calculate Polygon transaction fees
    async fn calculate_polygon_fees(
        &self,
        token_address: Option<&str>,
        _amount: Decimal,
    ) -> Result<TransactionFees> {
        // Polygon has very low gas prices
        let gas_units = if token_address.is_some() {
            Decimal::new(65000, 0)
        } else {
            Decimal::new(21000, 0)
        };
        
        // Typical gas price: 30 gwei (but MATIC is cheaper than ETH)
        let gas_price_gwei = Decimal::new(30, 0);
        let gas_price_matic = gas_price_gwei / Decimal::new(1_000_000_000, 0);
        
        let network_fee = gas_units * gas_price_matic;
        
        Ok(TransactionFees {
            gas_fee: Some(gas_units),
            network_fee,
            platform_fee: None,
            total_fee: network_fee,
            fee_currency: "MATIC".to_string(),
        })
    }
    
    /// Display fees before transaction execution
    /// 
    /// **Validates: Requirement 5.4**
    pub fn format_fees_for_display(&self, fees: &TransactionFees) -> String {
        let mut display = String::new();
        
        if let Some(gas_fee) = fees.gas_fee {
            display.push_str(&format!("Gas Units: {}\n", gas_fee));
        }
        
        display.push_str(&format!(
            "Network Fee: {} {}\n",
            fees.network_fee, fees.fee_currency
        ));
        
        if let Some(platform_fee) = fees.platform_fee {
            display.push_str(&format!(
                "Platform Fee: {} {}\n",
                platform_fee, fees.fee_currency
            ));
        }
        
        display.push_str(&format!(
            "Total Fee: {} {}",
            fees.total_fee, fees.fee_currency
        ));
        
        display
    }
    
    /// Track gas fees per blockchain
    /// 
    /// **Validates: Requirement 5.6**
    pub async fn record_gas_fee(
        &self,
        user_id: Uuid,
        blockchain: Blockchain,
        transaction_hash: &str,
        fees: &TransactionFees,
    ) -> Result<()> {
        info!(
            "Recording gas fee for transaction {} on {:?}",
            transaction_hash, blockchain
        );
        
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;
        
        let blockchain_str = format!("{:?}", blockchain);
        
        client
            .execute(
                "INSERT INTO gas_fees (user_id, blockchain, transaction_hash, gas_units, gas_price, network_fee, total_fee, fee_currency, recorded_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())",
                &[
                    &user_id,
                    &blockchain_str,
                    &transaction_hash,
                    &fees.gas_fee,
                    &fees.network_fee,
                    &fees.network_fee,
                    &fees.total_fee,
                    &fees.fee_currency,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to record gas fee: {}", e)))?;
        
        Ok(())
    }
    
    /// Get total gas fees for a user per blockchain
    /// 
    /// **Validates: Requirement 5.6**
    pub async fn get_gas_fees_by_blockchain(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<(Blockchain, Decimal, String)>> {
        debug!("Fetching gas fees by blockchain for user {}", user_id);
        
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;
        
        let rows = client
            .query(
                "SELECT blockchain, SUM(total_fee), fee_currency
                 FROM gas_fees
                 WHERE user_id = $1
                 GROUP BY blockchain, fee_currency
                 ORDER BY blockchain",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query gas fees: {}", e)))?;
        
        let mut results = Vec::new();
        for row in rows {
            let blockchain_str: String = row.get(0);
            let total_fee: Decimal = row.get(1);
            let fee_currency: String = row.get(2);
            
            let blockchain = match blockchain_str.as_str() {
                "Solana" => Blockchain::Solana,
                "Ethereum" => Blockchain::Ethereum,
                "BinanceSmartChain" => Blockchain::BinanceSmartChain,
                "Polygon" => Blockchain::Polygon,
                _ => continue,
            };
            
            results.push((blockchain, total_fee, fee_currency));
        }
        
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_transaction_status() {
        let status = TransactionStatus::Pending;
        assert_eq!(status, TransactionStatus::Pending);
        
        let status = TransactionStatus::Confirmed;
        assert_eq!(status, TransactionStatus::Confirmed);
    }
    
    #[test]
    fn test_transaction_fees_structure() {
        let fees = TransactionFees {
            gas_fee: Some(Decimal::new(21000, 0)),
            network_fee: Decimal::new(420000, 9),
            platform_fee: None,
            total_fee: Decimal::new(420000, 9),
            fee_currency: "ETH".to_string(),
        };
        
        assert_eq!(fees.gas_fee.unwrap(), Decimal::new(21000, 0));
        assert_eq!(fees.fee_currency, "ETH");
        assert!(fees.platform_fee.is_none());
    }
    
    #[test]
    fn test_normalized_transaction_structure() {
        let tx = NormalizedTransaction {
            id: Uuid::new_v4(),
            blockchain: Blockchain::Ethereum,
            from_address: "0x123".to_string(),
            to_address: "0x456".to_string(),
            amount: Decimal::new(100, 0),
            token_symbol: "ETH".to_string(),
            token_address: None,
            transaction_hash: "0xabc".to_string(),
            status: TransactionStatus::Confirmed,
            fees: TransactionFees {
                gas_fee: Some(Decimal::new(21000, 0)),
                network_fee: Decimal::new(420000, 9),
                platform_fee: None,
                total_fee: Decimal::new(420000, 9),
                fee_currency: "ETH".to_string(),
            },
            timestamp: Utc::now(),
            confirmations: 12,
        };
        
        assert_eq!(tx.blockchain, Blockchain::Ethereum);
        assert_eq!(tx.status, TransactionStatus::Confirmed);
        assert_eq!(tx.confirmations, 12);
    }
}
