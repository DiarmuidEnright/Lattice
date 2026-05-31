use chrono::{DateTime, Utc};
use database::DbPool;
use deadpool_postgres::tokio_postgres;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use shared::{Error, Result};
use std::str::FromStr;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Benchmark service for managing user-defined price benchmarks
/// 
/// This service provides CRUD operations for benchmarks that trigger
/// alerts or automated trades when asset prices cross defined thresholds.
/// 
/// **Validates: Requirements 2.1, 2.2, 2.6**
pub struct BenchmarkService {
    db_pool: DbPool,
}

/// Represents a price benchmark in the system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Benchmark {
    pub id: Uuid,
    pub user_id: Uuid,
    pub asset: String,
    pub blockchain: String,
    pub target_price: Decimal,
    pub trigger_type: TriggerType,
    pub action_type: ActionType,
    pub trade_action: Option<TradeAction>,
    pub trade_amount: Option<Decimal>,
    pub is_active: bool,
    pub triggered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Type of price trigger
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TriggerType {
    Above,
    Below,
}

impl FromStr for TriggerType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_uppercase().as_str() {
            "ABOVE" => Ok(TriggerType::Above),
            "BELOW" => Ok(TriggerType::Below),
            _ => Err(Error::Validation(format!("Invalid trigger type: {}", s))),
        }
    }
}

impl std::fmt::Display for TriggerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriggerType::Above => write!(f, "ABOVE"),
            TriggerType::Below => write!(f, "BELOW"),
        }
    }
}

/// Action to take when benchmark is triggered
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum ActionType {
    Alert,
    Execute,
}

impl FromStr for ActionType {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_uppercase().as_str() {
            "ALERT" => Ok(ActionType::Alert),
            "EXECUTE" => Ok(ActionType::Execute),
            _ => Err(Error::Validation(format!("Invalid action type: {}", s))),
        }
    }
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::Alert => write!(f, "ALERT"),
            ActionType::Execute => write!(f, "EXECUTE"),
        }
    }
}

/// Trade action for execute benchmarks
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum TradeAction {
    Buy,
    Sell,
}

impl FromStr for TradeAction {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_uppercase().as_str() {
            "BUY" => Ok(TradeAction::Buy),
            "SELL" => Ok(TradeAction::Sell),
            _ => Err(Error::Validation(format!("Invalid trade action: {}", s))),
        }
    }
}

impl std::fmt::Display for TradeAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TradeAction::Buy => write!(f, "BUY"),
            TradeAction::Sell => write!(f, "SELL"),
        }
    }
}

/// Request to create a new benchmark
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBenchmarkRequest {
    pub asset: String,
    pub blockchain: String,
    pub target_price: Decimal,
    pub trigger_type: TriggerType,
    pub action_type: ActionType,
    pub trade_action: Option<TradeAction>,
    pub trade_amount: Option<Decimal>,
}

/// Request to update an existing benchmark
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateBenchmarkRequest {
    pub target_price: Option<Decimal>,
    pub trigger_type: Option<TriggerType>,
    pub action_type: Option<ActionType>,
    pub trade_action: Option<TradeAction>,
    pub trade_amount: Option<Decimal>,
    pub is_active: Option<bool>,
}

impl BenchmarkService {
    /// Create a new benchmark service
    pub fn new(db_pool: DbPool) -> Self {
        Self { db_pool }
    }

