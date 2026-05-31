use anyhow::Result;
use proximity::PeerConnectionManager;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::coinmarketcap_service::CoinMarketCapService;
use crate::coordination_service::CoordinationService;
use crate::gossip_protocol::GossipProtocol;
use crate::mesh_metrics::MeshMetricsCollector;
use crate::mesh_types::{CachedPriceData, NetworkStatus, ProviderConfig, PriceUpdate};
use crate::message_tracker::MessageTracker;
use crate::network_status_tracker::NetworkStatusTracker;
use crate::price_cache::PriceCache;
use crate::provider_node::ProviderNode;
use crate::websocket_service::WebSocketService;

/// Helper structure for message handler closure
/// 
/// This structure contains only the components needed for message handling,
/// allowing it to be moved into async closures.
struct MeshPriceServiceHandler {
    gossip_protocol: Arc<GossipProtocol>,
    network_status_tracker: Arc<NetworkStatusTracker>,
    provider_config: Arc<RwLock<ProviderConfig>>,
    node_id: Uuid,
}

impl MeshPriceServiceHandler {
    /// Handle incoming price update from a peer
    async fn handle_price_update(&self, update: PriceUpdate, from_peer: String) -> Result<()> {
        tracing::debug!(
            message_id = %update.message_id,
            source_node = %update.source_node_id,
            from_peer = %from_peer,
            "Received price update via message handler"
        );
        
        // Check if this node is a provider for loop prevention
        let config = self.provider_config.read().await;
        let is_provider = config.enabled;
        drop(config);
        
        let is_from_provider = update.source_node_id != self.node_id;
        
        if is_provider && is_from_provider {
            tracing::debug!(
                message_id = %update.message_id,
                source_node = %update.source_node_id,
                "Provider node received update from another provider"
            );
        }
        
        // Delegate to gossip protocol for processing
        self.gossip_protocol
            .process_update(update, from_peer)
            .await?;
        
        Ok(())
    }
    
    /// Handle incoming network status message from a peer
    /// 
    /// Updates the network topology with provider information and
    /// updates routing preferences based on hop count.
    /// Handles auto-discovery of new providers joining the network.
    /// 
    /// Requirements: 9.3, 9.4, 10.3, 13.1, 13.2
    async fn handle_network_status(
        &self,
        peer_id: String,
        node_id: Uuid,
        is_provider: bool,
        hop_count: u32,
    ) -> Result<()> {
        tracing::debug!(
            peer_id = %peer_id,
            node_id = %node_id,
            is_provider = is_provider,
            hop_count = hop_count,
            "Received network status from peer"
        );
        
        // Check if this is a new provider joining the network
        let was_known_provider = if is_provider {
            let providers_before = self.network_status_tracker.get_active_providers().await?;
            providers_before.iter().any(|p| p.node_id == node_id)
        } else {
            false
        };
        
        // Update provider status if the peer is a provider
        if is_provider {
            self.network_status_tracker
                .update_provider_status(node_id, true)
                .await?;
            
            // Handle new provider auto-discovery (Requirement 9.4)
            if !was_known_provider {
                tracing::info!(
                    "Auto-discovered new provider node {} joining the network",
                    node_id
                );
            } else {
                // Handle provider reconnection (Requirement 9.3)
                tracing::info!("Provider node {} reconnected", node_id);
                self.network_status_tracker
                    .on_provider_reconnected(node_id)
                    .await?;
            }
        }
        
        // Update topology information
        self.network_status_tracker
            .update_topology(peer_id.clone(), hop_count)
            .await?;
        
        tracing::debug!("Network status updated for node {}", node_id);
        Ok(())
    }
}

