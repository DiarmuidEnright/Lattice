use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::NoTls;

pub mod migrations;
pub mod proximity;
pub mod redis_client;

pub use proximity::*;
pub use redis_client::{create_redis_client, create_redis_pool, RedisPool};

pub type DbPool = Pool;

pub async fn create_pool(database_url: &str, max_connections: u32) -> anyhow::Result<DbPool> {
    tracing::info!("Creating database connection pool");
    
    let mut cfg = Config::new();
    cfg.url = Some(database_url.to_string());
    cfg.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });
    cfg.pool = Some(deadpool_postgres::PoolConfig::new(max_connections as usize));
    
    let pool = cfg.create_pool(Some(Runtime::Tokio1), NoTls)?;
    
    Ok(pool)
}

/// The complete, ordered set of database migrations.
///
/// Each entry is `(version, sql)` where `version` is a stable unique key
/// recorded in the `schema_migrations` tracking table so a migration is applied
/// at most once. The list is ordered to respect foreign-key dependencies (a
/// table is created before anything that references it).
///
/// Notes on the historical migration files this list reconciles:
/// - Several files share a numeric prefix (three `...018_*`, two `...020_*`,
///   two `...025_*`). They touch *different* objects, so all are included with
///   distinct version keys.
/// - `...017_add_user_tag_to_users.sql` is a duplicate of
///   `...027_add_user_tag_column.sql` (identical columns + index). Only the
///   `027` form is included; including both would fail with a duplicate-column
///   error.
const MIGRATIONS: &[(&str, &str)] = &[
    ("20240101000001_create_users_table", include_str!("../migrations/20240101000001_create_users_table.sql")),
    ("20240101000002_create_wallets_table", include_str!("../migrations/20240101000002_create_wallets_table.sql")),
    ("20240101000003_create_portfolio_assets_table", include_str!("../migrations/20240101000003_create_portfolio_assets_table.sql")),
    ("20240101000004_create_whales_table", include_str!("../migrations/20240101000004_create_whales_table.sql")),
    ("20240101000005_create_user_whale_tracking_table", include_str!("../migrations/20240101000005_create_user_whale_tracking_table.sql")),
    ("20240101000006_create_whale_movements_table", include_str!("../migrations/20240101000006_create_whale_movements_table.sql")),
    ("20240101000007_create_recommendations_table", include_str!("../migrations/20240101000007_create_recommendations_table.sql")),
    ("20240101000008_create_trade_executions_table", include_str!("../migrations/20240101000008_create_trade_executions_table.sql")),
    ("20240101000009_create_notifications_table", include_str!("../migrations/20240101000009_create_notifications_table.sql")),
    ("20240101000010_create_notification_preferences_table", include_str!("../migrations/20240101000010_create_notification_preferences_table.sql")),
    ("20240101000011_create_subscriptions_table", include_str!("../migrations/20240101000011_create_subscriptions_table.sql")),
    ("20240101000012_create_user_settings_table", include_str!("../migrations/20240101000012_create_user_settings_table.sql")),
    ("20240101000013_create_portfolio_snapshots_table", include_str!("../migrations/20240101000013_create_portfolio_snapshots_table.sql")),
    ("20240101000014_create_multi_chain_wallets_table", include_str!("../migrations/20240101000014_create_multi_chain_wallets_table.sql")),
    ("20240101000015_create_benchmarks_table", include_str!("../migrations/20240101000015_create_benchmarks_table.sql")),
    ("20240101000016_create_conversions_table", include_str!("../migrations/20240101000016_create_conversions_table.sql")),
    ("20240101000017_create_staking_positions_table", include_str!("../migrations/20240101000017_create_staking_positions_table.sql")),
    ("20240101000018_create_trim_configs_table", include_str!("../migrations/20240101000018_create_trim_configs_table.sql")),
    ("20240101000018_create_staking_approval_and_config_tables", include_str!("../migrations/20240101000018_create_staking_approval_and_config_tables.sql")),
    ("20240101000018_add_2fa_to_users", include_str!("../migrations/20240101000018_add_2fa_to_users.sql")),
    ("20240101000019_create_trim_executions_table", include_str!("../migrations/20240101000019_create_trim_executions_table.sql")),
    ("20240101000020_create_pending_trims_table", include_str!("../migrations/20240101000020_create_pending_trims_table.sql")),
    ("20240101000020_create_voice_commands_table", include_str!("../migrations/20240101000020_create_voice_commands_table.sql")),
    ("20240101000021_create_blockchain_receipts_table", include_str!("../migrations/20240101000021_create_blockchain_receipts_table.sql")),
    ("20240101000022_create_chat_messages_table", include_str!("../migrations/20240101000022_create_chat_messages_table.sql")),
    ("20240101000023_create_p2p_offers_table", include_str!("../migrations/20240101000023_create_p2p_offers_table.sql")),
    ("20240101000024_create_p2p_exchanges_table", include_str!("../migrations/20240101000024_create_p2p_exchanges_table.sql")),
    ("20240101000025_create_identity_verifications_table", include_str!("../migrations/20240101000025_create_identity_verifications_table.sql")),
    ("20240101000025_add_receipt_retention_policy", include_str!("../migrations/20240101000025_add_receipt_retention_policy.sql")),
    ("20240101000026_create_wallet_verifications_table", include_str!("../migrations/20240101000026_create_wallet_verifications_table.sql")),
    ("20240101000027_add_user_tag_column", include_str!("../migrations/20240101000027_add_user_tag_column.sql")),
    ("20240101000028_create_position_modes_table", include_str!("../migrations/20240101000028_create_position_modes_table.sql")),
    ("20240101000029_create_proximity_transfers_table", include_str!("../migrations/20240101000029_create_proximity_transfers_table.sql")),
    ("20240101000030_create_discovery_sessions_table", include_str!("../migrations/20240101000030_create_discovery_sessions_table.sql")),
    ("20240101000031_create_peer_blocklist_table", include_str!("../migrations/20240101000031_create_peer_blocklist_table.sql")),
    ("20240101000032_add_proximity_transfer_to_receipts", include_str!("../migrations/20240101000032_add_proximity_transfer_to_receipts.sql")),
    ("20240101000033_add_proximity_flag_to_p2p_offers", include_str!("../migrations/20240101000033_add_proximity_flag_to_p2p_offers.sql")),
    ("20240101000034_add_transaction_type_to_proximity_transfers", include_str!("../migrations/20240101000034_add_transaction_type_to_proximity_transfers.sql")),
    ("20240101000035_add_acceptor_fields_to_p2p_offers", include_str!("../migrations/20240101000035_add_acceptor_fields_to_p2p_offers.sql")),
    ("20240101000036_create_chat_conversations_tables", include_str!("../migrations/20240101000036_create_chat_conversations_tables.sql")),
    ("20240101000037_create_mesh_price_cache_table", include_str!("../migrations/20240101000037_create_mesh_price_cache_table.sql")),
    ("20240101000038_create_mesh_seen_messages_table", include_str!("../migrations/20240101000038_create_mesh_seen_messages_table.sql")),
];

