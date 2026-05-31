use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a peer
pub type PeerId = String;

/// Discovery method for finding peers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoveryMethod {
    WiFi,
    Bluetooth,
}

impl std::fmt::Display for DiscoveryMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiscoveryMethod::WiFi => write!(f, "WiFi"),
            DiscoveryMethod::Bluetooth => write!(f, "Bluetooth"),
        }
    }
}

/// A discovered peer on the local network
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPeer {
    pub peer_id: PeerId,
    /// The peer's real user account id (UUID). Broadcast in the discovery
    /// announcement so a transfer can target a valid `users(id)` directly.
    /// `peer_id` remains the device-level identifier used for dedup/display.
    pub user_id: Uuid,
    pub user_tag: String,
    pub wallet_address: String,
    pub discovery_method: DiscoveryMethod,
    pub signal_strength: Option<i8>,
    pub verified: bool,
    pub discovered_at: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Status of a transfer request
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferStatus {
    Pending,
    Accepted,
    Executing,
    /// Accepted and recorded while offline (no blockchain reachable). The funds
    /// have NOT settled on-chain yet; a background sync task submits these to the
    /// chain once connectivity returns, then transitions them to Completed.
    PendingSettlement,
    Rejected,
    Completed,
    Failed,
    Expired,
}

impl std::fmt::Display for TransferStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferStatus::Pending => write!(f, "Pending"),
            TransferStatus::Accepted => write!(f, "Accepted"),
            TransferStatus::Executing => write!(f, "Executing"),
            TransferStatus::PendingSettlement => write!(f, "PendingSettlement"),
            TransferStatus::Rejected => write!(f, "Rejected"),
            TransferStatus::Completed => write!(f, "Completed"),
            TransferStatus::Failed => write!(f, "Failed"),
            TransferStatus::Expired => write!(f, "Expired"),
        }
    }
}

/// A transfer request between peers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRequest {
    pub id: Uuid,
    pub sender_user_id: Uuid,
    pub sender_wallet: String,
    pub recipient_user_id: Uuid,
    pub recipient_wallet: String,
    pub asset: String,
    pub amount: Decimal,
    pub status: TransferStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Discovery session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoverySession {
    pub session_id: Uuid,
    pub user_id: Uuid,
    pub discovery_method: DiscoveryMethod,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub auto_extend: bool,
}

/// Connection type for peer-to-peer communication
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionType {
    WebRTC,
    TcpSocket,
    BleConnection,
}

/// Connection quality metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionQuality {
    pub signal_strength: Option<i8>,
    pub latency_ms: u32,
    pub packet_loss_percent: f32,
}

/// Peer-to-peer message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PeerMessage {
    Challenge { nonce: Vec<u8> },
    ChallengeResponse { signature: Vec<u8>, public_key: Vec<u8> },
    TransferRequest { request: TransferRequest },
    TransferAccepted { request_id: Uuid },
    TransferRejected { request_id: Uuid, reason: Option<String> },
    TransferCompleted { request_id: Uuid, tx_hash: String },
    Ping,
    Pong,
    
    // Mesh network price distribution messages
    PriceUpdate {
        message_id: Uuid,
        source_node_id: Uuid,
        timestamp: DateTime<Utc>,
        prices: serde_json::Value, // HashMap<String, PriceData> serialized
        ttl: u32,
    },
    NetworkStatus {
        node_id: Uuid,
        is_provider: bool,
        hop_count: u32,
    },
}