/// Main orchestrator for the mesh network price distribution system
/// 
/// MeshPriceService coordinates all components of the P2P mesh network:
/// - Provider nodes that fetch price data from external APIs
/// - Gossip protocol for message propagation
/// - Message deduplication and caching
/// - Network status tracking
/// - WebSocket updates to clients
pub struct MeshPriceService {
    /// CoinMarketCap service for fetching price data
    coinmarketcap_service: Arc<CoinMarketCapService>,
    /// Peer connection manager for P2P communication
    peer_manager: Arc<PeerConnectionManager>,
    /// Message tracker for deduplication
    message_tracker: Arc<MessageTracker>,
    /// Price cache for local storage
    price_cache: Arc<PriceCache>,
    /// Provider configuration
    provider_config: Arc<RwLock<ProviderConfig>>,
    /// Coordination service for multi-provider coordination
    coordination_service: Arc<CoordinationService>,
    /// Network status tracker
    network_status_tracker: Arc<NetworkStatusTracker>,
    /// Gossip protocol handler
    gossip_protocol: Arc<GossipProtocol>,
    /// Provider node (optional, only when provider mode is enabled)
    provider_node: Arc<RwLock<Option<Arc<ProviderNode>>>>,
    /// Unique node identifier
    node_id: Uuid,
    /// Metrics collector for mesh network operations
    metrics: Arc<MeshMetricsCollector>,
}

impl MeshPriceService {
    /// Create a new MeshPriceService instance
    /// 
    /// Initializes all components required for the mesh network price distribution system.
    /// 
    /// # Arguments
    /// * `coinmarketcap_service` - Service for fetching price data from CoinMarketCap API
    /// * `peer_manager` - Manager for P2P connections
    /// * `redis` - Redis connection for caching and coordination
    /// * `db` - Database pool for persistent storage
    /// * `websocket_service` - Service for pushing updates to WebSocket clients
    /// 
    /// # Returns
    /// A new MeshPriceService instance ready to start
    pub fn new(
        coinmarketcap_service: Arc<CoinMarketCapService>,
        peer_manager: Arc<PeerConnectionManager>,
        redis: redis::aio::ConnectionManager,
        db: database::DbPool,
        websocket_service: Arc<WebSocketService>,
    ) -> Self {
        let node_id = Uuid::new_v4();
        
        // Initialize metrics collector
        let metrics = Arc::new(MeshMetricsCollector::new());
        
        // Initialize message tracker
        let message_tracker = Arc::new(MessageTracker::new(redis.clone()));
        
        // Initialize price cache with metrics
        let price_cache = Arc::new(PriceCache::new(redis.clone(), db, Arc::clone(&metrics)));
        
        // Initialize coordination service
        let coordination_service = Arc::new(CoordinationService::new(redis.clone(), node_id));
        
        // Initialize network status tracker
        let network_status_tracker = Arc::new(NetworkStatusTracker::new(
            Arc::clone(&peer_manager),
            redis.clone(),
            node_id,
        ));
        
        // Initialize gossip protocol with metrics
        let gossip_protocol = Arc::new(GossipProtocol::new(
            Arc::clone(&peer_manager),
            Arc::clone(&message_tracker),
            Arc::clone(&price_cache),
            Arc::clone(&websocket_service),
            Arc::clone(&metrics),
        ));
        
        // Initialize provider config with defaults
        let provider_config = Arc::new(RwLock::new(ProviderConfig::default()));
        
        Self {
            coinmarketcap_service,
            peer_manager,
            message_tracker,
            price_cache,
            provider_config,
            coordination_service,
            network_status_tracker,
            gossip_protocol,
            provider_node: Arc::new(RwLock::new(None)),
            node_id,
            metrics,
        }
    }
    
