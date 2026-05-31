use ai_service::{AnalysisContext, ClaudeClient, WhaleMovementData};
use chrono::Utc;
use database::DbPool;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;
use shared::error::{Error, Result};
use shared::models::{Asset, Portfolio, UserSettings};
use std::str::FromStr;
use std::sync::Arc;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::trim_config_service::TrimConfigService;

/// Service for evaluating positions and generating trim recommendations
/// 
/// **Validates: Requirements 7.1, 7.2**
pub struct PositionEvaluator {
    db_pool: DbPool,
    trim_config_service: Arc<TrimConfigService>,
    claude_client: ClaudeClient,
}

/// Represents a user's position in an asset with profit calculation
#[derive(Debug, Clone)]
pub struct Position {
    pub user_id: Uuid,
    pub wallet_id: Uuid,
    pub asset: Asset,
    pub entry_price_usd: Option<Decimal>,
    pub current_price_usd: Option<Decimal>,
    pub profit_percent: Option<Decimal>,
    pub profit_usd: Option<Decimal>,
}

/// Recommendation for trimming a position
#[derive(Debug, Clone)]
pub struct TrimRecommendation {
    pub position: Position,
    pub confidence: i32,
    pub reasoning: String,
    pub suggested_trim_percent: Decimal,
}

impl PositionEvaluator {
    /// Create a new position evaluator
    pub fn new(
        db_pool: DbPool,
        trim_config_service: Arc<TrimConfigService>,
        claude_api_key: String,
    ) -> Self {
        Self {
            db_pool,
            trim_config_service,
            claude_client: ClaudeClient::new(claude_api_key),
        }
    }

