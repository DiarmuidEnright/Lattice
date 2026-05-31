use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use database::DbPool;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};
use uuid::Uuid;

use crate::sideshift_client::SideShiftClient;

/// Configuration for auto-staking behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingConfig {
    /// Minimum balance amount to be eligible for staking
    pub minimum_idle_amount: Decimal,
    /// Duration in hours that balance must be idle before staking
    pub idle_duration_hours: u32,
    /// Whether to automatically compound rewards
    pub auto_compound: bool,
}

impl Default for StakingConfig {
    fn default() -> Self {
        Self {
            minimum_idle_amount: Decimal::from(100), // Default: 100 units
            idle_duration_hours: 24,                  // Default: 24 hours
            auto_compound: false,
        }
    }
}

/// Represents a staking position
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingPosition {
    pub id: Uuid,
    pub user_id: Uuid,
    pub asset: String,
    pub amount: Decimal,
    pub provider: String,
    pub apy: Option<Decimal>,
    pub rewards_earned: Decimal,
    pub auto_compound: bool,
    pub started_at: DateTime<Utc>,
    pub last_reward_at: Option<DateTime<Utc>>,
}

/// Request for user approval to stake assets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingApprovalRequest {
    pub request_id: Uuid,
    pub user_id: Uuid,
    pub asset: String,
    pub amount: Decimal,
    pub provider: String,
    pub apy: Decimal,
    pub lock_period_days: u32,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Result of staking initiation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StakingInitiationResult {
    pub position_id: Uuid,
    pub asset: String,
    pub amount: Decimal,
    pub apy: Decimal,
    pub started_at: DateTime<Utc>,
}

/// Service for managing auto-staking functionality
/// 
/// **Validates: Requirements 3.1, 3.3, 3.4**
pub struct StakingService {
    db_pool: DbPool,
    sideshift_client: Arc<SideShiftClient>,
}

impl StakingService {
    /// Create a new staking service
    pub fn new(db_pool: DbPool, sideshift_client: Arc<SideShiftClient>) -> Self {
        Self {
            db_pool,
            sideshift_client,
        }
    }

    /// Identify idle balances eligible for staking
    /// 
    /// An asset is considered idle if:
    /// 1. Balance >= minimum_idle_amount
    /// 2. No trades for the configured idle_duration_hours
    /// 3. Not already staked
    /// 
    /// **Validates: Requirement 3.1**
    pub async fn identify_idle_balances(
        &self,
        user_id: Uuid,
        config: &StakingConfig,
    ) -> Result<Vec<(String, Decimal)>> {
        debug!(
            "Identifying idle balances for user {} with config: min={}, duration={}h",
            user_id, config.minimum_idle_amount, config.idle_duration_hours
        );

        let client = self
            .db_pool
            .get()
            .await
            .context("Failed to get database connection")?;

        // Calculate the cutoff time for idle detection
        let idle_cutoff = Utc::now() - Duration::hours(config.idle_duration_hours as i64);

        // Query for assets that meet idle criteria
        let rows = client
            .query(
                "SELECT pa.token_symbol, pa.amount
                 FROM portfolio_assets pa
                 INNER JOIN wallets w ON pa.wallet_id = w.id
                 LEFT JOIN (
                     SELECT asset, MAX(created_at) as last_trade
                     FROM conversions
                     WHERE user_id = $1
                     GROUP BY asset
                 ) c ON pa.token_symbol = c.asset
                 LEFT JOIN (
                     SELECT asset, SUM(amount) as staked_amount
                     FROM staking_positions
                     WHERE user_id = $1
                     GROUP BY asset
                 ) sp ON pa.token_symbol = sp.asset
                 WHERE w.user_id = $1
                   AND pa.amount >= $2
                   AND (c.last_trade IS NULL OR c.last_trade < $3)
                   AND (sp.staked_amount IS NULL OR pa.amount > sp.staked_amount)
                 GROUP BY pa.token_symbol, pa.amount
                 HAVING SUM(pa.amount) >= $2",
                &[&user_id, &config.minimum_idle_amount, &idle_cutoff],
            )
            .await
            .context("Failed to query idle balances")?;

        let mut idle_balances = Vec::new();
        for row in rows {
            let asset: String = row.get(0);
            let amount: Decimal = row.get(1);
            
            debug!("Found idle balance: {} {} (idle for >{}h)", amount, asset, config.idle_duration_hours);
            idle_balances.push((asset, amount));
        }

        info!(
            "Identified {} idle balances for user {}",
            idle_balances.len(),
            user_id
        );

        Ok(idle_balances)
    }