    /// Start the mesh price distribution service
    /// 
    /// Initializes the service by:
    /// - Loading cached data from storage
    /// - Loading seen messages from cache
    /// - Starting provider node if provider mode is enabled
    /// - Setting up message handler for P2P messages
    /// 
    /// # Returns
    /// * `Ok(())` - Service started successfully
    /// * `Err(_)` - Error occurred during startup
    pub async fn start(&self) -> Result<()> {
        tracing::info!("Starting MeshPriceService with node ID: {}", self.node_id);
        
        // Load cached price data from storage
        if let Err(e) = self.price_cache.load_from_storage().await {
            tracing::warn!("Failed to load price cache from storage: {}", e);
        }
        
        // Load seen messages from cache
        if let Err(e) = self.message_tracker.load_from_cache().await {
            tracing::warn!("Failed to load seen messages from cache: {}", e);
        }
        
        // Set up message handler for P2P messages
        self.setup_message_handler().await;
        
        // Start provider node if provider mode is enabled
        let config = self.provider_config.read().await;
        if config.enabled {
            drop(config); // Release read lock before calling enable_provider_mode
            tracing::info!("Provider mode is enabled, starting provider node");
            // Provider node should already be initialized if config.enabled is true
            let provider_node = self.provider_node.read().await;
            if let Some(node) = provider_node.as_ref() {
                node.start().await?;
            }
        }
        
        tracing::info!("MeshPriceService started successfully");
        Ok(())
    }
    
    /// Set up message handler for routing P2P messages
    /// 
    /// Registers a handler with PeerConnectionManager to route:
    /// - PriceUpdate messages to handle_price_update
    /// - NetworkStatus messages to handle_network_status
    async fn setup_message_handler(&self) {
        use proximity::PeerMessage;
        
        let service = Arc::new(self.clone_for_handler());
        
        self.peer_manager.set_message_handler(move |peer_id, message| {
            let service = Arc::clone(&service);
            async move {
                let result: Result<()> = match message {
                    PeerMessage::PriceUpdate {
                        message_id,
                        source_node_id,
                        timestamp,
                        prices,
                        ttl,
                    } => {
                        // Deserialize prices from JSON
                        let prices_map: std::collections::HashMap<String, crate::mesh_types::PriceData> = 
                            serde_json::from_value(prices)
                                .map_err(|e| anyhow::anyhow!("Failed to deserialize prices: {}", e))?;
                        
                        let update = crate::mesh_types::PriceUpdate {
                            message_id,
                            source_node_id,
                            timestamp,
                            prices: prices_map,
                            ttl,
                        };
                        
                        service.handle_price_update(update, peer_id).await
                    }
                    PeerMessage::NetworkStatus {
                        node_id,
                        is_provider,
                        hop_count,
                    } => {
                        service.handle_network_status(peer_id, node_id, is_provider, hop_count).await
                    }
                    _ => {
                        // Other message types are not handled by mesh service
                        Ok(())
                    }
                };
                
                // Convert anyhow::Error to Box<dyn Error>
                result.map_err(|e| {
                    let err_msg = e.to_string();
                    Box::new(std::io::Error::new(std::io::ErrorKind::Other, err_msg)) as Box<dyn std::error::Error + Send + Sync>
                })
            }
        }).await;
        
        tracing::info!("Message handler registered with PeerConnectionManager");
    }
    
    /// Create a clone-like structure for use in async closures
    /// 
    /// This is needed because Self cannot be cloned directly,
    /// but we need to move it into the message handler closure.
    fn clone_for_handler(&self) -> MeshPriceServiceHandler {
        MeshPriceServiceHandler {
            gossip_protocol: Arc::clone(&self.gossip_protocol),
            network_status_tracker: Arc::clone(&self.network_status_tracker),
            provider_config: Arc::clone(&self.provider_config),
            node_id: self.node_id,
        }
    }
    
    /// Stop the service gracefully
    /// 
    /// Shuts down all components:
    /// - Stops provider node if running
    /// - Persists cache to storage
    /// - Persists seen messages
    /// 
    /// # Returns
    /// * `Ok(())` - Service stopped successfully
    /// * `Err(_)` - Error occurred during shutdown
    pub async fn stop(&self) -> Result<()> {
        tracing::info!("Stopping MeshPriceService");
        
        // Stop provider node if running
        let provider_node = self.provider_node.read().await;
        if let Some(node) = provider_node.as_ref() {
            node.stop().await;
        }
        drop(provider_node);
        
        // Persist cache to storage
        if let Err(e) = self.price_cache.persist_to_storage().await {
            tracing::error!("Failed to persist price cache: {}", e);
        }
        
        // Persist seen messages
        if let Err(e) = self.message_tracker.persist_to_cache().await {
            tracing::error!("Failed to persist seen messages: {}", e);
        }
        
        tracing::info!("MeshPriceService stopped");
        Ok(())
    }
    
