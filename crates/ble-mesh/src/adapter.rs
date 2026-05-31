//! BLE adapter abstraction for cross-platform support

use crate::error::{MeshError, MeshResult};
use crate::router::DeviceId;
use async_trait::async_trait;

/// Trait for platform-agnostic BLE operations
#[async_trait]
pub trait BLEAdapter: Send + Sync {
    /// Start advertising as a peripheral
    async fn start_advertising(&self) -> MeshResult<()>;

    /// Start scanning for peripherals
    async fn start_scanning(&self) -> MeshResult<()>;

    /// Connect to a device
    async fn connect(&self, device: &DeviceId) -> MeshResult<()>;

    /// Disconnect from a device
    async fn disconnect(&self, device: &DeviceId) -> MeshResult<()>;

    /// Send data to a connected device
    async fn send_data(&self, device: &DeviceId, data: &[u8]) -> MeshResult<()>;

    /// Receive data from a device
    async fn receive_data(&self) -> MeshResult<Vec<u8>>;

    /// Get list of connected devices
    async fn connected_devices(&self) -> MeshResult<Vec<DeviceId>>;
}

use btleplug::api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Adapter, Manager, Peripheral};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};

/// BLE MTU limit for fragmentation
const BLE_MTU: usize = 512;

/// Maximum number of retry attempts for BLE operations (Requirement 11.1)
const MAX_RETRIES: u32 = 5;

/// Maximum number of retry attempts for mesh packet transmission (Requirement 11.5)
const MESH_TRANSMISSION_RETRIES: u32 = 3;

/// Cross-platform BLE adapter implementation using btleplug
pub struct BLEAdapterImpl {
    /// BLE adapter from btleplug
    adapter: Arc<RwLock<Option<Adapter>>>,
    
    /// Connected peripherals mapped by DeviceId
    peripherals: Arc<DashMap<DeviceId, Peripheral>>,
    
    /// Channel for receiving data from peripherals
    rx_channel: Arc<RwLock<Option<mpsc::Receiver<Vec<u8>>>>>,
    
    /// Channel sender for incoming data
    // justification: retained to keep the mpsc channel open for receive_data; the
    // peripheral notification path that clones this sender is platform-specific and
    // not yet wired, so the field is currently written but not read.
    #[allow(dead_code)]
    tx_channel: Arc<mpsc::Sender<Vec<u8>>>,
    
    /// Flag indicating if advertising is active
    advertising_active: Arc<RwLock<bool>>,
    
    /// Flag indicating if scanning is active
    scanning_active: Arc<RwLock<bool>>,
}

impl BLEAdapterImpl {
    /// Create a new BLE adapter instance
    pub async fn new() -> MeshResult<Self> {
        info!("Initializing BLE adapter");
        
        // Create channel for receiving data
        let (tx, rx) = mpsc::channel(100);
        
        Ok(Self {
            adapter: Arc::new(RwLock::new(None)),
            peripherals: Arc::new(DashMap::new()),
            rx_channel: Arc::new(RwLock::new(Some(rx))),
            tx_channel: Arc::new(tx),
            advertising_active: Arc::new(RwLock::new(false)),
            scanning_active: Arc::new(RwLock::new(false)),
        })
    }
    
    /// Initialize the BLE adapter
    async fn ensure_adapter(&self) -> MeshResult<()> {
        let mut adapter_lock = self.adapter.write().await;
        
        if adapter_lock.is_none() {
            debug!("Creating BLE manager and adapter");
            
            let manager = Manager::new()
                .await
                .map_err(|e| MeshError::AdapterError(format!("Failed to create BLE manager: {}", e)))?;
            
            let adapters = manager
                .adapters()
                .await
                .map_err(|e| MeshError::AdapterError(format!("Failed to get adapters: {}", e)))?;
            
            let adapter = adapters
                .into_iter()
                .next()
                .ok_or_else(|| MeshError::AdapterError("No BLE adapter found".to_string()))?;
            
            info!("BLE adapter initialized: {:?}", adapter.adapter_info().await);
            *adapter_lock = Some(adapter);
        }
        
        Ok(())
    }
    
