use api::{ManualOrderRequest, PositionManagementService, PositionMode};
use database::{create_pool, DbPool};
use rust_decimal::Decimal;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

/// Acquire a test database pool, probing PostgreSQL with a short connect timeout.
///
/// Returns `Err` (with a human-readable reason) when the dependency is
/// unreachable so callers can skip-with-named-reason rather than fail
/// (Requirement 6.5: skipped distinct from failed for unreachable dependencies).
async fn setup_test_db() -> Result<DbPool, String> {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@localhost:5432/whale_tracker_test".to_string());

    let pool = create_pool(&database_url, 5)
        .await
        .map_err(|e| format!("failed to create pool: {}", e))?;

    // Probe the dependency: a deadpool `get()` establishes a real connection,
    // surfacing auth/connectivity errors (e.g. role missing). Bound it with a
    // short timeout so an unreachable host does not hang the suite.
    match tokio::time::timeout(Duration::from_secs(3), pool.get()).await {
        Ok(Ok(_client)) => Ok(pool),
        Ok(Err(e)) => Err(format!("connection failed: {}", e)),
        Err(_) => Err("connection timed out after 3s".to_string()),
    }
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

async fn create_test_user(pool: &DbPool) -> Uuid {
    let client = pool.get().await.expect("Failed to get client");
    let user_id = Uuid::new_v4();
    
    client
        .execute(
            "INSERT INTO users (id, email, password_hash, created_at, updated_at) VALUES ($1, $2, $3, NOW(), NOW())",
            &[&user_id, &format!("test_{}@example.com", user_id), &"test_hash"],
        )
        .await
        .unwrap();
    
    user_id
}

async fn create_test_wallet_with_balance(pool: &DbPool, user_id: Uuid, asset: &str, amount: Decimal) {
    let client = pool.get().await.expect("Failed to get client");
    let wallet_id = Uuid::new_v4();
    
    client
        .execute(
            "INSERT INTO wallets (id, user_id, address, blockchain, created_at, updated_at) VALUES ($1, $2, $3, $4, NOW(), NOW())",
            &[&wallet_id, &user_id, &format!("test_wallet_{}", wallet_id), &"Solana"],
        )
        .await
        .unwrap();
    
    client
        .execute(
            "INSERT INTO portfolio_assets (wallet_id, token_mint, token_symbol, amount, last_updated) VALUES ($1, $2, $3, $4, NOW())",
            &[&wallet_id, &format!("mint_{}", asset), &asset, &amount],
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn test_default_position_mode_is_manual() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    let mode = service
        .get_position_mode(user_id, "SOL", "Solana")
        .await
        .unwrap();
    
    assert_eq!(mode, PositionMode::Manual);
}

#[tokio::test]
async fn test_set_position_mode_to_automatic() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    let config = service
        .set_position_mode(user_id, "SOL", "Solana", PositionMode::Automatic)
        .await
        .unwrap();
    
    assert_eq!(config.mode, PositionMode::Automatic);
    assert_eq!(config.asset, "SOL");
    assert_eq!(config.blockchain, "Solana");
    
    // Verify it persists
    let mode = service
        .get_position_mode(user_id, "SOL", "Solana")
        .await
        .unwrap();
    
    assert_eq!(mode, PositionMode::Automatic);
}

#[tokio::test]
async fn test_switch_to_manual_cancels_pending_orders() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    // Set to automatic mode first
    service
        .set_position_mode(user_id, "SOL", "Solana", PositionMode::Automatic)
        .await
        .unwrap();
    
    // Register a pending automatic order
    let order_id = service
        .register_pending_automatic_order(
            user_id,
            "SOL",
            "Solana",
            "benchmark",
            None,
            "BUY",
            Decimal::from_str("10.0").unwrap(),
        )
        .await
        .unwrap();
    
    // Switch to manual mode
    service
        .set_position_mode(user_id, "SOL", "Solana", PositionMode::Manual)
        .await
        .unwrap();
    
    // Verify the order was cancelled
    let client = pool.get().await.expect("Failed to get client");
    let row = client
        .query_one(
            "SELECT status FROM pending_automatic_orders WHERE id = $1",
            &[&order_id],
        )
        .await
        .unwrap();
    
    let status: String = row.get(0);
    assert_eq!(status, "cancelled");
}

#[tokio::test]
async fn test_create_manual_buy_order() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    let request = ManualOrderRequest {
        asset: "SOL".to_string(),
        blockchain: Some("Solana".to_string()),
        action: "BUY".to_string(),
        amount: Decimal::from_str("5.0").unwrap(),
    };
    
    let order = service.create_manual_order(user_id, request).await.unwrap();
    
    assert_eq!(order.asset, "SOL");
    assert_eq!(order.action, "BUY");
    assert_eq!(order.amount, Decimal::from_str("5.0").unwrap());
    assert_eq!(order.status, "pending");
}

