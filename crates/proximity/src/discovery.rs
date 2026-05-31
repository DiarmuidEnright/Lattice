// Discovery Service - handles peer discovery via mDNS and BLE

use crate::mdns::{MdnsAnnouncer, MdnsListener};
use crate::{DiscoveredPeer, DiscoveryMethod, PeerId, ProximityError, Result};
use chrono::{Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, Notify};
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{debug, info, warn};

const PEER_REFRESH_INTERVAL_SECS: u64 = 5;
const PEER_TIMEOUT_SECS: i64 = 30;

pub struct DiscoveryService {
    discovered_peers: Arc<RwLock<HashMap<PeerId, DiscoveredPeer>>>,
    active_method: Arc<RwLock<Option<DiscoveryMethod>>>,
    shutdown_notify: Arc<Notify>,
    mdns_announcer: Arc<RwLock<Option<MdnsAnnouncer>>>,
    mdns_listener: Arc<RwLock<Option<MdnsListener>>>,
    // BLE components will be added in Task 4
    user_id: String,
    user_tag: String,
    device_id: String,
    wallet_address: String,
    max_peers: usize,
}

impl DiscoveryService {
    pub fn new(user_tag: String, device_id: String, wallet_address: String) -> Self {
        // Back-compat constructor: no explicit user_id (uses nil). Prefer
        // `with_identity` so peers can be targeted for transfers by real UUID.
        Self::with_identity(
            uuid::Uuid::nil().to_string(),
            user_tag,
            device_id,
            wallet_address,
        )
    }

    /// Construct a discovery service with an explicit user account id (UUID as
    /// string) that is broadcast so discovered peers can be used as transfer
    /// recipients.
    pub fn with_identity(
        user_id: String,
        user_tag: String,
        device_id: String,
        wallet_address: String,
    ) -> Self {
        Self {
            discovered_peers: Arc::new(RwLock::new(HashMap::new())),
            active_method: Arc::new(RwLock::new(None)),
            shutdown_notify: Arc::new(Notify::new()),
            mdns_announcer: Arc::new(RwLock::new(None)),
            mdns_listener: Arc::new(RwLock::new(None)),
            user_id,
            user_tag,
            device_id,
            wallet_address,
            max_peers: 50, // Default capacity limit
        }
    }

    /// Start discovery session with the specified method
    pub async fn start_discovery(&self, method: DiscoveryMethod) -> Result<()> {
        let mut active = self.active_method.write().await;
        
        if active.is_some() {
            warn!("Discovery already active, stopping previous session");
            drop(active);
            self.stop_discovery().await?;
            active = self.active_method.write().await;
        }

        info!("Starting discovery with method: {}", method);
        *active = Some(method);
        
        // Start the peer list refresh background task
        self.start_refresh_task();
        
        // Start mDNS or BLE discovery based on method
        match method {
            DiscoveryMethod::WiFi => {
                self.start_mdns_discovery().await?;
            }
            DiscoveryMethod::Bluetooth => {
                // TODO: Start BLE discovery (Task 4)
                warn!("Bluetooth discovery not yet implemented");
            }
        }
        
        Ok(())
    }

    /// Stop active discovery session
    pub async fn stop_discovery(&self) -> Result<()> {
        let mut active = self.active_method.write().await;
        
        if active.is_none() {
            debug!("No active discovery session to stop");
            return Ok(());
        }

        info!("Stopping discovery session");
        let method = *active;
        *active = None;
        
        // Signal shutdown to background tasks
        self.shutdown_notify.notify_waiters();
        
        // Stop mDNS or BLE discovery based on method
        if let Some(method) = method {
            match method {
                DiscoveryMethod::WiFi => {
                    self.stop_mdns_discovery().await?;
                }
                DiscoveryMethod::Bluetooth => {
                    // TODO: Stop BLE discovery (Task 4)
                }
            }
        }
        
        // Clear discovered peers
        let mut peers = self.discovered_peers.write().await;
        peers.clear();
        
        Ok(())
    }

    /// Get list of currently discovered peers
    pub async fn get_discovered_peers(&self) -> Result<Vec<DiscoveredPeer>> {
        let peers = self.discovered_peers.read().await;
        Ok(peers.values().cloned().collect())
    }

    /// Check if discovery is currently active
    pub async fn is_active(&self) -> bool {
        self.active_method.read().await.is_some()
    }

    /// Get the current active discovery method
    pub async fn get_active_method(&self) -> Option<DiscoveryMethod> {
        *self.active_method.read().await
    }

