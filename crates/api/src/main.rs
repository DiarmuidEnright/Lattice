use anyhow::{Context, Result};
use api::{AnalyticsService, AppState, BenchmarkService, ChatService, CoinMarketCapService, ConversionService, MeshPriceService, P2PService, PaymentReceiptService, PortfolioMonitor, PositionEvaluator, PositionManagementService, PriceMonitor, PrivacyService, ReceiptService, SideShiftClient, StakingService, TrimConfigService, TrimExecutor, VerificationService, WalletService, WebSocketService, WhaleDetectionService};
use blockchain::SolanaClient;
use database::{create_pool, create_redis_client, create_redis_pool, run_migrations};
use notification::NotificationService;
use shared::config::Config;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Best-effort system hostname for building a unique proximity device id when
/// `PROXIMITY_DEVICE_ID`/`HOSTNAME` are not set. Reads the `hostname` command
/// output; returns None if unavailable (the caller then falls back to "node").
fn hostname_fallback() -> Option<String> {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().split('.').next().unwrap_or("node").to_string())
        .filter(|s| !s.is_empty())
}

/// Load (or create) the backend-custodied devnet demo signing keypair and ensure
/// it holds some devnet SOL (via faucet airdrop) so it can fund real transfers.
///
/// The keypair is read from `DEMO_SOLANA_KEYPAIR` (base58-encoded 64-byte secret
/// key) when present, so the wallet is stable across restarts; otherwise a fresh
/// keypair is generated and its secret is logged once so the operator can pin it
/// via the env var. This is a DEVNET-ONLY demo facility — no real funds.
async fn load_or_create_demo_signer(
    solana_client: &std::sync::Arc<SolanaClient>,
) -> anyhow::Result<solana_sdk::signature::Keypair> {
    use solana_sdk::signature::{Keypair, Signer};

    let keypair = match std::env::var("DEMO_SOLANA_KEYPAIR") {
        Ok(b58) if !b58.trim().is_empty() => {
            let bytes = solana_sdk::bs58::decode(b58.trim())
                .into_vec()
                .context("DEMO_SOLANA_KEYPAIR is not valid base58")?;
            Keypair::from_bytes(&bytes)
                .map_err(|e| anyhow::anyhow!("DEMO_SOLANA_KEYPAIR invalid: {}", e))?
        }
        _ => {
            let kp = Keypair::new();
            tracing::warn!(
                "Generated ephemeral demo Solana keypair {}. To persist it across \
                 restarts set DEMO_SOLANA_KEYPAIR={}",
                kp.pubkey(),
                solana_sdk::bs58::encode(kp.to_bytes()).into_string()
            );
            kp
        }
    };

    let pubkey = keypair.pubkey();
    tracing::info!("Demo Solana signer pubkey: {}", pubkey);

    // Ensure the wallet has some balance; airdrop if it's low. Run the blocking
    // RPC calls off the async runtime. Airdrop failures (e.g. faucet rate limit)
    // are non-fatal — an already-funded wallet still works.
    let client = std::sync::Arc::clone(solana_client);
    tokio::task::spawn_blocking(move || {
        let rpc = client.primary_client();
        let balance = rpc.get_balance(&pubkey).unwrap_or(0);
        tracing::info!("Demo Solana signer balance: {} lamports", balance);
        // Top up if below 0.05 SOL.
        if balance < 50_000_000 {
            match rpc.request_airdrop(&pubkey, 1_000_000_000) {
                Ok(sig) => tracing::info!("Requested devnet airdrop (sig {}); funds may take a few seconds", sig),
                Err(e) => tracing::warn!("Devnet airdrop request failed (non-fatal): {}", e),
            }
        }
    })
    .await
    .ok();

    Ok(keypair)
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,solana_whale_tracker=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Solana Whale Tracker API");

    // Load configuration
    let config = Config::from_env()?;
    tracing::info!("Configuration loaded successfully");

    // Create database pool
    let db_pool = create_pool(&config.database.url, config.database.max_connections).await?;
    tracing::info!("Database connection pool created");

    // Run migrations (skip if SKIP_MIGRATIONS=true)
    if std::env::var("SKIP_MIGRATIONS").unwrap_or_default() != "true" {
        run_migrations(&db_pool).await?;
        tracing::info!("Database migrations completed");
    } else {
        tracing::info!("Skipping database migrations (SKIP_MIGRATIONS=true)");
    }

    // Create Redis client and pool
    let redis_client = create_redis_client(&config.redis.url).await?;
    let redis_pool = create_redis_pool(redis_client).await?;
    tracing::info!("Redis connection established");

    // Initialize Solana client. The public devnet RPC (api.devnet.solana.com)
    // is unreliable (frequent timeouts), so when a Helius key is available and
    // the configured RPC is the public devnet endpoint, prefer the Helius devnet
    // RPC for actual transaction submission.
    let helius_api_key = std::env::var("HELIUS_API_KEY")
        .context("Missing required environment variable: HELIUS_API_KEY")?;
    let solana_rpc_url = if config.solana.rpc_url.contains("api.devnet.solana.com")
        && !helius_api_key.is_empty()
    {
        let url = format!("https://devnet.helius-rpc.com/?api-key={}", helius_api_key);
        tracing::info!("Using Helius devnet RPC for Solana client (reliable endpoint)");
        url
    } else {
        config.solana.rpc_url.clone()
    };
    let solana_client = Arc::new(SolanaClient::new(solana_rpc_url, None));
    tracing::info!("Solana client initialized");

    // Initialize Helius client for wallet analytics
    let use_mainnet = std::env::var("USE_MAINNET")
        .unwrap_or_else(|_| "false".to_string())
        .parse::<bool>()
        .unwrap_or(false);
    
    let tantum_client = Arc::new(api::TantumClient::new(
        helius_api_key,
        use_mainnet,
    ));
    tracing::info!("Helius client initialized (using {})", if use_mainnet { "mainnet" } else { "devnet" });

    // Initialize services
    let wallet_service = Arc::new(WalletService::new_with_tantum(
        solana_client.clone(),
        tantum_client.clone(),
        db_pool.clone(),
        redis_pool.clone(),
        use_mainnet,
    ));
    tracing::info!("Wallet service initialized with Helius API integration");

    let whale_detection_service = Arc::new(WhaleDetectionService::new(
        solana_client.clone(),
        db_pool.clone(),
        redis_pool.clone(),
    ));
    tracing::info!("Whale detection service initialized");

    let analytics_service = Arc::new(AnalyticsService::new(db_pool.clone()));
    tracing::info!("Analytics service initialized");

    let benchmark_service = Arc::new(BenchmarkService::new(db_pool.clone()));
    tracing::info!("Benchmark service initialized");

    // Initialize CoinMarketCap service for real-time crypto prices
    let coinmarketcap_api_key = std::env::var("COINMARKETCAP_API_KEY")
        .context("Missing required environment variable: COINMARKETCAP_API_KEY")?;
    let coinmarketcap_service = Arc::new(CoinMarketCapService::new(
        coinmarketcap_api_key,
        redis_pool.clone(),
    ));
    tracing::info!("CoinMarketCap service initialized");

    // Initialize SideShift client for conversions
    let sideshift_client = Arc::new(SideShiftClient::new(
        config.sideshift.affiliate_id.clone(),
    ));
    tracing::info!("SideShift client initialized");

    // Initialize multi-chain blockchain client for receipts
    let multi_chain_client = Arc::new(blockchain::MultiChainClient::new());
    tracing::info!("Multi-chain blockchain client initialized");

    // Initialize receipt service for blockchain receipts
    let receipt_service = Arc::new(ReceiptService::new(
        db_pool.clone(),
        multi_chain_client.clone(),
    ));
    tracing::info!("Receipt service initialized");

    // Initialize payment receipt service for user-facing receipts
    let payment_receipt_service = Arc::new(PaymentReceiptService::new(
        db_pool.clone(),
        receipt_service.clone(),
    ));
    tracing::info!("Payment receipt service initialized");

    // Initialize conversion service with receipt generation
    let conversion_service = Arc::new(ConversionService::new_with_receipts(
        db_pool.clone(),
        sideshift_client.clone(),
        coinmarketcap_service.clone(),
        payment_receipt_service.clone(),
    ));
    tracing::info!("Conversion service initialized with receipt generation");

    // Initialize staking service
    let staking_service = Arc::new(StakingService::new(
        db_pool.clone(),
        sideshift_client.clone(),
    ));
    tracing::info!("Staking service initialized");

    // Initialize trim configuration service
    let trim_config_service = Arc::new(TrimConfigService::new(db_pool.clone()));
    tracing::info!("Trim configuration service initialized");

    // Initialize chat service for encrypted messaging
    let chat_service = Arc::new(ChatService::new(
        db_pool.clone(),
        receipt_service.clone(),
    ));
    tracing::info!("Chat service initialized");

    // Initialize WebSocket service for real-time dashboard updates
    let websocket_service = Arc::new(WebSocketService::new());
    tracing::info!("WebSocket service initialized");

    // Initialize position management service for manual/automatic trading
    let position_management_service = Arc::new(PositionManagementService::new(db_pool.clone()));
    tracing::info!("Position management service initialized");

    // Initialize P2P exchange service
    let p2p_service = Arc::new(P2PService::new(db_pool.clone()));
    tracing::info!("P2P exchange service initialized");

    // Initialize verification service for identity and wallet verification
    let verification_service = Arc::new(VerificationService::new(db_pool.clone()));
    tracing::info!("Verification service initialized");

    // Initialize privacy service for temporary wallets and user tags
    let privacy_service = Arc::new(PrivacyService::new(db_pool.clone()));
    tracing::info!("Privacy service initialized");

    // Initialize position evaluator for agentic trimming
    let position_evaluator = Arc::new(PositionEvaluator::new(
        db_pool.clone(),
        trim_config_service.clone(),
        config.claude.api_key.clone(),
    ));
    tracing::info!("Position evaluator initialized");

    // Initialize notification service for alerts and trade notifications
    let notification_service = Arc::new(NotificationService::new());
    tracing::info!("Notification service initialized");

    // Initialize and start portfolio monitor background job
    let portfolio_monitor = Arc::new(PortfolioMonitor::new(
        wallet_service.clone(),
        whale_detection_service.clone(),
        analytics_service.clone(),
        db_pool.clone(),
        None, // Use default 5-minute interval
    ));
    
    let _monitor_handle = portfolio_monitor.start();
    tracing::info!("Portfolio monitor background job started");

    // Initialize and start price monitor for benchmark triggers
    // Wires benchmark triggers to notification service (Requirement 2.3, 2.5)
    let price_monitor = Arc::new(PriceMonitor::new(
        benchmark_service.clone(),
        coinmarketcap_service.clone(),
        position_management_service.clone(),
        notification_service.clone(),
        db_pool.clone(),
    ));
    
    // Spawn price monitor as a background task with WebSocket integration
    let price_monitor_clone = price_monitor.clone();
    let _websocket_service_clone = websocket_service.clone();
    tokio::spawn(async move {
        // Note: Price monitor would need to be updated to accept websocket_service
        // For now, we'll integrate WebSocket broadcasting in the handlers
        price_monitor_clone.start().await;
    });
    tracing::info!("Price monitor background job started (checking every 10 seconds)");

    // Start position evaluator background job for agentic trimming
    // Evaluates positions every 5 minutes (Requirement 7.1, 7.2)
    let position_evaluator_clone = position_evaluator.clone();
    tokio::spawn(async move {
        position_evaluator_clone.start();
    });
    tracing::info!("Position evaluator background job started (checking every 5 minutes)");

    // Initialize trading service for trim execution
    let trading_service = Arc::new(trading::TradingService::new());
    tracing::info!("Trading service initialized");

    // Initialize trim executor for executing pending trim recommendations
    // Processes pending trims every 1 minute (Requirement 7.3, 7.4, 7.5, 7.6)
    let trim_executor = Arc::new(TrimExecutor::new(
        db_pool.clone(),
        trim_config_service.clone(),
        position_management_service.clone(),
        trading_service.clone(),
        notification_service.clone(),
    ));
    tracing::info!("Trim executor initialized");

    // Start trim executor background job
    let trim_executor_clone = trim_executor.clone();
    tokio::spawn(async move {
        trim_executor_clone.start();
    });
    tracing::info!("Trim executor background job started (processing pending trims every 1 minute)");

    // Initialize JWT config for authentication
    let jwt_config = Arc::new(api::auth::JwtConfig::new(config.jwt.secret.clone()));
    tracing::info!("JWT config initialized");

    // Initialize metrics collector for monitoring
    let metrics = api::MetricsCollector::new();
    tracing::info!("Metrics collector initialized");

    // Initialize proximity services for P2P transfers
    let proximity_auth_service = Arc::new(proximity::AuthenticationService::new());
    tracing::info!("Proximity authentication service initialized");

    // Proximity discovery identity. For a multi-device demo each running
    // instance must announce a DISTINCT identity, otherwise both laptops would
    // broadcast the same mDNS instance name ("crypto-p2p-server") and appear as
    // the same node (colliding dedup keys). The identity is read from the
    // environment so each machine can be labeled (e.g. PROXIMITY_USER_TAG=Alice
    // on one laptop, =Bob on the other); when unset we fall back to a per-process
    // unique device id derived from the hostname + a random suffix so two
    // instances never collide by accident.
    let device_id = std::env::var("PROXIMITY_DEVICE_ID").unwrap_or_else(|_| {
        let host = std::env::var("HOSTNAME")
            .ok()
            .or_else(|| hostname_fallback())
            .unwrap_or_else(|| "node".to_string());
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        format!("{}-{}", host, &suffix[..8])
    });
    let user_tag = std::env::var("PROXIMITY_USER_TAG").unwrap_or_else(|_| device_id.clone());
    let wallet_address =
        std::env::var("PROXIMITY_WALLET").unwrap_or_else(|_| format!("wallet_{}", device_id));
    // Real user account id broadcast so discovered peers can be used directly as
    // transfer recipients. Defaults to the demo user when unset.
    let proximity_user_id = std::env::var("PROXIMITY_USER_ID")
        .unwrap_or_else(|_| "00000000-0000-0000-0000-000000000001".to_string());

    tracing::info!(
        "Proximity identity: user_id={}, user_tag={}, device_id={}",
        proximity_user_id,
        user_tag,
        device_id
    );

    let proximity_discovery_service = Arc::new(proximity::DiscoveryService::with_identity(
        proximity_user_id,
        user_tag,
        device_id,
        wallet_address,
    ));
    tracing::info!("Proximity discovery service initialized");

    let proximity_session_manager = Arc::new(proximity::SessionManager::new());
    tracing::info!("Proximity session manager initialized");

    // Optionally attach a backend-custodied devnet demo wallet so proximity
    // transfers are actually submitted on-chain. This is enabled only on devnet
    // and only when explicitly turned on, so it can never touch mainnet funds.
    let mut transfer_service = proximity::TransferService::new(
        db_pool.clone(),
        solana_client.clone(),
    );
    if config.solana.network == "devnet"
        && std::env::var("ENABLE_DEMO_SOLANA_SIGNER").unwrap_or_default() == "true"
    {
        match load_or_create_demo_signer(&solana_client).await {
            Ok(signer) => {
                transfer_service = transfer_service.with_custodial_signer(Arc::new(signer));
            }
            Err(e) => {
                tracing::warn!("Demo Solana signer unavailable, on-chain transfers disabled: {}", e);
            }
        }
    }
    let proximity_transfer_service = Arc::new(transfer_service);
    // Start the background task that settles offline-queued transfers once the
    // blockchain becomes reachable again.
    proximity_transfer_service.start_settlement_sync_task();
    tracing::info!("Proximity transfer service initialized");

    // Initialize mesh price service for P2P price data distribution
    // Uses the proximity P2P connection infrastructure for message routing
    let peer_connection_manager = Arc::new(proximity::PeerConnectionManager::new());
    let mesh_price_service = Arc::new(MeshPriceService::new(
        coinmarketcap_service.clone(),
        peer_connection_manager,
        redis_pool.clone(),
        db_pool.clone(),
        websocket_service.clone(),
    ));
    tracing::info!("Mesh price service initialized");

    // Create application state
    let app_state = Arc::new(AppState::new(
        wallet_service,
        whale_detection_service,
        analytics_service,
        benchmark_service,
        coinmarketcap_service,
        conversion_service,
        staking_service,
        trim_config_service,
        trim_executor.clone(),
        payment_receipt_service,
        chat_service,
        websocket_service,
        position_management_service,
        p2p_service,
        verification_service,
        privacy_service,
        proximity_discovery_service,
        proximity_transfer_service,
        proximity_session_manager,
        proximity_auth_service,
        mesh_price_service,
        jwt_config,
        db_pool,
        redis_pool,
        solana_client,
        metrics.clone(),
    ));

    // Start alert manager background task (checks every 60 seconds)
    let alert_manager = api::AlertManager::new(metrics.clone());
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            alert_manager.check_and_alert().await;
        }
    });
    tracing::info!("Alert manager background task started (checking every 60 seconds)");

    // Create router with CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = api::routes::create_router(app_state)
        .layer(cors);

    // Start server
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        // Classify the bind failure: an `AddrInUse` error is a distinct
        // port-unavailable condition (an operator/environment problem),
        // every other error is a generic internal startup error.
        Err(e) => return Err(classify_bind_error(e, &addr, config.server.port)),
    };

    tracing::info!("API server listening on {}", addr);
    tracing::info!("Health check available at http://{}/health", addr);
    tracing::info!("Metrics available at http://{}/metrics", addr);

    axum::serve(listener, app)
        .await?;

    Ok(())
}

