// mDNS-based WiFi discovery implementation

use crate::{DiscoveredPeer, DiscoveryMethod, PeerId, ProximityError, Result};
use chrono::Utc;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

const SERVICE_TYPE: &str = "_crypto-p2p._tcp.local.";
const PROTOCOL_VERSION: &str = "1.0";
// Service port advertised in the SRV record. It must be non-zero for the
// announcement to be a resolvable, browsable mDNS service. This is the port a
// peer would use to open a direct connection after discovery.
const SERVICE_PORT: u16 = 3000;

/// mDNS Announcer for broadcasting user presence on WiFi
pub struct MdnsAnnouncer {
    daemon: Arc<ServiceDaemon>,
    service_info: Arc<RwLock<Option<ServiceInfo>>>,
    user_id: String,
    user_tag: String,
    device_id: String,
    wallet_address: String,
}

impl MdnsAnnouncer {
    /// Create a new MdnsAnnouncer
    pub fn new(
        user_id: String,
        user_tag: String,
        device_id: String,
        wallet_address: String,
    ) -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| ProximityError::NetworkError(format!("Failed to create mDNS daemon: {}", e)))?;

        Ok(Self {
            daemon: Arc::new(daemon),
            service_info: Arc::new(RwLock::new(None)),
            user_id,
            user_tag,
            device_id,
            wallet_address,
        })
    }

    /// Start broadcasting presence via mDNS
    pub async fn start(&self) -> Result<()> {
        let mut service_guard = self.service_info.write().await;

        if service_guard.is_some() {
            warn!("mDNS announcement already active");
            return Ok(());
        }

        info!("Starting mDNS announcement for user: {}", self.user_tag);

        // Create service instance name (unique per device)
        let instance_name = format!("crypto-p2p-{}", self.device_id);

        // Build TXT records with user information
        let mut properties = HashMap::new();
        properties.insert("user_id".to_string(), self.user_id.clone());
        properties.insert("user_tag".to_string(), self.user_tag.clone());
        properties.insert("wallet".to_string(), self.wallet_address.clone());
        properties.insert("device_id".to_string(), self.device_id.clone());
        properties.insert("version".to_string(), PROTOCOL_VERSION.to_string());


        // Create service info.
        // ServiceInfo::new(type, instance, hostname, ip, port, properties).
        //
        // The previous version passed an empty hostname/IP and port 0, which
        // mdns-sd accepts internally but cannot publish a resolvable SRV/A
        // record for — so the service registered locally but was invisible on
        // the wire (not browsable by other devices). We provide a valid
        // `<instance>.local.` hostname and a non-zero service port, then call
        // `enable_addr_auto()` so the daemon auto-detects and advertises this
        // host's LAN IP addresses, making the announcement actually
        // discoverable by peers.
        let hostname = format!("{}.local.", instance_name);
        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &hostname,
            "",
            SERVICE_PORT,
            Some(properties),
        )
        .map_err(|e| ProximityError::NetworkError(format!("Failed to create service info: {}", e)))?
        .enable_addr_auto();

        // Register the service
        self.daemon
            .register(service_info.clone())
            .map_err(|e| ProximityError::NetworkError(format!("Failed to register mDNS service: {}", e)))?;

        *service_guard = Some(service_info);

        info!("mDNS announcement started successfully");
        Ok(())
    }

    /// Stop broadcasting presence
    pub async fn stop(&self) -> Result<()> {
        let mut service_guard = self.service_info.write().await;

        if let Some(service_info) = service_guard.take() {
            info!("Stopping mDNS announcement");

            // Unregister the service
            self.daemon
                .unregister(service_info.get_fullname())
                .map_err(|e| ProximityError::NetworkError(format!("Failed to unregister mDNS service: {}", e)))?;

            info!("mDNS announcement stopped");
        } else {
            debug!("No active mDNS announcement to stop");
        }

        Ok(())
    }

    /// Check if announcement is active
    pub async fn is_active(&self) -> bool {
        self.service_info.read().await.is_some()
    }
}

/// mDNS Listener for discovering peers on WiFi
pub struct MdnsListener {
    daemon: Arc<ServiceDaemon>,
    discovered_peers: Arc<RwLock<HashMap<PeerId, DiscoveredPeer>>>,
    running: Arc<RwLock<bool>>,
}