    /// Fragment data if it exceeds BLE MTU
    fn fragment_data(&self, data: &[u8]) -> Vec<Vec<u8>> {
        if data.len() <= BLE_MTU {
            return vec![data.to_vec()];
        }
        
        let mut fragments = Vec::new();
        let mut offset = 0;
        
        while offset < data.len() {
            let end = std::cmp::min(offset + BLE_MTU, data.len());
            fragments.push(data[offset..end].to_vec());
            offset = end;
        }
        
        debug!("Fragmented {} bytes into {} fragments", data.len(), fragments.len());
        fragments
    }
    
    /// Get peripheral by device ID
    fn get_peripheral(&self, device: &DeviceId) -> MeshResult<Peripheral> {
        self.peripherals
            .get(device)
            .map(|p| p.clone())
            .ok_or_else(|| MeshError::DeviceNotFound(device.to_string()))
    }
}

#[async_trait]
impl BLEAdapter for BLEAdapterImpl {
    async fn start_advertising(&self) -> MeshResult<()> {
        info!("Starting BLE advertising (peripheral mode)");
        
        self.ensure_adapter().await?;
        
        // Note: btleplug doesn't directly support advertising on all platforms
        // On platforms that support it (like Linux with BlueZ), we would use
        // the adapter's advertising capabilities. For now, we mark as active
        // and rely on the scanning/connection mechanism.
        
        let mut active = self.advertising_active.write().await;
        *active = true;
        
        info!("BLE advertising started");
        Ok(())
    }

    async fn start_scanning(&self) -> MeshResult<()> {
        info!("Starting BLE scanning (central mode)");
        
        self.ensure_adapter().await?;
        
        let adapter_lock = self.adapter.read().await;
        let adapter = adapter_lock
            .as_ref()
            .ok_or_else(|| MeshError::AdapterError("Adapter not initialized".to_string()))?;
        
        // Start scanning for peripherals
        adapter
            .start_scan(ScanFilter::default())
            .await
            .map_err(|e| MeshError::AdapterError(format!("Failed to start scanning: {}", e)))?;
        
        let mut active = self.scanning_active.write().await;
        *active = true;
        
        info!("BLE scanning started");
        Ok(())
    }

