use api::{TrimConfigService, UpdateTrimConfigRequest};
use database::{create_pool, run_migrations};
use rust_decimal::Decimal;
use std::time::Duration;
use uuid::Uuid;

/// Helper to create a test database pool, probing PostgreSQL with a short
/// connect timeout.
///
/// Returns `Err` (with a human-readable reason) when the dependency is
/// unreachable so callers can skip-with-named-reason rather than fail
/// (Requirement 6.5: skipped distinct from failed for unreachable dependencies).
async fn setup_test_db() -> Result<database::DbPool, String> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@localhost:5432/whale_tracker_test".to_string());

    let pool = create_pool(&database_url, 5)
        .await
        .map_err(|e| format!("failed to create pool: {}", e))?;

    // Probe the dependency: a deadpool `get()` establishes a real connection,
    // surfacing auth/connectivity errors (e.g. role missing). Bound it with a
    // short timeout so an unreachable host does not hang the suite.
    match tokio::time::timeout(Duration::from_secs(3), pool.get()).await {
        Ok(Ok(_client)) => {}
        Ok(Err(e)) => return Err(format!("connection failed: {}", e)),
        Err(_) => return Err("connection timed out after 3s".to_string()),
    }

    // Try to run migrations, but ignore if tables already exist
    let _ = run_migrations(&pool).await;

    Ok(pool)
}

/// Acquire a pool or skip the current test with a named reason when PostgreSQL
/// is unreachable. Expands to an early `return` on the skip path.
macro_rules! pool_or_skip {
    () => {
        match setup_test_db().await {
            Ok(pool) => pool,
            Err(reason) => {
                eprintln!("SKIPPED: PostgreSQL unavailable — {}", reason);
                return;
            }
        }
    };
}

/// Helper to create a test user
async fn create_test_user(pool: &database::DbPool) -> Uuid {
    let client = pool.get().await.expect("Failed to get client");
    
    // Use a unique email for each test
    let unique_email = format!("test-{}@example.com", Uuid::new_v4());
    
    let row = client
        .query_one(
            "INSERT INTO users (email, password_hash, created_at, updated_at)
             VALUES ($1, $2, NOW(), NOW())
             RETURNING id",
            &[&unique_email, &"test_hash"],
        )
        .await
        .expect("Failed to create test user");
    
    row.get(0)
}

#[tokio::test]
async fn test_get_trim_config_returns_defaults_for_new_user() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    let config = service
        .get_trim_config(user_id)
        .await
        .expect("Failed to get config");

    assert_eq!(config.user_id, user_id);
    assert_eq!(config.enabled, false);
    assert_eq!(config.minimum_profit_percent, Decimal::from(20));
    assert_eq!(config.trim_percent, Decimal::from(25));
    assert_eq!(config.max_trims_per_day, 3);
}

#[tokio::test]
async fn test_upsert_trim_config_creates_new_config() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: Some(Decimal::from(30)),
        trim_percent: Some(Decimal::from(50)),
        max_trims_per_day: Some(5),
    };

    let config = service
        .upsert_trim_config(user_id, request)
        .await
        .expect("Failed to upsert config");

    assert_eq!(config.user_id, user_id);
    assert_eq!(config.enabled, true);
    assert_eq!(config.minimum_profit_percent, Decimal::from(30));
    assert_eq!(config.trim_percent, Decimal::from(50));
    assert_eq!(config.max_trims_per_day, 5);
}

#[tokio::test]
async fn test_upsert_trim_config_updates_existing_config() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    // Create initial config
    let request1 = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: Some(Decimal::from(20)),
        trim_percent: Some(Decimal::from(25)),
        max_trims_per_day: Some(3),
    };
    service
        .upsert_trim_config(user_id, request1)
        .await
        .expect("Failed to create config");

    // Update only enabled flag
    let request2 = UpdateTrimConfigRequest {
        enabled: Some(false),
        minimum_profit_percent: None,
        trim_percent: None,
        max_trims_per_day: None,
    };
    let config = service
        .upsert_trim_config(user_id, request2)
        .await
        .expect("Failed to update config");

    assert_eq!(config.enabled, false);
    assert_eq!(config.minimum_profit_percent, Decimal::from(20)); // Unchanged
    assert_eq!(config.trim_percent, Decimal::from(25)); // Unchanged
    assert_eq!(config.max_trims_per_day, 3); // Unchanged
}

#[tokio::test]
async fn test_upsert_rejects_non_positive_profit_percent() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: Some(Decimal::ZERO),
        trim_percent: None,
        max_trims_per_day: None,
    };

    let result = service.upsert_trim_config(user_id, request).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Minimum profit percent must be positive"));
}

#[tokio::test]
async fn test_upsert_rejects_profit_percent_over_1000() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: Some(Decimal::from(1001)),
        trim_percent: None,
        max_trims_per_day: None,
    };

    let result = service.upsert_trim_config(user_id, request).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Minimum profit percent cannot exceed 1000%"));
}

#[tokio::test]
async fn test_upsert_rejects_non_positive_trim_percent() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: None,
        trim_percent: Some(Decimal::ZERO),
        max_trims_per_day: None,
    };

    let result = service.upsert_trim_config(user_id, request).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Trim percent must be positive"));
}

