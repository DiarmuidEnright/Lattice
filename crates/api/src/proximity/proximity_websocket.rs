// Proximity WebSocket Handler - Real-time updates for proximity transfers

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info, warn};

use crate::AppState;

/// Maximum number of messages to buffer in the broadcast channel
const CHANNEL_CAPACITY: usize = 100;

/// Proximity event types that can be pushed to clients
/// 
/// **Validates: Requirements 6.1, 7.6**
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProximityEvent {
    /// Peer discovered on local network
    PeerDiscovered {
        peer_id: String,
        user_tag: String,
        wallet_address: String,
        discovery_method: String,
        signal_strength: Option<i8>,
        verified: bool,
        timestamp: i64,
    },
    /// Peer removed from discovered list (timeout or left network)
    PeerRemoved {
        peer_id: String,
        reason: String,
        timestamp: i64,
    },
    /// Transfer request received from a peer
    TransferRequestReceived {
        transfer_id: String,
        sender_user_tag: String,
        sender_wallet: String,
        asset: String,
        amount: String,
        expires_at: i64,
        timestamp: i64,
    },
    /// Transfer request accepted by recipient
    TransferAccepted {
        transfer_id: String,
        recipient_user_tag: String,
        timestamp: i64,
    },
    /// Transfer request rejected by recipient
    TransferRejected {
        transfer_id: String,
        recipient_user_tag: String,
        reason: Option<String>,
        timestamp: i64,
    },
    /// Transfer completed successfully
    TransferCompleted {
        transfer_id: String,
        transaction_hash: String,
        asset: String,
        amount: String,
        timestamp: i64,
    },
    /// Transfer failed
    TransferFailed {
        transfer_id: String,
        reason: String,
        timestamp: i64,
    },
    /// Discovery session started
    SessionStarted {
        session_id: String,
        discovery_method: String,
        expires_at: i64,
        timestamp: i64,
    },
    /// Discovery session ended
    SessionEnded {
        session_id: String,
        reason: String,
        timestamp: i64,
    },
}

/// WebSocket service for managing real-time proximity updates
#[derive(Clone)]
pub struct ProximityWebSocketService {
    tx: broadcast::Sender<ProximityEvent>,
}

impl ProximityWebSocketService {
    /// Create a new proximity WebSocket service
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx }
    }

    /// Get a receiver for proximity events
    pub fn subscribe(&self) -> broadcast::Receiver<ProximityEvent> {
        self.tx.subscribe()
    }

    /// Broadcast a peer discovered event
    pub fn broadcast_peer_discovered(
        &self,
        peer_id: String,
        user_tag: String,
        wallet_address: String,
        discovery_method: String,
        signal_strength: Option<i8>,
        verified: bool,
    ) {
        let event = ProximityEvent::PeerDiscovered {
            peer_id,
            user_tag,
            wallet_address,
            discovery_method,
            signal_strength,
            verified,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast peer discovered event: {}", e);
        }
    }

    /// Broadcast a peer removed event
    pub fn broadcast_peer_removed(&self, peer_id: String, reason: String) {
        let event = ProximityEvent::PeerRemoved {
            peer_id,
            reason,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast peer removed event: {}", e);
        }
    }

    /// Broadcast a transfer request received event
    pub fn broadcast_transfer_request_received(
        &self,
        transfer_id: String,
        sender_user_tag: String,
        sender_wallet: String,
        asset: String,
        amount: String,
        expires_at: i64,
    ) {
        let event = ProximityEvent::TransferRequestReceived {
            transfer_id,
            sender_user_tag,
            sender_wallet,
            asset,
            amount,
            expires_at,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast transfer request received event: {}", e);
        }
    }

    /// Broadcast a transfer accepted event
    pub fn broadcast_transfer_accepted(&self, transfer_id: String, recipient_user_tag: String) {
        let event = ProximityEvent::TransferAccepted {
            transfer_id,
            recipient_user_tag,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast transfer accepted event: {}", e);
        }
    }

    /// Broadcast a transfer rejected event
    pub fn broadcast_transfer_rejected(
        &self,
        transfer_id: String,
        recipient_user_tag: String,
        reason: Option<String>,
    ) {
        let event = ProximityEvent::TransferRejected {
            transfer_id,
            recipient_user_tag,
            reason,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast transfer rejected event: {}", e);
        }
    }

    /// Broadcast a transfer completed event
    pub fn broadcast_transfer_completed(
        &self,
        transfer_id: String,
        transaction_hash: String,
        asset: String,
        amount: String,
    ) {
        let event = ProximityEvent::TransferCompleted {
            transfer_id,
            transaction_hash,
            asset,
            amount,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast transfer completed event: {}", e);
        }
    }

    /// Broadcast a transfer failed event
    pub fn broadcast_transfer_failed(&self, transfer_id: String, reason: String) {
        let event = ProximityEvent::TransferFailed {
            transfer_id,
            reason,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast transfer failed event: {}", e);
        }
    }

    /// Broadcast a session started event
    pub fn broadcast_session_started(
        &self,
        session_id: String,
        discovery_method: String,
        expires_at: i64,
    ) {
        let event = ProximityEvent::SessionStarted {
            session_id,
            discovery_method,
            expires_at,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast session started event: {}", e);
        }
    }

    /// Broadcast a session ended event
    pub fn broadcast_session_ended(&self, session_id: String, reason: String) {
        let event = ProximityEvent::SessionEnded {
            session_id,
            reason,
            timestamp: chrono::Utc::now().timestamp(),
        };

        if let Err(e) = self.tx.send(event) {
            warn!("Failed to broadcast session ended event: {}", e);
        }
    }
}

/// WebSocket handler for proximity events
/// 
/// **Validates: Requirements 6.1, 7.6**
pub async fn proximity_websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(|socket| handle_proximity_socket(socket, state))
}

/// Handle individual WebSocket connection for proximity events
async fn handle_proximity_socket(socket: WebSocket, _state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    // Note: In production, proximity_websocket_service would be part of AppState
    // For now, we'll create a temporary service
    let proximity_ws_service = ProximityWebSocketService::new();
    let mut rx = proximity_ws_service.subscribe();

    info!("New proximity WebSocket connection established");

    // Spawn a task to send updates to the client
    let mut send_task = tokio::spawn(async move {
        while let Ok(event) = rx.recv().await {
            // Serialize the event to JSON
            match serde_json::to_string(&event) {
                Ok(json) => {
                    if sender.send(Message::Text(json)).await.is_err() {
                        error!("Failed to send proximity event to client");
                        break;
                    }
                }
                Err(e) => {
                    error!("Failed to serialize proximity event: {}", e);
                }
            }
        }
    });

    // Spawn a task to receive messages from the client (for ping/pong)
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    info!("Received text message from proximity WebSocket client: {}", text);
                }
                Message::Close(_) => {
                    info!("Proximity WebSocket client closed connection");
                    break;
                }
                Message::Ping(_data) => {
                    info!("Received ping from proximity WebSocket client");
                }
                Message::Pong(_) => {
                    info!("Received pong from proximity WebSocket client");
                }
                _ => {}
            }
        }
    });

    // Wait for either task to finish
    tokio::select! {
        _ = (&mut send_task) => {
            recv_task.abort();
        }
        _ = (&mut recv_task) => {
            send_task.abort();
        }
    }

    info!("Proximity WebSocket connection closed");
}
