use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use uuid::Uuid;
use crate::DbPool;

/// Represents a proximity transfer record from the database
#[derive(Debug, Clone)]
pub struct ProximityTransferRecord {
    pub id: Uuid,
    pub sender_user_id: Uuid,
    pub sender_wallet: String,
    pub recipient_user_id: Uuid,
    pub recipient_wallet: String,
    pub asset: String,
    pub amount: Decimal,
    pub transaction_hash: Option<String>,
    pub status: String,
    pub discovery_method: String,
    pub transaction_type: String,
    pub created_at: DateTime<Utc>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub failed_reason: Option<String>,
}

/// Represents a discovery session record from the database
#[derive(Debug, Clone)]
pub struct DiscoverySessionRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub discovery_method: String,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub auto_extend: bool,
}

/// Represents a blocked peer record from the database
#[derive(Debug, Clone)]
pub struct BlockedPeerRecord {
    pub user_id: Uuid,
    pub blocked_user_id: Uuid,
    pub blocked_at: DateTime<Utc>,
}

/// Filter options for querying proximity transfers
#[derive(Debug, Clone, Default)]
pub struct TransferFilter {
    pub user_id: Option<Uuid>,
    pub status: Option<String>,
    pub asset: Option<String>,
    pub transaction_type: Option<String>,
    pub from_date: Option<DateTime<Utc>>,
    pub to_date: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

// ============================================================================
// Proximity Transfers Functions
// ============================================================================

/// Insert a new proximity transfer record
pub async fn insert_proximity_transfer(
    pool: &DbPool,
    sender_user_id: Uuid,
    sender_wallet: &str,
    recipient_user_id: Uuid,
    recipient_wallet: &str,
    asset: &str,
    amount: Decimal,
    status: &str,
    discovery_method: &str,
    transaction_type: &str,
) -> anyhow::Result<Uuid> {
    let client = pool.get().await?;
    
    let row = client
        .query_one(
            "INSERT INTO proximity_transfers 
             (sender_user_id, sender_wallet, recipient_user_id, recipient_wallet, 
              asset, amount, status, discovery_method, transaction_type)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING id",
            &[
                &sender_user_id,
                &sender_wallet,
                &recipient_user_id,
                &recipient_wallet,
                &asset,
                &amount,
                &status,
                &discovery_method,
                &transaction_type,
            ],
        )
        .await?;
    
    Ok(row.get(0))
}

/// Update the status of a proximity transfer
pub async fn update_transfer_status(
    pool: &DbPool,
    transfer_id: Uuid,
    status: &str,
    transaction_hash: Option<&str>,
    failed_reason: Option<&str>,
) -> anyhow::Result<()> {
    let client = pool.get().await?;
    
    let now = Utc::now();
    
    // Handle different status update scenarios
    match (status, transaction_hash, failed_reason) {
        ("Accepted", None, None) => {
            client.execute(
                "UPDATE proximity_transfers SET status = $1, accepted_at = $2 WHERE id = $3",
                &[&status, &now, &transfer_id],
            ).await?;
        }
        ("Completed", Some(hash), None) => {
            client.execute(
                "UPDATE proximity_transfers SET status = $1, transaction_hash = $2, completed_at = $3 WHERE id = $4",
                &[&status, &hash, &now, &transfer_id],
            ).await?;
        }
        ("Completed", None, None) => {
            client.execute(
                "UPDATE proximity_transfers SET status = $1, completed_at = $2 WHERE id = $3",
                &[&status, &now, &transfer_id],
            ).await?;
        }
        ("Failed", hash_opt, Some(reason)) => {
            if let Some(hash) = hash_opt {
                client.execute(
                    "UPDATE proximity_transfers SET status = $1, transaction_hash = $2, failed_reason = $3 WHERE id = $4",
                    &[&status, &hash, &reason, &transfer_id],
                ).await?;
            } else {
                client.execute(
                    "UPDATE proximity_transfers SET status = $1, failed_reason = $2 WHERE id = $3",
                    &[&status, &reason, &transfer_id],
                ).await?;
            }
        }
        ("Executing", hash_opt, None) => {
            if let Some(hash) = hash_opt {
                client.execute(
                    "UPDATE proximity_transfers SET status = $1, transaction_hash = $2 WHERE id = $3",
                    &[&status, &hash, &transfer_id],
                ).await?;
            } else {
                client.execute(
                    "UPDATE proximity_transfers SET status = $1 WHERE id = $2",
                    &[&status, &transfer_id],
                ).await?;
            }
        }
        _ => {
            // Default case: just update status
            client.execute(
                "UPDATE proximity_transfers SET status = $1 WHERE id = $2",
                &[&status, &transfer_id],
            ).await?;
        }
    }
    
    Ok(())
}

/// Get a proximity transfer by ID
pub async fn get_transfer_by_id(
    pool: &DbPool,
    transfer_id: Uuid,
) -> anyhow::Result<Option<ProximityTransferRecord>> {
    let client = pool.get().await?;
    
    let row = client
        .query_opt(
            "SELECT id, sender_user_id, sender_wallet, recipient_user_id, recipient_wallet,
                    asset, amount, transaction_hash, status, discovery_method, transaction_type,
                    created_at, accepted_at, completed_at, failed_reason
             FROM proximity_transfers
             WHERE id = $1",
            &[&transfer_id],
        )
        .await?;
    
    Ok(row.map(|r| ProximityTransferRecord {
        id: r.get(0),
        sender_user_id: r.get(1),
        sender_wallet: r.get(2),
        recipient_user_id: r.get(3),
        recipient_wallet: r.get(4),
        asset: r.get(5),
        amount: r.get(6),
        transaction_hash: r.get(7),
        status: r.get(8),
        discovery_method: r.get(9),
        transaction_type: r.get(10),
        // These columns are `timestamp without time zone`; read as
        // NaiveDateTime and convert to UTC (reading into DateTime<Utc> panics).
        created_at: r.get::<_, chrono::NaiveDateTime>(11).and_utc(),
        accepted_at: r.get::<_, Option<chrono::NaiveDateTime>>(12).map(|t| t.and_utc()),
        completed_at: r.get::<_, Option<chrono::NaiveDateTime>>(13).map(|t| t.and_utc()),
        failed_reason: r.get(14),
    }))
}

/// Get proximity transfers for a user with optional filtering
pub async fn get_user_proximity_transfers(
    pool: &DbPool,
    filter: TransferFilter,
) -> anyhow::Result<Vec<ProximityTransferRecord>> {
    let client = pool.get().await?;
    
    // Build dynamic query
    let mut query = String::from(
        "SELECT id, sender_user_id, sender_wallet, recipient_user_id, recipient_wallet,
                asset, amount, transaction_hash, status, discovery_method, transaction_type,
                created_at, accepted_at, completed_at, failed_reason
         FROM proximity_transfers
         WHERE 1=1"
    );
    
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
    let mut param_count = 1;
    
    if let Some(user_id) = filter.user_id {
        query.push_str(&format!(
            " AND (sender_user_id = ${} OR recipient_user_id = ${})",
            param_count, param_count
        ));
        params.push(Box::new(user_id));
        param_count += 1;
    }
    
    if let Some(status) = filter.status {
        query.push_str(&format!(" AND status = ${}", param_count));
        params.push(Box::new(status));
        param_count += 1;
    }
    
    if let Some(asset) = filter.asset {
        query.push_str(&format!(" AND asset = ${}", param_count));
        params.push(Box::new(asset));
        param_count += 1;
    }
    
    if let Some(transaction_type) = filter.transaction_type {
        query.push_str(&format!(" AND transaction_type = ${}", param_count));
        params.push(Box::new(transaction_type));
        param_count += 1;
    }
    
    if let Some(from_date) = filter.from_date {
        query.push_str(&format!(" AND created_at >= ${}", param_count));
        params.push(Box::new(from_date));
        param_count += 1;
    }
    
    if let Some(to_date) = filter.to_date {
        query.push_str(&format!(" AND created_at <= ${}", param_count));
        params.push(Box::new(to_date));
        param_count += 1;
    }
    
    query.push_str(" ORDER BY created_at DESC");
    
    if let Some(limit) = filter.limit {
        query.push_str(&format!(" LIMIT ${}", param_count));
        params.push(Box::new(limit));
        param_count += 1;
    }
    
    if let Some(offset) = filter.offset {
        query.push_str(&format!(" OFFSET ${}", param_count));
        params.push(Box::new(offset));
    }
    
    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = 
        params.iter().map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync)).collect();
    
    let rows = client.query(&query, &param_refs).await?;
    
    Ok(rows
        .into_iter()
        .map(|r| ProximityTransferRecord {
            id: r.get(0),
            sender_user_id: r.get(1),
            sender_wallet: r.get(2),
            recipient_user_id: r.get(3),
            recipient_wallet: r.get(4),
            asset: r.get(5),
            amount: r.get(6),
            transaction_hash: r.get(7),
            status: r.get(8),
            discovery_method: r.get(9),
            transaction_type: r.get(10),
            // `timestamp without time zone` columns -> read as NaiveDateTime.
            created_at: r.get::<_, chrono::NaiveDateTime>(11).and_utc(),
            accepted_at: r.get::<_, Option<chrono::NaiveDateTime>>(12).map(|t| t.and_utc()),
            completed_at: r.get::<_, Option<chrono::NaiveDateTime>>(13).map(|t| t.and_utc()),
            failed_reason: r.get(14),
        })
        .collect())
}

