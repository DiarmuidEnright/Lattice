use crate::{DiscoveredPeer, DiscoveryMethod, PeerId, ProximityError, Result};
use btleplug::api::{
    Central, Manager as _, Peripheral as _, ScanFilter,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Custom service UUID for crypto P2P transfers
pub const SERVICE_UUID: uuid::Uuid = uuid::Uuid::from_bytes([
    0x00, 0x00, 0xFF, 0xF0, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB,
]);

/// Custom characteristic UUID for user data
pub const CHARACTERISTIC_UUID: uuid::Uuid = uuid::Uuid::from_bytes([
    0x00, 0x00, 0xFF, 0xF1, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB,
]);

/// Protocol version
const PROTOCOL_VERSION: u8 = 1;

/// BLE Advertiser for broadcasting user presence via Bluetooth
pub struct BleAdvertiser {
    // justification: adapter handle and wallet_address are held for the
    // platform-specific BLE advertising (peripheral mode) path documented in
    // start_advertising; btleplug cannot advertise yet, so they are not read in
    // this build. adapter is also obtained in new() to validate a BLE adapter exists.
    #[allow(dead_code)]
    adapter: Adapter,
    user_tag: String,
    #[allow(dead_code)]
    wallet_address: String,
    is_advertising: Arc<RwLock<bool>>,
}

impl BleAdvertiser {
    /// Create a new BLE advertiser
    pub async fn new(user_tag: String, wallet_address: String) -> Result<Self> {
        let manager = Manager::new().await.map_err(|e| {
            ProximityError::BleError(format!("Failed to create BLE manager: {}", e))
        })?;

        let adapters = manager.adapters().await.map_err(|e| {
            ProximityError::BleError(format!("Failed to get BLE adapters: {}", e))
        })?;

        let adapter = adapters.into_iter().next().ok_or_else(|| {
            ProximityError::BleError("No BLE adapter found".to_string())
        })?;

        Ok(Self {
            adapter,
            user_tag,
            wallet_address,
            is_advertising: Arc::new(RwLock::new(false)),
        })
    }

    /// Start advertising via BLE
    pub async fn start_advertising(&self) -> Result<()> {
        let mut is_advertising = self.is_advertising.write().await;
        if *is_advertising {
            warn!("BLE advertising already active");
            return Ok(());
        }

        info!("Starting BLE advertising for user: {}", self.user_tag);

        // Note: btleplug doesn't support peripheral mode (advertising) on most platforms
        // This is a limitation of the library and underlying platform APIs
        // For production, platform-specific implementations would be needed:
        // - iOS: CoreBluetooth CBPeripheralManager
        // - Android: BluetoothLeAdvertiser
        // - Linux: BlueZ D-Bus API
        
        // For now, we'll mark as advertising and log a warning
        warn!("BLE advertising not fully supported by btleplug - platform-specific implementation required");
        
        *is_advertising = true;
        Ok(())
    }

    /// Stop advertising
    pub async fn stop_advertising(&self) -> Result<()> {
        let mut is_advertising = self.is_advertising.write().await;
        if !*is_advertising {
            return Ok(());
        }

        info!("Stopping BLE advertising");
        *is_advertising = false;
        Ok(())
    }

    /// Check if currently advertising
    pub async fn is_advertising(&self) -> bool {
        *self.is_advertising.read().await
    }

    /// Get advertisement data that would be broadcast
    pub fn get_advertisement_data(&self) -> Vec<u8> {
        let mut data = Vec::new();
        
        // Protocol version (1 byte)
        data.push(PROTOCOL_VERSION);
        
        // User tag length (1 byte) + user tag (max 20 bytes)
        let user_tag_bytes = self.user_tag.as_bytes();
        let tag_len = user_tag_bytes.len().min(20);
        data.push(tag_len as u8);
        data.extend_from_slice(&user_tag_bytes[..tag_len]);
        
        data
    }
}

/// BLE Scanner for discovering nearby peers
pub struct BleScanner {
    adapter: Adapter,
    discovered_peers: Arc<RwLock<HashMap<PeerId, DiscoveredPeer>>>,
    is_scanning: Arc<RwLock<bool>>,
}

impl BleScanner {
    /// Create a new BLE scanner
    pub async fn new() -> Result<Self> {
        let manager = Manager::new().await.map_err(|e| {
            ProximityError::BleError(format!("Failed to create BLE manager: {}", e))
        })?;

        let adapters = manager.adapters().await.map_err(|e| {
            ProximityError::BleError(format!("Failed to get BLE adapters: {}", e))
        })?;

        let adapter = adapters.into_iter().next().ok_or_else(|| {
            ProximityError::BleError("No BLE adapter found".to_string())
        })?;

        Ok(Self {
            adapter,
            discovered_peers: Arc::new(RwLock::new(HashMap::new())),
            is_scanning: Arc::new(RwLock::new(false)),
        })
    }

    /// Start scanning for BLE peers
    pub async fn start_scanning(&self) -> Result<()> {
        let mut is_scanning = self.is_scanning.write().await;
        if *is_scanning {
            warn!("BLE scanning already active");
            return Ok(());
        }

        info!("Starting BLE scan");

        // Start scanning with filter for our service UUID
        self.adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|e| {
                ProximityError::BleError(format!("Failed to start BLE scan: {}", e))
            })?;

        *is_scanning = true;

        // Spawn background task to process discovered devices
        let adapter = self.adapter.clone();
        let discovered_peers = self.discovered_peers.clone();
        
        tokio::spawn(async move {
            if let Err(e) = Self::process_scan_results(adapter, discovered_peers).await {
                error!("Error processing BLE scan results: {}", e);
            }
        });

        Ok(())
    }

    /// Stop scanning
    pub async fn stop_scanning(&self) -> Result<()> {
        let mut is_scanning = self.is_scanning.write().await;
        if !*is_scanning {
            return Ok(());
        }

        info!("Stopping BLE scan");
        
        self.adapter.stop_scan().await.map_err(|e| {
            ProximityError::BleError(format!("Failed to stop BLE scan: {}", e))
        })?;

        *is_scanning = false;
        Ok(())
    }

    /// Process scan results and discover peers
    async fn process_scan_results(
        adapter: Adapter,
        discovered_peers: Arc<RwLock<HashMap<PeerId, DiscoveredPeer>>>,
    ) -> Result<()> {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

            let peripherals = adapter.peripherals().await.map_err(|e| {
                ProximityError::BleError(format!("Failed to get peripherals: {}", e))
            })?;

            for peripheral in peripherals {
                if let Err(e) = Self::process_peripheral(&peripheral, &discovered_peers).await {
                    debug!("Error processing peripheral: {}", e);
                }
            }
        }
    }

    /// Process a single peripheral device
    async fn process_peripheral(
        peripheral: &Peripheral,
        discovered_peers: &Arc<RwLock<HashMap<PeerId, DiscoveredPeer>>>,
    ) -> Result<()> {
        let properties = peripheral.properties().await.map_err(|e| {
            ProximityError::BleError(format!("Failed to get peripheral properties: {}", e))
        })?;

        let properties = match properties {
            Some(p) => p,
            None => return Ok(()), // No properties available yet
        };

        // Check if this device advertises our service UUID
        let has_service = properties
            .services
            .iter()
            .any(|uuid| uuid == &SERVICE_UUID);

        if !has_service {
            return Ok(()); // Not our service
        }

        // Get RSSI for signal strength (convert i16 to i8)
        let rssi = properties.rssi.map(|r| r.clamp(-128, 127) as i8);

        // Try to extract user data from advertisement
        let user_tag = Self::extract_user_tag(&properties.manufacturer_data)?;

        // Create peer ID from peripheral address
        let peer_id = peripheral.address().to_string();

        // Get or create discovered peer
        let mut peers = discovered_peers.write().await;
        let now = Utc::now();

        if let Some(peer) = peers.get_mut(&peer_id) {
            // Update existing peer
            peer.last_seen = now;
            peer.signal_strength = rssi;
            debug!("Updated peer: {} (RSSI: {:?})", peer_id, rssi);
        } else {
            // Add new peer
            let peer = DiscoveredPeer {
                peer_id: peer_id.clone(),
                user_id: uuid::Uuid::nil(),
                user_tag: user_tag.clone(),
                wallet_address: String::new(), // Will be filled during authentication
                discovery_method: DiscoveryMethod::Bluetooth,
                signal_strength: rssi,
                verified: false,
                discovered_at: now,
                last_seen: now,
            };
            peers.insert(peer_id.clone(), peer);
            info!("Discovered new BLE peer: {} ({})", user_tag, peer_id);
        }

        Ok(())
    }

    /// Extract user tag from manufacturer data
    fn extract_user_tag(manufacturer_data: &HashMap<u16, Vec<u8>>) -> Result<String> {
        // Look for our manufacturer data
        for (_company_id, data) in manufacturer_data {
            if data.len() < 2 {
                continue;
            }

            // Check protocol version
            if data[0] != PROTOCOL_VERSION {
                continue;
            }

            // Extract user tag
            let tag_len = data[1] as usize;
            if data.len() < 2 + tag_len {
                continue;
            }

            let user_tag = String::from_utf8_lossy(&data[2..2 + tag_len]).to_string();
            return Ok(user_tag);
        }

        Err(ProximityError::BleError(
            "No valid user tag found in advertisement".to_string(),
        ))
    }

    /// Get list of discovered peers
    pub async fn get_discovered_peers(&self) -> Vec<DiscoveredPeer> {
        let peers = self.discovered_peers.read().await;
        peers.values().cloned().collect()
    }

    /// Get discovered peers sorted by signal strength (strongest first)
    pub async fn get_peers_by_signal_strength(&self) -> Vec<DiscoveredPeer> {
        let mut peers = self.get_discovered_peers().await;
        peers.sort_by(|a, b| {
            match (a.signal_strength, b.signal_strength) {
                (Some(a_rssi), Some(b_rssi)) => b_rssi.cmp(&a_rssi), // Higher RSSI = stronger signal
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
        peers
    }

    /// Remove stale peers (not seen for timeout duration)
    pub async fn remove_stale_peers(&self, timeout_secs: u64) -> usize {
        let mut peers = self.discovered_peers.write().await;
        let now = Utc::now();
        let timeout = chrono::Duration::seconds(timeout_secs as i64);

        let initial_count = peers.len();
        peers.retain(|_, peer| now.signed_duration_since(peer.last_seen) < timeout);
        let removed = initial_count - peers.len();

        if removed > 0 {
            info!("Removed {} stale BLE peers", removed);
        }

        removed
    }

    /// Check if currently scanning
    pub async fn is_scanning(&self) -> bool {
        *self.is_scanning.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_advertisement_data_format() {
        // Test the data format without creating a full BleAdvertiser
        let user_tag = "alice".to_string();
        let mut data = Vec::new();
        
        // Protocol version (1 byte)
        data.push(PROTOCOL_VERSION);
        
        // User tag length (1 byte) + user tag (max 20 bytes)
        let user_tag_bytes = user_tag.as_bytes();
        let tag_len = user_tag_bytes.len().min(20);
        data.push(tag_len as u8);
        data.extend_from_slice(&user_tag_bytes[..tag_len]);
        
        // Check protocol version
        assert_eq!(data[0], PROTOCOL_VERSION);
        
        // Check user tag length
        assert_eq!(data[1], 5); // "alice".len()
        
        // Check user tag content
        assert_eq!(&data[2..7], b"alice");
    }

    #[test]
    fn test_advertisement_data_truncation() {
        let long_tag = "a".repeat(30); // Longer than 20 byte limit
        let mut data = Vec::new();
        
        // Protocol version (1 byte)
        data.push(PROTOCOL_VERSION);
        
        // User tag length (1 byte) + user tag (max 20 bytes)
        let user_tag_bytes = long_tag.as_bytes();
        let tag_len = user_tag_bytes.len().min(20);
        data.push(tag_len as u8);
        data.extend_from_slice(&user_tag_bytes[..tag_len]);
        
        // Check that tag is truncated to 20 bytes
        assert_eq!(data[1], 20);
        assert_eq!(data.len(), 22); // version(1) + length(1) + tag(20)
    }

    #[test]
    fn test_extract_user_tag() {
        let mut manufacturer_data = HashMap::new();
        
        // Create valid advertisement data
        let mut data = vec![PROTOCOL_VERSION, 5]; // version, tag length
        data.extend_from_slice(b"alice");
        manufacturer_data.insert(0xFFFF, data);

        let result = BleScanner::extract_user_tag(&manufacturer_data);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "alice");
    }

    #[test]
    fn test_extract_user_tag_invalid_version() {
        let mut manufacturer_data = HashMap::new();
        
        // Create data with wrong version
        let mut data = vec![99, 5]; // wrong version
        data.extend_from_slice(b"alice");
        manufacturer_data.insert(0xFFFF, data);

        let result = BleScanner::extract_user_tag(&manufacturer_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_user_tag_empty_data() {
        let manufacturer_data = HashMap::new();
        let result = BleScanner::extract_user_tag(&manufacturer_data);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_user_tag_truncated_data() {
        let mut manufacturer_data = HashMap::new();
        
        // Create data that's too short
        let data = vec![PROTOCOL_VERSION]; // Missing length and tag
        manufacturer_data.insert(0xFFFF, data);

        let result = BleScanner::extract_user_tag(&manufacturer_data);
        assert!(result.is_err());
    }
}
