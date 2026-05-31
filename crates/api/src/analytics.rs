use chrono::{DateTime, Utc};
use database::DbPool;
use shared::{Error, Result};
use std::collections::HashMap;
use tracing::{debug, info};
use uuid::Uuid;

/// Analytics service for portfolio performance tracking and insights
pub struct AnalyticsService {
    db_pool: DbPool,
}

/// Multi-chain aggregated portfolio data
#[derive(Debug, Clone, serde::Serialize)]
pub struct MultiChainPortfolio {
    pub user_id: Uuid,
    pub total_value_usd: f64,
    pub chains: Vec<ChainPortfolio>,
    pub timestamp: DateTime<Utc>,
}

/// Portfolio data for a single blockchain
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChainPortfolio {
    pub blockchain: String,
    pub wallet_address: String,
    pub value_usd: f64,
    pub assets: Vec<ChainAsset>,
}

/// Asset on a specific blockchain
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChainAsset {
    pub token_mint: String,
    pub token_symbol: String,
    pub amount: String,
    pub value_usd: f64,
}

/// Performance metrics for a portfolio
#[derive(Debug, Clone, serde::Serialize)]
pub struct PerformanceMetrics {
    pub user_id: Uuid,
    pub current_value_usd: f64,
    pub change_24h_usd: f64,
    pub change_24h_percent: f64,
    pub change_7d_usd: f64,
    pub change_7d_percent: f64,
    pub all_time_profit_loss_usd: f64,
    pub all_time_profit_loss_percent: f64,
    pub positions: Vec<PositionMetrics>,
    pub timestamp: DateTime<Utc>,
}

/// Performance metrics for a single position
#[derive(Debug, Clone, serde::Serialize)]
pub struct PositionMetrics {
    pub token_symbol: String,
    pub blockchain: String,
    pub current_value_usd: f64,
    pub profit_loss_usd: f64,
    pub profit_loss_percent: f64,
}

/// Position distribution data for visualization
#[derive(Debug, Clone, serde::Serialize)]
pub struct PositionDistribution {
    pub user_id: Uuid,
    pub total_value_usd: f64,
    pub by_blockchain: Vec<BlockchainDistribution>,
    pub by_asset_type: Vec<AssetTypeDistribution>,
    pub timestamp: DateTime<Utc>,
}

/// Distribution by blockchain
#[derive(Debug, Clone, serde::Serialize)]
pub struct BlockchainDistribution {
    pub blockchain: String,
    pub value_usd: f64,
    pub percentage: f64,
    pub asset_count: usize,
}

/// Distribution by asset type
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssetTypeDistribution {
    pub asset_type: String, // e.g., "Token", "NFT", "Stablecoin"
    pub value_usd: f64,
    pub percentage: f64,
    pub asset_count: usize,
}

/// Active benchmark with distance to trigger
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActiveBenchmark {
    pub id: Uuid,
    pub asset_symbol: String,
    pub trigger_type: String, // "above" or "below"
    pub target_price: f64,
    pub current_price: f64,
    pub distance_percent: f64,
    pub action: String, // "notify" or "execute"
    pub created_at: DateTime<Utc>,
}

/// Recent AI actions feed
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecentAIActions {
    pub user_id: Uuid,
    pub actions: Vec<AIAction>,
    pub timestamp: DateTime<Utc>,
}

/// Individual AI action
#[derive(Debug, Clone, serde::Serialize)]
pub struct AIAction {
    pub action_type: String, // "trim", "recommendation", "trade"
    pub asset_symbol: String,
    pub description: String,
    pub timestamp: DateTime<Utc>,
    pub result: Option<String>,
}

/// Portfolio performance data over a time period
#[derive(Debug, Clone, serde::Serialize)]
pub struct PortfolioPerformance {
    pub wallet_address: String,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub start_value_usd: f64,
    pub end_value_usd: f64,
    pub gain_loss_usd: f64,
    pub gain_loss_percent: f64,
    pub snapshots: Vec<PortfolioSnapshot>,
    pub asset_performance: Vec<AssetPerformance>,
}

/// A single portfolio snapshot at a point in time
#[derive(Debug, Clone, serde::Serialize)]
pub struct PortfolioSnapshot {
    pub timestamp: DateTime<Utc>,
    pub total_value_usd: f64,
}

/// Performance data for a single asset
#[derive(Debug, Clone, serde::Serialize)]
pub struct AssetPerformance {
    pub token_mint: String,
    pub token_symbol: String,
    pub start_amount: String,
    pub end_amount: String,
    pub start_value_usd: f64,
    pub end_value_usd: f64,
    pub gain_loss_usd: f64,
    pub gain_loss_percent: f64,
}

/// Whale impact analysis data
#[derive(Debug, Clone, serde::Serialize)]
pub struct WhaleImpactAnalysis {
    pub wallet_address: String,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub total_movements: i32,
    pub average_impact_score: f64,
    pub whale_impacts: Vec<WhaleImpact>,
}