    /// Check if this node is currently a provider
    /// 
    /// # Returns
    /// `true` if provider mode is enabled, `false` otherwise
    pub async fn is_provider(&self) -> bool {
        let config = self.provider_config.read().await;
        config.enabled
    }
}

impl MeshPriceService {
    /// Enable provider mode with API key validation
    /// 
    /// This method:
    /// 1. Validates the API key with CoinMarketCap API
    /// 2. Creates and starts a ProviderNode if validation succeeds
    /// 3. Updates the provider configuration
    /// 4. Updates network status to reflect provider mode
    /// 
    /// Requirements: 1.1, 1.2, 1.3, 1.5
    /// 
    /// # Arguments
    /// * `api_key` - The CoinMarketCap API key to validate and use
    /// 
    /// # Returns
    /// * `Ok(())` - Provider mode enabled successfully
    /// * `Err(_)` - API key validation failed or error occurred
    pub async fn enable_provider_mode(&self, api_key: String) -> Result<()> {
        tracing::info!("Enabling provider mode");
        
        // Create a temporary provider node for validation
        let temp_provider = ProviderNode::new(
            Arc::clone(&self.coinmarketcap_service),
            Arc::clone(&self.peer_manager),
            Arc::clone(&self.coordination_service),
            Arc::clone(&self.metrics),
            self.node_id,
        );
        
        // Validate API key
        tracing::info!("Validating API key");
        let is_valid = temp_provider.validate_api_key(&api_key).await?;
        
        if !is_valid {
            tracing::warn!("API key validation failed");
            return Err(anyhow::anyhow!("Invalid API key"));
        }
        
        tracing::info!("API key validation successful");
        
        // Update provider configuration
        {
            let mut config = self.provider_config.write().await;
            config.enabled = true;
            config.api_key = Some(api_key.clone());
            config.node_id = self.node_id;
        }
        
        // Create and start provider node
        let provider = Arc::new(ProviderNode::new(
            Arc::clone(&self.coinmarketcap_service),
            Arc::clone(&self.peer_manager),
            Arc::clone(&self.coordination_service),
            Arc::clone(&self.metrics),
            self.node_id,
        ));
        
        provider.start().await?;
        
        // Store provider node
        {
            let mut provider_node = self.provider_node.write().await;
            *provider_node = Some(provider);
        }
        
        // Update network status
        self.network_status_tracker
            .update_provider_status(self.node_id, true)
            .await?;
        
        tracing::info!("Provider mode enabled successfully");
        Ok(())
    }
    
    /// Disable provider mode
    /// 
    /// This method:
    /// 1. Stops the ProviderNode if running
    /// 2. Updates the provider configuration
    /// 3. Updates network status to reflect non-provider mode
    /// 
    /// Requirement: 1.5
    /// 
    /// # Returns
    /// * `Ok(())` - Provider mode disabled successfully
    /// * `Err(_)` - Error occurred during shutdown
    pub async fn disable_provider_mode(&self) -> Result<()> {
        tracing::info!("Disabling provider mode");
        
        // Stop provider node if running
        {
            let mut provider_node = self.provider_node.write().await;
            if let Some(node) = provider_node.as_ref() {
                node.stop().await;
            }
            *provider_node = None;
        }
        
        // Update provider configuration
        {
            let mut config = self.provider_config.write().await;
            config.enabled = false;
            config.api_key = None;
        }
        
        // Update network status
        self.network_status_tracker
            .update_provider_status(self.node_id, false)
            .await?;
        
        tracing::info!("Provider mode disabled successfully");
        Ok(())
    }
}