/// Returns true when the error is a benign "object already exists" class error,
/// i.e. the migration's effect is already present in the schema. This lets the
/// runner baseline itself against a database that was provisioned out-of-band
/// (so the objects exist but `schema_migrations` has no record of them) without
/// requiring every historical `.sql` file to be rewritten with `IF NOT EXISTS`.
fn is_already_exists_error(err: &tokio_postgres::Error) -> bool {
    use tokio_postgres::error::SqlState;
    match err.code() {
        Some(code) => matches!(
            *code,
            SqlState::DUPLICATE_TABLE      // relation/index already exists (42P07)
                | SqlState::DUPLICATE_COLUMN   // column already exists (42701)
                | SqlState::DUPLICATE_OBJECT   // e.g. trigger already exists (42710)
                | SqlState::DUPLICATE_FUNCTION // function already exists (42723)
                | SqlState::DUPLICATE_SCHEMA   // schema already exists (42P06)
        ),
        None => false,
    }
}

/// Apply all pending database migrations exactly once each, in dependency order.
///
/// Behavior:
/// - A `schema_migrations(version, applied_at)` table records which migrations
///   have been applied; already-applied versions are skipped.
/// - Each migration runs inside its own transaction, so a failure rolls back
///   that migration cleanly and aborts the run (no partial application).
/// - If a migration fails because its objects already exist (a database
///   provisioned out-of-band), it is treated as already-applied and recorded,
///   so the runner converges a hand-provisioned database to a tracked state
///   instead of erroring. This makes the runner safe on both a pristine
///   database and a pre-populated one, which is why `SKIP_MIGRATIONS` is no
///   longer required for the latter.
pub async fn run_migrations(pool: &DbPool) -> anyhow::Result<()> {
    tracing::info!("Running database migrations");

    let mut client = pool.get().await?;

    // Tracking table for applied migrations (idempotent).
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version TEXT PRIMARY KEY,
                applied_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
            )",
        )
        .await?;

    let mut applied = 0usize;
    let mut baselined = 0usize;
    let mut skipped = 0usize;

    for (version, sql) in MIGRATIONS {
        // Skip migrations already recorded as applied.
        let already = client
            .query_opt(
                "SELECT 1 FROM schema_migrations WHERE version = $1",
                &[version],
            )
            .await?
            .is_some();
        if already {
            skipped += 1;
            continue;
        }

        // Run the migration in its own transaction so a failure does not leave
        // the schema half-changed.
        let tx = client.transaction().await?;
        match tx.batch_execute(sql).await {
            Ok(()) => {
                tx.execute(
                    "INSERT INTO schema_migrations (version) VALUES ($1)
                     ON CONFLICT (version) DO NOTHING",
                    &[version],
                )
                .await?;
                tx.commit().await?;
                tracing::info!("Applied migration {}", version);
                applied += 1;
            }
            Err(err) if is_already_exists_error(&err) => {
                // The migration's objects already exist (out-of-band
                // provisioning). Roll back the failed attempt, then record the
                // version so future runs skip it.
                drop(tx);
                client
                    .execute(
                        "INSERT INTO schema_migrations (version) VALUES ($1)
                         ON CONFLICT (version) DO NOTHING",
                        &[version],
                    )
                    .await?;
                tracing::info!(
                    "Baselined migration {} (objects already present)",
                    version
                );
                baselined += 1;
            }
            Err(err) => {
                return Err(anyhow::anyhow!(
                    "Migration {} failed: {}",
                    version,
                    err
                ));
            }
        }
    }

    tracing::info!(
        "Database migrations completed: {} applied, {} baselined, {} already up to date",
        applied,
        baselined,
        skipped
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Only run with a real database
    async fn test_pool_creation() {
        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "postgresql://postgres:password@localhost:5432/test".to_string());
        
        let pool = create_pool(&database_url, 5).await;
        assert!(pool.is_ok());
    }
}