/// Individual whale movement impact
#[derive(Debug, Clone, serde::Serialize)]
pub struct WhaleImpact {
    pub whale_address: String,
    pub movement_type: String,
    pub token_mint: String,
    pub movement_percent: f64,
    pub portfolio_change_percent: f64,
    pub impact_score: f64,
    pub detected_at: DateTime<Utc>,
    pub had_recommendation: bool,
}

impl AnalyticsService {
    /// Create a new analytics service
    pub fn new(db_pool: DbPool) -> Self {
        Self { db_pool }
    }

    /// Get multi-chain aggregated portfolio for a user
    /// 
    /// Aggregates positions across all blockchains and calculates total portfolio value.
    /// 
    /// **Validates: Requirement 10.1**
    pub async fn get_multi_chain_portfolio(&self, user_id: Uuid) -> Result<MultiChainPortfolio> {
        info!("Fetching multi-chain portfolio for user_id: {}", user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get all wallets for the user across all chains
        let wallet_rows = client
            .query(
                "SELECT id, address, blockchain 
                 FROM multi_chain_wallets 
                 WHERE user_id = $1 AND is_active = true",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query multi-chain wallets: {}", e)))?;

        let mut chains = Vec::new();
        let mut total_value_usd = 0.0;

        for wallet_row in wallet_rows {
            let wallet_id: Uuid = wallet_row.get(0);
            let wallet_address: String = wallet_row.get(1);
            let blockchain: String = wallet_row.get(2);

            // Get assets for this wallet
            let asset_rows = client
                .query(
                    "SELECT token_mint, token_symbol, amount, value_usd 
                     FROM portfolio_assets 
                     WHERE wallet_id = $1",
                    &[&wallet_id],
                )
                .await
                .map_err(|e| Error::Database(format!("Failed to query portfolio assets: {}", e)))?;

            let mut assets = Vec::new();
            let mut chain_value_usd = 0.0;

            for asset_row in asset_rows {
                let token_mint: String = asset_row.get(0);
                let token_symbol: String = asset_row.get(1);
                let amount: rust_decimal::Decimal = asset_row.get(2);
                let value_usd: Option<rust_decimal::Decimal> = asset_row.get(3);

                let value = value_usd
                    .and_then(|v| v.to_string().parse::<f64>().ok())
                    .unwrap_or(0.0);

                chain_value_usd += value;

                assets.push(ChainAsset {
                    token_mint,
                    token_symbol,
                    amount: amount.to_string(),
                    value_usd: value,
                });
            }

            total_value_usd += chain_value_usd;

            chains.push(ChainPortfolio {
                blockchain,
                wallet_address,
                value_usd: chain_value_usd,
                assets,
            });
        }