/// Classify a `TcpListener::bind` failure into the correct startup error.
///
/// This preserves the runtime distinction required by Requirement 6.4:
/// an `io::ErrorKind::AddrInUse` failure is reported as a distinct
/// port-unavailable condition that names the port and tells the operator how
/// to recover, separate from the generic internal-startup-error path used for
/// every other bind failure (which keeps the underlying error as context).
///
/// Kept as a small, pure function so the classification can be unit-tested
/// without standing up the full server (`main` is a bin entrypoint).
fn classify_bind_error(e: std::io::Error, addr: &str, port: u16) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::AddrInUse {
        // Distinct, clearly-named port-unavailable condition.
        anyhow::anyhow!(
            "Port {} is already in use (set SERVER_PORT to use a different port)",
            port
        )
    } else {
        // Generic internal startup error: keep the underlying io error as the
        // source and add binding context, matching the prior behavior.
        anyhow::Error::new(e).context(format!("Failed to bind API server to {}", addr))
    }
}

#[cfg(test)]
mod tests {
    use super::classify_bind_error;
    use std::io::{Error, ErrorKind};

    /// Test A — the real distinction against the OS.
    ///
    /// Bind a listener to an OS-assigned free port, then attempt a second bind
    /// to the SAME port. The OS returns a real `AddrInUse` error, and
    /// `classify_bind_error` must turn it into the distinct port-unavailable
    /// message that names the port. This exercises the genuine `AddrInUse` path
    /// deterministically without depending on a fixed port being free/busy.
    #[tokio::test]
    async fn addr_in_use_maps_to_distinct_port_unavailable_message() {
        // First bind succeeds and reserves an OS-assigned free port.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("first bind to an ephemeral port should succeed");
        let port = listener.local_addr().unwrap().port();
        let addr = format!("127.0.0.1:{}", port);

        // Second bind to the same port must fail with AddrInUse.
        let err = tokio::net::TcpListener::bind(&addr)
            .await
            .expect_err("second bind to the same port must fail");
        assert_eq!(
            err.kind(),
            ErrorKind::AddrInUse,
            "the OS should report the second bind as AddrInUse"
        );

        let classified = classify_bind_error(err, &addr, port);
        let message = classified.to_string();

        // Distinct port-unavailable message: names the port and says it's in use,
        // and is NOT the generic "Failed to bind" startup-error message.
        assert!(
            message.contains(&port.to_string()),
            "port-unavailable message must name the port: {message}"
        );
        assert!(
            message.contains("already in use"),
            "port-unavailable message must indicate the port is in use: {message}"
        );
        assert!(
            !message.contains("Failed to bind"),
            "port-unavailable must be distinct from the generic startup error: {message}"
        );
    }

    /// Test B — the generic startup-error path.
    ///
    /// A non-`AddrInUse` io error (e.g. `PermissionDenied`) must be classified
    /// as the generic internal startup error: it keeps the "Failed to bind"
    /// context, names the address, and must NOT be reported as a
    /// port-unavailable ("already in use") condition.
    #[test]
    fn non_addr_in_use_maps_to_generic_startup_error() {
        let addr = "127.0.0.1:3000";
        let err = Error::new(ErrorKind::PermissionDenied, "permission denied");

        let classified = classify_bind_error(err, addr, 3000);
        let message = classified.to_string();

        assert!(
            message.contains("Failed to bind"),
            "generic startup error must use the 'Failed to bind' message: {message}"
        );
        assert!(
            message.contains(addr),
            "generic startup error must name the bind address: {message}"
        );
        assert!(
            !message.contains("already in use"),
            "a non-AddrInUse error must NOT be reported as port-unavailable: {message}"
        );

        // The underlying io error is preserved as the error source (distinct
        // from the AddrInUse path which produces a fresh message).
        let source = std::error::Error::source(classified.as_ref() as &dyn std::error::Error);
        assert!(
            source.is_some(),
            "generic startup error should retain the underlying io error as its source"
        );
    }
}
