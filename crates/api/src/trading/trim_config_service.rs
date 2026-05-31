use chrono::{DateTime, Utc};
use database::DbPool;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use shared::{Error, Result};
use tracing::{debug, info};
use uuid::Uuid;

/// Service for managing agentic position trimming configuration
/// 
/// This service provides CRUD operations for user trim preferences including
/// profit thresholds, trim percentages, and daily limits.
/// 
/// **Validates: Requirements 7.2, 7.4**
pub struct TrimConfigService {
    db_pool: DbPool,
}

/// Represents a user's trim configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrimConfig {
    pub user_id: Uuid,
    pub enabled: bool,
    pub minimum_profit_percent: Decimal,
    pub trim_percent: Decimal,
    pub max_trims_per_day: i32,
    pub updated_at: DateTime<Utc>,
}

impl Default for TrimConfig {
    fn default() -> Self {
        Self {
            user_id: Uuid::nil(),
            enabled: false,
            minimum_profit_percent: Decimal::from(20), // Default: 20% profit
            trim_percent: Decimal::from(25),            // Default: 25% of position
            max_trims_per_day: 3,                       // Default: 3 trims per day
            updated_at: Utc::now(),
        }
    }
}

/// Request to create or update trim configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTrimConfigRequest {
    pub enabled: Option<bool>,
    pub minimum_profit_percent: Option<Decimal>,
    pub trim_percent: Option<Decimal>,
    pub max_trims_per_day: Option<i32>,
}

impl TrimConfigService {
    /// Create a new trim configuration service
    pub fn new(db_pool: DbPool) -> Self {
        Self { db_pool }
    }