    /// Get the maximum peer capacity
    pub fn get_max_peers(&self) -> usize {
        self.max_peers
    }

    /// Refresh peer list by removing stale peers
    pub async fn refresh_peer_list(&self) -> Result<()> {
        let now = Utc::now();
        let timeout_threshold = now - Duration::seconds(PEER_TIMEOUT_SECS);
        
        let mut peers = self.discovered_peers.write().await;
        let initial_count = peers.len();
        
        // Remove peers that haven't been seen within the timeout period
        peers.retain(|peer_id, peer| {
            let is_active = peer.last_seen > timeout_threshold;
            if !is_active {
                debug!("Removing stale peer: {} (last seen: {})", peer_id, peer.last_seen);
            }
            is_active
        });
        
        let removed_count = initial_count - peers.len();
        if removed_count > 0 {
            info!("Removed {} stale peer(s) from discovery list", removed_count);
        }
        
        Ok(())
    }

    /// Add or update a discovered peer
    pub async fn add_or_update_peer(&self, peer: DiscoveredPeer) -> Result<()> {
        let mut peers = self.discovered_peers.write().await;
        
        if let Some(existing) = peers.get_mut(&peer.peer_id) {
            // Update existing peer's last_seen timestamp
            existing.last_seen = peer.last_seen;
            existing.signal_strength = peer.signal_strength;
            debug!("Updated peer: {}", peer.peer_id);
        } else {
            // Check if we've reached capacity
            if peers.len() >= self.max_peers {
                // Find the peer with the weakest signal strength
                let weakest_peer = peers.iter()
                    .min_by_key(|(_, p)| p.signal_strength.unwrap_or(i8::MIN))
                    .map(|(id, _)| id.clone());
                
                if let Some(weakest_id) = weakest_peer {
                    let weakest_signal = peers.get(&weakest_id)
                        .and_then(|p| p.signal_strength)
                        .unwrap_or(i8::MIN);
                    let new_signal = peer.signal_strength.unwrap_or(i8::MIN);
                    
                    // Only replace if new peer has stronger signal
                    if new_signal > weakest_signal {
                        peers.remove(&weakest_id);
                        info!("Capacity limit reached, replaced weakest peer {} (signal: {}) with {} (signal: {})",
                            weakest_id, weakest_signal, peer.peer_id, new_signal);
                        peers.insert(peer.peer_id.clone(), peer);
                    } else {
                        debug!("Capacity limit reached, ignoring peer {} with weaker signal ({})", 
                            peer.peer_id, new_signal);
                    }
                }
            } else {
                // Add new peer
                info!("Discovered new peer: {} ({})", peer.user_tag, peer.peer_id);
                peers.insert(peer.peer_id.clone(), peer);
            }
        }
        
        Ok(())
    }

    /// Remove a specific peer from the discovered list
    pub async fn remove_peer(&self, peer_id: &PeerId) -> Result<()> {
        let mut peers = self.discovered_peers.write().await;
        
        if peers.remove(peer_id).is_some() {
            info!("Removed peer: {}", peer_id);
            Ok(())
        } else {
            Err(ProximityError::PeerNotFound(peer_id.clone()))
        }
    }

    /// Start mDNS discovery (announcer + listener)
    async fn start_mdns_discovery(&self) -> Result<()> {
        info!("Starting mDNS discovery");

        // Create and start mDNS announcer
        let announcer = MdnsAnnouncer::new(
            self.user_id.clone(),
            self.user_tag.clone(),
            self.device_id.clone(),
            self.wallet_address.clone(),
        )?;
        announcer.start().await?;

        let mut announcer_guard = self.mdns_announcer.write().await;
        *announcer_guard = Some(announcer);
        drop(announcer_guard);

        // Create and start mDNS listener
        let listener = MdnsListener::new()?;
        
        // Set up callback to add discovered peers. Exclude our own announcement
        // (same device_id): mDNS browse echoes back the service this node itself
        // registered, and listing ourselves as a peer would be confusing in a
        // multi-device demo. We only record peers whose device_id differs from
        // ours.
        let discovered_peers = Arc::clone(&self.discovered_peers);
        let own_device_id = self.device_id.clone();
        listener.start(move |peer| {
            if peer.peer_id == own_device_id {
                debug!("Ignoring self-announcement for device {}", own_device_id);
                return;
            }
            let peers = Arc::clone(&discovered_peers);
            tokio::spawn(async move {
                let mut peer_map = peers.write().await;
                peer_map.insert(peer.peer_id.clone(), peer.clone());
                info!("Added peer via mDNS: {} ({})", peer.user_tag, peer.peer_id);
            });
        }).await?;

        let mut listener_guard = self.mdns_listener.write().await;
        *listener_guard = Some(listener);

        info!("mDNS discovery started successfully");
        Ok(())
    }