#[tokio::test]
async fn test_upsert_rejects_trim_percent_over_100() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: None,
        trim_percent: Some(Decimal::from(101)),
        max_trims_per_day: None,
    };

    let result = service.upsert_trim_config(user_id, request).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Trim percent cannot exceed 100%"));
}

#[tokio::test]
async fn test_upsert_rejects_non_positive_max_trims() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: None,
        trim_percent: None,
        max_trims_per_day: Some(0),
    };

    let result = service.upsert_trim_config(user_id, request).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Max trims per day must be positive"));
}

#[tokio::test]
async fn test_upsert_rejects_max_trims_over_100() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: None,
        trim_percent: None,
        max_trims_per_day: Some(101),
    };

    let result = service.upsert_trim_config(user_id, request).await;
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Max trims per day cannot exceed 100"));
}

#[tokio::test]
async fn test_set_enabled_updates_only_enabled_flag() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    // Create initial config
    let request = UpdateTrimConfigRequest {
        enabled: Some(false),
        minimum_profit_percent: Some(Decimal::from(30)),
        trim_percent: Some(Decimal::from(40)),
        max_trims_per_day: Some(5),
    };
    service
        .upsert_trim_config(user_id, request)
        .await
        .expect("Failed to create config");

    // Enable trimming
    let config = service
        .set_enabled(user_id, true)
        .await
        .expect("Failed to set enabled");

    assert_eq!(config.enabled, true);
    assert_eq!(config.minimum_profit_percent, Decimal::from(30)); // Unchanged
    assert_eq!(config.trim_percent, Decimal::from(40)); // Unchanged
    assert_eq!(config.max_trims_per_day, 5); // Unchanged
}

#[tokio::test]
async fn test_delete_trim_config() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    // Create config
    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: Some(Decimal::from(30)),
        trim_percent: Some(Decimal::from(50)),
        max_trims_per_day: Some(5),
    };
    service
        .upsert_trim_config(user_id, request)
        .await
        .expect("Failed to create config");

    // Delete config
    service
        .delete_trim_config(user_id)
        .await
        .expect("Failed to delete config");

    // Verify it returns defaults now
    let config = service
        .get_trim_config(user_id)
        .await
        .expect("Failed to get config");
    assert_eq!(config.enabled, false);
    assert_eq!(config.minimum_profit_percent, Decimal::from(20));
}

#[tokio::test]
async fn test_delete_nonexistent_config_succeeds() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    // Delete non-existent config should not error
    let result = service.delete_trim_config(user_id).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_get_enabled_users() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());

    let user1 = create_test_user(&pool).await;
    let user2 = create_test_user(&pool).await;
    let user3 = create_test_user(&pool).await;

    // Enable for user1 and user2
    service
        .set_enabled(user1, true)
        .await
        .expect("Failed to enable user1");
    service
        .set_enabled(user2, true)
        .await
        .expect("Failed to enable user2");
    service
        .set_enabled(user3, false)
        .await
        .expect("Failed to disable user3");

    let enabled_users = service
        .get_enabled_users()
        .await
        .expect("Failed to get enabled users");

    assert!(enabled_users.contains(&user1));
    assert!(enabled_users.contains(&user2));
    assert!(!enabled_users.contains(&user3));
}

#[tokio::test]
async fn test_has_reached_daily_limit_no_trims() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    // Set config with limit of 3
    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: None,
        trim_percent: None,
        max_trims_per_day: Some(3),
    };
    service
        .upsert_trim_config(user_id, request)
        .await
        .expect("Failed to create config");

    let reached = service
        .has_reached_daily_limit(user_id)
        .await
        .expect("Failed to check limit");

    assert_eq!(reached, false);
}

#[tokio::test]
async fn test_has_reached_daily_limit_with_trims() {
    let pool = pool_or_skip!();
    let service = TrimConfigService::new(pool.clone());
    let user_id = create_test_user(&pool).await;

    // Set config with limit of 2
    let request = UpdateTrimConfigRequest {
        enabled: Some(true),
        minimum_profit_percent: None,
        trim_percent: None,
        max_trims_per_day: Some(2),
    };
    service
        .upsert_trim_config(user_id, request)
        .await
        .expect("Failed to create config");

    // Insert 2 trim executions for today
    let client = pool.get().await.expect("Failed to get client");
    for _ in 0..2 {
        client
            .execute(
                "INSERT INTO trim_executions 
                 (user_id, position_id, asset, amount_sold, price_usd, profit_realized, 
                  confidence, reasoning, executed_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NOW())",
                &[
                    &user_id,
                    &Uuid::new_v4(),
                    &"SOL",
                    &Decimal::from(100),
                    &Decimal::from(150),
                    &Decimal::from(50),
                    &90i32,
                    &"Test trim",
                ],
            )
            .await
            .expect("Failed to insert trim execution");
    }

    let reached = service
        .has_reached_daily_limit(user_id)
        .await
        .expect("Failed to check limit");

    assert_eq!(reached, true);
}
