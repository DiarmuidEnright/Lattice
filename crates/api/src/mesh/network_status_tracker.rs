use anyhow::Result;
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use std::sync::Arc;
use uuid::Uuid;

use crate::mesh_types::{DataFreshness, NetworkStatus, ProviderInfo};
use proximity::PeerConnectionManager;

/// Tracks network topology and provider availability
/// 
/// This service maintains information about active provider nodes in the mesh network,
/// including their hop counts (distance from this node) and last seen timestamps.
/// It also tracks the overall network topology to help with routing decisions.
pub struct NetworkStatusTracker {
    /// Peer connection manager for accessing connected peers
    peer_manager: Arc<PeerConnectionManager>,
    /// Redis connection for distributed state tracking
    redis: redis::aio::ConnectionManager,
    /// Unique identifier for this node
    local_node_id: Uuid,
    /// Timestamp when all providers went offline (for extended offline indicator)
    all_providers_offline_since: Arc<tokio::sync::RwLock<Option<DateTime<Utc>>>>,
}

impl NetworkStatusTracker {
    /// Create a new NetworkStatusTracker
    /// 
    /// # Arguments
    /// * `peer_manager` - Manager for peer connections
    /// * `redis` - Redis connection for distributed state
    /// * `local_node_id` - Unique identifier for this node
    pub fn new(
        peer_manager: Arc<PeerConnectionManager>,
        redis: redis::aio::ConnectionManager,
        local_node_id: Uuid,
    ) -> Self {
        Self {
            peer_manager,
            redis,
            local_node_id,
            all_providers_offline_since: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }
    
    /// Update provider status in network
    /// 
    /// Records or removes a provider node in the network registry.
    /// This is called when a node enables/disables provider mode or when
    /// we learn about a provider through network status messages.
    /// 
    /// # Arguments
    /// * `node_id` - The provider node's unique identifier
    /// * `is_provider` - Whether the node is currently a provider
    /// 
    /// # Returns
    /// * `Ok(())` - Status updated successfully
    /// * `Err(_)` - Redis error occurred
    pub async fn update_provider_status(&self, node_id: Uuid, is_provider: bool) -> Result<()> {
        let redis_key = format!("mesh:provider:{}:status", node_id);
        let mut conn = self.redis.clone();
        
        if is_provider {
            // Store provider status with timestamp
            let status = serde_json::json!({
                "node_id": node_id,
                "is_provider": true,
                "last_seen": Utc::now().to_rfc3339(),
            });
            let status_str = serde_json::to_string(&status)?;
            
            // Set with 5-minute TTL (providers should refresh periodically)
            let _: () = conn.set_ex(&redis_key, status_str, 300).await?;
            
            // Add to provider set
            let _: () = conn.sadd("mesh:network:providers", node_id.to_string()).await?;
            
            tracing::debug!("Updated provider status for node {}: active", node_id);
        } else {
            // Remove provider status
            let _: () = conn.del(&redis_key).await?;
            let _: () = conn.srem("mesh:network:providers", node_id.to_string()).await?;
            
            tracing::debug!("Updated provider status for node {}: inactive", node_id);
        }
        
        Ok(())
    }
    
    /// Get list of active providers with hop counts
    /// 
    /// Returns information about all currently active provider nodes,
    /// including their last seen timestamp and hop count (distance from this node).
    /// 
    /// # Returns
    /// * `Ok(Vec<ProviderInfo>)` - List of active providers
    /// * `Err(_)` - Redis error occurred
    pub async fn get_active_providers(&self) -> Result<Vec<ProviderInfo>> {
        let mut conn = self.redis.clone();
        
        // Get all provider node IDs from the set
        let provider_ids: Vec<String> = conn.smembers("mesh:network:providers").await?;
        
        let mut providers = Vec::new();
        
        for provider_id_str in provider_ids {
            let node_id = match Uuid::parse_str(&provider_id_str) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!("Invalid provider node ID {}: {}", provider_id_str, e);
                    continue;
                }
            };
            
            // Get provider status
            let redis_key = format!("mesh:provider:{}:status", node_id);
            let status_str: Option<String> = conn.get(&redis_key).await?;
            
            if let Some(status_json) = status_str {
                match serde_json::from_str::<serde_json::Value>(&status_json) {
                    Ok(status) => {
                        let last_seen_str = status["last_seen"]
                            .as_str()
                            .unwrap_or_else(|| {
                                tracing::warn!("Missing last_seen for provider {}", node_id);
                                ""
                            });
                        
                        let last_seen = DateTime::parse_from_rfc3339(last_seen_str)
                            .map(|dt| dt.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now());
                        
                        // Get hop count from topology data
                        let hop_count = self.get_hop_count(node_id).await.unwrap_or(99);
                        
                        providers.push(ProviderInfo {
                            node_id,
                            last_seen,
                            hop_count,
                        });
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse provider status for {}: {}", node_id, e);
                    }
                }
            }
        }
        
