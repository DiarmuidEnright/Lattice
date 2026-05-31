// Proximity API Handlers

use crate::{ApiError, ApiResult, AppState};
use axum::{
    extract::{Path, State},
    Json,
};
use proximity::{DiscoveryMethod, DiscoveredPeer, DiscoverySession, TransferRequest, TransferStatus};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

// ============================================================================
// Request/Response Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct StartDiscoveryRequest {
    pub user_id: Uuid,
    pub method: DiscoveryMethod,
    #[serde(default)]
    pub duration_minutes: u32, // 0 = use default (30 minutes)
}

#[derive(Debug, Serialize)]
pub struct StartDiscoveryResponse {
    pub session: DiscoverySession,
}

#[derive(Debug, Deserialize)]
pub struct StopDiscoveryRequest {
    pub session_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct StopDiscoveryResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct GetPeersResponse {
    pub peers: Vec<DiscoveredPeer>,
}

#[derive(Debug, Deserialize)]
pub struct BlockPeerRequest {
    pub user_id: Uuid,
    pub blocked_user_id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct BlockPeerResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateTransferRequest {
    pub sender_user_id: Uuid,
    /// Optional: when absent/null, the backend uses a configured default sender
    /// wallet so a proximity transfer works even if the client hasn't connected
    /// a wallet. Accepts null to avoid deserialize failures from the UI.
    #[serde(default)]
    pub sender_wallet: Option<String>,
    pub recipient_user_id: Uuid,
    pub recipient_wallet: String,
    pub asset: String,
    pub amount: Decimal,
}

#[derive(Debug, Serialize)]
pub struct CreateTransferResponse {
    pub transfer: TransferRequest,
}

#[derive(Debug, Deserialize)]
pub struct AcceptTransferRequest {
    // No body needed, transfer_id comes from path
}

#[derive(Debug, Serialize)]
pub struct AcceptTransferResponse {
    pub success: bool,
    pub message: String,
    pub transaction_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RejectTransferRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RejectTransferResponse {
    pub success: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct GetTransferStatusResponse {
    pub transfer_id: Uuid,
    pub status: TransferStatus,
    pub transfer: Option<TransferRequest>,
}

#[derive(Debug, Deserialize)]
pub struct GetTransferHistoryQuery {
    pub user_id: Uuid,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct TransferHistoryItem {
    pub id: Uuid,
    pub sender_user_id: Uuid,
    pub sender_wallet: String,
    pub recipient_user_id: Uuid,
    pub recipient_wallet: String,
    pub asset: String,
    pub amount: String,
    pub status: String,
    pub transaction_hash: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GetTransferHistoryResponse {
    pub transfers: Vec<TransferHistoryItem>,
    pub total: i64,
}

// ============================================================================
// Discovery Endpoints
// ============================================================================

/// POST /api/proximity/discovery/start - Start discovery session
/// 
/// **Validates: Requirements 1.1, 1.3**
pub async fn start_discovery(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartDiscoveryRequest>,
) -> ApiResult<Json<StartDiscoveryResponse>> {
    let duration = if req.duration_minutes == 0 { 30 } else { req.duration_minutes };
    
    let session = state.proximity_session_manager
        .start_session(req.user_id, req.method, duration)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to start session: {}", e)))?;
    
    state.proximity_discovery_service
        .start_discovery(req.method)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to start discovery: {}", e)))?;
    
    Ok(Json(StartDiscoveryResponse { session }))
}

/// POST /api/proximity/discovery/stop - Stop discovery session
/// 
/// **Validates: Requirements 1.3**
pub async fn stop_discovery(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StopDiscoveryRequest>,
) -> ApiResult<Json<StopDiscoveryResponse>> {
    state.proximity_session_manager
        .end_session(req.session_id)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to end session: {}", e)))?;
    
    state.proximity_discovery_service
        .stop_discovery()
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to stop discovery: {}", e)))?;
    
    Ok(Json(StopDiscoveryResponse {
        success: true,
        message: "Discovery stopped successfully".to_string(),
    }))
}

/// GET /api/proximity/peers - Get discovered peers
/// 
/// **Validates: Requirements 2.4, 3.4**
pub async fn get_discovered_peers(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<GetPeersResponse>> {
    let peers = state.proximity_discovery_service
        .get_discovered_peers()
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to get peers: {}", e)))?;
    
    Ok(Json(GetPeersResponse { peers }))
}

/// POST /api/proximity/peers/{peer_id}/block - Block a peer
/// 
/// **Validates: Requirements 17.4**
pub async fn block_peer(
    State(state): State<Arc<AppState>>,
    Path(_peer_id): Path<String>,
    Json(req): Json<BlockPeerRequest>,
) -> ApiResult<Json<BlockPeerResponse>> {
    database::proximity::add_blocked_peer(
        &state.db_pool,
        req.user_id,
        req.blocked_user_id,
    )
    .await
    .map_err(|e| ApiError::InternalError(format!("Failed to block peer: {}", e)))?;
    
    Ok(Json(BlockPeerResponse {
        success: true,
        message: "Peer blocked successfully".to_string(),
    }))
}

// ============================================================================
// Transfer Endpoints
// ============================================================================

/// POST /api/proximity/transfers - Create transfer request
/// 
/// **Validates: Requirements 5.3, 5.5**
pub async fn create_transfer(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateTransferRequest>,
) -> ApiResult<Json<CreateTransferResponse>> {
    // Resolve the sender wallet: use the one provided by the client, else fall
    // back to the configured proximity wallet (PROXIMITY_WALLET) so a transfer
    // works without a connected wallet on the client.
    let sender_wallet = req
        .sender_wallet
        .filter(|w| !w.trim().is_empty())
        .or_else(|| std::env::var("PROXIMITY_WALLET").ok())
        .ok_or_else(|| ApiError::ValidationError("sender_wallet is required".to_string()))?;

    let transfer = state.proximity_transfer_service
        .create_transfer_request(
            req.sender_user_id,
            sender_wallet,
            req.recipient_user_id,
            req.recipient_wallet,
            req.asset,
            req.amount,
        )
        .await
        .map_err(map_transfer_error)?;
    
    Ok(Json(CreateTransferResponse { transfer }))
}

/// Map a proximity transfer error to an API error. Validation-class failures
/// (e.g. insufficient balance, invalid wallet) become a 400 with a clear,
/// user-facing message so the UI can show the real reason instead of a generic
/// "internal error".
fn map_transfer_error(e: proximity::ProximityError) -> ApiError {
    use proximity::error::ErrorCategory;
    match e.category() {
        ErrorCategory::Validation => ApiError::ValidationError(e.user_message()),
        ErrorCategory::NotFound => ApiError::NotFound(e.user_message()),
        ErrorCategory::Authentication => ApiError::Unauthorized(e.user_message()),
        _ => ApiError::InternalError(format!("Failed to create transfer: {}", e)),
    }
}

/// POST /api/proximity/transfers/{id}/accept - Accept transfer
/// 
/// **Validates: Requirements 6.4, 7.1**
pub async fn accept_transfer(
    State(state): State<Arc<AppState>>,
    Path(transfer_id): Path<Uuid>,
) -> ApiResult<Json<AcceptTransferResponse>> {
    // Mark the request accepted, then settle it. If the blockchain is reachable
    // the transfer is submitted on-chain immediately and we return the real tx
    // hash. If offline, it is durably queued as PendingSettlement and the
    // background sync task will submit it once connectivity returns.
    state.proximity_transfer_service
        .accept_transfer(transfer_id)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to accept transfer: {}", e)))?;

    let tx_hash = state.proximity_transfer_service
        .execute_or_queue_transfer(transfer_id)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to settle transfer: {}", e)))?;

    let message = if tx_hash.is_some() {
        "Transfer accepted and settled on-chain".to_string()
    } else {
        "Transfer accepted and queued for settlement (offline); it will sync to the blockchain when connectivity returns".to_string()
    };

    Ok(Json(AcceptTransferResponse {
        success: true,
        message,
        transaction_hash: tx_hash,
    }))
}

/// POST /api/proximity/transfers/sync - Settle any offline-queued transfers now
pub async fn sync_pending_transfers(
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let settled = state.proximity_transfer_service
        .sync_pending_settlements()
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to sync pending transfers: {}", e)))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "settled": settled,
    })))
}

/// POST /api/proximity/transfers/{id}/reject - Reject transfer
/// 
/// **Validates: Requirements 6.5**
pub async fn reject_transfer(
    State(state): State<Arc<AppState>>,
    Path(transfer_id): Path<Uuid>,
    Json(req): Json<RejectTransferRequest>,
) -> ApiResult<Json<RejectTransferResponse>> {
    state.proximity_transfer_service
        .reject_transfer(transfer_id, req.reason)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to reject transfer: {}", e)))?;
    
    Ok(Json(RejectTransferResponse {
        success: true,
        message: "Transfer rejected".to_string(),
    }))
}

/// GET /api/proximity/transfers/{id} - Get transfer status
/// 
/// **Validates: Requirements 6.3**
pub async fn get_transfer_status(
    State(state): State<Arc<AppState>>,
    Path(transfer_id): Path<Uuid>,
) -> ApiResult<Json<GetTransferStatusResponse>> {
    let status = state.proximity_transfer_service
        .get_transfer_status(transfer_id)
        .await
        .map_err(|e| ApiError::InternalError(format!("Failed to get transfer status: {}", e)))?;
    
    Ok(Json(GetTransferStatusResponse {
        transfer_id,
        status,
        transfer: None, // Transfer details would be retrieved from database if needed
    }))
}

/// GET /api/proximity/transfers/history - Get transfer history
/// 
/// **Validates: Requirements 10.5, 10.6**
pub async fn get_transfer_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<GetTransferHistoryQuery>,
) -> ApiResult<Json<GetTransferHistoryResponse>> {
    let filter = database::proximity::TransferFilter {
        user_id: Some(query.user_id),
        status: None,
        asset: None,
        transaction_type: None,
        from_date: None,
        to_date: None,
        limit: query.limit,
        offset: query.offset,
    };
    
    let transfers = database::proximity::get_user_proximity_transfers(
        &state.db_pool,
        filter,
    )
    .await
    .map_err(|e| ApiError::InternalError(format!("Failed to get transfer history: {}", e)))?;
    
    let total = transfers.len() as i64;
    
    let history_items: Vec<TransferHistoryItem> = transfers
        .into_iter()
        .map(|t| TransferHistoryItem {
            id: t.id,
            sender_user_id: t.sender_user_id,
            sender_wallet: t.sender_wallet,
            recipient_user_id: t.recipient_user_id,
            recipient_wallet: t.recipient_wallet,
            asset: t.asset,
            amount: t.amount.to_string(),
            status: t.status,
            transaction_hash: t.transaction_hash,
            created_at: t.created_at.to_string(),
            completed_at: t.completed_at.map(|dt| dt.to_string()),
        })
        .collect();
    
    Ok(Json(GetTransferHistoryResponse {
        transfers: history_items,
        total,
    }))
}
