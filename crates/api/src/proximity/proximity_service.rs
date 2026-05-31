// Proximity Service - API layer for proximity-based P2P transfers

use proximity::{
    DiscoveryService, SessionManager, TransferService, AuthenticationService,
    PeerConnectionManager, DiscoveryMethod, DiscoveredPeer, TransferRequest,
    TransferStatus, DiscoverySession, ProximityError,
};
use blockchain::SolanaClient;
use database::DbPool;
use std::sync::Arc;
use uuid::Uuid;
use rust_decimal::Decimal;

/// Proximity service aggregates all proximity-related services
pub struct ProximityService {
    pub discovery_service: Arc<DiscoveryService>,
    pub session_manager: Arc<SessionManager>,
    pub transfer_service: Arc<TransferService>,
    pub auth_service: Arc<AuthenticationService>,
    pub connection_manager: Arc<PeerConnectionManager>,
}

impl ProximityService {
    pub fn new(
        db_pool: DbPool,
        solana_client: Arc<SolanaClient>,
        user_tag: String,
        device_id: String,
        wallet_address: String,
    ) -> Self {
        let discovery_service = Arc::new(DiscoveryService::new(
            user_tag,
            device_id,
            wallet_address,
        ));
        
        let session_manager = Arc::new(SessionManager::new());
        
        let transfer_service = Arc::new(TransferService::new(
            db_pool,
            solana_client,
        ));
        
        let auth_service = Arc::new(AuthenticationService::new());
        
        let connection_manager = Arc::new(PeerConnectionManager::new());

        Self {
            discovery_service,
            session_manager,
            transfer_service,
            auth_service,
            connection_manager,
        }
    }

    /// Start discovery session
    pub async fn start_discovery(
        &self,
        user_id: Uuid,
        method: DiscoveryMethod,
        duration_minutes: u32,
    ) -> Result<DiscoverySession, ProximityError> {
        // Start session
        let session = self.session_manager
            .start_session(user_id, method, duration_minutes)
            .await?;

        // Start discovery
        self.discovery_service.start_discovery(method).await?;

        Ok(session)
    }

    /// Stop discovery session
    pub async fn stop_discovery(&self, session_id: Uuid) -> Result<(), ProximityError> {
        // End session
        self.session_manager.end_session(session_id).await?;

        // Stop discovery
        self.discovery_service.stop_discovery().await?;

        Ok(())
    }

    /// Get discovered peers
    pub async fn get_discovered_peers(&self) -> Result<Vec<DiscoveredPeer>, ProximityError> {
        self.discovery_service.get_discovered_peers().await
    }

    /// Block a peer
    pub async fn block_peer(
        &self,
        db_pool: &DbPool,
        user_id: Uuid,
        peer_user_id: Uuid,
    ) -> Result<(), ProximityError> {
        let client = db_pool.get().await.map_err(|e| {
            ProximityError::InternalError(format!("Database connection error: {}", e))
        })?;

        client
            .execute(
                "INSERT INTO peer_blocklist (user_id, blocked_user_id, blocked_at)
                 VALUES ($1, $2, NOW())
                 ON CONFLICT (user_id, blocked_user_id) DO NOTHING",
                &[&user_id, &peer_user_id],
            )
            .await
            .map_err(|e| {
                ProximityError::InternalError(format!("Failed to block peer: {}", e))
            })?;

        Ok(())
    }

    /// Create transfer request
    pub async fn create_transfer(
        &self,
        sender_user_id: Uuid,
        sender_wallet: String,
        recipient_user_id: Uuid,
        recipient_wallet: String,
        asset: String,
        amount: Decimal,
    ) -> Result<TransferRequest, ProximityError> {
        self.transfer_service
            .create_transfer_request(
                sender_user_id,
                sender_wallet,
                recipient_user_id,
                recipient_wallet,
                asset,
                amount,
            )
            .await
    }

    /// Accept transfer
    pub async fn accept_transfer(&self, request_id: Uuid) -> Result<(), ProximityError> {
        self.transfer_service.accept_transfer(request_id).await
    }

    /// Reject transfer
    pub async fn reject_transfer(
        &self,
        request_id: Uuid,
        reason: Option<String>,
    ) -> Result<(), ProximityError> {
        self.transfer_service.reject_transfer(request_id, reason).await
    }

    /// Get transfer status
    pub async fn get_transfer_status(&self, request_id: Uuid) -> Result<TransferStatus, ProximityError> {
        self.transfer_service.get_transfer_status(request_id).await
    }

    /// Get transfer request
    pub async fn get_transfer_request(&self, request_id: Uuid) -> Result<TransferRequest, ProximityError> {
        self.transfer_service.get_transfer_request(request_id).await
    }

    /// Execute and monitor transfer
    pub async fn execute_transfer(&self, request_id: Uuid) -> Result<String, ProximityError> {
        self.transfer_service.execute_transfer(request_id).await
    }
}