    /// Stop mDNS discovery
    async fn stop_mdns_discovery(&self) -> Result<()> {
        info!("Stopping mDNS discovery");

        // Stop announcer
        let mut announcer_guard = self.mdns_announcer.write().await;
        if let Some(announcer) = announcer_guard.take() {
            announcer.stop().await?;
        }

        // Stop listener
        let mut listener_guard = self.mdns_listener.write().await;
        if let Some(listener) = listener_guard.take() {
            listener.stop().await?;
        }

        info!("mDNS discovery stopped");
        Ok(())
    }

    /// Start background task to refresh peer list at regular intervals
    fn start_refresh_task(&self) {
        let peers = Arc::clone(&self.discovered_peers);
        let active_method = Arc::clone(&self.active_method);
        let shutdown = Arc::clone(&self.shutdown_notify);
        let listener_handle = Arc::clone(&self.mdns_listener);
        let own_device_id = self.device_id.clone();
        
        tokio::spawn(async move {
            let mut refresh_interval = interval(TokioDuration::from_secs(PEER_REFRESH_INTERVAL_SECS));
            
            loop {
                tokio::select! {
                    _ = refresh_interval.tick() => {
                        // Check if discovery is still active
                        let is_active = active_method.read().await.is_some();
                        if !is_active {
                            debug!("Discovery inactive, stopping refresh task");
                            break;
                        }

                        // Re-sync from the mDNS listener's live peer set before
                        // reaping. mDNS `ServiceResolved` fires once on initial
                        // discovery, so the one-shot callback can't keep
                        // `last_seen` fresh; without this re-sync a peer that is
                        // still on the network would be evicted after
                        // PEER_TIMEOUT_SECS. The listener maintains an
                        // authoritative set (updated on resolve and pruned on
                        // `ServiceRemoved`), so we refresh `last_seen` for every
                        // peer it still reports as present (excluding ourselves).
                        let now = Utc::now();
                        {
                            let listener_guard = listener_handle.read().await;
                            if let Some(listener) = listener_guard.as_ref() {
                                let live_peers = listener.get_discovered_peers().await;
                                let mut peer_map = peers.write().await;
                                for mut live in live_peers {
                                    if live.peer_id == own_device_id {
                                        continue;
                                    }
                                    live.last_seen = now;
                                    peer_map
                                        .entry(live.peer_id.clone())
                                        .and_modify(|p| p.last_seen = now)
                                        .or_insert(live);
                                }
                            }
                        }

                        // Reap peers the listener no longer reports (left the
                        // network) after the staleness window.
                        let timeout_threshold = now - Duration::seconds(PEER_TIMEOUT_SECS);
                        
                        let mut peer_map = peers.write().await;

                        let initial_count = peer_map.len();
                        
                        peer_map.retain(|peer_id, peer| {
                            let is_active = peer.last_seen > timeout_threshold;
                            if !is_active {
                                debug!("Removing stale peer: {} (last seen: {})", peer_id, peer.last_seen);
                            }
                            is_active
                        });
                        
                        let removed_count = initial_count - peer_map.len();
                        if removed_count > 0 {
                            info!("Refresh task removed {} stale peer(s)", removed_count);
                        }
                    }
                    _ = shutdown.notified() => {
                        debug!("Refresh task received shutdown signal");
                        break;
                    }
                }
            }
            
            debug!("Peer refresh task terminated");
        });
    }
}

impl Default for DiscoveryService {
    fn default() -> Self {
        Self::new(
            "default_user".to_string(),
            "default_device".to_string(),
            "default_wallet".to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration as TokioDuration};

    #[tokio::test]
    async fn test_discovery_service_creation() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        assert!(!service.is_active().await);
        assert!(service.get_active_method().await.is_none());
    }

    #[tokio::test]
    async fn test_start_stop_discovery() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        // Start WiFi discovery
        service.start_discovery(DiscoveryMethod::WiFi).await.unwrap();
        assert!(service.is_active().await);
        assert_eq!(service.get_active_method().await, Some(DiscoveryMethod::WiFi));
        