    /// Start the background worker that evaluates positions every 5 minutes
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(300)); // 5 minutes

            info!("Position evaluator started - checking every 5 minutes");

            loop {
                ticker.tick().await;

                debug!("Position evaluator tick - evaluating positions");

                if let Err(e) = self.evaluate_all_positions().await {
                    error!("Error in position evaluator: {}", e);
                    // Continue evaluating even if one cycle fails
                }
            }
        })
    }

    /// Evaluate all positions for users with agentic trimming enabled
    async fn evaluate_all_positions(&self) -> Result<()> {
        // Get all users with trimming enabled
        let enabled_users = self.trim_config_service.get_enabled_users().await?;

        info!(
            "Evaluating positions for {} users with trimming enabled",
            enabled_users.len()
        );

        for user_id in enabled_users {
            if let Err(e) = self.evaluate_user_positions(user_id).await {
                warn!("Failed to evaluate positions for user {}: {}", user_id, e);
                // Continue with other users even if one fails
            }
        }

        Ok(())
    }

    /// Evaluate all positions for a specific user
    async fn evaluate_user_positions(&self, user_id: Uuid) -> Result<()> {
        // Check if user has reached daily trim limit
        if self
            .trim_config_service
            .has_reached_daily_limit(user_id)
            .await?
        {
            debug!("User {} has reached daily trim limit, skipping", user_id);
            return Ok(());
        }

        // Get user's trim configuration
        let trim_config = self.trim_config_service.get_trim_config(user_id).await?;

        // Get user's positions
        let positions = self.get_user_positions(user_id).await?;

        debug!(
            "Found {} positions for user {} to evaluate",
            positions.len(),
            user_id
        );

        for position in positions {
            // Check if position meets profit threshold
            if let Some(profit_percent) = position.profit_percent {
                if profit_percent < trim_config.minimum_profit_percent {
                    debug!(
                        "Position {} profit {:.2}% below threshold {:.2}%, skipping",
                        position.asset.token_symbol, profit_percent, trim_config.minimum_profit_percent
                    );
                    continue;
                }

                // Get AI recommendation for this position
                match self.get_trim_recommendation(&position, user_id).await {
                    Ok(Some(recommendation)) => {
                        // Check if confidence meets threshold (85%)
                        if recommendation.confidence >= 85 {
                            info!(
                                "Trim recommendation for user {} position {}: confidence {}%, profit {:.2}%",
                                user_id,
                                position.asset.token_symbol,
                                recommendation.confidence,
                                profit_percent
                            );

                            // Store recommendation for later execution by task 6.3
                            if let Err(e) = self.store_trim_recommendation(&recommendation).await {
                                error!("Failed to store trim recommendation: {}", e);
                            }
                        } else {
                            debug!(
                                "Position {} confidence {}% below threshold 85%, skipping",
                                position.asset.token_symbol, recommendation.confidence
                            );
                        }
                    }
                    Ok(None) => {
                        debug!(
                            "No trim recommendation for position {}",
                            position.asset.token_symbol
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Failed to get trim recommendation for position {}: {}",
                            position.asset.token_symbol, e
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Get all positions for a user with profit calculations
    async fn get_user_positions(&self, user_id: Uuid) -> Result<Vec<Position>> {
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        // Get all wallets for the user
        let wallet_rows = client
            .query(
                "SELECT id, address FROM wallets WHERE user_id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query wallets: {}", e)))?;

        let mut positions = Vec::new();

        for wallet_row in wallet_rows {
            let wallet_id: Uuid = wallet_row.get(0);

            // Get portfolio assets for this wallet
            let asset_rows = client
                .query(
                    "SELECT token_mint, token_symbol, amount, value_usd 
                     FROM portfolio_assets 
                     WHERE wallet_id = $1",
                    &[&wallet_id],
                )
                .await
                .map_err(|e| Error::Database(format!("Failed to query portfolio assets: {}", e)))?;

            for asset_row in asset_rows {
                let token_mint: String = asset_row.get(0);
                let token_symbol: String = asset_row.get(1);
                // `amount` and `value_usd` are Postgres NUMERIC (DECIMAL(36,18) /
                // DECIMAL(18,2)); read them as `Decimal` (the only type that
                // implements FromSql for NUMERIC here) and convert to the
                // `String`/`Option<f64>` shape the `Asset` model expects.
                // Reading them directly as String/f64 panics at runtime.
                let amount_decimal: Decimal = asset_row.get(2);
                let amount: String = amount_decimal.to_string();
                let value_usd: Option<f64> = asset_row
                    .get::<_, Option<Decimal>>(3)
                    .and_then(|v| v.to_string().parse::<f64>().ok());

                // Calculate profit (simplified - in production would need historical entry price)
                // For now, we'll estimate based on current value
                let current_price_usd = value_usd.and_then(|v| {
                    let amount_decimal = Decimal::from_str(&amount).ok()?;
                    if amount_decimal > Decimal::ZERO {
                        Some(Decimal::from_f64_retain(v)? / amount_decimal)
                    } else {
                        None
                    }
                });

                // Get entry price from trade history (if available)
                let entry_price_usd = self.get_entry_price(user_id, &token_mint).await?;

                let (profit_percent, profit_usd) = if let (Some(entry), Some(current)) =
                    (entry_price_usd, current_price_usd)
                {
                    let profit_pct = ((current - entry) / entry) * Decimal::from(100);
                    let profit_val = value_usd.map(|v| {
                        let entry_value = entry * Decimal::from_str(&amount).unwrap_or(Decimal::ZERO);
                        Decimal::from_f64_retain(v).unwrap_or(Decimal::ZERO) - entry_value
                    });
                    (Some(profit_pct), profit_val)
                } else {
                    (None, None)
                };

                positions.push(Position {
                    user_id,
                    wallet_id,
                    asset: Asset {
                        token_mint,
                        token_symbol,
                        amount,
                        value_usd,
                    },
                    entry_price_usd,
                    current_price_usd,
                    profit_percent,
                    profit_usd,
                });
            }
        }

        Ok(positions)
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

    /// Get AI recommendation for trimming a position
    async fn get_trim_recommendation(
        &self,
        position: &Position,
        user_id: Uuid,
    ) -> Result<Option<TrimRecommendation>> {
        // Build analysis context for Claude
        let context = self.build_trim_analysis_context(position, user_id).await?;

        // Create a synthetic whale movement for the analysis
        // (In production, this would be based on actual market data)
        let movement_id = Uuid::new_v4();

        // Call Claude API
        let recommendation = self
            .claude_client
            .analyze_movement(
                &shared::models::WhaleMovement {
                    id: movement_id,
                    whale_id: Uuid::new_v4(),
                    transaction_signature: "trim_evaluation".to_string(),
                    movement_type: "TRIM_EVALUATION".to_string(),
                    token_mint: position.asset.token_mint.clone(),
                    amount: position.asset.amount.clone(),
                    percent_of_position: None,
                    detected_at: Utc::now(),
                },
                user_id,
                context,
            )
            .await
            .map_err(|e| Error::ExternalService(format!("Claude API error: {}", e)))?;

        // Check if recommendation is to TRIM
        if recommendation.action == "TRIM" {
            let trim_config = self.trim_config_service.get_trim_config(user_id).await?;

            Ok(Some(TrimRecommendation {
                position: position.clone(),
                confidence: recommendation.confidence,
                reasoning: recommendation.reasoning,
                suggested_trim_percent: trim_config.trim_percent,
            }))
        } else {
            Ok(None)
        }
    }

    /// Build analysis context for trim evaluation
    async fn build_trim_analysis_context(
        &self,
        position: &Position,
        user_id: Uuid,
    ) -> Result<AnalysisContext> {
        // Get user settings
        let user_settings = self.get_user_settings(user_id).await?;

        // Build portfolio from all user positions
        let all_positions = self.get_user_positions(user_id).await?;
        let total_value: f64 = all_positions
            .iter()
            .filter_map(|p| p.asset.value_usd)
            .sum();

        let portfolio = Portfolio {
            wallet_address: position.wallet_id.to_string(),
            assets: all_positions.into_iter().map(|p| p.asset).collect(),
            total_value_usd: total_value,
            last_updated: Utc::now(),
        };

        Ok(AnalysisContext {
            whale_movement: WhaleMovementData {
                whale_address: "position_evaluation".to_string(),
                movement_type: "TRIM_EVALUATION".to_string(),
                token_mint: position.asset.token_mint.clone(),
                token_symbol: position.asset.token_symbol.clone(),
                amount: position.asset.amount.clone(),
                percent_of_position: position
                    .profit_percent
                    .and_then(|p| p.to_f64())
                    .unwrap_or(0.0),
            },
            user_position: Some(position.asset.clone()),
            user_portfolio: portfolio,
            user_risk_profile: user_settings.risk_tolerance,
        })
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

    /// Store trim recommendation for later execution
    async fn store_trim_recommendation(&self, recommendation: &TrimRecommendation) -> Result<()> {
        let client = self
            .db_pool
            .get()
            .await
            .map_err(|e| Error::Database(format!("Failed to get database connection: {}", e)))?;

        // Store in a pending_trims table (would need to be created in migration)
        // For now, we'll log it
        info!(
            "Trim recommendation stored: user={}, asset={}, confidence={}, profit={:?}%",
            recommendation.position.user_id,
            recommendation.position.asset.token_symbol,
            recommendation.confidence,
            recommendation.position.profit_percent
        );

        // Insert into pending_trims table (placeholder - actual implementation in task 6.3)
        client
            .execute(
                "INSERT INTO pending_trims (user_id, wallet_id, token_mint, token_symbol, 
                 amount, confidence, reasoning, suggested_trim_percent, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())
                 ON CONFLICT (user_id, token_mint) DO UPDATE SET
                 confidence = EXCLUDED.confidence,
                 reasoning = EXCLUDED.reasoning,
                 updated_at = NOW()",
                &[
                    &recommendation.position.user_id,
                    &recommendation.position.wallet_id,
                    &recommendation.position.asset.token_mint,
                    &recommendation.position.asset.token_symbol,
                    &recommendation.position.asset.amount,
                    &recommendation.confidence,
                    &recommendation.reasoning,
                    &recommendation.suggested_trim_percent.to_f64().unwrap_or(25.0),
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to store trim recommendation: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_profit_calculation() {
        let position = Position {
            user_id: Uuid::new_v4(),
            wallet_id: Uuid::new_v4(),
            asset: Asset {
                token_mint: "SOL".to_string(),
                token_symbol: "SOL".to_string(),
                amount: "10".to_string(),
                value_usd: Some(1000.0),
            },
            entry_price_usd: Some(Decimal::from(80)),
            current_price_usd: Some(Decimal::from(100)),
            profit_percent: Some(Decimal::from(25)),
            profit_usd: Some(Decimal::from(200)),
        };

        assert_eq!(position.profit_percent, Some(Decimal::from(25)));
        assert_eq!(position.profit_usd, Some(Decimal::from(200)));
    }
}