    /// Create a staking approval request for user
    /// 
    /// **Validates: Requirement 3.3**
    pub async fn create_staking_approval_request(
        &self,
        user_id: Uuid,
        asset: &str,
        amount: Decimal,
    ) -> Result<StakingApprovalRequest> {
        info!(
            "Creating staking approval request for user {}: {} {}",
            user_id, amount, asset
        );

        // Get staking info from SideShift
        let staking_info = self
            .sideshift_client
            .get_staking_info(asset)
            .await
            .context("Failed to get staking info from SideShift")?;

        // Validate amount meets minimum
        if amount < staking_info.minimum_amount {
            anyhow::bail!(
                "Amount {} is below minimum staking amount {} for {}",
                amount,
                staking_info.minimum_amount,
                asset
            );
        }

        let request_id = Uuid::new_v4();
        let created_at = Utc::now();
        let expires_at = created_at + Duration::hours(24); // Request expires in 24 hours

        let client = self
            .db_pool
            .get()
            .await
            .context("Failed to get database connection")?;

        // Store approval request in database
        client
            .execute(
                "INSERT INTO staking_approval_requests 
                 (id, user_id, asset, amount, provider, apy, lock_period_days, created_at, expires_at, status)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                &[
                    &request_id,
                    &user_id,
                    &asset,
                    &amount,
                    &"SideShift",
                    &staking_info.apy,
                    &(staking_info.lock_period_days as i32),
                    &created_at,
                    &expires_at,
                    &"pending",
                ],
            )
            .await
            .context("Failed to insert staking approval request")?;

        Ok(StakingApprovalRequest {
            request_id,
            user_id,
            asset: asset.to_string(),
            amount,
            provider: "SideShift".to_string(),
            apy: staking_info.apy,
            lock_period_days: staking_info.lock_period_days,
            created_at,
            expires_at,
        })
    }