        // Sort by hop count (closest providers first)
        providers.sort_by_key(|p| p.hop_count);
        
        Ok(providers)
    }
    
    /// Update network topology information
    /// 
    /// Records the distance (hop count) to a peer in the network.
    /// This information is used for routing decisions and provider selection.
    /// 
    /// # Arguments
    /// * `peer_id` - The peer's identifier
    /// * `hop_count` - Number of hops to reach this peer
    /// 
    /// # Returns
    /// * `Ok(())` - Topology updated successfully
    /// * `Err(_)` - Redis error occurred
    pub async fn update_topology(&self, peer_id: String, hop_count: u32) -> Result<()> {
        let redis_key = format!("mesh:topology:{}:{}", self.local_node_id, peer_id);
        let mut conn = self.redis.clone();
        
        let topology_data = serde_json::json!({
            "peer_id": peer_id,
            "hop_count": hop_count,
            "updated_at": Utc::now().to_rfc3339(),
        });
        let topology_str = serde_json::to_string(&topology_data)?;
        
        // Set with 10-minute TTL (topology should be refreshed periodically)
        let _: () = conn.set_ex(&redis_key, topology_str, 600).await?;
        
        tracing::debug!(
            "Updated topology: peer {} is {} hops away",
            peer_id,
            hop_count
        );
        
        Ok(())
    }
    
    /// Get current network status
    /// 
    /// Builds a comprehensive NetworkStatus summary including active providers,
    /// connected peers, and data freshness information.
    /// 
    /// Tracks when all providers go offline and provides warnings:
    /// - Immediate warning when no providers are online
    /// - Extended offline indicator after 10 minutes without providers
    /// 
    /// Requirements: 9.1, 9.2, 9.5
    /// 
    /// # Returns
    /// * `Ok(NetworkStatus)` - Current network status
    /// * `Err(_)` - Error building status
    pub async fn get_status(&self) -> Result<NetworkStatus> {
        // Get active providers
        let active_providers = self.get_active_providers().await?;
        
        // Track when all providers went offline
        let mut offline_since = self.all_providers_offline_since.write().await;
        if active_providers.is_empty() {
            // If we don't have a timestamp yet, record when providers went offline
            if offline_since.is_none() {
                *offline_since = Some(Utc::now());
                tracing::warn!("All providers are now offline");
            }
        } else {
            // Providers are back online, clear the offline timestamp
            if offline_since.is_some() {
                tracing::info!("Providers are back online");
                *offline_since = None;
            }
        }
        let offline_duration = offline_since.as_ref().map(|since| {
            Utc::now().signed_duration_since(*since)
        });
        drop(offline_since);
        
        // Get connected peers count from PeerConnectionManager
        let active_connections = self.peer_manager.get_active_connections().await;
        let connected_peers = active_connections.len();
        
        tracing::info!(
            "Network status: {} active providers, {} connected peers",
            active_providers.len(),
            connected_peers
        );
        
        // Calculate total network size (estimate based on providers + peers)
        let total_network_size = active_providers.len() + connected_peers;
        
        // Determine last update time from most recent provider
        let last_update_time = active_providers
            .iter()
            .map(|p| p.last_seen)
            .max();
        
        // Calculate data freshness based on most recent provider update
        let data_freshness = if let Some(last_update) = last_update_time {
            DataFreshness::from_timestamp(last_update)
        } else {
            DataFreshness::Stale
        };
        
        // Calculate extended offline status
        let offline_duration_minutes = offline_duration.map(|d| d.num_minutes());
        let extended_offline = offline_duration_minutes
            .map(|minutes| minutes >= 10)
            .unwrap_or(false);
        
        // Log warnings based on provider status
        if active_providers.is_empty() {
            if let Some(minutes) = offline_duration_minutes {
                if minutes >= 10 {
                    tracing::warn!(
                        "Extended offline: No providers have been online for {} minutes",
                        minutes
                    );
                }
            }
        }
        
        Ok(NetworkStatus {
            active_providers,
            connected_peers,
            total_network_size,
            last_update_time,
            data_freshness,
            extended_offline,
            offline_duration_minutes,
        })
    }
    
    /// Get hop count to a specific node
    /// 
    /// Internal helper to retrieve the hop count to a node from topology data.
    /// 
    /// # Arguments
    /// * `node_id` - The node's unique identifier
    /// 
    /// # Returns
    /// * `Ok(u32)` - Hop count to the node
    /// * `Err(_)` - Node not found in topology or Redis error
    async fn get_hop_count(&self, node_id: Uuid) -> Result<u32> {
        let redis_key = format!("mesh:topology:{}:{}", self.local_node_id, node_id);
        let mut conn = self.redis.clone();
        
        let topology_str: Option<String> = conn.get(&redis_key).await?;
        
        if let Some(topology_json) = topology_str {
            let topology: serde_json::Value = serde_json::from_str(&topology_json)?;
            let hop_count = topology["hop_count"]
                .as_u64()
                .unwrap_or(99) as u32;
            Ok(hop_count)
        } else {
            // If this is the local node, hop count is 0
            if node_id == self.local_node_id {
                Ok(0)
            } else {
                // Node not in topology, return high value
                Ok(99)
            }
        }
    }
    
    /// Check if all providers have been offline for extended period
    /// 
    /// Returns true if no providers have been online for 10 minutes or more.
    /// This is used to display a prominent offline indicator in the UI.
    /// 
    /// Requirement: 9.5
    /// 
    /// # Returns
    /// * `true` - Providers have been offline for 10+ minutes
    /// * `false` - Providers are online or offline for less than 10 minutes
    pub async fn is_extended_offline(&self) -> bool {
        let offline_since = self.all_providers_offline_since.read().await;
        
        if let Some(since) = *offline_since {
            let duration = Utc::now().signed_duration_since(since);
            duration.num_minutes() >= 10
        } else {
            false
        }
    }
    
    /// Get the duration that all providers have been offline
    /// 
    /// Returns None if providers are currently online.
    /// 
    /// # Returns
    /// * `Some(Duration)` - How long all providers have been offline
    /// * `None` - Providers are currently online
    pub async fn get_offline_duration(&self) -> Option<chrono::Duration> {
        let offline_since = self.all_providers_offline_since.read().await;
        
        offline_since.as_ref().map(|since| {
            Utc::now().signed_duration_since(*since)
        })
    }
    
    /// Handle provider reconnection
    /// 
    /// Called when a provider comes back online after being offline.
    /// Clears the offline tracking and logs the recovery.
    /// 
    /// Requirement: 9.3
    /// 
    /// # Arguments
    /// * `node_id` - The provider node that reconnected
    pub async fn on_provider_reconnected(&self, node_id: Uuid) -> Result<()> {
        tracing::info!("Provider {} reconnected", node_id);
        
        // Update provider status to active
        self.update_provider_status(node_id, true).await?;
        
        // Check if this brings us back online
        let active_providers = self.get_active_providers().await?;
        if !active_providers.is_empty() {
            let mut offline_since = self.all_providers_offline_since.write().await;
            if let Some(since) = *offline_since {
                let duration = Utc::now().signed_duration_since(since);
                tracing::info!(
                    "Network recovered: Providers back online after {} minutes",
                    duration.num_minutes()
                );
                *offline_since = None;
            }
        }
        
        Ok(())
    }
    
    /// Handle provider disconnect
    /// 
    /// Called when a provider goes offline. Updates the provider status
    /// and tracks if all providers are now offline.
    /// 
    /// Requirement: 9.1, 9.2
    /// 
    /// # Arguments
    /// * `node_id` - The provider node that disconnected
    pub async fn on_provider_disconnected(&self, node_id: Uuid) -> Result<()> {
        tracing::info!("Provider {} disconnected", node_id);
        
        // Update provider status to inactive
        self.update_provider_status(node_id, false).await?;
        
        // Check if all providers are now offline
        let active_providers = self.get_active_providers().await?;
        if active_providers.is_empty() {
            let mut offline_since = self.all_providers_offline_since.write().await;
            if offline_since.is_none() {
                *offline_since = Some(Utc::now());
                tracing::warn!("All providers are now offline - cached data will be served");
            }
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximity::PeerConnectionManager;
    
    // Helper to create a test Redis connection
    async fn create_test_redis() -> redis::aio::ConnectionManager {
        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        let client = redis::Client::open(redis_url).expect("Failed to create Redis client");
        redis::aio::ConnectionManager::new(client)
            .await
            .expect("Failed to connect to Redis")
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_update_provider_status_active() {
        let redis = create_test_redis().await;
        let peer_manager = Arc::new(PeerConnectionManager::new());
        let local_node_id = Uuid::new_v4();
        let provider_node_id = Uuid::new_v4();
        
        let tracker = NetworkStatusTracker::new(peer_manager, redis, local_node_id);
        
        // Update provider status to active
        tracker.update_provider_status(provider_node_id, true).await.unwrap();
        
        // Verify provider is in the active list
        let providers = tracker.get_active_providers().await.unwrap();
        assert!(
            providers.iter().any(|p| p.node_id == provider_node_id),
            "Provider should be in active list"
        );
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_update_provider_status_inactive() {
        let redis = create_test_redis().await;
        let peer_manager = Arc::new(PeerConnectionManager::new());
        let local_node_id = Uuid::new_v4();
        let provider_node_id = Uuid::new_v4();
        
        let tracker = NetworkStatusTracker::new(peer_manager, redis, local_node_id);
        
        // First activate the provider
        tracker.update_provider_status(provider_node_id, true).await.unwrap();
        
        // Then deactivate it
        tracker.update_provider_status(provider_node_id, false).await.unwrap();
        
        // Verify provider is not in the active list
        let providers = tracker.get_active_providers().await.unwrap();
        assert!(
            !providers.iter().any(|p| p.node_id == provider_node_id),
            "Provider should not be in active list after deactivation"
        );
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_update_topology() {
        let redis = create_test_redis().await;
        let peer_manager = Arc::new(PeerConnectionManager::new());
        let local_node_id = Uuid::new_v4();
        
        let tracker = NetworkStatusTracker::new(peer_manager, redis, local_node_id);
        
        let peer_id = format!("peer_{}", Uuid::new_v4());
        let hop_count = 3;
        
        // Update topology
        tracker.update_topology(peer_id.clone(), hop_count).await.unwrap();
        
        // Verify topology was stored (we can't directly test get_hop_count as it's private,
        // but we can verify no error occurred)
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_get_status_no_providers() {
        let redis = create_test_redis().await;
        let peer_manager = Arc::new(PeerConnectionManager::new());
        let local_node_id = Uuid::new_v4();
        
        let tracker = NetworkStatusTracker::new(peer_manager, redis, local_node_id);
        
        // Get status with no providers
        let status = tracker.get_status().await.unwrap();
        
        assert_eq!(status.active_providers.len(), 0, "Should have no active providers");
        assert_eq!(status.connected_peers, 0, "Should have no connected peers");
        assert!(matches!(status.data_freshness, DataFreshness::Stale), "Data should be stale");
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_get_active_providers_sorted_by_hop_count() {
        let redis = create_test_redis().await;
        let peer_manager = Arc::new(PeerConnectionManager::new());
        let local_node_id = Uuid::new_v4();
        
        let tracker = NetworkStatusTracker::new(peer_manager, redis, local_node_id);
        
        // Create multiple providers with different hop counts
        let provider1 = Uuid::new_v4();
        let provider2 = Uuid::new_v4();
        let provider3 = Uuid::new_v4();
        
        // Activate providers
        tracker.update_provider_status(provider1, true).await.unwrap();
        tracker.update_provider_status(provider2, true).await.unwrap();
        tracker.update_provider_status(provider3, true).await.unwrap();
        
        // Set topology (hop counts)
        tracker.update_topology(provider1.to_string(), 5).await.unwrap();
        tracker.update_topology(provider2.to_string(), 2).await.unwrap();
        tracker.update_topology(provider3.to_string(), 8).await.unwrap();
        
        // Get providers
        let providers = tracker.get_active_providers().await.unwrap();
        
        // Verify they are sorted by hop count
        assert_eq!(providers.len(), 3, "Should have 3 providers");
        assert!(
            providers[0].hop_count <= providers[1].hop_count,
            "Providers should be sorted by hop count"
        );
        assert!(
            providers[1].hop_count <= providers[2].hop_count,
            "Providers should be sorted by hop count"
        );
    }
}