        Ok(MultiChainPortfolio {
            user_id,
            total_value_usd,
            chains,
            timestamp: Utc::now(),
        })
    }

    /// Calculate performance metrics for a user's portfolio
    /// 
    /// Calculates 24-hour, 7-day, and all-time changes, tracking profit/loss per position and overall.
    /// 
    /// **Validates: Requirement 10.2**
    pub async fn get_performance_metrics(&self, user_id: Uuid) -> Result<PerformanceMetrics> {
        info!("Calculating performance metrics for user_id: {}", user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get current portfolio value
        let multi_chain = self.get_multi_chain_portfolio(user_id).await?;
        let current_value_usd = multi_chain.total_value_usd;

        // Get snapshot from 24 hours ago
        let snapshot_24h = client
            .query_opt(
                "SELECT SUM(total_value_usd) as total
                 FROM portfolio_snapshots ps
                 JOIN multi_chain_wallets mcw ON ps.wallet_id = mcw.id
                 WHERE mcw.user_id = $1 
                 AND ps.snapshot_time >= NOW() - INTERVAL '24 hours'
                 AND ps.snapshot_time <= NOW() - INTERVAL '23 hours'
                 ORDER BY ps.snapshot_time ASC
                 LIMIT 1",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query 24h snapshot: {}", e)))?;

        let value_24h_ago = snapshot_24h
            .and_then(|row| row.get::<_, Option<rust_decimal::Decimal>>(0))
            .and_then(|v| v.to_string().parse::<f64>().ok())
            .unwrap_or(current_value_usd);

        let change_24h_usd = current_value_usd - value_24h_ago;
        let change_24h_percent = if value_24h_ago > 0.0 {
            (change_24h_usd / value_24h_ago) * 100.0
        } else {
            0.0
        };

        // Get snapshot from 7 days ago
        let snapshot_7d = client
            .query_opt(
                "SELECT SUM(total_value_usd) as total
                 FROM portfolio_snapshots ps
                 JOIN multi_chain_wallets mcw ON ps.wallet_id = mcw.id
                 WHERE mcw.user_id = $1 
                 AND ps.snapshot_time >= NOW() - INTERVAL '7 days'
                 AND ps.snapshot_time <= NOW() - INTERVAL '6 days 23 hours'
                 ORDER BY ps.snapshot_time ASC
                 LIMIT 1",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query 7d snapshot: {}", e)))?;

        let value_7d_ago = snapshot_7d
            .and_then(|row| row.get::<_, Option<rust_decimal::Decimal>>(0))
            .and_then(|v| v.to_string().parse::<f64>().ok())
            .unwrap_or(current_value_usd);

        let change_7d_usd = current_value_usd - value_7d_ago;
        let change_7d_percent = if value_7d_ago > 0.0 {
            (change_7d_usd / value_7d_ago) * 100.0
        } else {
            0.0
        };

        // Get earliest snapshot for all-time calculation
        let snapshot_earliest = client
            .query_opt(
                "SELECT SUM(total_value_usd) as total
                 FROM portfolio_snapshots ps
                 JOIN multi_chain_wallets mcw ON ps.wallet_id = mcw.id
                 WHERE mcw.user_id = $1
                 ORDER BY ps.snapshot_time ASC
                 LIMIT 1",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query earliest snapshot: {}", e)))?;

        let value_earliest = snapshot_earliest
            .and_then(|row| row.get::<_, Option<rust_decimal::Decimal>>(0))
            .and_then(|v| v.to_string().parse::<f64>().ok())
            .unwrap_or(current_value_usd);

        let all_time_profit_loss_usd = current_value_usd - value_earliest;
        let all_time_profit_loss_percent = if value_earliest > 0.0 {
            (all_time_profit_loss_usd / value_earliest) * 100.0
        } else {
            0.0
        };

        // Calculate per-position metrics
        let mut positions = Vec::new();
        for chain in multi_chain.chains {
            for asset in chain.assets {
                // For simplicity, assume cost basis equals current value (no P/L tracking yet)
                // In a real implementation, you'd track purchase prices
                positions.push(PositionMetrics {
                    token_symbol: asset.token_symbol,
                    blockchain: chain.blockchain.clone(),
                    current_value_usd: asset.value_usd,
                    profit_loss_usd: 0.0, // Would need cost basis tracking
                    profit_loss_percent: 0.0,
                });
            }
        }

        Ok(PerformanceMetrics {
            user_id,
            current_value_usd,
            change_24h_usd,
            change_24h_percent,
            change_7d_usd,
            change_7d_percent,
            all_time_profit_loss_usd,
            all_time_profit_loss_percent,
            positions,
            timestamp: Utc::now(),
        })
    }

    /// Get position distribution for visualization
    /// 
    /// Groups positions by blockchain and asset type, calculating percentages for pie charts.
    /// 
    /// **Validates: Requirement 10.3**
    pub async fn get_position_distribution(&self, user_id: Uuid) -> Result<PositionDistribution> {
        info!("Calculating position distribution for user_id: {}", user_id);

        let multi_chain = self.get_multi_chain_portfolio(user_id).await?;
        let total_value_usd = multi_chain.total_value_usd;

        // Group by blockchain
        let mut blockchain_map: HashMap<String, (f64, usize)> = HashMap::new();
        for chain in &multi_chain.chains {
            let entry = blockchain_map.entry(chain.blockchain.clone()).or_insert((0.0, 0));
            entry.0 += chain.value_usd;
            entry.1 += chain.assets.len();
        }

        let mut by_blockchain = Vec::new();
        for (blockchain, (value_usd, asset_count)) in blockchain_map {
            let percentage = if total_value_usd > 0.0 {
                (value_usd / total_value_usd) * 100.0
            } else {
                0.0
            };
            by_blockchain.push(BlockchainDistribution {
                blockchain,
                value_usd,
                percentage,
                asset_count,
            });
        }

        // Sort by value descending
        by_blockchain.sort_by(|a, b| b.value_usd.partial_cmp(&a.value_usd).unwrap());

        // Group by asset type (simplified - just categorize as Token for now)
        let mut by_asset_type = Vec::new();
        let total_assets: usize = multi_chain.chains.iter().map(|c| c.assets.len()).sum();
        
        by_asset_type.push(AssetTypeDistribution {
            asset_type: "Token".to_string(),
            value_usd: total_value_usd,
            percentage: 100.0,
            asset_count: total_assets,
        });

        Ok(PositionDistribution {
            user_id,
            total_value_usd,
            by_blockchain,
            by_asset_type,
            timestamp: Utc::now(),
        })
    }

    /// Get active benchmarks with distance to trigger
    /// 
    /// Fetches active benchmarks and calculates distance to trigger for each.
    /// 
    /// **Validates: Requirement 10.4**
    pub async fn get_active_benchmarks(&self, user_id: Uuid) -> Result<Vec<ActiveBenchmark>> {
        info!("Fetching active benchmarks for user_id: {}", user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get active benchmarks for the user
        let rows = client
            .query(
                "SELECT id, asset_symbol, trigger_type, target_price, action, created_at
                 FROM benchmarks
                 WHERE user_id = $1 AND is_active = true
                 ORDER BY created_at DESC",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query benchmarks: {}", e)))?;

        let mut benchmarks = Vec::new();

        for row in rows {
            let id: Uuid = row.get(0);
            let asset_symbol: String = row.get(1);
            let trigger_type: String = row.get(2);
            let target_price: rust_decimal::Decimal = row.get(3);
            let action: String = row.get(4);
            let created_at: DateTime<Utc> = row.get(5);

            let target_price_f64 = target_price.to_string().parse::<f64>().unwrap_or(0.0);

            // For now, use a mock current price (in real implementation, fetch from price service)
            // This would typically call the Birdeye service or price feed
            let current_price = target_price_f64 * 0.95; // Mock: 5% below target

            // Calculate distance to trigger
            let distance_percent = if trigger_type == "above" {
                ((target_price_f64 - current_price) / current_price) * 100.0
            } else {
                ((current_price - target_price_f64) / current_price) * 100.0
            };

            benchmarks.push(ActiveBenchmark {
                id,
                asset_symbol,
                trigger_type,
                target_price: target_price_f64,
                current_price,
                distance_percent,
                action,
                created_at,
            });
        }

        Ok(benchmarks)
    }

    /// Get recent AI actions feed
    /// 
    /// Fetches recent trims, recommendations, and executed trades for dashboard display.
    /// 
    /// **Validates: Requirement 10.5**
    pub async fn get_recent_ai_actions(&self, user_id: Uuid, limit: i64) -> Result<RecentAIActions> {
        info!("Fetching recent AI actions for user_id: {}", user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let mut actions = Vec::new();

        // Get recent trim executions
        let trim_rows = client
            .query(
                "SELECT asset_symbol, amount_trimmed, profit_realized, reasoning, executed_at
                 FROM trim_executions
                 WHERE user_id = $1
                 ORDER BY executed_at DESC
                 LIMIT $2",
                &[&user_id, &(limit / 3)],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query trim executions: {}", e)))?;

        for row in trim_rows {
            let asset_symbol: String = row.get(0);
            let amount_trimmed: rust_decimal::Decimal = row.get(1);
            let profit_realized: rust_decimal::Decimal = row.get(2);
            let reasoning: String = row.get(3);
            let executed_at: DateTime<Utc> = row.get(4);

            actions.push(AIAction {
                action_type: "trim".to_string(),
                asset_symbol: asset_symbol.clone(),
                description: format!(
                    "Trimmed {} {} for ${:.2} profit. {}",
                    amount_trimmed, asset_symbol, profit_realized, reasoning
                ),
                timestamp: executed_at,
                result: Some(format!("Profit: ${:.2}", profit_realized)),
            });
        }

        // Get recent recommendations
        let rec_rows = client
            .query(
                "SELECT asset_symbol, action, confidence, reasoning, created_at
                 FROM recommendations
                 WHERE user_id = $1
                 ORDER BY created_at DESC
                 LIMIT $2",
                &[&user_id, &(limit / 3)],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query recommendations: {}", e)))?;

        for row in rec_rows {
            let asset_symbol: String = row.get(0);
            let action: String = row.get(1);
            let confidence: rust_decimal::Decimal = row.get(2);
            let reasoning: String = row.get(3);
            let created_at: DateTime<Utc> = row.get(4);

            actions.push(AIAction {
                action_type: "recommendation".to_string(),
                asset_symbol: asset_symbol.clone(),
                description: format!(
                    "{} {} ({}% confidence). {}",
                    action, asset_symbol, confidence, reasoning
                ),
                timestamp: created_at,
                result: None,
            });
        }

        // Get recent executed trades
        let trade_rows = client
            .query(
                "SELECT asset_symbol, action, amount, executed_at
                 FROM trade_executions
                 WHERE user_id = $1 AND status = 'completed'
                 ORDER BY executed_at DESC
                 LIMIT $2",
                &[&user_id, &(limit / 3)],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query trade executions: {}", e)))?;

        for row in trade_rows {
            let asset_symbol: String = row.get(0);
            let action: String = row.get(1);
            let amount: rust_decimal::Decimal = row.get(2);
            let executed_at: DateTime<Utc> = row.get(3);

            actions.push(AIAction {
                action_type: "trade".to_string(),
                asset_symbol: asset_symbol.clone(),
                description: format!("{} {} {}", action, amount, asset_symbol),
                timestamp: executed_at,
                result: Some("Completed".to_string()),
            });
        }

        // Sort all actions by timestamp descending
        actions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        // Limit to requested number
        actions.truncate(limit as usize);

        Ok(RecentAIActions {
            user_id,
            actions,
            timestamp: Utc::now(),
        })
    }

    /// Create a portfolio snapshot for the current state
    /// 
    /// This captures the current portfolio value and asset holdings for historical tracking.
    /// Should be called periodically (e.g., every hour or when significant changes occur).
    /// 
    /// **Validates: Requirement 8.1**
    pub async fn create_portfolio_snapshot(&self, wallet_id: Uuid) -> Result<Uuid> {
        info!("Creating portfolio snapshot for wallet_id: {}", wallet_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get current portfolio assets
        let asset_rows = client
            .query(
                "SELECT token_mint, token_symbol, amount, value_usd 
                 FROM portfolio_assets 
                 WHERE wallet_id = $1",
                &[&wallet_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query portfolio assets: {}", e)))?;

        if asset_rows.is_empty() {
            return Err(Error::Internal("No portfolio assets found".to_string()));
        }

        // Calculate total portfolio value
        let mut total_value_usd = 0.0;
        for row in &asset_rows {
            let value_usd: Option<rust_decimal::Decimal> = row.get(3);
            if let Some(value) = value_usd {
                total_value_usd += value.to_string().parse::<f64>().unwrap_or(0.0);
            }
        }

        // Insert portfolio snapshot
        let snapshot_row = client
            .query_one(
                "INSERT INTO portfolio_snapshots (wallet_id, total_value_usd, snapshot_time)
                 VALUES ($1, $2, NOW())
                 ON CONFLICT (wallet_id, snapshot_time) 
                 DO UPDATE SET total_value_usd = $2
                 RETURNING id, snapshot_time",
                &[
                    &wallet_id,
                    &rust_decimal::Decimal::from_f64_retain(total_value_usd)
                        .ok_or_else(|| Error::Internal("Failed to convert total value".to_string()))?,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to insert portfolio snapshot: {}", e)))?;

        let snapshot_id: Uuid = snapshot_row.get(0);
        // snapshot_time is `timestamp without time zone`; read as NaiveDateTime
        // (reading into DateTime<Utc> panics) and convert to UTC.
        let snapshot_time: DateTime<Utc> =
            snapshot_row.get::<_, chrono::NaiveDateTime>(1).and_utc();

        // Insert asset snapshots
        for row in asset_rows {
            let token_mint: String = row.get(0);
            let token_symbol: String = row.get(1);
            let amount: rust_decimal::Decimal = row.get(2);
            let value_usd: Option<rust_decimal::Decimal> = row.get(3);

            client
                .execute(
                    "INSERT INTO portfolio_asset_snapshots (snapshot_id, token_mint, token_symbol, amount, value_usd)
                     VALUES ($1, $2, $3, $4, $5)",
                    &[&snapshot_id, &token_mint, &token_symbol, &amount, &value_usd],
                )
                .await
                .map_err(|e| Error::Database(format!("Failed to insert asset snapshot: {}", e)))?;
        }

        info!(
            "Created portfolio snapshot {} at {} with total value ${:.2}",
            snapshot_id, snapshot_time, total_value_usd
        );

        Ok(snapshot_id)
    }

    /// Get portfolio performance over a time period
    /// 
    /// Retrieves historical snapshots and calculates gains/losses for the portfolio
    /// and individual assets.
    /// 
    /// **Validates: Requirement 8.1**
    pub async fn get_portfolio_performance(
        &self,
        wallet_address: &str,
        start_date: DateTime<Utc>,
        end_date: DateTime<Utc>,
    ) -> Result<PortfolioPerformance> {
        debug!(
            "Fetching portfolio performance for wallet {} from {} to {}",
            wallet_address, start_date, end_date
        );

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get wallet_id
        let wallet_row = client
            .query_one(
                "SELECT id FROM wallets WHERE address = $1",
                &[&wallet_address],
            )
            .await
            .map_err(|e| {
                if e.to_string().contains("no rows") {
                    Error::WalletNotFound(wallet_address.to_string())
                } else {
                    Error::Database(format!("Failed to query wallet: {}", e))
                }
            })?;

        let wallet_id: Uuid = wallet_row.get(0);

        // Get all snapshots in the time range. snapshot_time is `timestamp
        // without time zone`, so bind the bounds as NaiveDateTime (binding
        // DateTime<Utc> fails with a parameter serialization error).
        let start_naive = start_date.naive_utc();
        let end_naive = end_date.naive_utc();
        let snapshot_rows = client
            .query(
                "SELECT id, total_value_usd, snapshot_time 
                 FROM portfolio_snapshots 
                 WHERE wallet_id = $1 
                   AND snapshot_time >= $2 
                   AND snapshot_time <= $3
                 ORDER BY snapshot_time ASC",
                &[&wallet_id, &start_naive, &end_naive],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query snapshots: {}", e)))?;

        if snapshot_rows.is_empty() {
            return Err(Error::Internal(
                "No portfolio snapshots found in the specified time range".to_string(),
            ));
        }

        // Build snapshot list
        let mut snapshots = Vec::new();
        for row in &snapshot_rows {
            let total_value: rust_decimal::Decimal = row.get(1);
            // snapshot_time is `timestamp without time zone`; read as
            // NaiveDateTime then convert (reading into DateTime<Utc> panics).
            let timestamp: DateTime<Utc> = row.get::<_, chrono::NaiveDateTime>(2).and_utc();

            snapshots.push(PortfolioSnapshot {
                timestamp,
                total_value_usd: total_value.to_string().parse::<f64>().unwrap_or(0.0),
            });
        }

        // Get first and last snapshot for overall performance
        let first_snapshot_id: Uuid = snapshot_rows[0].get(0);
        let last_snapshot_id: Uuid = snapshot_rows[snapshot_rows.len() - 1].get(0);

        let start_value_usd = snapshots[0].total_value_usd;
        let end_value_usd = snapshots[snapshots.len() - 1].total_value_usd;
        let gain_loss_usd = end_value_usd - start_value_usd;
        let gain_loss_percent = if start_value_usd > 0.0 {
            (gain_loss_usd / start_value_usd) * 100.0
        } else {
            0.0
        };

        // Calculate per-asset performance
        let asset_performance = self
            .calculate_asset_performance(first_snapshot_id, last_snapshot_id)
            .await?;

        Ok(PortfolioPerformance {
            wallet_address: wallet_address.to_string(),
            start_date: snapshots[0].timestamp,
            end_date: snapshots[snapshots.len() - 1].timestamp,
            start_value_usd,
            end_value_usd,
            gain_loss_usd,
            gain_loss_percent,
            snapshots,
            asset_performance,
        })
    }

    /// Calculate performance for individual assets between two snapshots
    async fn calculate_asset_performance(
        &self,
        start_snapshot_id: Uuid,
        end_snapshot_id: Uuid,
    ) -> Result<Vec<AssetPerformance>> {
        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get assets from start snapshot
        let start_assets = client
            .query(
                "SELECT token_mint, token_symbol, amount, value_usd 
                 FROM portfolio_asset_snapshots 
                 WHERE snapshot_id = $1",
                &[&start_snapshot_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query start assets: {}", e)))?;

        // Get assets from end snapshot
        let end_assets = client
            .query(
                "SELECT token_mint, token_symbol, amount, value_usd 
                 FROM portfolio_asset_snapshots 
                 WHERE snapshot_id = $1",
                &[&end_snapshot_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query end assets: {}", e)))?;

        // Build maps for easy lookup
        let mut start_map: HashMap<String, (String, String, f64)> = HashMap::new();
        for row in start_assets {
            let token_mint: String = row.get(0);
            let token_symbol: String = row.get(1);
            let amount: rust_decimal::Decimal = row.get(2);
            let value_usd: Option<rust_decimal::Decimal> = row.get(3);

            start_map.insert(
                token_mint,
                (
                    token_symbol,
                    amount.to_string(),
                    value_usd
                        .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0),
                ),
            );
        }

        let mut end_map: HashMap<String, (String, String, f64)> = HashMap::new();
        for row in end_assets {
            let token_mint: String = row.get(0);
            let token_symbol: String = row.get(1);
            let amount: rust_decimal::Decimal = row.get(2);
            let value_usd: Option<rust_decimal::Decimal> = row.get(3);

            end_map.insert(
                token_mint,
                (
                    token_symbol,
                    amount.to_string(),
                    value_usd
                        .map(|v| v.to_string().parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0),
                ),
            );
        }

        // Calculate performance for each asset
        let mut performance = Vec::new();

        // Get all unique token mints from both snapshots
        let mut all_mints: Vec<String> = start_map.keys().cloned().collect();
        for mint in end_map.keys() {
            if !all_mints.contains(mint) {
                all_mints.push(mint.clone());
            }
        }

        for token_mint in all_mints {
            let (start_symbol, start_amount, start_value) = start_map
                .get(&token_mint)
                .cloned()
                .unwrap_or_else(|| ("UNKNOWN".to_string(), "0".to_string(), 0.0));

            let (end_symbol, end_amount, end_value) = end_map
                .get(&token_mint)
                .cloned()
                .unwrap_or_else(|| (start_symbol.clone(), "0".to_string(), 0.0));

            let gain_loss_usd = end_value - start_value;
            let gain_loss_percent = if start_value > 0.0 {
                (gain_loss_usd / start_value) * 100.0
            } else if end_value > 0.0 {
                100.0 // New asset, 100% gain
            } else {
                0.0
            };

            performance.push(AssetPerformance {
                token_mint,
                token_symbol: end_symbol,
                start_amount,
                end_amount,
                start_value_usd: start_value,
                end_value_usd: end_value,
                gain_loss_usd,
                gain_loss_percent,
            });
        }

        // Sort by absolute gain/loss (largest changes first)
        performance.sort_by(|a, b| {
            b.gain_loss_usd
                .abs()
                .partial_cmp(&a.gain_loss_usd.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(performance)
    }

    /// Get the latest portfolio snapshot for a wallet
    pub async fn get_latest_snapshot(&self, wallet_id: Uuid) -> Result<Option<PortfolioSnapshot>> {
        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let row = client
            .query_opt(
                "SELECT total_value_usd, snapshot_time 
                 FROM portfolio_snapshots 
                 WHERE wallet_id = $1 
                 ORDER BY snapshot_time DESC 
                 LIMIT 1",
                &[&wallet_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query latest snapshot: {}", e)))?;

        Ok(row.map(|r| {
            let total_value: rust_decimal::Decimal = r.get(0);
            // snapshot_time is `timestamp without time zone`.
            let timestamp: DateTime<Utc> = r.get::<_, chrono::NaiveDateTime>(1).and_utc();

            PortfolioSnapshot {
                timestamp,
                total_value_usd: total_value.to_string().parse::<f64>().unwrap_or(0.0),
            }
        }))
    }

    /// Get whale impact analysis for a user
    /// 
    /// Correlates whale movements with portfolio changes to calculate impact scores.
    /// 
    /// **Validates: Requirements 8.2, 8.3**
    pub async fn get_whale_impact_analysis(
        &self,
        user_id: Uuid,
        start_date: DateTime<Utc>,
        end_date: DateTime<Utc>,
    ) -> Result<WhaleImpactAnalysis> {
        debug!(
            "Fetching whale impact analysis for user {} from {} to {}",
            user_id, start_date, end_date
        );

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get user's wallet
        let wallet_row = client
            .query_opt(
                "SELECT id, address FROM wallets WHERE user_id = $1 LIMIT 1",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query wallet: {}", e)))?
            .ok_or_else(|| Error::Internal("No wallet found for user".to_string()))?;

        let wallet_id: Uuid = wallet_row.get(0);
        let wallet_address: String = wallet_row.get(1);

        // Get whale movements in the time range for whales tracked by this user
        let movement_rows = client
            .query(
                "SELECT wm.id, wm.whale_id, wm.transaction_signature, wm.movement_type, 
                        wm.token_mint, wm.amount, wm.percent_of_position, wm.detected_at,
                        w.address as whale_address
                 FROM whale_movements wm
                 JOIN whales w ON w.id = wm.whale_id
                 JOIN user_whale_tracking uwt ON uwt.whale_id = wm.whale_id
                 WHERE uwt.user_id = $1
                   AND wm.detected_at >= $2
                   AND wm.detected_at <= $3
                 ORDER BY wm.detected_at DESC",
                &[&user_id, &start_date, &end_date],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query whale movements: {}", e)))?;

        let mut whale_impacts = Vec::new();
        let mut total_movements = 0;
        let mut total_impact_score = 0.0;

        for row in movement_rows {
            let movement_id: Uuid = row.get(0);
            let whale_address: String = row.get(8);
            let movement_type: String = row.get(3);
            let token_mint: String = row.get(4);
            let percent_of_position: Option<rust_decimal::Decimal> = row.get(6);
            let detected_at: DateTime<Utc> = row.get(7);

            // Calculate impact score based on movement size and portfolio correlation
            let movement_percent = percent_of_position
                .map(|p| p.to_string().parse::<f64>().unwrap_or(0.0))
                .unwrap_or(0.0);

            // Get portfolio value change around the time of the movement
            let portfolio_change = self
                .get_portfolio_change_around_time(wallet_id, detected_at)
                .await
                .unwrap_or(0.0);

            // Impact score: combination of movement size and portfolio correlation
            // Higher score = larger movement with correlated portfolio change
            let impact_score = movement_percent * portfolio_change.abs() / 100.0;

            total_movements += 1;
            total_impact_score += impact_score;

            // Check if there's a recommendation for this movement
            let recommendation_exists = client
                .query_opt(
                    "SELECT id FROM recommendations WHERE movement_id = $1 AND user_id = $2",
                    &[&movement_id, &user_id],
                )
                .await
                .map_err(|e| Error::Database(format!("Failed to query recommendation: {}", e)))?
                .is_some();

            whale_impacts.push(WhaleImpact {
                whale_address,
                movement_type,
                token_mint,
                movement_percent,
                portfolio_change_percent: portfolio_change,
                impact_score,
                detected_at,
                had_recommendation: recommendation_exists,
            });
        }

        // Sort by impact score (highest first)
        whale_impacts.sort_by(|a, b| {
            b.impact_score
                .partial_cmp(&a.impact_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let average_impact_score = if total_movements > 0 {
            total_impact_score / total_movements as f64
        } else {
            0.0
        };

        Ok(WhaleImpactAnalysis {
            wallet_address,
            start_date,
            end_date,
            total_movements,
            average_impact_score,
            whale_impacts,
        })
    }

    /// Get portfolio value change around a specific time
    /// 
    /// Compares portfolio value before and after a timestamp to detect correlation
    /// with whale movements.
    async fn get_portfolio_change_around_time(
        &self,
        wallet_id: Uuid,
        timestamp: DateTime<Utc>,
    ) -> Result<f64> {
        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get snapshot before the event (within 1 hour before). snapshot_time
        // is `timestamp without time zone`, so bind NaiveDateTime bounds.
        let ts_naive = timestamp.naive_utc();
        let before_naive = (timestamp - chrono::Duration::hours(1)).naive_utc();
        let after_naive = (timestamp + chrono::Duration::hours(1)).naive_utc();
        let before_row = client
            .query_opt(
                "SELECT total_value_usd 
                 FROM portfolio_snapshots 
                 WHERE wallet_id = $1 
                   AND snapshot_time <= $2
                   AND snapshot_time >= $3
                 ORDER BY snapshot_time DESC 
                 LIMIT 1",
                &[&wallet_id, &ts_naive, &before_naive],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query before snapshot: {}", e)))?;

        // Get snapshot after the event (within 1 hour after)
        let after_row = client
            .query_opt(
                "SELECT total_value_usd 
                 FROM portfolio_snapshots 
                 WHERE wallet_id = $1 
                   AND snapshot_time >= $2
                   AND snapshot_time <= $3
                 ORDER BY snapshot_time ASC 
                 LIMIT 1",
                &[&wallet_id, &ts_naive, &after_naive],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query after snapshot: {}", e)))?;

        if let (Some(before), Some(after)) = (before_row, after_row) {
            let before_value: rust_decimal::Decimal = before.get(0);
            let after_value: rust_decimal::Decimal = after.get(0);

            let before_f64 = before_value.to_string().parse::<f64>().unwrap_or(0.0);
            let after_f64 = after_value.to_string().parse::<f64>().unwrap_or(0.0);

            if before_f64 > 0.0 {
                let change_percent = ((after_f64 - before_f64) / before_f64) * 100.0;
                return Ok(change_percent);
            }
        }

        Ok(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analytics_service_creation() {
        // Placeholder for actual tests with database fixtures
    }
}

/// Recommendation accuracy analysis data
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecommendationAccuracy {
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub total_recommendations: i32,
    pub recommendations_followed: i32,
    pub successful_recommendations: i32,
    pub accuracy_rate: f64,
    pub average_confidence: f64,
    pub recommendations_by_action: Vec<ActionAccuracy>,
}

/// Accuracy breakdown by recommendation action
#[derive(Debug, Clone, serde::Serialize)]
pub struct ActionAccuracy {
    pub action: String,
    pub total: i32,
    pub successful: i32,
    pub accuracy_rate: f64,
}

impl AnalyticsService {
    /// Get recommendation accuracy tracking
    /// 
    /// Tracks recommendation outcomes and calculates accuracy metrics.
    /// 
    /// **Validates: Requirement 8.4**
    pub async fn get_recommendation_accuracy(
        &self,
        user_id: Uuid,
        start_date: DateTime<Utc>,
        end_date: DateTime<Utc>,
    ) -> Result<RecommendationAccuracy> {
        debug!(
            "Fetching recommendation accuracy for user {} from {} to {}",
            user_id, start_date, end_date
        );

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get all recommendations in the time range
        let recommendation_rows = client
            .query(
                "SELECT r.id, r.action, r.confidence, r.created_at,
                        te.id as trade_id, te.status as trade_status
                 FROM recommendations r
                 LEFT JOIN trade_executions te ON te.recommendation_id = r.id
                 WHERE r.user_id = $1
                   AND r.created_at >= $2
                   AND r.created_at <= $3
                 ORDER BY r.created_at DESC",
                &[&user_id, &start_date, &end_date],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query recommendations: {}", e)))?;

        let mut total_recommendations = 0;
        let mut recommendations_followed = 0;
        let mut successful_recommendations = 0;
        let mut total_confidence = 0.0;
        let mut action_stats: HashMap<String, (i32, i32)> = HashMap::new(); // (total, successful)

        for row in recommendation_rows {
            let action: String = row.get(1);
            let confidence: i32 = row.get(2);
            let trade_id: Option<Uuid> = row.get(4);
            let trade_status: Option<String> = row.get(5);

            total_recommendations += 1;
            total_confidence += confidence as f64;

            // Track by action type
            let stats = action_stats.entry(action.clone()).or_insert((0, 0));
            stats.0 += 1;

            // Check if recommendation was followed (has associated trade)
            if trade_id.is_some() {
                recommendations_followed += 1;

                // Check if trade was successful (confirmed status)
                if let Some(status) = trade_status {
                    if status == "CONFIRMED" {
                        successful_recommendations += 1;
                        stats.1 += 1;
                    }
                }
            }
        }

        let accuracy_rate = if recommendations_followed > 0 {
            (successful_recommendations as f64 / recommendations_followed as f64) * 100.0
        } else {
            0.0
        };

        let average_confidence = if total_recommendations > 0 {
            total_confidence / total_recommendations as f64
        } else {
            0.0
        };

        // Build action accuracy breakdown
        let mut recommendations_by_action = Vec::new();
        for (action, (total, successful)) in action_stats {
            let action_accuracy = if total > 0 {
                (successful as f64 / total as f64) * 100.0
            } else {
                0.0
            };

            recommendations_by_action.push(ActionAccuracy {
                action,
                total,
                successful,
                accuracy_rate: action_accuracy,
            });
        }

        // Sort by total count (most common actions first)
        recommendations_by_action.sort_by(|a, b| b.total.cmp(&a.total));

        Ok(RecommendationAccuracy {
            start_date,
            end_date,
            total_recommendations,
            recommendations_followed,
            successful_recommendations,
            accuracy_rate,
            average_confidence,
            recommendations_by_action,
        })
    }
}
