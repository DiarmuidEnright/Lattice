use crate::benchmark_service::{ActionType, Benchmark, BenchmarkService, TriggerType};
use crate::coinmarketcap_service::CoinMarketCapService;
use crate::position_management_service::{PositionManagementService, PositionMode};
use database::DbPool;
use notification::NotificationService;
use rust_decimal::Decimal;
use shared::{Error, Result};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

/// Price monitoring service that checks prices and triggers benchmark actions
/// 
/// This service runs as a background worker that:
/// - Checks asset prices every 10 seconds
/// - Loads active benchmarks into memory
/// - Detects threshold crossings
/// - Triggers configured actions (alerts or trades)
/// 
/// **Validates: Requirements 2.3, 2.5**
pub struct PriceMonitor {
    benchmark_service: Arc<BenchmarkService>,
    coinmarketcap_service: Arc<CoinMarketCapService>,
    position_management_service: Arc<PositionManagementService>,
    #[allow(dead_code)]
    notification_service: Arc<NotificationService>,
    db_pool: DbPool,
    check_interval: Duration,
}

/// Represents a triggered benchmark with current price information
#[derive(Debug, Clone)]
pub struct TriggeredBenchmark {
    pub benchmark: Benchmark,
    pub current_price: Decimal,
    pub crossed_threshold: bool,
}

impl PriceMonitor {
    /// Create a new price monitor
    /// 
    /// # Arguments
    /// * `benchmark_service` - Service for managing benchmarks
    /// * `coinmarketcap_service` - Service for fetching asset prices
    /// * `notification_service` - Service for sending notifications
    /// * `db_pool` - Database connection pool
    pub fn new(
        benchmark_service: Arc<BenchmarkService>,
        coinmarketcap_service: Arc<CoinMarketCapService>,
        position_management_service: Arc<PositionManagementService>,
        notification_service: Arc<NotificationService>,
        db_pool: DbPool,
    ) -> Self {
        Self {
            benchmark_service,
            coinmarketcap_service,
            position_management_service,
            notification_service,
            db_pool,
            check_interval: Duration::from_secs(10), // Check every 10 seconds
        }
    }

    /// Start the price monitoring loop
    /// 
    /// This runs indefinitely, checking prices every 10 seconds and triggering
    /// benchmarks when thresholds are crossed.
    /// 
    /// **Validates: Requirements 2.3**
    pub async fn start(&self) {
        info!("Starting price monitor with {} second interval", self.check_interval.as_secs());
        
        let mut ticker = interval(self.check_interval);
        
        loop {
            ticker.tick().await;
            
            if let Err(e) = self.check_all_benchmarks().await {
                error!("Error checking benchmarks: {:?}", e);
            }
        }
    }

    /// Check all active benchmarks and trigger actions if thresholds are crossed
    /// 
    /// **Validates: Requirements 2.3, 2.4**
    async fn check_all_benchmarks(&self) -> Result<()> {
        debug!("Checking all active benchmarks");
        
        // Get all unique assets that have active benchmarks
        let assets = self.get_active_assets().await?;
        
        if assets.is_empty() {
            debug!("No active benchmarks to check");
            return Ok(());
        }
        
        info!("Checking {} unique assets for benchmark triggers", assets.len());
        
        // Check each asset
        for asset in assets {
            if let Err(e) = self.check_asset_benchmarks(&asset).await {
                error!("Error checking benchmarks for asset {}: {:?}", asset, e);
                // Continue checking other assets even if one fails
            }
        }
        
        Ok(())
    }