        // Stop discovery
        service.stop_discovery().await.unwrap();
        assert!(!service.is_active().await);
        assert!(service.get_active_method().await.is_none());
    }

    #[tokio::test]
    async fn test_start_discovery_replaces_previous_session() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        // Start WiFi discovery
        service.start_discovery(DiscoveryMethod::WiFi).await.unwrap();
        assert_eq!(service.get_active_method().await, Some(DiscoveryMethod::WiFi));
        
        // Start Bluetooth discovery (should replace WiFi)
        service.start_discovery(DiscoveryMethod::Bluetooth).await.unwrap();
        assert_eq!(service.get_active_method().await, Some(DiscoveryMethod::Bluetooth));
    }

    #[tokio::test]
    async fn test_add_and_get_peers() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        let peer = DiscoveredPeer {
            peer_id: "peer1".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Alice".to_string(),
            wallet_address: "wallet123".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-50),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.add_or_update_peer(peer.clone()).await.unwrap();
        
        let peers = service.get_discovered_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, "peer1");
        assert_eq!(peers[0].user_tag, "Alice");
    }

    #[tokio::test]
    async fn test_update_existing_peer() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        let peer1 = DiscoveredPeer {
            peer_id: "peer1".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Alice".to_string(),
            wallet_address: "wallet123".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-50),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.add_or_update_peer(peer1).await.unwrap();
        
        // Update with new signal strength
        let peer2 = DiscoveredPeer {
            peer_id: "peer1".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Alice".to_string(),
            wallet_address: "wallet123".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-40),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.add_or_update_peer(peer2).await.unwrap();
        
        let peers = service.get_discovered_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].signal_strength, Some(-40));
    }

    #[tokio::test]
    async fn test_remove_peer() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        let peer = DiscoveredPeer {
            peer_id: "peer1".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Alice".to_string(),
            wallet_address: "wallet123".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-50),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.add_or_update_peer(peer).await.unwrap();
        assert_eq!(service.get_discovered_peers().await.unwrap().len(), 1);
        
        service.remove_peer(&"peer1".to_string()).await.unwrap();
        assert_eq!(service.get_discovered_peers().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_remove_nonexistent_peer() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        let result = service.remove_peer(&"nonexistent".to_string()).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ProximityError::PeerNotFound(_)));
    }

    #[tokio::test]
    async fn test_refresh_removes_stale_peers() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        // Add a peer with old last_seen timestamp
        let old_peer = DiscoveredPeer {
            peer_id: "old_peer".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Old".to_string(),
            wallet_address: "wallet1".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-50),
            verified: false,
            discovered_at: Utc::now() - Duration::seconds(60),
            last_seen: Utc::now() - Duration::seconds(60),
        };
        
        // Add a peer with recent last_seen timestamp
        let recent_peer = DiscoveredPeer {
            peer_id: "recent_peer".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Recent".to_string(),
            wallet_address: "wallet2".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-40),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.add_or_update_peer(old_peer).await.unwrap();
        service.add_or_update_peer(recent_peer).await.unwrap();
        
        assert_eq!(service.get_discovered_peers().await.unwrap().len(), 2);
        
        // Refresh should remove the old peer
        service.refresh_peer_list().await.unwrap();
        
        let peers = service.get_discovered_peers().await.unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].peer_id, "recent_peer");
    }

    #[tokio::test]
    async fn test_stop_discovery_clears_peers() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        let peer = DiscoveredPeer {
            peer_id: "peer1".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Alice".to_string(),
            wallet_address: "wallet123".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-50),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.start_discovery(DiscoveryMethod::WiFi).await.unwrap();
        service.add_or_update_peer(peer).await.unwrap();
        assert_eq!(service.get_discovered_peers().await.unwrap().len(), 1);
        
        service.stop_discovery().await.unwrap();
        assert_eq!(service.get_discovered_peers().await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_refresh_task_runs_periodically() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        // Start discovery to trigger refresh task
        service.start_discovery(DiscoveryMethod::WiFi).await.unwrap();
        
        // Add a peer with old timestamp
        let old_peer = DiscoveredPeer {
            peer_id: "old_peer".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Old".to_string(),
            wallet_address: "wallet1".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-50),
            verified: false,
            discovered_at: Utc::now() - Duration::seconds(60),
            last_seen: Utc::now() - Duration::seconds(60),
        };
        
        service.add_or_update_peer(old_peer).await.unwrap();
        assert!(service
            .get_discovered_peers()
            .await
            .unwrap()
            .iter()
            .any(|p| p.peer_id == "old_peer"));
        
        // Wait for refresh task to run (5 second interval + buffer)
        sleep(TokioDuration::from_secs(6)).await;
        
        // The stale `old_peer` (not present on the live network) must be removed
        // by the background task. We assert specifically on that peer rather than
        // on an empty list, because the refresh task also re-syncs any *real*
        // mDNS peers present on the host's network into the list, which would
        // otherwise make this assertion environment-dependent and flaky.
        let peers = service.get_discovered_peers().await.unwrap();
        assert!(
            !peers.iter().any(|p| p.peer_id == "old_peer"),
            "stale peer should have been removed by the refresh task"
        );
        
        service.stop_discovery().await.unwrap();
    }

    #[tokio::test]
    async fn test_peer_capacity_limit() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        assert_eq!(service.get_max_peers(), 50);
        
        // Add 50 peers (at capacity)
        for i in 0..50 {
            let peer = DiscoveredPeer {
                peer_id: format!("peer{}", i),
                user_id: uuid::Uuid::nil(),
                user_tag: format!("User{}", i),
                wallet_address: format!("wallet{}", i),
                discovery_method: DiscoveryMethod::WiFi,
                signal_strength: Some(-50 - i as i8),
                verified: false,
                discovered_at: Utc::now(),
                last_seen: Utc::now(),
            };
            service.add_or_update_peer(peer).await.unwrap();
        }
        
        assert_eq!(service.get_discovered_peers().await.unwrap().len(), 50);
        
        // Try to add a peer with stronger signal than the weakest
        let strong_peer = DiscoveredPeer {
            peer_id: "strong_peer".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Strong".to_string(),
            wallet_address: "wallet_strong".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-40),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.add_or_update_peer(strong_peer).await.unwrap();
        
        // Should still have 50 peers
        let peers = service.get_discovered_peers().await.unwrap();
        assert_eq!(peers.len(), 50);
        
        // Strong peer should be in the list
        assert!(peers.iter().any(|p| p.peer_id == "strong_peer"));
        
        // Weakest peer (peer49 with signal -99) should be removed
        assert!(!peers.iter().any(|p| p.peer_id == "peer49"));
    }

    #[tokio::test]
    async fn test_peer_capacity_limit_rejects_weak_peer() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        // Add 50 peers with strong signals
        for i in 0..50 {
            let peer = DiscoveredPeer {
                peer_id: format!("peer{}", i),
                user_id: uuid::Uuid::nil(),
                user_tag: format!("User{}", i),
                wallet_address: format!("wallet{}", i),
                discovery_method: DiscoveryMethod::WiFi,
                signal_strength: Some(-30 - i as i8),
                verified: false,
                discovered_at: Utc::now(),
                last_seen: Utc::now(),
            };
            service.add_or_update_peer(peer).await.unwrap();
        }
        
        // Try to add a peer with weaker signal than all existing peers
        let weak_peer = DiscoveredPeer {
            peer_id: "weak_peer".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "Weak".to_string(),
            wallet_address: "wallet_weak".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-100),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.add_or_update_peer(weak_peer).await.unwrap();
        
        // Should still have 50 peers
        let peers = service.get_discovered_peers().await.unwrap();
        assert_eq!(peers.len(), 50);
        
        // Weak peer should NOT be in the list
        assert!(!peers.iter().any(|p| p.peer_id == "weak_peer"));
    }

    #[tokio::test]
    async fn test_peer_capacity_limit_with_no_signal_strength() {
        let service = DiscoveryService::new(
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );
        
        // Add 50 peers with no signal strength
        for i in 0..50 {
            let peer = DiscoveredPeer {
                peer_id: format!("peer{}", i),
                user_id: uuid::Uuid::nil(),
                user_tag: format!("User{}", i),
                wallet_address: format!("wallet{}", i),
                discovery_method: DiscoveryMethod::WiFi,
                signal_strength: None,
                verified: false,
                discovered_at: Utc::now(),
                last_seen: Utc::now(),
            };
            service.add_or_update_peer(peer).await.unwrap();
        }
        
        // Try to add a peer with signal strength
        let peer_with_signal = DiscoveredPeer {
            peer_id: "peer_with_signal".to_string(),
            user_id: uuid::Uuid::nil(),
            user_tag: "WithSignal".to_string(),
            wallet_address: "wallet_signal".to_string(),
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: Some(-50),
            verified: false,
            discovered_at: Utc::now(),
            last_seen: Utc::now(),
        };
        
        service.add_or_update_peer(peer_with_signal).await.unwrap();
        
        // Should still have 50 peers
        let peers = service.get_discovered_peers().await.unwrap();
        assert_eq!(peers.len(), 50);
        
        // Peer with signal should be in the list (stronger than None)
        assert!(peers.iter().any(|p| p.peer_id == "peer_with_signal"));
    }
}