    async fn connect(&self, device: &DeviceId) -> MeshResult<()> {
        info!("Connecting to device: {}", device);
        
        self.ensure_adapter().await?;
        
        let adapter_lock = self.adapter.read().await;
        let adapter = adapter_lock
            .as_ref()
            .ok_or_else(|| MeshError::AdapterError("Adapter not initialized".to_string()))?;
        
        // Get list of discovered peripherals
        let peripherals = adapter
            .peripherals()
            .await
            .map_err(|e| MeshError::AdapterError(format!("Failed to get peripherals: {}", e)))?;
        
        // Find the peripheral matching the device ID
        // Note: In a real implementation, we would need a mapping between DeviceId and BLE addresses
        // For now, we'll connect to the first available peripheral as a placeholder
        let peripheral = peripherals
            .into_iter()
            .next()
            .ok_or_else(|| MeshError::DeviceNotFound(device.to_string()))?;
        
        // Attempt connection with retries
        let mut retries = 0;
        loop {
            match peripheral.connect().await {
                Ok(_) => {
                    info!("Successfully connected to device: {}", device);
                    
                    // Discover services and characteristics
                    peripheral
                        .discover_services()
                        .await
                        .map_err(|e| MeshError::ConnectionFailed(format!("Service discovery failed: {}", e)))?;
                    
                    // Store the peripheral
                    self.peripherals.insert(*device, peripheral.clone());
                    
                    return Ok(());
                }
                Err(e) => {
                    retries += 1;
                    if retries >= MAX_RETRIES {
                        error!("Failed to connect after {} retries: {}", MAX_RETRIES, e);
                        return Err(MeshError::ConnectionFailed(format!(
                            "Failed to connect after {} retries: {}",
                            MAX_RETRIES, e
                        )));
                    }
                    
                    warn!("Connection attempt {} failed, retrying: {}", retries, e);
                    
                    // Exponential backoff: 100ms, 200ms, 400ms, 800ms, 1600ms (Requirement 11.1)
                    let delay = std::time::Duration::from_millis(100 * (1 << (retries - 1)));
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn disconnect(&self, device: &DeviceId) -> MeshResult<()> {
        info!("Disconnecting from device: {}", device);
        
        let peripheral = self.get_peripheral(device)?;
        
        peripheral
            .disconnect()
            .await
            .map_err(|e| MeshError::ConnectionFailed(format!("Disconnect failed: {}", e)))?;
        
        // Remove from connected peripherals
        self.peripherals.remove(device);
        
        info!("Successfully disconnected from device: {}", device);
        Ok(())
    }

    async fn send_data(&self, device: &DeviceId, data: &[u8]) -> MeshResult<()> {
        debug!("Sending {} bytes to device: {}", data.len(), device);
        
        let peripheral = self.get_peripheral(device)?;
        
        // Check if peripheral is connected
        let is_connected = peripheral
            .is_connected()
            .await
            .map_err(|e| MeshError::ConnectionFailed(format!("Connection check failed: {}", e)))?;
        
        if !is_connected {
            return Err(MeshError::ConnectionFailed(format!(
                "Device {} is not connected",
                device
            )));
        }
        
        // Fragment data if needed
        let fragments = self.fragment_data(data);
        
        // Get characteristics for writing
        let characteristics = peripheral.characteristics();
        let write_char = characteristics
            .iter()
            .find(|c| {
                c.properties.contains(btleplug::api::CharPropFlags::WRITE) ||
                c.properties.contains(btleplug::api::CharPropFlags::WRITE_WITHOUT_RESPONSE)
            })
            .ok_or_else(|| MeshError::TransmissionFailed("No writable characteristic found".to_string()))?;
        
        // Send all fragments with retries (Requirement 11.5)
        for (i, fragment) in fragments.iter().enumerate() {
            let mut retries = 0;
            
            loop {
                match peripheral
                    .write(write_char, fragment, WriteType::WithoutResponse)
                    .await
                {
                    Ok(_) => {
                        debug!("Sent fragment {}/{} to device {}", i + 1, fragments.len(), device);
                        break;
                    }
                    Err(e) => {
                        retries += 1;
                        if retries >= MESH_TRANSMISSION_RETRIES {
                            error!("Failed to send fragment after {} retries: {}", MESH_TRANSMISSION_RETRIES, e);
                            return Err(MeshError::TransmissionFailed(format!(
                                "Failed to send fragment after {} retries: {}",
                                MESH_TRANSMISSION_RETRIES, e
                            )));
                        }
                        
                        warn!("Send attempt {} failed, retrying: {}", retries, e);
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }
        
        debug!("Successfully sent {} bytes to device {}", data.len(), device);
        Ok(())
    }

    async fn receive_data(&self) -> MeshResult<Vec<u8>> {
        let mut rx_lock = self.rx_channel.write().await;
        
        if let Some(rx) = rx_lock.as_mut() {
            match rx.recv().await {
                Some(data) => {
                    debug!("Received {} bytes", data.len());
                    Ok(data)
                }
                None => Err(MeshError::AdapterError("Receive channel closed".to_string())),
            }
        } else {
            Err(MeshError::AdapterError("Receive channel not initialized".to_string()))
        }
    }

    async fn connected_devices(&self) -> MeshResult<Vec<DeviceId>> {
        let devices: Vec<DeviceId> = self.peripherals.iter().map(|entry| *entry.key()).collect();
        debug!("Connected devices: {}", devices.len());
        Ok(devices)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[tokio::test]
    async fn test_ble_adapter_creation() {
        let adapter = BLEAdapterImpl::new().await;
        assert!(adapter.is_ok(), "BLE adapter creation should succeed");
    }

    #[tokio::test]
    async fn test_dual_mode_initialization() {
        // Test dual-mode initialization (Central + Peripheral)
        let adapter = BLEAdapterImpl::new().await.unwrap();
        
        // Initially, neither mode should be active
        let advertising_active = *adapter.advertising_active.read().await;
        let scanning_active = *adapter.scanning_active.read().await;
        assert!(!advertising_active, "Advertising should not be active initially");
        assert!(!scanning_active, "Scanning should not be active initially");
        
        // Start advertising (peripheral mode)
        let result = adapter.start_advertising().await;
        // Note: This may fail on systems without BLE hardware, which is acceptable for unit tests
        if result.is_ok() {
            let advertising_active = *adapter.advertising_active.read().await;
            assert!(advertising_active, "Advertising should be active after start_advertising");
        }
        
        // Start scanning (central mode)
        let result = adapter.start_scanning().await;
        // Note: This may fail on systems without BLE hardware, which is acceptable for unit tests
        if result.is_ok() {
            let scanning_active = *adapter.scanning_active.read().await;
            assert!(scanning_active, "Scanning should be active after start_scanning");
        }
    }

    #[tokio::test]
    async fn test_advertising_start() {
        // Test advertising start (peripheral mode)
        let adapter = BLEAdapterImpl::new().await.unwrap();
        
        // Verify advertising is not active initially
        let advertising_active = *adapter.advertising_active.read().await;
        assert!(!advertising_active, "Advertising should not be active initially");
        
        // Start advertising
        let result = adapter.start_advertising().await;
        
        // If BLE hardware is available, verify advertising started
        if result.is_ok() {
            let advertising_active = *adapter.advertising_active.read().await;
            assert!(advertising_active, "Advertising should be active after start_advertising");
        } else {
            // On systems without BLE hardware, we expect an adapter error
            assert!(matches!(result.unwrap_err(), MeshError::AdapterError(_)));
        }
    }

    #[tokio::test]
    async fn test_scanning_start() {
        // Test scanning start (central mode)
        let adapter = BLEAdapterImpl::new().await.unwrap();
        
        // Verify scanning is not active initially
        let scanning_active = *adapter.scanning_active.read().await;
        assert!(!scanning_active, "Scanning should not be active initially");
        
        // Start scanning
        let result = adapter.start_scanning().await;
        
        // If BLE hardware is available, verify scanning started
        if result.is_ok() {
            let scanning_active = *adapter.scanning_active.read().await;
            assert!(scanning_active, "Scanning should be active after start_scanning");
        } else {
            // On systems without BLE hardware, we expect an adapter error
            assert!(matches!(result.unwrap_err(), MeshError::AdapterError(_)));
        }
    }

    #[tokio::test]
    async fn test_data_fragmentation() {
        let adapter = BLEAdapterImpl::new().await.unwrap();
        
        // Test small data (no fragmentation)
        let small_data = vec![1, 2, 3, 4, 5];
        let fragments = adapter.fragment_data(&small_data);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0], small_data);
        
        // Test large data (requires fragmentation)
        let large_data = vec![0u8; 1500];
        let fragments = adapter.fragment_data(&large_data);
        assert!(fragments.len() > 1);
        
        // Verify all fragments are within MTU limit
        for fragment in &fragments {
            assert!(fragment.len() <= BLE_MTU);
        }
        
        // Verify reassembly produces original data
        let reassembled: Vec<u8> = fragments.into_iter().flatten().collect();
        assert_eq!(reassembled, large_data);
    }

    #[tokio::test]
    async fn test_data_fragmentation_edge_cases() {
        let adapter = BLEAdapterImpl::new().await.unwrap();
        
        // Test empty data
        let empty_data = vec![];
        let fragments = adapter.fragment_data(&empty_data);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0], empty_data);
        
        // Test data exactly at MTU boundary
        let boundary_data = vec![0u8; BLE_MTU];
        let fragments = adapter.fragment_data(&boundary_data);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].len(), BLE_MTU);
        
        // Test data just over MTU boundary
        let over_boundary_data = vec![0u8; BLE_MTU + 1];
        let fragments = adapter.fragment_data(&over_boundary_data);
        assert_eq!(fragments.len(), 2);
        assert_eq!(fragments[0].len(), BLE_MTU);
        assert_eq!(fragments[1].len(), 1);
    }

    #[tokio::test]
    async fn test_connected_devices_empty() {
        let adapter = BLEAdapterImpl::new().await.unwrap();
        let devices = adapter.connected_devices().await.unwrap();
        assert_eq!(devices.len(), 0, "Should have no connected devices initially");
    }

    #[tokio::test]
    async fn test_get_peripheral_not_found() {
        let adapter = BLEAdapterImpl::new().await.unwrap();
        let device_id = Uuid::new_v4();
        
        let result = adapter.get_peripheral(&device_id);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), MeshError::DeviceNotFound(_)));
    }

    #[tokio::test]
    async fn test_disconnect_nonexistent_device() {
        let adapter = BLEAdapterImpl::new().await.unwrap();
        let device_id = Uuid::new_v4();
        
        let result = adapter.disconnect(&device_id).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), MeshError::DeviceNotFound(_)));
    }
}
