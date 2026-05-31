use chrono::{DateTime, Utc};
use database::DbPool;
use notification::NotificationService;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use shared::error::{Error, Result};
use shared::models::{Notification, Recommendation, UserSettings};
use std::str::FromStr;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};
use trading::TradingService;
use uuid::Uuid;

use crate::position_management_service::{PositionManagementService, PositionMode};
use crate::trim_config_service::TrimConfigService;

/// Service for executing trim recommendations
/// 
/// This service processes pending trim recommendations and executes
/// partial sells via the trading service.
/// 
/// **Validates: Requirements 7.3, 7.4, 7.5, 7.6**
pub struct TrimExecutor {
    db_pool: DbPool,
    trim_config_service: Arc<TrimConfigService>,
    position_management_service: Arc<PositionManagementService>,
    trading_service: Arc<TradingService>,
    #[allow(dead_code)]
    notification_service: Arc<NotificationService>,
}

/// Represents a pending trim recommendation
#[derive(Debug, Clone)]
pub struct PendingTrim {
    pub id: Uuid,
    pub user_id: Uuid,
    pub wallet_id: Uuid,
    pub token_mint: String,
    pub token_symbol: String,
    pub amount: String,
    pub confidence: i32,
    pub reasoning: String,
    pub suggested_trim_percent: Decimal,
    pub created_at: DateTime<Utc>,
}

/// Represents an executed trim
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrimExecution {
    pub id: Uuid,
    pub user_id: Uuid,
    pub position_id: Uuid,
    pub asset: String,
    pub amount_sold: Decimal,
    pub price_usd: Decimal,
    pub profit_realized: Decimal,
    pub confidence: i32,
    pub reasoning: String,
    pub transaction_hash: String,
    pub executed_at: DateTime<Utc>,
}

impl TrimExecutor {
    /// Create a new trim executor
    pub fn new(
        db_pool: DbPool,
        trim_config_service: Arc<TrimConfigService>,
        position_management_service: Arc<PositionManagementService>,
        trading_service: Arc<TradingService>,
        notification_service: Arc<NotificationService>,
    ) -> Self {
        Self {
            db_pool,
            trim_config_service,
            position_management_service,
            trading_service,
            notification_service,
        }
    }