// ============================================================================
// Discovery Sessions Functions
// ============================================================================

/// Insert a new discovery session
pub async fn insert_discovery_session(
    pool: &DbPool,
    user_id: Uuid,
    discovery_method: &str,
    expires_at: DateTime<Utc>,
    auto_extend: bool,
) -> anyhow::Result<Uuid> {
    let client = pool.get().await?;
    
    let row = client
        .query_one(
            "INSERT INTO discovery_sessions 
             (user_id, discovery_method, expires_at, auto_extend)
             VALUES ($1, $2, $3, $4)
             RETURNING id",
            &[&user_id, &discovery_method, &expires_at, &auto_extend],
        )
        .await?;
    
    Ok(row.get(0))
}

/// Update the expiration time of a discovery session
pub async fn update_session_expiration(
    pool: &DbPool,
    session_id: Uuid,
    new_expires_at: DateTime<Utc>,
) -> anyhow::Result<()> {
    let client = pool.get().await?;
    
    client
        .execute(
            "UPDATE discovery_sessions 
             SET expires_at = $1
             WHERE id = $2 AND ended_at IS NULL",
            &[&new_expires_at, &session_id],
        )
        .await?;
    
    Ok(())
}

/// End a discovery session
pub async fn end_discovery_session(
    pool: &DbPool,
    session_id: Uuid,
) -> anyhow::Result<()> {
    let client = pool.get().await?;
    
    let now = Utc::now();
    client
        .execute(
            "UPDATE discovery_sessions 
             SET ended_at = $1
             WHERE id = $2 AND ended_at IS NULL",
            &[&now, &session_id],
        )
        .await?;
    
    Ok(())
}

