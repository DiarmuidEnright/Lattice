use chrono::Utc;
use database::DbPool;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use shared::{Error, Result};
use std::str::FromStr;
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PositionMode {
    Manual,
    Automatic,
}

impl FromStr for PositionMode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "manual" => Ok(PositionMode::Manual),
            "automatic" => Ok(PositionMode::Automatic),
            _ => Err(Error::Validation(format!("Invalid position mode: {}", s))),
        }
    }
}

impl std::fmt::Display for PositionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PositionMode::Manual => write!(f, "manual"),
            PositionMode::Automatic => write!(f, "automatic"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionModeConfig {
    pub id: Uuid,
    pub user_id: Uuid,
    pub asset: String,
    pub blockchain: String,
    pub mode: PositionMode,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrder {
    pub id: Uuid,
    pub user_id: Uuid,
    pub asset: String,
    pub blockchain: String,
    pub action: String,
    pub amount: Decimal,
    pub price_usd: Option<Decimal>,
    pub total_value_usd: Option<Decimal>,
    pub status: String,
    pub transaction_hash: Option<String>,
    pub error_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub executed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub cancelled_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOrderRequest {
    pub asset: String,
    pub blockchain: Option<String>,
    pub action: String, // "BUY" or "SELL"
    pub amount: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAutomaticOrder {
    pub id: Uuid,
    pub user_id: Uuid,
    pub asset: String,
    pub blockchain: String,
    pub order_type: String,
    pub order_reference_id: Option<Uuid>,
    pub action: String,
    pub amount: Decimal,
    pub status: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub executed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub cancelled_at: Option<chrono::DateTime<chrono::Utc>>,
}

pub struct PositionManagementService {
    db: DbPool,
}

impl PositionManagementService {
    pub fn new(db: DbPool) -> Self {
        Self { db }
    }

    /// Get position mode for a specific asset
    pub async fn get_position_mode(
        &self,
        user_id: Uuid,
        asset: &str,
        blockchain: &str,
    ) -> Result<PositionMode> {
        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let result = client
            .query_opt(
                "SELECT mode FROM position_modes WHERE user_id = $1 AND asset = $2 AND blockchain = $3",
                &[&user_id, &asset, &blockchain],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query position mode: {}", e)))?;

        if let Some(row) = result {
            let mode_str: String = row.get(0);
            Ok(PositionMode::from_str(&mode_str)?)
        } else {
            Ok(PositionMode::Manual)
        }
    }

    /// Set position mode for a specific asset
    pub async fn set_position_mode(
        &self,
        user_id: Uuid,
        asset: &str,
        blockchain: &str,
        mode: PositionMode,
    ) -> Result<PositionModeConfig> {
        info!(
            "Setting position mode for user {} asset {} to {:?}",
            user_id, asset, mode
        );

        // If switching to manual mode, cancel pending automatic orders
        if mode == PositionMode::Manual {
            self.cancel_pending_automatic_orders(user_id, asset, blockchain)
                .await?;
        }

        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let mode_str = mode.to_string();
        let now = Utc::now();
        
        let row = client
            .query_one(
                "INSERT INTO position_modes (user_id, asset, blockchain, mode, updated_at)
                 VALUES ($1, $2, $3, $4, $5)
                 ON CONFLICT (user_id, asset, blockchain)
                 DO UPDATE SET mode = $4, updated_at = $5
                 RETURNING id, user_id, asset, blockchain, mode, created_at, updated_at",
                &[&user_id, &asset, &blockchain, &mode_str, &now],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to set position mode: {}", e)))?;

        let result = PositionModeConfig {
            id: row.get(0),
            user_id: row.get(1),
            asset: row.get(2),
            blockchain: row.get(3),
            mode: PositionMode::from_str(&row.get::<_, String>(4))?,
            created_at: row.get(5),
            updated_at: row.get(6),
        };

        info!(
            "Position mode set successfully for user {} asset {}: {:?}",
            user_id, asset, mode
        );

        Ok(result)
    }

    /// Cancel all pending automatic orders for an asset when switching to manual mode
    async fn cancel_pending_automatic_orders(
        &self,
        user_id: Uuid,
        asset: &str,
        blockchain: &str,
    ) -> Result<u64> {
        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let now = Utc::now();
        let result = client
            .execute(
                "UPDATE pending_automatic_orders
                 SET status = 'cancelled', cancelled_at = $4
                 WHERE user_id = $1 AND asset = $2 AND blockchain = $3 AND status = 'pending'",
                &[&user_id, &asset, &blockchain, &now],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to cancel pending orders: {}", e)))?;

        let cancelled_count = result;
        if cancelled_count > 0 {
            info!(
                "Cancelled {} pending automatic orders for user {} asset {}",
                cancelled_count, user_id, asset
            );
        }

        Ok(cancelled_count)
    }

    /// Create a manual order
    pub async fn create_manual_order(
        &self,
        user_id: Uuid,
        request: ManualOrderRequest,
    ) -> Result<ManualOrder> {
        let blockchain = request.blockchain.unwrap_or_else(|| "Solana".to_string());

        // Validate action
        let action = request.action.to_uppercase();
        if action != "BUY" && action != "SELL" {
            return Err(Error::Validation("Invalid action: must be BUY or SELL".to_string()));
        }

        // Validate amount is positive
        if request.amount <= Decimal::ZERO {
            return Err(Error::Validation("Amount must be positive".to_string()));
        }

        // Validate balance for SELL orders
        if action == "SELL" {
            self.validate_balance(user_id, &request.asset, &blockchain, request.amount)
                .await?;
        }

        info!(
            "Creating manual order for user {}: {} {} {}",
            user_id, action, request.amount, request.asset
        );

        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let order_id = Uuid::new_v4();
        let now = Utc::now();
        
        client
            .execute(
                "INSERT INTO manual_orders (id, user_id, asset, blockchain, action, amount, status, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)",
                &[&order_id, &user_id, &request.asset, &blockchain, &action, &request.amount, &now],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to create manual order: {}", e)))?;

        info!("Manual order created successfully: {}", order_id);

        Ok(ManualOrder {
            id: order_id,
            user_id,
            asset: request.asset,
            blockchain,
            action,
            amount: request.amount,
            price_usd: None,
            total_value_usd: None,
            status: "pending".to_string(),
            transaction_hash: None,
            error_message: None,
            created_at: now,
            executed_at: None,
            cancelled_at: None,
        })
    }

    /// Validate user has sufficient balance for a sell order
    async fn validate_balance(
        &self,
        user_id: Uuid,
        asset: &str,
        blockchain: &str,
        amount: Decimal,
    ) -> Result<()> {
        // For Solana, check portfolio_assets table
        if blockchain == "Solana" {
            let client = self.db.get().await
                .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

            let result = client
                .query_opt(
                    "SELECT COALESCE(SUM(pa.amount), 0) as total_amount
                     FROM portfolio_assets pa
                     JOIN wallets w ON pa.wallet_id = w.id
                     WHERE w.user_id = $1 AND pa.token_symbol = $2",
                    &[&user_id, &asset],
                )
                .await
                .map_err(|e| Error::Database(format!("Failed to query balance: {}", e)))?;

            if let Some(row) = result {
                let total_balance: Decimal = row.get(0);

                if total_balance < amount {
                    return Err(Error::Database(format!(
                        "Insufficient balance: have {}, need {}",
                        total_balance,
                        amount
                    )));
                }
            } else {
                return Err(Error::Validation(format!("No balance found for asset {}", asset)));
            }
        } else {
            // For multi-chain, check multi_chain_wallets and their balances
            // This would require integration with Birdeye or blockchain clients
            // For now, we'll allow the order and let execution handle validation
            warn!(
                "Balance validation not fully implemented for blockchain: {}",
                blockchain
            );
        }

        Ok(())
    }

    /// Get manual order by ID
    pub async fn get_manual_order(&self, order_id: Uuid, user_id: Uuid) -> Result<ManualOrder> {
        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let row = client
            .query_opt(
                "SELECT id, user_id, asset, blockchain, action, amount, price_usd, total_value_usd,
                        status, transaction_hash, error_message, created_at, executed_at, cancelled_at
                 FROM manual_orders
                 WHERE id = $1 AND user_id = $2",
                &[&order_id, &user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query manual order: {}", e)))?
            .ok_or_else(|| Error::Validation(format!("Manual order not found")))?;

        Ok(ManualOrder {
            id: row.get(0),
            user_id: row.get(1),
            asset: row.get(2),
            blockchain: row.get(3),
            action: row.get(4),
            amount: row.get(5),
            price_usd: row.get(6),
            total_value_usd: row.get(7),
            status: row.get(8),
            transaction_hash: row.get(9),
            error_message: row.get(10),
            created_at: row.get(11),
            executed_at: row.get(12),
            cancelled_at: row.get(13),
        })
    }

    /// Get all manual orders for a user
    pub async fn get_user_manual_orders(
        &self,
        user_id: Uuid,
        limit: Option<i64>,
    ) -> Result<Vec<ManualOrder>> {
        let limit = limit.unwrap_or(50).min(100);

        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let rows = client
            .query(
                "SELECT id, user_id, asset, blockchain, action, amount, price_usd, total_value_usd,
                        status, transaction_hash, error_message, created_at, executed_at, cancelled_at
                 FROM manual_orders
                 WHERE user_id = $1
                 ORDER BY created_at DESC
                 LIMIT $2",
                &[&user_id, &limit],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query manual orders: {}", e)))?;

        let orders = rows
            .iter()
            .map(|row| ManualOrder {
                id: row.get(0),
                user_id: row.get(1),
                asset: row.get(2),
                blockchain: row.get(3),
                action: row.get(4),
                amount: row.get(5),
                price_usd: row.get(6),
                total_value_usd: row.get(7),
                status: row.get(8),
                transaction_hash: row.get(9),
                error_message: row.get(10),
                created_at: row.get(11),
                executed_at: row.get(12),
                cancelled_at: row.get(13),
            })
            .collect();

        Ok(orders)
    }

    /// Update manual order status (used by trading service after execution)
    pub async fn update_order_status(
        &self,
        order_id: Uuid,
        status: &str,
        transaction_hash: Option<String>,
        price_usd: Option<Decimal>,
        total_value_usd: Option<Decimal>,
        error_message: Option<String>,
    ) -> Result<()> {
        let executed_at = if status == "executed" {
            Some(Utc::now())
        } else {
            None
        };

        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        client
            .execute(
                "UPDATE manual_orders
                 SET status = $2, transaction_hash = $3, price_usd = $4, total_value_usd = $5,
                     error_message = $6, executed_at = $7
                 WHERE id = $1",
                &[&order_id, &status, &transaction_hash, &price_usd, &total_value_usd, &error_message, &executed_at],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to update order status: {}", e)))?;

        info!("Manual order {} status updated to {}", order_id, status);

        Ok(())
    }

    /// Cancel a manual order
    pub async fn cancel_manual_order(&self, order_id: Uuid, user_id: Uuid) -> Result<()> {
        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let now = Utc::now();
        let result = client
            .execute(
                "UPDATE manual_orders
                 SET status = 'cancelled', cancelled_at = $3
                 WHERE id = $1 AND user_id = $2 AND status = 'pending'",
                &[&order_id, &user_id, &now],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to cancel order: {}", e)))?;

        if result == 0 {
            return Err(Error::Database(format!(
                "Order not found or cannot be cancelled (not in pending status)"
            )));
        }

        info!("Manual order {} cancelled by user {}", order_id, user_id);

        Ok(())
    }

    /// Register a pending automatic order (called by benchmark/trim/AI services)
    pub async fn register_pending_automatic_order(
        &self,
        user_id: Uuid,
        asset: &str,
        blockchain: &str,
        order_type: &str,
        order_reference_id: Option<Uuid>,
        action: &str,
        amount: Decimal,
    ) -> Result<Uuid> {
        // Check if asset is in automatic mode
        let mode = self.get_position_mode(user_id, asset, blockchain).await?;
        if mode != PositionMode::Automatic {
            return Err(Error::Database(format!(
                "Cannot register automatic order: asset {} is in manual mode",
                asset
            )));
        }

        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let order_id = Uuid::new_v4();
        client
            .execute(
                "INSERT INTO pending_automatic_orders 
                    (id, user_id, asset, blockchain, order_type, order_reference_id, action, amount, status)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending')",
                &[&order_id, &user_id, &asset, &blockchain, &order_type, &order_reference_id, &action, &amount],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to register pending order: {}", e)))?;

        info!(
            "Registered pending automatic order {} for user {} asset {}",
            order_id, user_id, asset
        );

        Ok(order_id)
    }

    /// Mark automatic order as executed
    pub async fn mark_automatic_order_executed(&self, order_id: Uuid) -> Result<()> {
        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let now = Utc::now();
        client
            .execute(
                "UPDATE pending_automatic_orders
                 SET status = 'executed', executed_at = $2
                 WHERE id = $1",
                &[&order_id, &now],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to mark order as executed: {}", e)))?;

        Ok(())
    }

    /// Get all position modes for a user
    pub async fn get_user_position_modes(
        &self,
        user_id: Uuid,
    ) -> Result<Vec<PositionModeConfig>> {
        let client = self.db.get().await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let rows = client
            .query(
                "SELECT id, user_id, asset, blockchain, mode, created_at, updated_at
                 FROM position_modes
                 WHERE user_id = $1
                 ORDER BY updated_at DESC",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query position modes: {}", e)))?;

        let modes = rows
            .iter()
            .map(|row| {
                let mode_str: String = row.get(4);
                Ok(PositionModeConfig {
                    id: row.get(0),
                    user_id: row.get(1),
                    asset: row.get(2),
                    blockchain: row.get(3),
                    mode: PositionMode::from_str(&mode_str)?,
                    created_at: row.get(5),
                    updated_at: row.get(6),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(modes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_mode_from_str() {
        assert_eq!(
            PositionMode::from_str("manual").unwrap(),
            PositionMode::Manual
        );
        assert_eq!(
            PositionMode::from_str("MANUAL").unwrap(),
            PositionMode::Manual
        );
        assert_eq!(
            PositionMode::from_str("automatic").unwrap(),
            PositionMode::Automatic
        );
        assert_eq!(
            PositionMode::from_str("AUTOMATIC").unwrap(),
            PositionMode::Automatic
        );
        assert!(PositionMode::from_str("invalid").is_err());
    }

    #[test]
    fn test_position_mode_display() {
        assert_eq!(PositionMode::Manual.to_string(), "manual");
        assert_eq!(PositionMode::Automatic.to_string(), "automatic");
    }
}