    /// Initiate staking after user approval
    /// 
    /// **Validates: Requirement 3.3**
    pub async fn initiate_staking(
        &self,
        request_id: Uuid,
        approved: bool,
    ) -> Result<Option<StakingInitiationResult>> {
        info!(
            "Processing staking approval request {}: approved={}",
            request_id, approved
        );

        let client = self
            .db_pool
            .get()
            .await
            .context("Failed to get database connection")?;

        // Get approval request
        let row = client
            .query_one(
                "SELECT user_id, asset, amount, provider, apy, status, expires_at
                 FROM staking_approval_requests
                 WHERE id = $1",
                &[&request_id],
            )
            .await
            .context("Failed to query staking approval request")?;

        let user_id: Uuid = row.get(0);
        let asset: String = row.get(1);
        let amount: Decimal = row.get(2);
        let provider: String = row.get(3);
        let apy: Decimal = row.get(4);
        let status: String = row.get(5);
        let expires_at: DateTime<Utc> = row.get(6);

        // Validate request is still pending
        if status != "pending" {
            anyhow::bail!("Staking approval request {} is not pending (status: {})", request_id, status);
        }

        // Validate request hasn't expired
        if Utc::now() > expires_at {
            client
                .execute(
                    "UPDATE staking_approval_requests SET status = $1 WHERE id = $2",
                    &[&"expired", &request_id],
                )
                .await
                .context("Failed to update expired request")?;
            anyhow::bail!("Staking approval request {} has expired", request_id);
        }

        if !approved {
            // User rejected the request
            client
                .execute(
                    "UPDATE staking_approval_requests SET status = $1 WHERE id = $2",
                    &[&"rejected", &request_id],
                )
                .await
                .context("Failed to update rejected request")?;
            
            info!("User {} rejected staking request {}", user_id, request_id);
            return Ok(None);
        }

        // User approved - create staking position
        let position_id = Uuid::new_v4();
        let started_at = Utc::now();

        client
            .execute(
                "INSERT INTO staking_positions 
                 (id, user_id, asset, amount, provider, apy, rewards_earned, auto_compound, started_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
                &[
                    &position_id,
                    &user_id,
                    &asset,
                    &amount,
                    &provider,
                    &apy,
                    &Decimal::ZERO,
                    &false, // Default to no auto-compound
                    &started_at,
                ],
            )
            .await
            .context("Failed to insert staking position")?;

        // Update approval request status
        client
            .execute(
                "UPDATE staking_approval_requests SET status = $1 WHERE id = $2",
                &[&"approved", &request_id],
            )
            .await
            .context("Failed to update approved request")?;

        info!(
            "Created staking position {} for user {}: {} {} at {}% APY",
            position_id, user_id, amount, asset, apy
        );

        Ok(Some(StakingInitiationResult {
            position_id,
            asset,
            amount,
            apy,
            started_at,
        }))
    }

    /// Get all staking positions for a user
    /// 
    /// **Validates: Requirement 3.4**
    pub async fn get_staking_positions(&self, user_id: Uuid) -> Result<Vec<StakingPosition>> {
        debug!("Fetching staking positions for user {}", user_id);

        let client = self
            .db_pool
            .get()
            .await
            .context("Failed to get database connection")?;

        let rows = client
            .query(
                "SELECT id, user_id, asset, amount, provider, apy, rewards_earned, 
                        auto_compound, started_at, last_reward_at
                 FROM staking_positions
                 WHERE user_id = $1
                 ORDER BY started_at DESC",
                &[&user_id],
            )
            .await
            .context("Failed to query staking positions")?;

        let positions: Vec<StakingPosition> = rows
            .iter()
            .map(|row| StakingPosition {
                id: row.get(0),
                user_id: row.get(1),
                asset: row.get(2),
                amount: row.get(3),
                provider: row.get(4),
                apy: row.get(5),
                rewards_earned: row.get(6),
                auto_compound: row.get(7),
                started_at: row.get(8),
                last_reward_at: row.get(9),
            })
            .collect();

        info!("Found {} staking positions for user {}", positions.len(), user_id);

        Ok(positions)
    }

    /// Get a specific staking position
    /// 
    /// **Validates: Requirement 3.4**
    pub async fn get_staking_position(&self, position_id: Uuid) -> Result<StakingPosition> {
        debug!("Fetching staking position {}", position_id);

        let client = self
            .db_pool
            .get()
            .await
            .context("Failed to get database connection")?;

        let row = client
            .query_one(
                "SELECT id, user_id, asset, amount, provider, apy, rewards_earned, 
                        auto_compound, started_at, last_reward_at
                 FROM staking_positions
                 WHERE id = $1",
                &[&position_id],
            )
            .await
            .context("Failed to query staking position")?;

        Ok(StakingPosition {
            id: row.get(0),
            user_id: row.get(1),
            asset: row.get(2),
            amount: row.get(3),
            provider: row.get(4),
            apy: row.get(5),
            rewards_earned: row.get(6),
            auto_compound: row.get(7),
            started_at: row.get(8),
            last_reward_at: row.get(9),
        })
    }

    /// Update rewards for a staking position
    /// 
    /// This would typically be called by a background job that periodically
    /// checks with the staking provider for updated reward amounts.
    /// 
    /// **Validates: Requirement 3.4**
    pub async fn update_staking_rewards(
        &self,
        position_id: Uuid,
        rewards_earned: Decimal,
    ) -> Result<()> {
        debug!(
            "Updating rewards for staking position {}: {}",
            position_id, rewards_earned
        );

        let client = self
            .db_pool
            .get()
            .await
            .context("Failed to get database connection")?;

        let now = Utc::now();

        client
            .execute(
                "UPDATE staking_positions 
                 SET rewards_earned = $1, last_reward_at = $2
                 WHERE id = $3",
                &[&rewards_earned, &now, &position_id],
            )
            .await
            .context("Failed to update staking rewards")?;

        info!(
            "Updated staking position {} with rewards: {}",
            position_id, rewards_earned
        );

        Ok(())
    }

    /// Enable or disable auto-staking for a user's asset
    pub async fn set_auto_staking(
        &self,
        user_id: Uuid,
        asset: &str,
        enabled: bool,
        config: Option<StakingConfig>,
    ) -> Result<()> {
        info!(
            "Setting auto-staking for user {} asset {}: enabled={}",
            user_id, asset, enabled
        );

        let client = self
            .db_pool
            .get()
            .await
            .context("Failed to get database connection")?;

        let config = config.unwrap_or_default();

        client
            .execute(
                "INSERT INTO auto_staking_configs 
                 (user_id, asset, enabled, minimum_idle_amount, idle_duration_hours, auto_compound)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (user_id, asset)
                 DO UPDATE SET enabled = $3, minimum_idle_amount = $4, 
                               idle_duration_hours = $5, auto_compound = $6",
                &[
                    &user_id,
                    &asset,
                    &enabled,
                    &config.minimum_idle_amount,
                    &(config.idle_duration_hours as i32),
                    &config.auto_compound,
                ],
            )
            .await
            .context("Failed to update auto-staking config")?;

        Ok(())
    }

    /// Get auto-staking configuration for a user's asset
    pub async fn get_auto_staking_config(
        &self,
        user_id: Uuid,
        asset: &str,
    ) -> Result<Option<(bool, StakingConfig)>> {
        let client = self
            .db_pool
            .get()
            .await
            .context("Failed to get database connection")?;

        let result = client
            .query_opt(
                "SELECT enabled, minimum_idle_amount, idle_duration_hours, auto_compound
                 FROM auto_staking_configs
                 WHERE user_id = $1 AND asset = $2",
                &[&user_id, &asset],
            )
            .await
            .context("Failed to query auto-staking config")?;

        match result {
            Some(row) => {
                let enabled: bool = row.get(0);
                let config = StakingConfig {
                    minimum_idle_amount: row.get(1),
                    idle_duration_hours: row.get::<_, i32>(2) as u32,
                    auto_compound: row.get(3),
                };
                Ok(Some((enabled, config)))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_staking_config_default() {
        let config = StakingConfig::default();
        assert_eq!(config.minimum_idle_amount, Decimal::from(100));
        assert_eq!(config.idle_duration_hours, 24);
        assert_eq!(config.auto_compound, false);
    }

    #[test]
    fn test_staking_position_serialization() {
        let position = StakingPosition {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            asset: "SOL".to_string(),
            amount: Decimal::from(1000),
            provider: "SideShift".to_string(),
            apy: Some(Decimal::from(5)),
            rewards_earned: Decimal::from(10),
            auto_compound: false,
            started_at: Utc::now(),
            last_reward_at: None,
        };

        let json = serde_json::to_string(&position).unwrap();
        let deserialized: StakingPosition = serde_json::from_str(&json).unwrap();

        assert_eq!(position.id, deserialized.id);
        assert_eq!(position.asset, deserialized.asset);
        assert_eq!(position.amount, deserialized.amount);
    }
}
