pub mod wallet_service;
pub mod whale_detection;
pub mod portfolio_monitor;
pub mod analytics;
pub mod auth;
pub mod routes;
pub mod handlers;
pub mod receipt_service;
pub mod payment_receipt_service;

// Cross-cutting infrastructure: error types, monitoring/metrics, logging,
// rate limiting, security middleware, and the dashboard WebSocket service.
// NOTE: the `monitoring` child shares its name with the external `monitoring`
// crate dependency; the local module shadows it at the crate root as before.
pub mod infra;
pub use infra::{error, monitoring, logging, rate_limit, security, websocket_service};

// Market-data provider integrations (CoinMarketCap, Birdeye, Tantum/Helius,
// SideShift, token metadata, portfolio cache).
pub mod market_data;
pub use market_data::{
    coinmarketcap_service, birdeye_service, tantum_client, sideshift_client,
    token_metadata_service, portfolio_cache,
};

// Trading subsystem (benchmarks/triggers, positions, staking, conversion, trims).
// NOTE: this local module shares its name with the external `trading` crate
// dependency; child modules refer to the external crate as `::trading`.
pub mod trading;
pub use trading::{
    benchmark_service, price_monitor, conversion_service, staking_service,
    trim_config_service, trim_executor, position_evaluator, position_management_service,
};

// Peer-to-peer exchange subsystem (offers/exchanges, chat, verification, privacy).
pub mod p2p;
pub use p2p::{p2p_service, chat_service, verification_service, privacy_service};

pub mod cross_chain_transaction_service;

// Proximity-based P2P transfer subsystem (API layer).
// NOTE: this local module shares its name with the external `proximity`
// crate dependency; at the crate root the external crate is referred to as
// `::proximity` (see the `use ::proximity::{...}` import below).
pub mod proximity;
pub use proximity::{
    proximity_receipt_integration, proximity_service, proximity_handlers, proximity_websocket,
};

// Mesh networking + distributed price-data subsystem.
// Child modules are re-exported below so existing paths
// (`crate::mesh_types::*`, `api::MeshPriceService`, ...) keep resolving.
pub mod mesh;
pub use mesh::{
    mesh_types, mesh_metrics, message_tracker, price_cache, price_update_validator,
    coordination_service, gossip_protocol, provider_node, network_status_tracker,
    mesh_price_service,
};

pub use wallet_service::WalletService;
pub use portfolio_cache::PortfolioCache;
pub use whale_detection::{WhaleDetectionService, RankedWhale, WhaleAsset};
pub use portfolio_monitor::PortfolioMonitor;
pub use analytics::AnalyticsService;
pub use coinmarketcap_service::{CoinMarketCapService, CmcPriceData, CmcConversionResult};
pub use tantum_client::{TantumClient, TantumWalletInfo, TantumBalance, TantumToken};
pub use benchmark_service::{BenchmarkService, Benchmark, CreateBenchmarkRequest, UpdateBenchmarkRequest, TriggerType, ActionType, TradeAction};
pub use price_monitor::{PriceMonitor, TriggeredBenchmark};
pub use sideshift_client::{SideShiftClient, ConversionQuote, ConversionOrder, OrderStatus, SupportedCoin, StakingInfo, AmountType};
pub use conversion_service::{ConversionService, ConversionQuoteWithFees, ConversionResult, ConversionRecord, ConversionProvider, ConversionStatus};
pub use staking_service::{StakingService, StakingConfig, StakingPosition, StakingApprovalRequest, StakingInitiationResult};
pub use trim_config_service::{TrimConfigService, TrimConfig, UpdateTrimConfigRequest};
pub use position_evaluator::{PositionEvaluator, Position, TrimRecommendation};
pub use trim_executor::{TrimExecutor, TrimExecution, PendingTrim};
pub use receipt_service::{ReceiptService, Receipt, ReceiptData, VerificationStatus};
pub use payment_receipt_service::{
    PaymentReceiptService, PaymentReceipt, TransactionType, TransactionFees, 
    BlockchainConfirmation, ReceiptSearchFilters, Pagination, ReceiptSearchResults
};
pub use chat_service::{ChatService, ChatMessage};
pub use p2p_service::{P2PService, P2POffer, P2PExchange, OfferType, OfferStatus};
pub use verification_service::{VerificationService, WalletVerification, VerificationLevel, VerificationStatus as IdentityVerificationStatus};
pub use privacy_service::{PrivacyService, TemporaryWallet};
pub use websocket_service::{WebSocketService, DashboardUpdate, websocket_handler};
pub use position_management_service::{
    PositionManagementService, PositionMode, PositionModeConfig, ManualOrder, 
    ManualOrderRequest, PendingAutomaticOrder
};
pub use token_metadata_service::{TokenMetadataService, TokenMetadata, TokenType};
pub use cross_chain_transaction_service::{
    CrossChainTransactionService, NormalizedTransaction, TransactionStatus,
    TransactionFees as CrossChainTransactionFees
};
pub use proximity_receipt_integration::{create_proximity_receipt, create_receipt_service};
pub use proximity_websocket::{ProximityWebSocketService, ProximityEvent, proximity_websocket_handler};
pub use message_tracker::MessageTracker;
pub use price_cache::PriceCache;
pub use coordination_service::CoordinationService;
pub use gossip_protocol::GossipProtocol;
pub use provider_node::ProviderNode;
pub use network_status_tracker::NetworkStatusTracker;
pub use mesh_price_service::MeshPriceService;
pub use price_update_validator::PriceUpdateValidator;
pub use mesh_metrics::{MeshMetricsCollector, MeshMetrics, MeshMetricsSummary};
pub use error::{ApiError, ApiResult, ErrorResponse};
pub use monitoring::{MetricsCollector, ServiceMetrics, ServiceMetric, HealthStatus, RequestTimer, AlertManager};