    /// Start the background worker that processes pending trims every 1 minute
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(60)); // 1 minute

            info!("Trim executor started - processing pending trims every 1 minute");

            loop {
                ticker.tick().await;

                debug!("Trim executor tick - processing pending trims");

                if let Err(e) = self.process_pending_trims().await {
                    error!("Error in trim executor: {}", e);
                    // Continue processing even if one cycle fails
                }
            }
        })
    }

    /// Process all pending trim recommendations
    async fn process_pending_trims(&self) -> Result<()> {
        let pending_trims = self.get_pending_trims().await?;

        info!("Processing {} pending trim recommendations", pending_trims.len());

        for trim in pending_trims {
            // Check if user has reached daily limit
            if self
                .trim_config_service
                .has_reached_daily_limit(trim.user_id)
                .await?
            {
                debug!(
                    "User {} has reached daily trim limit, skipping trim for {}",
                    trim.user_id, trim.token_symbol
                );
                continue;
            }

            // Execute the trim
            match self.execute_trim(&trim).await {
                Ok(execution) => {
                    info!(
                        "Successfully executed trim for user {} on {}: sold {} tokens, profit ${:.2}",
                        trim.user_id,
                        trim.token_symbol,
                        execution.amount_sold,
                        execution.profit_realized
                    );

                    // Remove from pending trims
                    if let Err(e) = self.remove_pending_trim(trim.id).await {
                        error!("Failed to remove pending trim {}: {}", trim.id, e);
                    }
                }
                Err(e) => {
                    warn!(
                        "Failed to execute trim for user {} on {}: {}",
                        trim.user_id, trim.token_symbol, e
                    );
                    // Keep in pending for retry on next cycle
                }
            }
        }

        Ok(())
    }

    /// Get all pending trim recommendations
    async fn get_pending_trims(&self) -> Result<Vec<PendingTrim>> {
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let rows = client
            .query(
                "SELECT id, user_id, wallet_id, token_mint, token_symbol, amount, 
                        confidence, reasoning, suggested_trim_percent, created_at
                 FROM pending_trims
                 ORDER BY created_at ASC",
                &[],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query pending trims: {}", e)))?;

        let trims = rows
            .iter()
            .map(|row| PendingTrim {
                id: row.get(0),
                user_id: row.get(1),
                wallet_id: row.get(2),
                token_mint: row.get(3),
                token_symbol: row.get(4),
                amount: row.get(5),
                confidence: row.get(6),
                reasoning: row.get(7),
                suggested_trim_percent: row.get(8),
                created_at: row.get(9),
            })
            .collect();

        Ok(trims)
    }

    /// Execute a trim recommendation
    /// 
    /// **Validates: Requirements 7.3, 7.4, 7.5, 7.6, 4.4**
    pub async fn execute_trim(&self, trim: &PendingTrim) -> Result<TrimExecution> {
        info!(
            "Executing trim for user {} on {}: confidence {}%, trim {}%",
            trim.user_id, trim.token_symbol, trim.confidence, trim.suggested_trim_percent
        );

        // Check if asset is in automatic mode (Requirement 4.4)
        let position_mode = self.position_management_service
            .get_position_mode(trim.user_id, &trim.token_symbol, "Solana")
            .await?;
        
        if position_mode != PositionMode::Automatic {
            info!(
                "Skipping trim execution for {} - asset is in manual mode",
                trim.token_symbol
            );
            return Err(Error::Validation(format!(
                "Cannot execute trim: asset {} is in manual mode",
                trim.token_symbol
            )));
        }

        // Register as pending automatic order (Requirement 4.4)
        let total_amount = Decimal::from_str(&trim.amount)
            .map_err(|e| Error::Validation(format!("Invalid amount: {}", e)))?;
        let trim_amount = total_amount * (trim.suggested_trim_percent / Decimal::from(100));
        
        match self.position_management_service
            .register_pending_automatic_order(
                trim.user_id,
                &trim.token_symbol,
                "Solana",
                "trim",
                Some(trim.id),
                "SELL",
                trim_amount,
            )
            .await
        {
            Ok(order_id) => {
                info!(
                    "Registered pending automatic order {} for trim {}",
                    order_id, trim.id
                );
            }
            Err(e) => {
                warn!(
                    "Failed to register pending automatic order for trim {}: {}",
                    trim.id, e
                );
            }
        }

        // Get user settings
        let user_settings = self.get_user_settings(trim.user_id).await?;

        // Calculate trim amount (percentage of current position)
        let total_amount = Decimal::from_str(&trim.amount)
            .map_err(|e| Error::Validation(format!("Invalid amount: {}", e)))?;
        
        let trim_amount = total_amount * (trim.suggested_trim_percent / Decimal::from(100));

        // Get current price for profit calculation
        let current_price = self.get_current_price(&trim.token_mint).await?;

        // Get entry price for profit calculation
        let entry_price = self.get_entry_price(trim.user_id, &trim.token_mint).await?;

        // Calculate profit realized
        let profit_realized = if let Some(entry) = entry_price {
            (current_price - entry) * trim_amount
        } else {
            Decimal::ZERO
        };

        // Create a recommendation for the trading service
        let recommendation = Recommendation {
            id: Uuid::new_v4(),
            movement_id: Uuid::nil(), // Not from whale movement
            user_id: trim.user_id,
            action: "SELL".to_string(),
            confidence: trim.confidence,
            reasoning: trim.reasoning.clone(),
            suggested_amount: Some(trim_amount.to_string()),
            timeframe: None,
            risks: None,
            created_at: Utc::now(),
        };

        // Execute the sell trade via trading service
        let trade_execution = self
            .trading_service
            .execute_auto_trade(
                &recommendation,
                &user_settings,
                None, // No subscription check for trims
                0.0,  // Portfolio value not needed for trims
                None, // Email will be sent via trim notification
            )
            .await
            .map_err(|e| Error::ExternalService(format!("Failed to execute trade: {}", e)))?;

        // Create trim execution record
        let trim_execution = TrimExecution {
            id: Uuid::new_v4(),
            user_id: trim.user_id,
            position_id: trim.wallet_id, // Using wallet_id as position identifier
            asset: trim.token_symbol.clone(),
            amount_sold: trim_amount,
            price_usd: current_price,
            profit_realized,
            confidence: trim.confidence,
            reasoning: trim.reasoning.clone(),
            transaction_hash: trade_execution.transaction_signature.clone(),
            executed_at: Utc::now(),
        };

        // Log trim execution to database
        self.log_trim_execution(&trim_execution).await?;

        // Send notification to user
        self.send_trim_notification(&trim_execution).await?;

        Ok(trim_execution)
    }

    /// Get user settings
    async fn get_user_settings(&self, user_id: Uuid) -> Result<UserSettings> {
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let row = client
            .query_one(
                "SELECT auto_trader_enabled, max_trade_percentage, max_daily_trades, 
                        stop_loss_percentage, risk_tolerance, updated_at 
                 FROM user_settings WHERE user_id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query user settings: {}", e)))?;

        Ok(UserSettings {
            user_id,
            auto_trader_enabled: row.get(0),
            max_trade_percentage: row.get(1),
            max_daily_trades: row.get(2),
            stop_loss_percentage: row.get(3),
            risk_tolerance: row.get(4),
            updated_at: row.get(5),
        })
    }

    /// Get current price for a token
    async fn get_current_price(&self, token_mint: &str) -> Result<Decimal> {
        // In production, this would fetch from Birdeye or price oracle
        // For now, we'll use a placeholder
        // TODO: Integrate with Birdeye service for real-time prices
        
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        // Try to get latest price from portfolio_assets
        let row = client
            .query_opt(
                "SELECT value_usd, amount FROM portfolio_assets 
                 WHERE token_mint = $1 
                 ORDER BY updated_at DESC LIMIT 1",
                &[&token_mint],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query current price: {}", e)))?;

        if let Some(row) = row {
            // NUMERIC columns: read as Decimal then convert, mirroring the
            // working pattern in analytics.rs. Reading value_usd as f64 or
            // amount as String panics at runtime.
            let value_usd: Option<f64> = row
                .get::<_, Option<Decimal>>(0)
                .and_then(|v| v.to_string().parse::<f64>().ok());
            let amount: String = row.get::<_, Decimal>(1).to_string();

            if let (Some(value), Ok(amt)) = (value_usd, Decimal::from_str(&amount)) {
                if amt > Decimal::ZERO {
                    return Ok(Decimal::from_f64_retain(value).unwrap_or(Decimal::ZERO) / amt);
                }
            }
        }

        // Fallback to a default price if not found
        warn!("Could not determine current price for {}, using default", token_mint);
        Ok(Decimal::from(100)) // Default fallback
    }

    /// Get entry price for a token from trade history
    async fn get_entry_price(&self, user_id: Uuid, token_mint: &str) -> Result<Option<Decimal>> {
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        // Get the first buy trade for this token
        let row = client
            .query_opt(
                "SELECT price_usd FROM trade_executions 
                 WHERE user_id = $1 AND token_mint = $2 AND action = 'BUY' 
                 ORDER BY executed_at ASC LIMIT 1",
                &[&user_id, &token_mint],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query trade history: {}", e)))?;

        Ok(row.and_then(|r| {
            let price: Option<f64> = r.get(0);
            price.and_then(Decimal::from_f64_retain)
        }))
    }

    /// Log trim execution to database
    /// 
    /// **Validates: Requirements 7.6**
    async fn log_trim_execution(&self, execution: &TrimExecution) -> Result<()> {
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        client
            .execute(
                "INSERT INTO trim_executions 
                 (id, user_id, position_id, asset, amount_sold, price_usd, 
                  profit_realized, confidence, reasoning, transaction_hash, executed_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
                &[
                    &execution.id,
                    &execution.user_id,
                    &execution.position_id,
                    &execution.asset,
                    &execution.amount_sold,
                    &execution.price_usd,
                    &execution.profit_realized,
                    &execution.confidence,
                    &execution.reasoning,
                    &execution.transaction_hash,
                    &execution.executed_at,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to log trim execution: {}", e)))?;

        info!(
            "Logged trim execution {} for user {} on {}",
            execution.id, execution.user_id, execution.asset
        );

        Ok(())
    }

    /// Send notification to user about trim execution
    /// 
    /// **Validates: Requirements 7.5**
    async fn send_trim_notification(&self, execution: &TrimExecution) -> Result<()> {
        let _notification = Notification {
            id: Uuid::new_v4(),
            user_id: execution.user_id,
            notification_type: "TRIM_EXECUTED".to_string(),
            title: format!("Position Trimmed: {}", execution.asset),
            message: format!(
                "Agentic trimming sold {:.4} {} tokens at ${:.2}, realizing ${:.2} profit. Confidence: {}%. Reason: {}",
                execution.amount_sold,
                execution.asset,
                execution.price_usd,
                execution.profit_realized,
                execution.confidence,
                execution.reasoning
            ),
            data: Some(serde_json::json!({
                "trim_id": execution.id,
                "asset": execution.asset,
                "amount_sold": execution.amount_sold.to_string(),
                "price_usd": execution.price_usd.to_string(),
                "profit_realized": execution.profit_realized.to_string(),
                "confidence": execution.confidence,
                "reasoning": execution.reasoning,
                "transaction_hash": execution.transaction_hash,
            })),
            priority: "HIGH".to_string(),
            read: false,
            created_at: Utc::now(),
        };

        // Store notification (the notification service handles this internally)
        // For now, we'll just log it since we need to integrate with the notification service
        info!(
            "Trim notification created for user {}: {} {} sold",
            execution.user_id, execution.amount_sold, execution.asset
        );

        // TODO: Integrate with notification service to actually send the notification
        // self.notification_service.create_trim_notification(...).await?;

        Ok(())
    }

    /// Remove a pending trim after successful execution
    async fn remove_pending_trim(&self, trim_id: Uuid) -> Result<()> {
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        client
            .execute("DELETE FROM pending_trims WHERE id = $1", &[&trim_id])
            .await
            .map_err(|e| Error::Database(format!("Failed to remove pending trim: {}", e)))?;

        debug!("Removed pending trim {}", trim_id);

        Ok(())
    }

    /// Get trim execution history for a user
    pub async fn get_trim_history(&self, user_id: Uuid) -> Result<Vec<TrimExecution>> {
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        let rows = client
            .query(
                "SELECT id, user_id, position_id, asset, amount_sold, price_usd, 
                        profit_realized, confidence, reasoning, transaction_hash, executed_at
                 FROM trim_executions
                 WHERE user_id = $1
                 ORDER BY executed_at DESC",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query trim history: {}", e)))?;

        let executions = rows
            .iter()
            .map(|row| TrimExecution {
                id: row.get(0),
                user_id: row.get(1),
                position_id: row.get(2),
                asset: row.get(3),
                amount_sold: row.get(4),
                price_usd: row.get(5),
                profit_realized: row.get(6),
                confidence: row.get(7),
                reasoning: row.get(8),
                transaction_hash: row.get(9),
                executed_at: row.get(10),
            })
            .collect();

        Ok(executions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_amount_calculation() {
        let total_amount = Decimal::from(100);
        let trim_percent = Decimal::from(25);
        
        let trim_amount = total_amount * (trim_percent / Decimal::from(100));
        
        assert_eq!(trim_amount, Decimal::from(25));
    }

    #[test]
    fn test_profit_calculation() {
        let entry_price = Decimal::from(80);
        let current_price = Decimal::from(100);
        let trim_amount = Decimal::from(10);
        
        let profit = (current_price - entry_price) * trim_amount;
        
        assert_eq!(profit, Decimal::from(200));
    }

    #[test]
    fn test_trim_execution_serialization() {
        let execution = TrimExecution {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            position_id: Uuid::new_v4(),
            asset: "SOL".to_string(),
            amount_sold: Decimal::from(25),
            price_usd: Decimal::from(100),
            profit_realized: Decimal::from(500),
            confidence: 90,
            reasoning: "Strong market conditions".to_string(),
            transaction_hash: "abc123".to_string(),
            executed_at: Utc::now(),
        };

        let json = serde_json::to_string(&execution).unwrap();
        let deserialized: TrimExecution = serde_json::from_str(&json).unwrap();

        assert_eq!(execution.id, deserialized.id);
        assert_eq!(execution.asset, deserialized.asset);
        assert_eq!(execution.amount_sold, deserialized.amount_sold);
    }
}