// ============================================================================
// Peer Blocklist Functions
// ============================================================================

/// Add a peer to the blocklist
pub async fn add_blocked_peer(
    pool: &DbPool,
    user_id: Uuid,
    blocked_user_id: Uuid,
) -> anyhow::Result<()> {
    let client = pool.get().await?;
    
    client
        .execute(
            "INSERT INTO peer_blocklist (user_id, blocked_user_id)
             VALUES ($1, $2)
             ON CONFLICT (user_id, blocked_user_id) DO NOTHING",
            &[&user_id, &blocked_user_id],
        )
        .await?;
    
    Ok(())
}

/// Remove a peer from the blocklist
pub async fn remove_blocked_peer(
    pool: &DbPool,
    user_id: Uuid,
    blocked_user_id: Uuid,
) -> anyhow::Result<()> {
    let client = pool.get().await?;
    
    client
        .execute(
            "DELETE FROM peer_blocklist
             WHERE user_id = $1 AND blocked_user_id = $2",
            &[&user_id, &blocked_user_id],
        )
        .await?;
    
    Ok(())
}

/// Get all blocked peers for a user
pub async fn get_blocked_peers(
    pool: &DbPool,
    user_id: Uuid,
) -> anyhow::Result<Vec<BlockedPeerRecord>> {
    let client = pool.get().await?;
    
    let rows = client
        .query(
            "SELECT user_id, blocked_user_id, blocked_at
             FROM peer_blocklist
             WHERE user_id = $1
             ORDER BY blocked_at DESC",
            &[&user_id],
        )
        .await?;
    
    Ok(rows
        .into_iter()
        .map(|r| BlockedPeerRecord {
            user_id: r.get(0),
            blocked_user_id: r.get(1),
            blocked_at: r.get(2),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Only run with a real database
    async fn test_insert_proximity_transfer() {
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://postgres:password@localhost:5432/test".to_string());
        
        let pool = crate::create_pool(&database_url, 5).await.unwrap();
        
        let sender_id = Uuid::new_v4();
        let recipient_id = Uuid::new_v4();
        
        let transfer_id = insert_proximity_transfer(
            &pool,
            sender_id,
            "sender_wallet_address",
            recipient_id,
            "recipient_wallet_address",
            "SOL",
            Decimal::new(100, 2), // 1.00 SOL
            "Pending",
            "WiFi",
            "DIRECT_TRANSFER",
        )
        .await
        .unwrap();
        
        assert!(!transfer_id.is_nil());
    }
}