impl MeshPriceService {
    /// Handle incoming price update from a peer
    /// 
    /// This method:
    /// 1. Checks if this node is a provider (loop prevention)
    /// 2. Delegates to GossipProtocol for processing
    /// 3. Handles deduplication, caching, and relay
    /// 
    /// Requirements: 3.5, 6.4, 8.5
    /// 
    /// # Arguments
    /// * `update` - The price update message received
    /// * `from_peer` - The peer ID that sent the message
    /// 
    /// # Returns
    /// * `Ok(())` - Message processed successfully
    /// * `Err(_)` - Error occurred during processing
    pub async fn handle_price_update(&self, update: PriceUpdate, from_peer: String) -> Result<()> {
        tracing::debug!(
            message_id = %update.message_id,
            source_node = %update.source_node_id,
            from_peer = %from_peer,
            "Received price update"
        );
        
        // Loop prevention: If this node is a provider and the message is from another provider,
        // we should not re-broadcast it
        let is_provider = self.is_provider().await;
        let is_from_provider = update.source_node_id != self.node_id;
        
        if is_provider && is_from_provider {
            tracing::debug!(
                message_id = %update.message_id,
                source_node = %update.source_node_id,
                "Provider node received update from another provider - processing but not re-broadcasting"
            );
            // We still process and cache the data, but the gossip protocol will handle
            // not re-broadcasting from providers
        }
        
        // Delegate to gossip protocol for processing
        self.gossip_protocol
            .process_update(update, from_peer)
            .await?;
        
        Ok(())
    }
    
    /// Get cached price data for a specific asset
    /// 
    /// Retrieves price data from the local cache. If no providers are online,
    /// this serves cached data as a fallback.
    /// 
    /// Requirements: 6.4, 9.1
    /// 
    /// # Arguments
    /// * `asset` - The asset symbol to retrieve price for
    /// 
    /// # Returns
    /// * `Ok(Some(CachedPriceData))` - Price data found in cache
    /// * `Ok(None)` - No price data available for this asset
    /// * `Err(_)` - Error occurred during retrieval
    pub async fn get_price_data(&self, asset: &str) -> Result<Option<CachedPriceData>> {
        let data = self.price_cache.get(asset).await?;
        
        // Log if we're serving cached data due to no providers being online
        if data.is_some() {
            let status = self.network_status_tracker.get_status().await?;
            if status.active_providers.is_empty() {
                tracing::debug!(
                    "Serving cached data for {} - no providers online",
                    asset
                );
            }
        }
        
        Ok(data)
    }
    
    /// Get all cached price data
    /// 
    /// Retrieves all price data currently in the cache.
    /// When no providers are online, this serves as the fallback data source.
    /// 
    /// Requirements: 6.4, 9.1
    /// 
    /// # Returns
    /// * `Ok(HashMap)` - Map of asset symbols to cached price data
    /// * `Err(_)` - Error occurred during retrieval
    pub async fn get_all_price_data(&self) -> Result<HashMap<String, CachedPriceData>> {
        let data = self.price_cache.get_all().await?;
        
        // Log if we're serving cached data due to no providers being online
        if !data.is_empty() {
            let status = self.network_status_tracker.get_status().await?;
            if status.active_providers.is_empty() {
                tracing::debug!(
                    "Serving {} cached price entries - no providers online",
                    data.len()
                );
            }
        }
        
        Ok(data)
    }
    
    /// Get current network status
    /// 
    /// Returns comprehensive network status including:
    /// - Active provider nodes
    /// - Connected peers count
    /// - Network size estimate
    /// - Data freshness
    /// 
    /// Requirement: 8.5
    /// 
    /// # Returns
    /// * `Ok(NetworkStatus)` - Current network status
    /// * `Err(_)` - Error occurred building status
    pub async fn get_network_status(&self) -> Result<NetworkStatus> {
        self.network_status_tracker.get_status().await
    }
    
