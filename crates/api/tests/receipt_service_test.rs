use api::{ReceiptData, ReceiptService, VerificationStatus};
use blockchain::{Blockchain, MultiChainClient};
use database::{create_pool, run_migrations};
use rust_decimal::Decimal;
use std::sync::Arc;
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

/// Helper to create a test receipt service. Returns `Err` with a named reason
/// when PostgreSQL is unreachable.
async fn create_test_service() -> Result<ReceiptService, String> {
    let db = setup_test_db().await?;

    // Create a multi-chain client with test configuration
    let blockchain_client = Arc::new(
        MultiChainClient::new()
            .with_solana("https://api.mainnet-beta.solana.com".to_string(), None)
            .with_ethereum("https://eth.llamarpc.com".to_string(), None)
            .with_polygon("https://polygon-rpc.com".to_string(), None),
    );

    Ok(ReceiptService::new(db, blockchain_client))
}

/// Build a receipt service or skip the current test with a named reason when
/// PostgreSQL is unreachable. Expands to an early `return` on the skip path.
macro_rules! service_or_skip {
    () => {
        match create_test_service().await {
            Ok(service) => service,
            Err(reason) => {
                eprintln!("SKIPPED: PostgreSQL unavailable — {}", reason);
                return;
            }
        }
    };
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

#[tokio::test]
async fn test_create_receipt_for_payment() {
    let service = service_or_skip!();

    let payment_id = Uuid::new_v4();
    let data = ReceiptData {
        payment_id: Some(payment_id),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10050, 2), // 100.50
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let result = service.create_receipt(data.clone()).await;
    assert!(result.is_ok(), "Failed to create receipt: {:?}", result.err());

    let receipt = result.unwrap();
    assert_eq!(receipt.payment_id, Some(payment_id));
    assert_eq!(receipt.amount, data.amount);
    assert_eq!(receipt.currency, data.currency);
    assert_eq!(receipt.sender, data.sender);
    assert_eq!(receipt.recipient, data.recipient);
    assert_eq!(receipt.blockchain, Blockchain::Ethereum);
    assert!(!receipt.transaction_hash.is_empty());
    assert_eq!(receipt.verification_status, VerificationStatus::Pending);
}

#[tokio::test]
async fn test_create_receipt_for_trade() {
    let service = service_or_skip!();

    let trade_id = Uuid::new_v4();
    let data = ReceiptData {
        payment_id: None,
        trade_id: Some(trade_id),
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(50000, 2), // 500.00
        currency: "SOL".to_string(),
        sender: "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK".to_string(),
        recipient: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
        blockchain: Blockchain::Solana,
    };

    let result = service.create_receipt(data.clone()).await;
    assert!(result.is_ok(), "Failed to create receipt: {:?}", result.err());

    let receipt = result.unwrap();
    assert_eq!(receipt.trade_id, Some(trade_id));
    assert_eq!(receipt.amount, data.amount);
    assert_eq!(receipt.blockchain, Blockchain::Solana);
}

#[tokio::test]
async fn test_create_receipt_for_conversion() {
    let service = service_or_skip!();

    // First create a user and conversion in the database
    let user_id = Uuid::new_v4();
    let conversion_id = Uuid::new_v4();
    
    let db = pool_or_skip!();
    let client = db.get().await.expect("Failed to get client");
    
    // Create user
    client
        .execute(
            "INSERT INTO users (id, email, password_hash, created_at, updated_at)
             VALUES ($1, $2, 'hash', NOW(), NOW())",
            &[&user_id, &format!("test-{}@example.com", user_id)],
        )
        .await
        .expect("Failed to create test user");
    
    // Create conversion
    client
        .execute(
            "INSERT INTO conversions (id, user_id, from_asset, to_asset, from_amount, to_amount, exchange_rate, provider, status, created_at)
             VALUES ($1, $2, 'USDC', 'USDT', 250.75, 250.50, 0.999, 'SIDESHIFT', 'COMPLETED', NOW())",
            &[&conversion_id, &user_id],
        )
        .await
        .expect("Failed to create test conversion");

    let data = ReceiptData {
        payment_id: None,
        trade_id: None,
        conversion_id: Some(conversion_id),
        proximity_transfer_id: None,
        amount: Decimal::new(25075, 2), // 250.75
        currency: "USDC".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Polygon,
    };

    let result = service.create_receipt(data.clone()).await;
    assert!(result.is_ok(), "Failed to create receipt: {:?}", result.err());

    let receipt = result.unwrap();
    assert_eq!(receipt.conversion_id, Some(conversion_id));
    assert_eq!(receipt.blockchain, Blockchain::Polygon);
}

#[tokio::test]
async fn test_get_receipt_by_id() {
    let service = service_or_skip!();

    // Create a receipt first
    let payment_id = Uuid::new_v4();
    let data = ReceiptData {
        payment_id: Some(payment_id),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();

    // Retrieve the receipt by ID
    let result = service.get_receipt(created_receipt.id).await;
    assert!(result.is_ok());

    let retrieved_receipt = result.unwrap();
    assert_eq!(retrieved_receipt.id, created_receipt.id);
    assert_eq!(retrieved_receipt.payment_id, created_receipt.payment_id);
    assert_eq!(retrieved_receipt.amount, created_receipt.amount);
    assert_eq!(retrieved_receipt.transaction_hash, created_receipt.transaction_hash);
}

#[tokio::test]
async fn test_get_receipt_by_payment_id() {
    let service = service_or_skip!();

    let payment_id = Uuid::new_v4();
    let data = ReceiptData {
        payment_id: Some(payment_id),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(15000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();

    // Retrieve by payment_id
    let result = service
        .get_receipt_by_source(Some(payment_id), None, None, None)
        .await;
    assert!(result.is_ok());

    let retrieved_receipt = result.unwrap();
    assert!(retrieved_receipt.is_some());
    let receipt = retrieved_receipt.unwrap();
    assert_eq!(receipt.id, created_receipt.id);
    assert_eq!(receipt.payment_id, Some(payment_id));
}

#[tokio::test]
async fn test_get_receipt_by_trade_id() {
    let service = service_or_skip!();

    let trade_id = Uuid::new_v4();
    let data = ReceiptData {
        payment_id: None,
        trade_id: Some(trade_id),
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(20000, 2),
        currency: "ETH".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();

    // Retrieve by trade_id
    let result = service
        .get_receipt_by_source(None, Some(trade_id), None, None)
        .await;
    assert!(result.is_ok());

    let retrieved_receipt = result.unwrap();
    assert!(retrieved_receipt.is_some());
    let receipt = retrieved_receipt.unwrap();
    assert_eq!(receipt.id, created_receipt.id);
    assert_eq!(receipt.trade_id, Some(trade_id));
}

#[tokio::test]
async fn test_get_receipt_by_conversion_id() {
    let service = service_or_skip!();

    // First create a user and conversion in the database
    let user_id = Uuid::new_v4();
    let conversion_id = Uuid::new_v4();
    
    let db = pool_or_skip!();
    let client = db.get().await.expect("Failed to get client");
    
    // Create user
    client
        .execute(
            "INSERT INTO users (id, email, password_hash, created_at, updated_at)
             VALUES ($1, $2, 'hash', NOW(), NOW())",
            &[&user_id, &format!("test-{}@example.com", user_id)],
        )
        .await
        .expect("Failed to create test user");
    
    // Create conversion
    client
        .execute(
            "INSERT INTO conversions (id, user_id, from_asset, to_asset, from_amount, to_amount, exchange_rate, provider, status, created_at)
             VALUES ($1, $2, 'USDC', 'USDT', 300.00, 299.70, 0.999, 'SIDESHIFT', 'COMPLETED', NOW())",
            &[&conversion_id, &user_id],
        )
        .await
        .expect("Failed to create test conversion");

    let data = ReceiptData {
        payment_id: None,
        trade_id: None,
        conversion_id: Some(conversion_id),
        proximity_transfer_id: None,
        amount: Decimal::new(30000, 2),
        currency: "USDC".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Polygon,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();

    // Retrieve by conversion_id
    let result = service
        .get_receipt_by_source(None, None, Some(conversion_id), None)
        .await;
    assert!(result.is_ok());

    let retrieved_receipt = result.unwrap();
    assert!(retrieved_receipt.is_some());
    let receipt = retrieved_receipt.unwrap();
    assert_eq!(receipt.id, created_receipt.id);
    assert_eq!(receipt.conversion_id, Some(conversion_id));
}

#[tokio::test]
async fn test_get_nonexistent_receipt() {
    let service = service_or_skip!();

    let nonexistent_id = Uuid::new_v4();
    let result = service.get_receipt(nonexistent_id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_receipt_has_transaction_hash() {
    let service = service_or_skip!();

    let data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let receipt = service.create_receipt(data).await.unwrap();

    // Verify transaction hash is not empty
    assert!(!receipt.transaction_hash.is_empty());
    // Verify it has some reasonable format
    assert!(receipt.transaction_hash.len() > 10);
}

#[tokio::test]
async fn test_receipt_links_to_source_transaction() {
    let service = service_or_skip!();

    let payment_id = Uuid::new_v4();
    let data = ReceiptData {
        payment_id: Some(payment_id),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let receipt = service.create_receipt(data).await.unwrap();

    // Verify the receipt is linked to the payment
    assert_eq!(receipt.payment_id, Some(payment_id));
    assert!(receipt.trade_id.is_none());
    assert!(receipt.conversion_id.is_none());
}

#[tokio::test]
async fn test_receipt_stored_in_database() {
    let service = service_or_skip!();

    let payment_id = Uuid::new_v4();
    let data = ReceiptData {
        payment_id: Some(payment_id),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();

    // Verify we can retrieve it from the database
    let retrieved = service.get_receipt(created_receipt.id).await;
    assert!(retrieved.is_ok());

    let receipt = retrieved.unwrap();
    assert_eq!(receipt.id, created_receipt.id);
    assert_eq!(receipt.amount, created_receipt.amount);
    assert_eq!(receipt.currency, created_receipt.currency);
    assert_eq!(receipt.sender, created_receipt.sender);
    assert_eq!(receipt.recipient, created_receipt.recipient);
    assert_eq!(receipt.blockchain, created_receipt.blockchain);
    assert_eq!(receipt.transaction_hash, created_receipt.transaction_hash);
}

#[tokio::test]
async fn test_multiple_receipts_for_different_blockchains() {
    let service = service_or_skip!();

    // Create receipt on Ethereum
    let eth_data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let eth_receipt = service.create_receipt(eth_data).await.unwrap();

    // Create receipt on Solana
    let sol_data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(20000, 2),
        currency: "SOL".to_string(),
        sender: "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK".to_string(),
        recipient: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
        blockchain: Blockchain::Solana,
    };

    let sol_receipt = service.create_receipt(sol_data).await.unwrap();

    // Create receipt on Polygon
    let poly_data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(30000, 2),
        currency: "MATIC".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Polygon,
    };

    let poly_receipt = service.create_receipt(poly_data).await.unwrap();

    // Verify all receipts are distinct and have correct blockchains
    assert_ne!(eth_receipt.id, sol_receipt.id);
    assert_ne!(eth_receipt.id, poly_receipt.id);
    assert_ne!(sol_receipt.id, poly_receipt.id);

    assert_eq!(eth_receipt.blockchain, Blockchain::Ethereum);
    assert_eq!(sol_receipt.blockchain, Blockchain::Solana);
    assert_eq!(poly_receipt.blockchain, Blockchain::Polygon);
}

#[tokio::test]
async fn test_verify_receipt_success() {
    let service = service_or_skip!();

    // Create a receipt
    let data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();
    assert_eq!(created_receipt.verification_status, VerificationStatus::Pending);

    // Verify the receipt
    let result = service.verify_receipt(created_receipt.id).await;
    assert!(result.is_ok(), "Failed to verify receipt: {:?}", result.err());

    let verified_receipt = result.unwrap();
    assert_eq!(verified_receipt.id, created_receipt.id);
    assert_eq!(verified_receipt.verification_status, VerificationStatus::Confirmed);
    assert!(verified_receipt.verified_at.is_some());
}

#[tokio::test]
async fn test_verify_receipt_already_confirmed() {
    let service = service_or_skip!();

    // Create and verify a receipt
    let data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();
    let verified_receipt = service.verify_receipt(created_receipt.id).await.unwrap();
    assert_eq!(verified_receipt.verification_status, VerificationStatus::Confirmed);

    // Verify again - should return immediately without re-checking
    let result = service.verify_receipt(created_receipt.id).await;
    assert!(result.is_ok());

    let second_verification = result.unwrap();
    assert_eq!(second_verification.verification_status, VerificationStatus::Confirmed);
    assert_eq!(second_verification.verified_at, verified_receipt.verified_at);
}

#[tokio::test]
async fn test_verify_receipt_for_solana() {
    let service = service_or_skip!();

    // Create a Solana receipt
    let data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(50000, 2),
        currency: "SOL".to_string(),
        sender: "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK".to_string(),
        recipient: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
        blockchain: Blockchain::Solana,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();
    assert_eq!(created_receipt.blockchain, Blockchain::Solana);

    // Verify the receipt
    let result = service.verify_receipt(created_receipt.id).await;
    assert!(result.is_ok());

    let verified_receipt = result.unwrap();
    assert_eq!(verified_receipt.verification_status, VerificationStatus::Confirmed);
}

#[tokio::test]
async fn test_verify_receipt_for_polygon() {
    let service = service_or_skip!();

    // Create a Polygon receipt
    let data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(30000, 2),
        currency: "MATIC".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Polygon,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();
    assert_eq!(created_receipt.blockchain, Blockchain::Polygon);

    // Verify the receipt
    let result = service.verify_receipt(created_receipt.id).await;
    assert!(result.is_ok());

    let verified_receipt = result.unwrap();
    assert_eq!(verified_receipt.verification_status, VerificationStatus::Confirmed);
}

#[tokio::test]
async fn test_verify_nonexistent_receipt() {
    let service = service_or_skip!();

    let nonexistent_id = Uuid::new_v4();
    let result = service.verify_receipt(nonexistent_id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_verify_receipt_updates_database() {
    let service = service_or_skip!();

    // Create a receipt
    let data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();

    // Verify the receipt
    service.verify_receipt(created_receipt.id).await.unwrap();

    // Fetch the receipt again to verify the status was persisted
    let fetched_receipt = service.get_receipt(created_receipt.id).await.unwrap();
    assert_eq!(fetched_receipt.verification_status, VerificationStatus::Confirmed);
    assert!(fetched_receipt.verified_at.is_some());
}

#[tokio::test]
async fn test_verify_receipt_returns_verification_status() {
    let service = service_or_skip!();

    // Create a receipt
    let data = ReceiptData {
        payment_id: Some(Uuid::new_v4()),
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: None,
        amount: Decimal::new(10000, 2),
        currency: "USD".to_string(),
        sender: "0x742d35Cc6634C0532925a3b844Bc9e7595f0bEb0".to_string(),
        recipient: "0x8ba1f109551bD432803012645Ac136ddd64DBA72".to_string(),
        blockchain: Blockchain::Ethereum,
    };

    let created_receipt = service.create_receipt(data).await.unwrap();

    // Verify and check the returned receipt has the correct status
    let verified_receipt = service.verify_receipt(created_receipt.id).await.unwrap();
    
    // The verification status should be Confirmed for valid transaction hashes
    assert_eq!(verified_receipt.verification_status, VerificationStatus::Confirmed);
}