#[tokio::test]
async fn test_create_manual_sell_order_validates_balance() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    // Create wallet with 10 SOL
    create_test_wallet_with_balance(&pool, user_id, "SOL", Decimal::from_str("10.0").unwrap()).await;
    
    // Try to sell 5 SOL - should succeed
    let request = ManualOrderRequest {
        asset: "SOL".to_string(),
        blockchain: Some("Solana".to_string()),
        action: "SELL".to_string(),
        amount: Decimal::from_str("5.0").unwrap(),
    };
    
    let order = service.create_manual_order(user_id, request).await.unwrap();
    assert_eq!(order.status, "pending");
    
    // Try to sell 20 SOL - should fail
    let request = ManualOrderRequest {
        asset: "SOL".to_string(),
        blockchain: Some("Solana".to_string()),
        action: "SELL".to_string(),
        amount: Decimal::from_str("20.0").unwrap(),
    };
    
    let result = service.create_manual_order(user_id, request).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Insufficient balance"));
}

#[tokio::test]
async fn test_create_manual_order_rejects_invalid_action() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    let request = ManualOrderRequest {
        asset: "SOL".to_string(),
        blockchain: Some("Solana".to_string()),
        action: "INVALID".to_string(),
        amount: Decimal::from_str("5.0").unwrap(),
    };
    
    let result = service.create_manual_order(user_id, request).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid action"));
}

#[tokio::test]
async fn test_create_manual_order_rejects_negative_amount() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    let request = ManualOrderRequest {
        asset: "SOL".to_string(),
        blockchain: Some("Solana".to_string()),
        action: "BUY".to_string(),
        amount: Decimal::from_str("-5.0").unwrap(),
    };
    
    let result = service.create_manual_order(user_id, request).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Amount must be positive"));
}

#[tokio::test]
async fn test_get_user_manual_orders() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    // Create multiple orders
    for i in 1..=3 {
        let request = ManualOrderRequest {
            asset: "SOL".to_string(),
            blockchain: Some("Solana".to_string()),
            action: "BUY".to_string(),
            amount: Decimal::from_str(&format!("{}.0", i)).unwrap(),
        };
        service.create_manual_order(user_id, request).await.unwrap();
    }
    
    let orders = service.get_user_manual_orders(user_id, None).await.unwrap();
    assert_eq!(orders.len(), 3);
    
    // Should be ordered by created_at DESC
    assert!(orders[0].amount > orders[1].amount);
}

#[tokio::test]
async fn test_cancel_manual_order() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    let request = ManualOrderRequest {
        asset: "SOL".to_string(),
        blockchain: Some("Solana".to_string()),
        action: "BUY".to_string(),
        amount: Decimal::from_str("5.0").unwrap(),
    };
    
    let order = service.create_manual_order(user_id, request).await.unwrap();
    
    // Cancel the order
    service.cancel_manual_order(order.id, user_id).await.unwrap();
    
    // Verify it's cancelled
    let cancelled_order = service.get_manual_order(order.id, user_id).await.unwrap();
    assert_eq!(cancelled_order.status, "cancelled");
    assert!(cancelled_order.cancelled_at.is_some());
}

#[tokio::test]
async fn test_cannot_cancel_executed_order() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    let request = ManualOrderRequest {
        asset: "SOL".to_string(),
        blockchain: Some("Solana".to_string()),
        action: "BUY".to_string(),
        amount: Decimal::from_str("5.0").unwrap(),
    };
    
    let order = service.create_manual_order(user_id, request).await.unwrap();
    
    // Mark as executed
    service
        .update_order_status(
            order.id,
            "executed",
            Some("tx_hash_123".to_string()),
            Some(Decimal::from_str("100.0").unwrap()),
            Some(Decimal::from_str("500.0").unwrap()),
            None,
        )
        .await
        .unwrap();
    
    // Try to cancel - should fail
    let result = service.cancel_manual_order(order.id, user_id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_register_pending_automatic_order_requires_automatic_mode() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    // Asset is in manual mode by default
    let result = service
        .register_pending_automatic_order(
            user_id,
            "SOL",
            "Solana",
            "benchmark",
            None,
            "BUY",
            Decimal::from_str("10.0").unwrap(),
        )
        .await;
    
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("manual mode"));
    
    // Switch to automatic mode
    service
        .set_position_mode(user_id, "SOL", "Solana", PositionMode::Automatic)
        .await
        .unwrap();
    
    // Now it should work
    let order_id = service
        .register_pending_automatic_order(
            user_id,
            "SOL",
            "Solana",
            "benchmark",
            None,
            "BUY",
            Decimal::from_str("10.0").unwrap(),
        )
        .await
        .unwrap();
    
    assert!(!order_id.is_nil());
}

#[tokio::test]
async fn test_get_user_position_modes() {
    let pool = pool_or_skip!();
    let service = PositionManagementService::new(pool.clone());
    let user_id = create_test_user(&pool).await;
    
    // Set modes for multiple assets
    service
        .set_position_mode(user_id, "SOL", "Solana", PositionMode::Automatic)
        .await
        .unwrap();
    
    service
        .set_position_mode(user_id, "ETH", "Ethereum", PositionMode::Manual)
        .await
        .unwrap();
    
    let modes = service.get_user_position_modes(user_id).await.unwrap();
    assert_eq!(modes.len(), 2);
    
    // Find SOL mode
    let sol_mode = modes.iter().find(|m| m.asset == "SOL").unwrap();
    assert_eq!(sol_mode.mode, PositionMode::Automatic);
    
    // Find ETH mode
    let eth_mode = modes.iter().find(|m| m.asset == "ETH").unwrap();
    assert_eq!(eth_mode.mode, PositionMode::Manual);
}