    /// Get the unique node identifier for this service
    /// 
    /// # Returns
    /// The UUID identifying this node in the mesh network
    pub fn node_id(&self) -> Uuid {
        self.node_id
    }
    
    /// Send network status to a newly connected peer
    /// 
    /// This should be called when a new peer connection is established
    /// to exchange network status information.
    /// 
    /// Requirement: 10.3
    /// 
    /// # Arguments
    /// * `peer_id` - The peer to send network status to
    /// 
    /// # Returns
    /// * `Ok(())` - Network status sent successfully
    /// * `Err(_)` - Error occurred sending message
    pub async fn send_network_status_to_peer(&self, peer_id: String) -> Result<()> {
        tracing::debug!("Sending network status to peer: {}", peer_id);
        
        // Get current provider status
        let config = self.provider_config.read().await;
        let is_provider = config.enabled;
        drop(config);
        
        // Calculate hop count (0 for direct connection, will be incremented by receiver)
        let hop_count = 0u32;
        
        // Create network status message
        let message = proximity::PeerMessage::NetworkStatus {
            node_id: self.node_id,
            is_provider,
            hop_count,
        };
        
        // Send to peer
        self.peer_manager
            .send_message(peer_id.clone(), message)
            .await?;
        
        tracing::debug!("Network status sent to peer: {}", peer_id);
        Ok(())
    }
    
    /// Handle a new peer connection
    /// 
    /// Called when a new peer is discovered and connected.
    /// Exchanges network status information with the peer.
    /// 
    /// Requirement: 10.2, 10.3
    /// 
    /// # Arguments
    /// * `peer_id` - The newly connected peer
    /// 
    /// # Returns
    /// * `Ok(())` - Connection handled successfully
    /// * `Err(_)` - Error occurred during handling
    pub async fn on_peer_connected(&self, peer_id: String) -> Result<()> {
        tracing::info!("New peer connected: {}", peer_id);
        
        // Record peer connection
        self.metrics.record_peer_connected(peer_id.clone()).await;
        
        // Update peer count
        let peer_count = self.peer_manager.get_active_connections().await.len();
        self.metrics.record_peer_count(peer_count).await;
        
        // Send our network status to the new peer
        self.send_network_status_to_peer(peer_id).await?;
        
        Ok(())
    }
    
    /// Handle a peer disconnection
    /// 
    /// Called when a peer disconnects from the network.
    /// Updates network status to reflect the disconnection.
    /// If the disconnected peer was a provider, handles provider failover.
    /// 
    /// Requirements: 9.1, 9.2, 10.4
    /// 
    /// # Arguments
    /// * `peer_id` - The disconnected peer
    /// 
    /// # Returns
    /// * `Ok(())` - Disconnection handled successfully
    /// * `Err(_)` - Error occurred during handling
    pub async fn on_peer_disconnected(&self, peer_id: String) -> Result<()> {
        tracing::info!("Peer disconnected: {}", peer_id);
        
        // Record peer disconnection
        self.metrics.record_peer_disconnected(peer_id.clone()).await;
        
        // Update peer count
        let peer_count = self.peer_manager.get_active_connections().await.len();
        self.metrics.record_peer_count(peer_count).await;
        
        // Check if this peer was a provider by querying active providers
        let providers_before = self.network_status_tracker.get_active_providers().await?;
        
        // Try to parse peer_id as UUID to check if it was a provider
        if let Ok(peer_uuid) = Uuid::parse_str(&peer_id) {
            let was_provider = providers_before.iter().any(|p| p.node_id == peer_uuid);
            
            if was_provider {
                tracing::warn!("Provider node {} disconnected", peer_uuid);
                
                // Handle provider disconnect
                self.network_status_tracker
                    .on_provider_disconnected(peer_uuid)
                    .await?;
                
                // Check if we still have providers
                let providers_after = self.network_status_tracker.get_active_providers().await?;
                
                if providers_after.is_empty() {
                    tracing::warn!(
                        "No live data sources available - serving cached data only"
                    );
                    
                    // Note: Cached data is automatically preserved (Requirement 9.1)
                    // The price cache retains all data even when providers disconnect
                }
            }
        }
        
        Ok(())
    }
    