use blockchain::SolanaClient;
use deadpool_postgres::Pool;
use redis::aio::ConnectionManager;
use std::sync::Arc;
// `::proximity` (leading `::`) refers to the external `proximity` crate, not the
// local `proximity` submodule declared above which shares the same name.
use ::proximity::{DiscoveryService, TransferService, SessionManager, AuthenticationService};

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub wallet_service: Arc<WalletService>,
    pub whale_detection_service: Arc<WhaleDetectionService>,
    pub analytics_service: Arc<AnalyticsService>,
    pub benchmark_service: Arc<BenchmarkService>,
    pub coinmarketcap_service: Arc<CoinMarketCapService>,
    pub conversion_service: Arc<ConversionService>,
    pub staking_service: Arc<StakingService>,
    pub trim_config_service: Arc<TrimConfigService>,
    pub trim_executor: Arc<TrimExecutor>,
    pub payment_receipt_service: Arc<PaymentReceiptService>,
    pub chat_service: Arc<ChatService>,
    pub websocket_service: Arc<WebSocketService>,
    pub position_management_service: Arc<PositionManagementService>,
    pub p2p_service: Arc<P2PService>,
    pub verification_service: Arc<VerificationService>,
    pub privacy_service: Arc<PrivacyService>,
    pub proximity_discovery_service: Arc<DiscoveryService>,
    pub proximity_transfer_service: Arc<TransferService>,
    pub proximity_session_manager: Arc<SessionManager>,
    pub proximity_auth_service: Arc<AuthenticationService>,
    pub mesh_price_service: Arc<MeshPriceService>,
    pub jwt_config: Arc<auth::JwtConfig>,
    pub db_pool: Pool,
    pub redis_pool: ConnectionManager,
    pub solana_client: Arc<SolanaClient>,
    pub metrics: MetricsCollector,
}

impl AppState {
    pub fn new(
        wallet_service: Arc<WalletService>,
        whale_detection_service: Arc<WhaleDetectionService>,
        analytics_service: Arc<AnalyticsService>,
        benchmark_service: Arc<BenchmarkService>,
        coinmarketcap_service: Arc<CoinMarketCapService>,
        conversion_service: Arc<ConversionService>,
        staking_service: Arc<StakingService>,
        trim_config_service: Arc<TrimConfigService>,
        trim_executor: Arc<TrimExecutor>,
        payment_receipt_service: Arc<PaymentReceiptService>,
        chat_service: Arc<ChatService>,
        websocket_service: Arc<WebSocketService>,
        position_management_service: Arc<PositionManagementService>,
        p2p_service: Arc<P2PService>,
        verification_service: Arc<VerificationService>,
        privacy_service: Arc<PrivacyService>,
        proximity_discovery_service: Arc<DiscoveryService>,
        proximity_transfer_service: Arc<TransferService>,
        proximity_session_manager: Arc<SessionManager>,
        proximity_auth_service: Arc<AuthenticationService>,
        mesh_price_service: Arc<MeshPriceService>,
        jwt_config: Arc<auth::JwtConfig>,
        db_pool: Pool,
        redis_pool: ConnectionManager,
        solana_client: Arc<SolanaClient>,
        metrics: MetricsCollector,
    ) -> Self {
        Self {
            wallet_service,
            whale_detection_service,
            analytics_service,
            benchmark_service,
            coinmarketcap_service,
            conversion_service,
            staking_service,
            trim_config_service,
            trim_executor,
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
            metrics,
        }
    }
}