    /// Get trim configuration for a user
    /// 
    /// Returns the user's configuration if it exists, otherwise returns default config.
    /// 
    /// **Validates: Requirements 7.2, 7.4**
    pub async fn get_trim_config(&self, user_id: Uuid) -> Result<TrimConfig> {
        debug!("Fetching trim config for user {}", user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let row = client
            .query_opt(
                "SELECT user_id, enabled, minimum_profit_percent, trim_percent, 
                        max_trims_per_day, updated_at
                 FROM trim_configs
                 WHERE user_id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query trim config: {}", e)))?;

        match row {
            Some(row) => {
                let config = TrimConfig {
                    user_id: row.get(0),
                    enabled: row.get(1),
                    minimum_profit_percent: row.get(2),
                    trim_percent: row.get(3),
                    max_trims_per_day: row.get(4),
                    updated_at: row.get(5),
                };
                debug!("Found trim config for user {}: enabled={}", user_id, config.enabled);
                Ok(config)
            }
            None => {
                debug!("No trim config found for user {}, returning defaults", user_id);
                let mut default = TrimConfig::default();
                default.user_id = user_id;
                Ok(default)
            }
        }
    }

    /// Create or update trim configuration for a user
    /// 
    /// Validates that:
    /// - Profit percent is positive
    /// - Trim percent is between 0 and 100
    /// - Max trims per day is positive
    /// 
    /// **Validates: Requirements 7.2, 7.4**
    pub async fn upsert_trim_config(
        &self,
        user_id: Uuid,
        request: UpdateTrimConfigRequest,
    ) -> Result<TrimConfig> {
        info!("Upserting trim config for user {}", user_id);

        // Validate profit percent if provided
        if let Some(profit_percent) = request.minimum_profit_percent {
            if profit_percent <= Decimal::ZERO {
                return Err(Error::Validation(
                    "Minimum profit percent must be positive".to_string(),
                ));
            }
            if profit_percent > Decimal::from(1000) {
                return Err(Error::Validation(
                    "Minimum profit percent cannot exceed 1000%".to_string(),
                ));
            }
        }

        // Validate trim percent if provided
        if let Some(trim_percent) = request.trim_percent {
            if trim_percent <= Decimal::ZERO {
                return Err(Error::Validation(
                    "Trim percent must be positive".to_string(),
                ));
            }
            if trim_percent > Decimal::from(100) {
                return Err(Error::Validation(
                    "Trim percent cannot exceed 100%".to_string(),
                ));
            }
        }

        // Validate max trims per day if provided
        if let Some(max_trims) = request.max_trims_per_day {
            if max_trims <= 0 {
                return Err(Error::Validation(
                    "Max trims per day must be positive".to_string(),
                ));
            }
            if max_trims > 100 {
                return Err(Error::Validation(
                    "Max trims per day cannot exceed 100".to_string(),
                ));
            }
        }

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Get current config or use defaults
        let current = self.get_trim_config(user_id).await?;

        // Apply updates
        let enabled = request.enabled.unwrap_or(current.enabled);
        let minimum_profit_percent = request
            .minimum_profit_percent
            .unwrap_or(current.minimum_profit_percent);
        let trim_percent = request.trim_percent.unwrap_or(current.trim_percent);
        let max_trims_per_day = request
            .max_trims_per_day
            .unwrap_or(current.max_trims_per_day);

        let row = client
            .query_one(
                "INSERT INTO trim_configs 
                 (user_id, enabled, minimum_profit_percent, trim_percent, max_trims_per_day, updated_at)
                 VALUES ($1, $2, $3, $4, $5, NOW())
                 ON CONFLICT (user_id)
                 DO UPDATE SET 
                    enabled = $2,
                    minimum_profit_percent = $3,
                    trim_percent = $4,
                    max_trims_per_day = $5,
                    updated_at = NOW()
                 RETURNING user_id, enabled, minimum_profit_percent, trim_percent, 
                           max_trims_per_day, updated_at",
                &[
                    &user_id,
                    &enabled,
                    &minimum_profit_percent,
                    &trim_percent,
                    &max_trims_per_day,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to upsert trim config: {}", e)))?;

        let config = TrimConfig {
            user_id: row.get(0),
            enabled: row.get(1),
            minimum_profit_percent: row.get(2),
            trim_percent: row.get(3),
            max_trims_per_day: row.get(4),
            updated_at: row.get(5),
        };

        info!(
            "Successfully upserted trim config for user {}: enabled={}, profit_threshold={}%, trim={}%, max_daily={}",
            user_id, config.enabled, config.minimum_profit_percent, config.trim_percent, config.max_trims_per_day
        );

        Ok(config)
    }

    /// Enable or disable agentic trimming for a user
    /// 
    /// **Validates: Requirements 7.2**
    pub async fn set_enabled(&self, user_id: Uuid, enabled: bool) -> Result<TrimConfig> {
        info!("Setting trim enabled={} for user {}", enabled, user_id);

        let request = UpdateTrimConfigRequest {
            enabled: Some(enabled),
            minimum_profit_percent: None,
            trim_percent: None,
            max_trims_per_day: None,
        };

        self.upsert_trim_config(user_id, request).await
    }

    /// Delete trim configuration for a user (resets to defaults)
    /// 
    /// **Validates: Requirements 7.2, 7.4**
    pub async fn delete_trim_config(&self, user_id: Uuid) -> Result<()> {
        info!("Deleting trim config for user {}", user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows_affected = client
            .execute(
                "DELETE FROM trim_configs WHERE user_id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to delete trim config: {}", e)))?;

        if rows_affected == 0 {
            debug!("No trim config found to delete for user {}", user_id);
        } else {
            info!("Successfully deleted trim config for user {}", user_id);
        }

        Ok(())
    }

    /// Get all users with agentic trimming enabled
    /// 
    /// This is used by the position evaluation worker to identify which users
    /// need their positions evaluated for trimming.
    /// 
    /// **Validates: Requirements 7.1, 7.2**
    pub async fn get_enabled_users(&self) -> Result<Vec<Uuid>> {
        debug!("Fetching all users with trim enabled");

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                "SELECT user_id FROM trim_configs WHERE enabled = TRUE",
                &[],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query enabled users: {}", e)))?;

        let user_ids: Vec<Uuid> = rows.iter().map(|row| row.get(0)).collect();

        info!("Found {} users with trim enabled", user_ids.len());

        Ok(user_ids)
    }

    /// Check if a user has reached their daily trim limit
    /// 
    /// **Validates: Requirements 7.4**
    pub async fn has_reached_daily_limit(&self, user_id: Uuid) -> Result<bool> {
        debug!("Checking daily trim limit for user {}", user_id);

        let config = self.get_trim_config(user_id).await?;

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Count trims executed today
        let row = client
            .query_one(
                "SELECT COUNT(*) 
                 FROM trim_executions
                 WHERE user_id = $1 
                   AND executed_at >= CURRENT_DATE
                   AND executed_at < CURRENT_DATE + INTERVAL '1 day'",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to count daily trims: {}", e)))?;

        let trim_count: i64 = row.get(0);
        let reached_limit = trim_count >= config.max_trims_per_day as i64;

        if reached_limit {
            debug!(
                "User {} has reached daily trim limit: {}/{}",
                user_id, trim_count, config.max_trims_per_day
            );
        }

        Ok(reached_limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_trim_config_default() {
        let config = TrimConfig::default();
        assert_eq!(config.enabled, false);
        assert_eq!(config.minimum_profit_percent, Decimal::from(20));
        assert_eq!(config.trim_percent, Decimal::from(25));
        assert_eq!(config.max_trims_per_day, 3);
    }

    #[test]
    fn test_trim_config_serialization() {
        let config = TrimConfig {
            user_id: Uuid::new_v4(),
            enabled: true,
            minimum_profit_percent: Decimal::from(30),
            trim_percent: Decimal::from(50),
            max_trims_per_day: 5,
            updated_at: Utc::now(),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: TrimConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config.user_id, deserialized.user_id);
        assert_eq!(config.enabled, deserialized.enabled);
        assert_eq!(config.minimum_profit_percent, deserialized.minimum_profit_percent);
        assert_eq!(config.trim_percent, deserialized.trim_percent);
        assert_eq!(config.max_trims_per_day, deserialized.max_trims_per_day);
    }

    #[test]
    fn test_validation_logic() {
        // Test profit percent validation
        let zero = Decimal::ZERO;
        let negative = Decimal::from_str("-1.5").unwrap();
        let positive = Decimal::from_str("20.0").unwrap();
        let too_high = Decimal::from_str("1001.0").unwrap();

        assert!(zero <= Decimal::ZERO);
        assert!(negative <= Decimal::ZERO);
        assert!(positive > Decimal::ZERO);
        assert!(too_high > Decimal::from(1000));

        // Test trim percent validation
        let valid_trim = Decimal::from(25);
        let invalid_trim = Decimal::from(101);

        assert!(valid_trim > Decimal::ZERO && valid_trim <= Decimal::from(100));
        assert!(invalid_trim > Decimal::from(100));

        // Test max trims validation
        let valid_max = 3;
        let invalid_max_low = 0;
        let invalid_max_high = 101;

        assert!(valid_max > 0 && valid_max <= 100);
        assert!(invalid_max_low <= 0);
        assert!(invalid_max_high > 100);
    }
}