    /// Update routing preferences based on network topology
    /// 
    /// Recalculates routing preferences for all connected peers based on
    /// their hop count to provider nodes. This should be called when
    /// network topology changes.
    /// 
    /// Requirement: 13.2
    /// 
    /// # Returns
    /// * `Ok(())` - Routing preferences updated successfully
    /// * `Err(_)` - Error occurred during update
    pub async fn update_routing_preferences(&self) -> Result<()> {
        tracing::debug!("Updating routing preferences based on network topology");
        
        // Get all active providers with their hop counts
        let providers = self.network_status_tracker.get_active_providers().await?;
        
        // Get all connected peers
        let peers = self.peer_manager.get_active_connections().await;
        
        // For each peer, calculate the minimum hop count to any provider
        for peer_id in peers {
            // Query the hop count for this peer from network status tracker
            // For now, we'll use a default hop count of 1 for direct connections
            // In a full implementation, this would query the actual topology
            let hop_count = if providers.iter().any(|p| p.node_id.to_string() == peer_id) {
                0 // Direct connection to provider
            } else {
                1 // One hop away (simplified)
            };
            
            self.peer_manager
                .update_routing_preference(peer_id, hop_count)
                .await;
        }
        
        tracing::debug!("Routing preferences updated");
        Ok(())
    }
    
    /// Optimize peer connections based on routing preferences
    /// 
    /// Evaluates current connections and maintains connections on shortest
    /// paths to providers. Drops connections with poor routing if at capacity.
    /// 
    /// Requirement: 13.2, 13.3, 13.5
    /// 
    /// # Returns
    /// * `Ok(usize)` - Number of connections dropped during optimization
    /// * `Err(_)` - Error occurred during optimization
    pub async fn optimize_peer_connections(&self) -> Result<usize> {
        tracing::debug!("Optimizing peer connections");
        
        // Update routing preferences first
        self.update_routing_preferences().await?;
        
        // Optimize connections in peer manager
        let dropped = self.peer_manager.optimize_connections().await?;
        
        tracing::info!("Connection optimization complete, dropped {} connections", dropped);
        Ok(dropped)
    }
    
    /// Check if a new peer connection should be accepted
    /// 
    /// Determines whether to accept a new peer based on connection limits
    /// and routing preferences. If at capacity, may suggest dropping an
    /// existing peer with worse routing.
    /// 
    /// Requirement: 13.5
    /// 
    /// # Arguments
    /// * `peer_hop_count` - Hop count to providers through the new peer
    /// 
    /// # Returns
    /// * `Ok(None)` - Can accept without dropping any peer
    /// * `Ok(Some(peer_id))` - Should drop this peer to make room
    /// * `Err(_)` - Should not accept new peer
    pub async fn should_accept_peer(&self, peer_hop_count: u32) -> Result<Option<String>> {
        self.peer_manager
            .should_accept_connection(peer_hop_count)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to check connection acceptance: {}", e))
    }
    
    /// Get mesh network metrics
    /// 
    /// Returns current metrics for monitoring and observability.
    /// 
    /// # Returns
    /// * `MeshMetricsSummary` - Summary of mesh network metrics
    pub async fn get_metrics(&self) -> crate::mesh_metrics::MeshMetricsSummary {
        self.metrics.get_summary().await
    }
    
    /// Get the metrics collector
    /// 
    /// Returns a reference to the metrics collector for external use.
    pub fn metrics(&self) -> Arc<MeshMetricsCollector> {
        Arc::clone(&self.metrics)
    }
}