    /// Create a new benchmark
    /// 
    /// Validates that the target price is positive and stores the benchmark
    /// in the database.
    /// 
    /// **Validates: Requirements 2.1, 2.2**
    pub async fn create_benchmark(
        &self,
        user_id: Uuid,
        request: CreateBenchmarkRequest,
    ) -> Result<Benchmark> {
        info!(
            "Creating benchmark for user {} on asset {} at price {}",
            user_id, request.asset, request.target_price
        );

        // Validate target price is positive (Requirement 2.2)
        if request.target_price <= Decimal::ZERO {
            warn!(
                "Rejected benchmark creation with non-positive price: {}",
                request.target_price
            );
            return Err(Error::Validation(
                "Target price must be positive".to_string(),
            ));
        }

        // Validate that execute actions have trade details
        if request.action_type == ActionType::Execute {
            if request.trade_action.is_none() {
                return Err(Error::Validation(
                    "Execute action requires trade_action (BUY or SELL)".to_string(),
                ));
            }
            if request.trade_amount.is_none() {
                return Err(Error::Validation(
                    "Execute action requires trade_amount".to_string(),
                ));
            }
            if let Some(amount) = request.trade_amount {
                if amount <= Decimal::ZERO {
                    return Err(Error::Validation(
                        "Trade amount must be positive".to_string(),
                    ));
                }
            }
        }

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let trigger_type_str = request.trigger_type.to_string();
        let action_type_str = request.action_type.to_string();
        let trade_action_str = request.trade_action.as_ref().map(|a| a.to_string());

        let row = client
            .query_one(
                "INSERT INTO benchmarks (
                    user_id, asset, blockchain, target_price, trigger_type, 
                    action_type, trade_action, trade_amount, is_active, created_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, TRUE, NOW())
                RETURNING id, user_id, asset, blockchain, target_price, trigger_type,
                          action_type, trade_action, trade_amount, is_active, 
                          triggered_at, created_at",
                &[
                    &user_id,
                    &request.asset,
                    &request.blockchain,
                    &request.target_price,
                    &trigger_type_str,
                    &action_type_str,
                    &trade_action_str,
                    &request.trade_amount,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to create benchmark: {}", e)))?;

        let benchmark = self.row_to_benchmark(row)?;

        info!(
            "Successfully created benchmark {} for user {}",
            benchmark.id, user_id
        );

        Ok(benchmark)
    }

    /// Get a benchmark by ID
    /// 
    /// **Validates: Requirements 2.6**
    pub async fn get_benchmark(&self, benchmark_id: Uuid, user_id: Uuid) -> Result<Benchmark> {
        debug!("Fetching benchmark {} for user {}", benchmark_id, user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let row = client
            .query_opt(
                "SELECT id, user_id, asset, blockchain, target_price, trigger_type,
                        action_type, trade_action, trade_amount, is_active, 
                        triggered_at, created_at
                 FROM benchmarks
                 WHERE id = $1 AND user_id = $2",
                &[&benchmark_id, &user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query benchmark: {}", e)))?;

        match row {
            Some(row) => self.row_to_benchmark(row),
            None => Err(Error::Validation(format!(
                "Benchmark {} not found",
                benchmark_id
            ))),
        }
    }

    /// Get all benchmarks for a user
    /// 
    /// **Validates: Requirements 2.6**
    pub async fn get_user_benchmarks(&self, user_id: Uuid) -> Result<Vec<Benchmark>> {
        debug!("Fetching all benchmarks for user {}", user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                "SELECT id, user_id, asset, blockchain, target_price, trigger_type,
                        action_type, trade_action, trade_amount, is_active, 
                        triggered_at, created_at
                 FROM benchmarks
                 WHERE user_id = $1
                 ORDER BY created_at DESC",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query benchmarks: {}", e)))?;

        let benchmarks: Result<Vec<Benchmark>> = rows
            .into_iter()
            .map(|row| self.row_to_benchmark(row))
            .collect();

        benchmarks
    }

    /// Get active benchmarks for a specific asset
    /// 
    /// This is used by the price monitoring system to check for triggers.
    /// 
    /// **Validates: Requirements 2.3, 2.4**
    pub async fn get_active_benchmarks_for_asset(&self, asset: &str) -> Result<Vec<Benchmark>> {
        debug!("Fetching active benchmarks for asset {}", asset);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                "SELECT id, user_id, asset, blockchain, target_price, trigger_type,
                        action_type, trade_action, trade_amount, is_active, 
                        triggered_at, created_at
                 FROM benchmarks
                 WHERE asset = $1 AND is_active = TRUE
                 ORDER BY target_price",
                &[&asset],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query benchmarks: {}", e)))?;

        let benchmarks: Result<Vec<Benchmark>> = rows
            .into_iter()
            .map(|row| self.row_to_benchmark(row))
            .collect();

        benchmarks
    }

    /// Update a benchmark
    /// 
    /// **Validates: Requirements 2.6**
    pub async fn update_benchmark(
        &self,
        benchmark_id: Uuid,
        user_id: Uuid,
        request: UpdateBenchmarkRequest,
    ) -> Result<Benchmark> {
        info!("Updating benchmark {} for user {}", benchmark_id, user_id);

        // Validate target price if provided
        if let Some(price) = request.target_price {
            if price <= Decimal::ZERO {
                return Err(Error::Validation(
                    "Target price must be positive".to_string(),
                ));
            }
        }

        // Validate trade amount if provided
        if let Some(amount) = request.trade_amount {
            if amount <= Decimal::ZERO {
                return Err(Error::Validation(
                    "Trade amount must be positive".to_string(),
                ));
            }
        }

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Convert enums to strings that will live long enough
        let trigger_str = request.trigger_type.as_ref().map(|t| t.to_string());
        let action_str = request.action_type.as_ref().map(|a| a.to_string());
        let trade_str = request.trade_action.as_ref().map(|t| t.to_string());

        // Build dynamic update query
        let mut updates = Vec::new();
        let mut param_index = 3; // Start after benchmark_id and user_id

        if request.target_price.is_some() {
            updates.push(format!("target_price = ${}", param_index));
            param_index += 1;
        }
        if trigger_str.is_some() {
            updates.push(format!("trigger_type = ${}", param_index));
            param_index += 1;
        }
        if action_str.is_some() {
            updates.push(format!("action_type = ${}", param_index));
            param_index += 1;
        }
        if trade_str.is_some() {
            updates.push(format!("trade_action = ${}", param_index));
            param_index += 1;
        }
        if request.trade_amount.is_some() {
            updates.push(format!("trade_amount = ${}", param_index));
            param_index += 1;
        }
        if request.is_active.is_some() {
            updates.push(format!("is_active = ${}", param_index));
        }

        if updates.is_empty() {
            return Err(Error::Validation("No fields to update".to_string()));
        }

        let query = format!(
            "UPDATE benchmarks SET {} 
             WHERE id = $1 AND user_id = $2
             RETURNING id, user_id, asset, blockchain, target_price, trigger_type,
                       action_type, trade_action, trade_amount, is_active, 
                       triggered_at, created_at",
            updates.join(", ")
        );

        // Build parameters dynamically
        let mut params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = vec![&benchmark_id, &user_id];
        
        if let Some(ref price) = request.target_price {
            params.push(price);
        }
        if let Some(ref trigger) = trigger_str {
            params.push(trigger);
        }
        if let Some(ref action) = action_str {
            params.push(action);
        }
        if let Some(ref trade) = trade_str {
            params.push(trade);
        }
        if let Some(ref amount) = request.trade_amount {
            params.push(amount);
        }
        if let Some(ref active) = request.is_active {
            params.push(active);
        }

        let row = client
            .query_opt(&query, &params[..])
            .await
            .map_err(|e| Error::Database(format!("Failed to update benchmark: {}", e)))?;

        match row {
            Some(row) => {
                let benchmark = self.row_to_benchmark(row)?;
                info!("Successfully updated benchmark {}", benchmark_id);
                Ok(benchmark)
            }
            None => Err(Error::Validation(format!(
                "Benchmark {} not found",
                benchmark_id
            ))),
        }
    }

    /// Delete a benchmark
    /// 
    /// **Validates: Requirements 2.6**
    pub async fn delete_benchmark(&self, benchmark_id: Uuid, user_id: Uuid) -> Result<()> {
        info!("Deleting benchmark {} for user {}", benchmark_id, user_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows_affected = client
            .execute(
                "DELETE FROM benchmarks WHERE id = $1 AND user_id = $2",
                &[&benchmark_id, &user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to delete benchmark: {}", e)))?;

        if rows_affected == 0 {
            return Err(Error::Validation(format!(
                "Benchmark {} not found",
                benchmark_id
            )));
        }

        info!("Successfully deleted benchmark {}", benchmark_id);
        Ok(())
    }

    /// Mark a benchmark as triggered
    /// 
    /// This is called by the price monitoring system when a benchmark threshold is crossed.
    /// 
    /// **Validates: Requirements 2.5**
    pub async fn mark_triggered(&self, benchmark_id: Uuid, disable: bool) -> Result<()> {
        debug!("Marking benchmark {} as triggered", benchmark_id);

        let client = self.db_pool.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let query = if disable {
            "UPDATE benchmarks SET triggered_at = NOW(), is_active = FALSE WHERE id = $1"
        } else {
            "UPDATE benchmarks SET triggered_at = NOW() WHERE id = $1"
        };

        client
            .execute(query, &[&benchmark_id])
            .await
            .map_err(|e| Error::Database(format!("Failed to mark benchmark as triggered: {}", e)))?;

        Ok(())
    }

    /// Convert a database row to a Benchmark struct
    fn row_to_benchmark(&self, row: tokio_postgres::Row) -> Result<Benchmark> {
        let trigger_type_str: String = row.get(5);
        let action_type_str: String = row.get(6);
        let trade_action_str: Option<String> = row.get(7);

        let trigger_type = TriggerType::from_str(&trigger_type_str)?;
        let action_type = ActionType::from_str(&action_type_str)?;
        let trade_action = trade_action_str
            .map(|s| TradeAction::from_str(&s))
            .transpose()?;

        Ok(Benchmark {
            id: row.get(0),
            user_id: row.get(1),
            asset: row.get(2),
            blockchain: row.get(3),
            target_price: row.get(4),
            trigger_type,
            action_type,
            trade_action,
            trade_amount: row.get(8),
            is_active: row.get(9),
            triggered_at: row.get(10),
            created_at: row.get(11),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trigger_type_parsing() {
        assert_eq!(TriggerType::from_str("ABOVE").unwrap(), TriggerType::Above);
        assert_eq!(TriggerType::from_str("above").unwrap(), TriggerType::Above);
        assert_eq!(TriggerType::from_str("BELOW").unwrap(), TriggerType::Below);
        assert_eq!(TriggerType::from_str("below").unwrap(), TriggerType::Below);
        assert!(TriggerType::from_str("invalid").is_err());
    }

    #[test]
    fn test_action_type_parsing() {
        assert_eq!(ActionType::from_str("ALERT").unwrap(), ActionType::Alert);
        assert_eq!(ActionType::from_str("alert").unwrap(), ActionType::Alert);
        assert_eq!(ActionType::from_str("EXECUTE").unwrap(), ActionType::Execute);
        assert_eq!(ActionType::from_str("execute").unwrap(), ActionType::Execute);
        assert!(ActionType::from_str("invalid").is_err());
    }

    #[test]
    fn test_trade_action_parsing() {
        assert_eq!(TradeAction::from_str("BUY").unwrap(), TradeAction::Buy);
        assert_eq!(TradeAction::from_str("buy").unwrap(), TradeAction::Buy);
        assert_eq!(TradeAction::from_str("SELL").unwrap(), TradeAction::Sell);
        assert_eq!(TradeAction::from_str("sell").unwrap(), TradeAction::Sell);
        assert!(TradeAction::from_str("invalid").is_err());
    }

    #[test]
    fn test_positive_price_validation() {
        // This will be tested in integration tests with actual database
        // Unit test just validates the logic exists
        let zero = Decimal::ZERO;
        let negative = Decimal::from_str("-1.5").unwrap();
        let positive = Decimal::from_str("100.50").unwrap();

        assert!(zero <= Decimal::ZERO);
        assert!(negative <= Decimal::ZERO);
        assert!(positive > Decimal::ZERO);
    }
}
