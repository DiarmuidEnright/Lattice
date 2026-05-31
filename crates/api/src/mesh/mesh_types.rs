use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// A price update message for mesh network distribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceUpdate {
    pub message_id: Uuid,
    pub source_node_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub prices: HashMap<String, PriceData>,
    pub ttl: u32,
}

/// Price data for a single asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceData {
    pub asset: String,
    pub price: String,
    pub blockchain: String,
    pub change_24h: Option<String>,
}

/// Cached price data with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPriceData {
    pub asset: String,
    pub price: String,
    pub timestamp: DateTime<Utc>,
    pub source_node_id: Uuid,
    pub blockchain: String,
    pub change_24h: Option<String>,
}

/// Provider node configuration
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub enabled: bool,
    pub api_key: Option<String>,
    pub fetch_interval_secs: u64,
    pub node_id: Uuid,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            fetch_interval_secs: 30,
            node_id: Uuid::new_v4(),
        }
    }
}

/// Network status information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkStatus {
    pub active_providers: Vec<ProviderInfo>,
    pub connected_peers: usize,
    pub total_network_size: usize,
    pub last_update_time: Option<DateTime<Utc>>,
    pub data_freshness: DataFreshness,
    /// True if no providers have been online for 10+ minutes
    pub extended_offline: bool,
    /// Duration in minutes that all providers have been offline (None if online)
    pub offline_duration_minutes: Option<i64>,
}

/// Information about a provider node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub node_id: Uuid,
    pub last_seen: DateTime<Utc>,
    pub hop_count: u32,
}

/// Data freshness indicator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DataFreshness {
    JustNow,
    MinutesAgo(u32),
    HoursAgo(u32),
    Stale,
}

impl DataFreshness {
    /// Calculate freshness from a timestamp
    pub fn from_timestamp(timestamp: DateTime<Utc>) -> Self {
        let now = Utc::now();
        let duration = now.signed_duration_since(timestamp);
        
        let seconds = duration.num_seconds();
        
        if seconds < 60 {
            DataFreshness::JustNow
        } else if seconds < 3600 {
            DataFreshness::MinutesAgo((seconds / 60) as u32)
        } else {
            let hours = (seconds / 3600) as u32;
            if hours < 24 {
                DataFreshness::HoursAgo(hours)
            } else {
                DataFreshness::Stale
            }
        }
    }
    
    /// Check if data is stale (older than 1 hour)
    pub fn is_stale(&self) -> bool {
        matches!(self, DataFreshness::HoursAgo(_) | DataFreshness::Stale)
    }
}