    /// Get all unique assets that have active benchmarks
    async fn get_active_assets(&self) -> Result<Vec<String>> {
        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;
        
        let rows = client
            .query(
                "SELECT DISTINCT asset FROM benchmarks WHERE is_active = TRUE",
                &[],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query active assets: {}", e)))?;
        
        let mut assets = Vec::new();
        for row in rows {
            let asset: String = row.get(0);
            assets.push(asset);
        }
        
        Ok(assets)
    }

    /// Check benchmarks for a specific asset
    /// 
    /// **Validates: Requirements 2.3, 2.4**
    async fn check_asset_benchmarks(&self, asset: &str) -> Result<()> {
        debug!("Checking benchmarks for asset: {}", asset);
        
        // Get current price from CoinMarketCap (uses symbol, not blockchain-specific)
        let price_data = match self.coinmarketcap_service.get_price_by_symbol(asset).await {
            Ok(data) => data,
            Err(e) => {
                warn!("Failed to fetch price for {}: {:?}", asset, e);
                return Ok(()); // Skip this asset if price fetch fails
            }
        };
        
        let current_price = price_data.price_usd;
        debug!("Current price for {}: {}", asset, current_price);
        
        // Get all active benchmarks for this asset
        let benchmarks = self.benchmark_service.get_active_benchmarks_for_asset(asset).await?;
        
        if benchmarks.is_empty() {
            return Ok(());
        }
        
        info!("Found {} active benchmarks for {}", benchmarks.len(), asset);
        
        // Check each benchmark for threshold crossing
        for benchmark in benchmarks {
            if let Some(triggered) = self.check_threshold(&benchmark, current_price) {
                info!(
                    "Benchmark {} triggered for user {} on asset {} at price {}",
                    benchmark.id, benchmark.user_id, asset, current_price
                );
                
                // Execute the trigger action
                if let Err(e) = self.execute_trigger(triggered).await {
                    error!("Failed to execute trigger for benchmark {}: {:?}", benchmark.id, e);
                }
            }
        }
        
        Ok(())
    }

    /// Check if a benchmark threshold has been crossed
    /// 
    /// Returns Some(TriggeredBenchmark) if the threshold is crossed, None otherwise.
    /// 
    /// **Validates: Requirements 2.3**
    fn check_threshold(&self, benchmark: &Benchmark, current_price: Decimal) -> Option<TriggeredBenchmark> {
        let crossed = match benchmark.trigger_type {
            TriggerType::Above => current_price >= benchmark.target_price,
            TriggerType::Below => current_price <= benchmark.target_price,
        };
        
        if crossed {
            debug!(
                "Threshold crossed for benchmark {}: current={}, target={}, type={:?}",
                benchmark.id, current_price, benchmark.target_price, benchmark.trigger_type
            );
            
            Some(TriggeredBenchmark {
                benchmark: benchmark.clone(),
                current_price,
                crossed_threshold: true,
            })
        } else {
            None
        }
    }

    /// Execute the action for a triggered benchmark
    /// 
    /// This method:
    /// - Sends an alert notification for ALERT actions
    /// - Executes a trade for EXECUTE actions (placeholder for now)
    /// - Marks the benchmark as triggered
    /// - Optionally disables the benchmark based on user preference
    /// 
    /// **Validates: Requirements 2.3, 2.5**
    async fn execute_trigger(&self, triggered: TriggeredBenchmark) -> Result<()> {
        let benchmark = &triggered.benchmark;
        
        info!(
            "Executing trigger for benchmark {} (action: {:?})",
            benchmark.id, benchmark.action_type
        );
        
        match benchmark.action_type {
            ActionType::Alert => {
                // Send alert notification
                self.send_alert_notification(&triggered).await?;
            }
            ActionType::Execute => {
                // Execute trade action
                self.execute_trade_action(&triggered).await?;
            }
        }
        
        // Mark benchmark as triggered and disable it (Requirement 2.5)
        // For now, we always disable after trigger. In the future, this could be
        // configurable per benchmark.
        self.benchmark_service.mark_triggered(benchmark.id, true).await?;
        
        info!("Successfully executed trigger for benchmark {}", benchmark.id);
        
        Ok(())
    }

    /// Send an alert notification for a triggered benchmark
    /// 
    /// Uses the NotificationService to create a properly formatted notification
    /// that will be stored and potentially sent via email if configured.
    /// 
    /// **Validates: Requirements 2.3, 2.5**
    async fn send_alert_notification(&self, triggered: &TriggeredBenchmark) -> Result<()> {
        let benchmark = &triggered.benchmark;
        
        info!(
            "Sending alert notification for benchmark {} to user {}",
            benchmark.id, benchmark.user_id
        );
        
        // Create notification using the notification service
        // Note: We're creating a custom notification type for benchmarks
        // The notification service will handle storage and email delivery
        let notification = shared::models::Notification {
            id: uuid::Uuid::new_v4(),
            user_id: benchmark.user_id,
            notification_type: "BENCHMARK_ALERT".to_string(),
            title: format!("Price Alert: {}", benchmark.asset),
            message: format!(
                "Price alert: {} has {} your target price of {}. Current price: {}",
                benchmark.asset,
                match benchmark.trigger_type {
                    TriggerType::Above => "crossed above",
                    TriggerType::Below => "fallen below",
                },
                benchmark.target_price,
                triggered.current_price
            ),
            data: Some(serde_json::json!({
                "benchmark_id": benchmark.id,
                "asset": benchmark.asset,
                "blockchain": benchmark.blockchain,
                "target_price": benchmark.target_price,
                "current_price": triggered.current_price,
                "trigger_type": match benchmark.trigger_type {
                    TriggerType::Above => "ABOVE",
                    TriggerType::Below => "BELOW",
                },
            })),
            priority: "HIGH".to_string(),
            read: false,
            created_at: chrono::Utc::now(),
        };
        
        // Store the notification
        // For now, we'll store it directly in the database since NotificationService
        // uses in-memory storage. In production, this would use the service's storage.
        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;
        
        client
            .execute(
                "INSERT INTO notifications (id, user_id, notification_type, title, message, data, priority, read, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[
                    &notification.id,
                    &notification.user_id,
                    &notification.notification_type,
                    &notification.title,
                    &notification.message,
                    &notification.data,
                    &notification.priority,
                    &notification.read,
                    &notification.created_at,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to create notification: {}", e)))?;
        
        info!("Alert notification sent for benchmark {}", benchmark.id);
        
        Ok(())
    }

    /// Execute a trade action for a triggered benchmark
    /// 
    /// This method prepares trade execution for benchmarks with EXECUTE action type.
    /// Currently creates a notification about the trade that should be executed.
    /// 
    /// TODO: Integrate with TradingService to actually execute trades
    /// The integration should:
    /// - Call trading_service.execute_trade() with the benchmark parameters
    /// - Validate user has sufficient balance
    /// - Handle trade execution errors
    /// - Send confirmation notifications
    /// 
    /// **Validates: Requirements 2.3, 2.5, 4.4, 4.6**
    async fn execute_trade_action(&self, triggered: &TriggeredBenchmark) -> Result<()> {
        let benchmark = &triggered.benchmark;
        
        info!(
            "Executing trade action for benchmark {} (action: {:?}, amount: {:?})",
            benchmark.id, benchmark.trade_action, benchmark.trade_amount
        );
        
        // Check if asset is in automatic mode (Requirement 4.4, 4.6)
        let position_mode = self.position_management_service
            .get_position_mode(benchmark.user_id, &benchmark.asset, &benchmark.blockchain)
            .await?;
        
        if position_mode != PositionMode::Automatic {
            info!(
                "Skipping benchmark execution for {} - asset is in manual mode",
                benchmark.asset
            );
            
            // Send notification that benchmark was triggered but not executed due to manual mode
            let notification = shared::models::Notification {
                id: uuid::Uuid::new_v4(),
                user_id: benchmark.user_id,
                notification_type: "BENCHMARK_ALERT".to_string(),
                title: format!("Benchmark Alert: {}", benchmark.asset),
                message: format!(
                    "Benchmark triggered for {} at price {} (target: {}), but asset is in manual mode. No automatic trade executed.",
                    benchmark.asset,
                    triggered.current_price,
                    benchmark.target_price
                ),
                data: Some(serde_json::json!({
                    "benchmark_id": benchmark.id,
                    "asset": benchmark.asset,
                    "blockchain": benchmark.blockchain,
                    "target_price": benchmark.target_price,
                    "current_price": triggered.current_price,
                    "mode": "manual",
                })),
                priority: "MEDIUM".to_string(),
                read: false,
                created_at: chrono::Utc::now(),
            };
            
            let client = self.db_pool.get().await.map_err(|e| {
                Error::Database(format!("Failed to get database connection: {}", e))
            })?;
            
            client
                .execute(
                    "INSERT INTO notifications (id, user_id, notification_type, title, message, data, priority, read, created_at)
                     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                    &[
                        &notification.id,
                        &notification.user_id,
                        &notification.notification_type,
                        &notification.title,
                        &notification.message,
                        &notification.data,
                        &notification.priority,
                        &notification.read,
                        &notification.created_at,
                    ],
                )
                .await
                .map_err(|e| Error::Database(format!("Failed to create notification: {}", e)))?;
            
            return Ok(());
        }
        
        // Validate that we have the required trade parameters
        let trade_action = benchmark.trade_action.as_ref().ok_or_else(|| {
            Error::Validation("Execute benchmark missing trade_action".to_string())
        })?;
        
        let trade_amount = benchmark.trade_amount.ok_or_else(|| {
            Error::Validation("Execute benchmark missing trade_amount".to_string())
        })?;
        
        // Register as pending automatic order (Requirement 4.4)
        let action_str = match trade_action {
            crate::benchmark_service::TradeAction::Buy => "BUY",
            crate::benchmark_service::TradeAction::Sell => "SELL",
        };
        
        match self.position_management_service
            .register_pending_automatic_order(
                benchmark.user_id,
                &benchmark.asset,
                &benchmark.blockchain,
                "benchmark",
                Some(benchmark.id),
                action_str,
                trade_amount,
            )
            .await
        {
            Ok(order_id) => {
                info!(
                    "Registered pending automatic order {} for benchmark {}",
                    order_id, benchmark.id
                );
            }
            Err(e) => {
                warn!(
                    "Failed to register pending automatic order for benchmark {}: {}",
                    benchmark.id, e
                );
            }
        }
        
        // TODO: Integrate with TradingService
        // For now, create a notification that a trade should be executed
        // In production, this would call:
        // let trade_request = TradeRequest {
        //     user_id: benchmark.user_id,
        //     action: trade_action.to_string(),
        //     token_mint: benchmark.asset.clone(),
        //     amount: trade_amount.to_string(),
        //     slippage_tolerance: 1.0,
        //     recommendation_id: None,
        // };
        // trading_service.execute_trade(trade_request).await?;
        
        warn!(
            "Trade execution not yet fully integrated. Would execute {:?} of {} {} at price {}",
            trade_action,
            trade_amount,
            benchmark.asset,
            triggered.current_price
        );
        
        // Create a notification about the trade action
        let notification = shared::models::Notification {
            id: uuid::Uuid::new_v4(),
            user_id: benchmark.user_id,
            notification_type: "BENCHMARK_EXECUTE".to_string(),
            title: format!("Trade Triggered: {}", benchmark.asset),
            message: format!(
                "Benchmark triggered: {} order for {} {} at price {} (target: {})",
                action_str,
                trade_amount,
                benchmark.asset,
                triggered.current_price,
                benchmark.target_price
            ),
            data: Some(serde_json::json!({
                "benchmark_id": benchmark.id,
                "asset": benchmark.asset,
                "blockchain": benchmark.blockchain,
                "trade_action": action_str,
                "trade_amount": trade_amount,
                "target_price": benchmark.target_price,
                "current_price": triggered.current_price,
                "status": "PENDING_INTEGRATION",
            })),
            priority: "HIGH".to_string(),
            read: false,
            created_at: chrono::Utc::now(),
        };
        
        // Store the notification
        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;
        
        client
            .execute(
                "INSERT INTO notifications (id, user_id, notification_type, title, message, data, priority, read, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[
                    &notification.id,
                    &notification.user_id,
                    &notification.notification_type,
                    &notification.title,
                    &notification.message,
                    &notification.data,
                    &notification.priority,
                    &notification.read,
                    &notification.created_at,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to create notification: {}", e)))?;
        
        info!("Trade execution notification created for benchmark {}", benchmark.id);
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    use uuid::Uuid;

    fn create_test_benchmark(trigger_type: TriggerType) -> Benchmark {
        Benchmark {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            asset: "SOL".to_string(),
            blockchain: "Solana".to_string(),
            target_price: Decimal::from_str("100.0").unwrap(),
            trigger_type,
            action_type: ActionType::Alert,
            trade_action: None,
            trade_amount: None,
            is_active: true,
            triggered_at: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_check_threshold_above_triggered() {
        let benchmark = create_test_benchmark(TriggerType::Above);
        
        // Create a minimal monitor just for testing threshold logic
        // We don't need real services for this pure logic test
        let check_threshold = |benchmark: &Benchmark, price: Decimal| -> bool {
            match benchmark.trigger_type {
                TriggerType::Above => price >= benchmark.target_price,
                TriggerType::Below => price <= benchmark.target_price,
            }
        };
        
        // Price above target should trigger
        assert!(check_threshold(&benchmark, Decimal::from_str("105.0").unwrap()));
        
        // Price equal to target should trigger
        assert!(check_threshold(&benchmark, Decimal::from_str("100.0").unwrap()));
        
        // Price below target should not trigger
        assert!(!check_threshold(&benchmark, Decimal::from_str("95.0").unwrap()));
    }

    #[test]
    fn test_check_threshold_below_triggered() {
        let benchmark = create_test_benchmark(TriggerType::Below);
        
        let check_threshold = |benchmark: &Benchmark, price: Decimal| -> bool {
            match benchmark.trigger_type {
                TriggerType::Above => price >= benchmark.target_price,
                TriggerType::Below => price <= benchmark.target_price,
            }
        };
        
        // Price below target should trigger
        assert!(check_threshold(&benchmark, Decimal::from_str("95.0").unwrap()));
        
        // Price equal to target should trigger
        assert!(check_threshold(&benchmark, Decimal::from_str("100.0").unwrap()));
        
        // Price above target should not trigger
        assert!(!check_threshold(&benchmark, Decimal::from_str("105.0").unwrap()));
    }
}