impl MdnsListener {
    /// Create a new MdnsListener
    pub fn new() -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| ProximityError::NetworkError(format!("Failed to create mDNS daemon: {}", e)))?;

        Ok(Self {
            daemon: Arc::new(daemon),
            discovered_peers: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(false)),
        })
    }

    /// Start listening for mDNS announcements
    pub async fn start<F>(&self, on_peer_discovered: F) -> Result<()>
    where
        F: Fn(DiscoveredPeer) + Send + Sync + 'static,
    {
        let mut running = self.running.write().await;

        if *running {
            warn!("mDNS listener already running");
            return Ok(());
        }

        info!("Starting mDNS listener for service type: {}", SERVICE_TYPE);

        // Browse for services
        let receiver = self.daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| ProximityError::NetworkError(format!("Failed to browse mDNS services: {}", e)))?;

        *running = true;
        drop(running);

        let peers = Arc::clone(&self.discovered_peers);
        let running_flag = Arc::clone(&self.running);
        let on_peer_discovered = Arc::new(on_peer_discovered);

        // Spawn background task to process discovered services
        tokio::spawn(async move {
            while let Ok(event) = receiver.recv_async().await {
                // Check if still running
                if !*running_flag.read().await {
                    debug!("mDNS listener stopped, exiting event loop");
                    break;
                }

                match event {
                    mdns_sd::ServiceEvent::ServiceResolved(info) => {
                        debug!("mDNS service resolved: {}", info.get_fullname());

                        // Extract peer information from TXT records
                        if let Some(peer) = Self::parse_service_info(&info) {
                            let peer_id = peer.peer_id.clone();

                            // Add to discovered peers
                            let mut peer_map = peers.write().await;
                            peer_map.insert(peer_id.clone(), peer.clone());
                            drop(peer_map);

                            // Notify callback
                            on_peer_discovered(peer);
                        }
                    }
                    mdns_sd::ServiceEvent::ServiceRemoved(_, fullname) => {
                        debug!("mDNS service removed: {}", fullname);

                        // Extract device_id from fullname to remove peer
                        if let Some(device_id) = Self::extract_device_id(&fullname) {
                            let mut peer_map = peers.write().await;
                            peer_map.remove(&device_id);
                            info!("Removed peer: {}", device_id);
                        }
                    }
                    mdns_sd::ServiceEvent::SearchStarted(_) => {
                        debug!("mDNS search started");
                    }
                    mdns_sd::ServiceEvent::SearchStopped(_) => {
                        debug!("mDNS search stopped");
                    }
                    _ => {
                        debug!("Unhandled mDNS event: {:?}", event);
                    }
                }
            }

            info!("mDNS listener event loop terminated");
        });

        info!("mDNS listener started successfully");
        Ok(())
    }

    /// Stop listening for mDNS announcements
    pub async fn stop(&self) -> Result<()> {
        let mut running = self.running.write().await;

        if !*running {
            debug!("mDNS listener not running");
            return Ok(());
        }

        info!("Stopping mDNS listener");
        *running = false;

        // Clear discovered peers
        let mut peers = self.discovered_peers.write().await;
        peers.clear();

        info!("mDNS listener stopped");
        Ok(())
    }

    /// Check if listener is running
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }

    /// Get currently discovered peers
    pub async fn get_discovered_peers(&self) -> Vec<DiscoveredPeer> {
        let peers = self.discovered_peers.read().await;
        peers.values().cloned().collect()
    }

    /// Parse ServiceInfo into DiscoveredPeer
    fn parse_service_info(info: &ServiceInfo) -> Option<DiscoveredPeer> {
        let properties = info.get_properties();

        // Extract required fields from TXT records.
        //
        // NOTE: `TxtProperty::to_string()` renders the full "key=value" pair, so
        // using it here previously produced values like "user_tag=system" and a
        // version of "version=1.0", which made the version check below always
        // fail ("version=1.0" != "1.0") and silently rejected every peer. Use
        // `val_str()` to read just the value.
        let user_tag = properties.get("user_tag")?.val_str().to_string();
        let wallet_address = properties.get("wallet")?.val_str().to_string();
        let device_id = properties.get("device_id")?.val_str().to_string();
        let version = properties.get("version").map(|v| v.val_str().to_string());

        // Validate protocol version
        if let Some(v) = version {
            if v != PROTOCOL_VERSION {
                warn!("Incompatible protocol version: {} (expected {})", v, PROTOCOL_VERSION);
                return None;
            }
        }

        // The peer's real user account id. Older/foreign announcements may not
        // carry it; in that case fall back to a nil UUID so discovery/display
        // still works (a transfer to a nil recipient will simply be rejected
        // downstream rather than panicking here).
        let user_id = properties
            .get("user_id")
            .and_then(|v| uuid::Uuid::parse_str(v.val_str()).ok())
            .unwrap_or_else(uuid::Uuid::nil);

        let now = Utc::now();

        Some(DiscoveredPeer {
            peer_id: device_id.clone(),
            user_id,
            user_tag,
            wallet_address,
            discovery_method: DiscoveryMethod::WiFi,
            signal_strength: None, // WiFi doesn't provide signal strength via mDNS
            verified: false,
            discovered_at: now,
            last_seen: now,
        })
    }

    /// Extract device_id from mDNS fullname
    fn extract_device_id(fullname: &str) -> Option<String> {
        // Fullname format: "crypto-p2p-{device_id}._crypto-p2p._tcp.local."
        if let Some(instance_name) = fullname.split('.').next() {
            if let Some(device_id) = instance_name.strip_prefix("crypto-p2p-") {
                return Some(device_id.to_string());
            }
        }
        None
    }
}

impl Default for MdnsListener {
    fn default() -> Self {
        Self::new().expect("Failed to create default MdnsListener")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mdns_announcer_creation() {
        let announcer = MdnsAnnouncer::new(
            "00000000-0000-0000-0000-000000000001".to_string(),
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        );

        assert!(announcer.is_ok());
        let announcer = announcer.unwrap();
        assert!(!announcer.is_active().await);
    }

    #[tokio::test]
    async fn test_mdns_announcer_start_stop() {
        let announcer = MdnsAnnouncer::new(
            "00000000-0000-0000-0000-000000000001".to_string(),
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        )
        .unwrap();

        // Start announcement
        let result = announcer.start().await;
        assert!(result.is_ok());
        assert!(announcer.is_active().await);

        // Stop announcement
        let result = announcer.stop().await;
        assert!(result.is_ok());
        assert!(!announcer.is_active().await);
    }

    #[tokio::test]
    async fn test_mdns_announcer_double_start() {
        let announcer = MdnsAnnouncer::new(
            "00000000-0000-0000-0000-000000000001".to_string(),
            "TestUser".to_string(),
            "device123".to_string(),
            "wallet456".to_string(),
        )
        .unwrap();

        // Start announcement
        announcer.start().await.unwrap();
        assert!(announcer.is_active().await);

        // Try to start again (should succeed but not create duplicate)
        let result = announcer.start().await;
        assert!(result.is_ok());
        assert!(announcer.is_active().await);

        announcer.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_mdns_listener_creation() {
        let listener = MdnsListener::new();
        assert!(listener.is_ok());

        let listener = listener.unwrap();
        assert!(!listener.is_running().await);
    }

    #[tokio::test]
    async fn test_mdns_listener_start_stop() {
        let listener = MdnsListener::new().unwrap();

        // Start listener
        let result = listener.start(|_peer| {
            // Callback for discovered peers
        }).await;
        assert!(result.is_ok());
        assert!(listener.is_running().await);

        // Stop listener
        let result = listener.stop().await;
        assert!(result.is_ok());
        assert!(!listener.is_running().await);
    }

    #[tokio::test]
    async fn test_extract_device_id() {
        let fullname = "crypto-p2p-device123._crypto-p2p._tcp.local.";
        let device_id = MdnsListener::extract_device_id(fullname);
        assert_eq!(device_id, Some("device123".to_string()));

        let invalid_fullname = "invalid-service._http._tcp.local.";
        let device_id = MdnsListener::extract_device_id(invalid_fullname);
        assert_eq!(device_id, None);
    }
}
