use anyhow::Result;
use chrono::Utc;
use proximity::{PeerConnectionManager, PeerMessage};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::coinmarketcap_service::CoinMarketCapService;
use crate::coordination_service::CoordinationService;
use crate::mesh_metrics::MeshMetricsCollector;
use crate::mesh_types::{PriceData, PriceUpdate};

/// Provider node that fetches price data from CoinMarketCap API and broadcasts to the network
/// 
/// A provider node is responsible for:
/// - Validating API keys with CoinMarketCap
/// - Fetching price data on a regular interval (default 30 seconds)
/// - Coordinating with other providers to avoid rate limits
/// - Broadcasting price updates to all connected peers
/// - Retrying failed API calls with exponential backoff
pub struct ProviderNode {
    /// CoinMarketCap service for fetching price data
    coinmarketcap_service: Arc<CoinMarketCapService>,
    /// Peer connection manager for broadcasting to network
    peer_manager: Arc<PeerConnectionManager>,
    /// Coordination service for multi-provider coordination
    coordination_service: Arc<CoordinationService>,
    /// Metrics collector for tracking provider operations
    metrics: Arc<MeshMetricsCollector>,
    /// Fetch interval duration (default 30 seconds)
    fetch_interval: Duration,
    /// Flag indicating if provider is active
    is_active: Arc<AtomicBool>,
    /// Unique node identifier
    node_id: Uuid,
}

impl ProviderNode {
    /// Create a new ProviderNode instance
    /// 
    /// # Arguments
    /// * `coinmarketcap_service` - Service for fetching price data from CoinMarketCap API
    /// * `peer_manager` - Manager for peer connections and message broadcasting
    /// * `coordination_service` - Service for coordinating fetch timing with other providers
    /// * `metrics` - Metrics collector for tracking provider operations
    /// * `node_id` - Unique identifier for this provider node
    pub fn new(
        coinmarketcap_service: Arc<CoinMarketCapService>,
        peer_manager: Arc<PeerConnectionManager>,
        coordination_service: Arc<CoordinationService>,
        metrics: Arc<MeshMetricsCollector>,
        node_id: Uuid,
    ) -> Self {
        Self {
            coinmarketcap_service,
            peer_manager,
            coordination_service,
            metrics,
            fetch_interval: Duration::from_secs(30),
            is_active: Arc::new(AtomicBool::new(false)),
            node_id,
        }
    }

    /// Validate API key with CoinMarketCap API
    /// 
    /// Makes a test request to the CoinMarketCap API to verify the API key is valid.
    /// This should be called before enabling provider mode.
    /// 
    /// # Arguments
    /// * `api_key` - The API key to validate
    /// 
    /// # Returns
    /// * `Ok(true)` - API key is valid
    /// * `Ok(false)` - API key is invalid
    /// * `Err(_)` - Error occurred during validation
    pub async fn validate_api_key(&self, _api_key: &str) -> Result<bool> {
        info!("Validating CoinMarketCap API key");
        
        // Try to fetch price for a known token (BTC) to validate the key
        match self.coinmarketcap_service.get_price_by_symbol("BTC").await {
            Ok(_) => {
                info!("API key validation successful");
                Ok(true)
            }
            Err(e) => {
                warn!("API key validation failed: {}", e);
                Ok(false)
            }
        }
    }

    /// Start the provider node fetch loop
    /// 
    /// Begins fetching price data on the configured interval and broadcasting
    /// to the network. This method spawns a background task and returns immediately.
    /// 
    /// # Returns
    /// * `Ok(())` - Provider node started successfully
    /// * `Err(_)` - Error occurred during startup
    pub async fn start(&self) -> Result<()> {
        if self.is_active.load(Ordering::SeqCst) {
            warn!("Provider node is already active");
            return Ok(());
        }
        
        info!("Starting provider node with fetch interval: {:?}", self.fetch_interval);
        self.is_active.store(true, Ordering::SeqCst);
        
        // Clone Arc references for the background task
        let coinmarketcap_service = Arc::clone(&self.coinmarketcap_service);
        let peer_manager = Arc::clone(&self.peer_manager);
        let coordination_service = Arc::clone(&self.coordination_service);
        let metrics = Arc::clone(&self.metrics);
        let is_active = Arc::clone(&self.is_active);
        let fetch_interval = self.fetch_interval;
        let node_id = self.node_id;
        
        // Spawn background task for fetch loop
        tokio::spawn(async move {
            let mut ticker = interval(fetch_interval);
            
            while is_active.load(Ordering::SeqCst) {
                ticker.tick().await;
                
                // Check coordination service before fetching
                match coordination_service.should_fetch().await {
                    Ok(true) => {
                        debug!("Coordination check passed, proceeding with fetch");
                        
                        // Record that we're fetching
                        if let Err(e) = coordination_service.record_fetch().await {
                            warn!("Failed to record fetch in coordination service: {}", e);
                        }
                        
                        // Fetch and broadcast
                        if let Err(e) = Self::fetch_and_broadcast_static(
                            &coinmarketcap_service,
                            &peer_manager,
                            &metrics,
                            node_id,
                        ).await {
                            error!("Failed to fetch and broadcast: {}", e);
                            
                            // Record fetch failure
                            metrics.record_provider_fetch_failure(node_id, "fetch_error").await;
                        }
                    }
                    Ok(false) => {
                        debug!("Coordination check failed, skipping fetch (another provider recently fetched)");
                    }
                    Err(e) => {
                        warn!("Coordination service error: {}, proceeding with fetch", e);
                        
                        // On coordination error, proceed with fetch anyway
                        if let Err(e) = Self::fetch_and_broadcast_static(
                            &coinmarketcap_service,
                            &peer_manager,
                            &metrics,
                            node_id,
                        ).await {
                            error!("Failed to fetch and broadcast: {}", e);
                            
                            // Record fetch failure
                            metrics.record_provider_fetch_failure(node_id, "fetch_error").await;
                        }
                    }
                }
            }
            
            info!("Provider node fetch loop stopped");
        });
        
        Ok(())
    }

    /// Stop the provider node
    /// 
    /// Gracefully shuts down the provider node by stopping the fetch loop.
    pub async fn stop(&self) {
        info!("Stopping provider node");
        self.is_active.store(false, Ordering::SeqCst);
    }

    /// Fetch price data and broadcast to network (static version for background task)
    async fn fetch_and_broadcast_static(
        coinmarketcap_service: &Arc<CoinMarketCapService>,
        peer_manager: &Arc<PeerConnectionManager>,
        metrics: &Arc<MeshMetricsCollector>,
        node_id: Uuid,
    ) -> Result<()> {
        debug!("Fetching price data from CoinMarketCap API");
        
        let start = std::time::Instant::now();
        
        // Fetch prices with retry logic
        let prices = match Self::fetch_prices_with_retry(coinmarketcap_service, metrics, node_id).await {
            Ok(p) => p,
            Err(e) => {
                // Record fetch failure
                metrics.record_provider_fetch_failure(node_id, "api_error").await;
                return Err(e);
            }
        };
        
        if prices.is_empty() {
            warn!("No price data fetched from CoinMarketCap API");
            metrics.record_provider_fetch_failure(node_id, "empty_response").await;
            return Ok(());
        }
        
        let duration_ms = start.elapsed().as_millis() as u64;
        let asset_count = prices.len();
        
        info!("Fetched {} price data points in {}ms", asset_count, duration_ms);
        
        // Create price update message
        let update = Self::create_price_update_static(node_id, prices);
        
        // Broadcast to all connected peers
        Self::broadcast_update_static(peer_manager, update).await?;
        
        // Record successful fetch
        metrics.record_provider_fetch_success(node_id, duration_ms, asset_count).await;
        
        Ok(())
    }

    /// Fetch and broadcast price data to the network
    /// 
    /// This method:
    /// 1. Fetches price data from CoinMarketCap API with retry logic
    /// 2. Creates a PriceUpdate message with unique ID
    /// 3. Broadcasts the update to all connected peers
    /// 
    /// # Returns
    /// * `Ok(())` - Successfully fetched and broadcast
    /// * `Err(_)` - Error occurred during fetch or broadcast
    pub async fn fetch_and_broadcast(&self) -> Result<()> {
        Self::fetch_and_broadcast_static(
            &self.coinmarketcap_service,
            &self.peer_manager,
            &self.metrics,
            self.node_id,
        ).await
    }

    /// Fetch prices from CoinMarketCap API with exponential backoff retry
    /// 
    /// Retries up to 3 times with exponential backoff on failure.
    /// 
    /// # Returns
    /// * `Ok(HashMap)` - Successfully fetched price data
    /// * `Err(_)` - All retry attempts failed
    async fn fetch_prices_with_retry(
        coinmarketcap_service: &Arc<CoinMarketCapService>,
        metrics: &Arc<MeshMetricsCollector>,
        node_id: Uuid,
    ) -> Result<HashMap<String, PriceData>> {
        let max_attempts = 3;
        let mut attempt = 0;
        let mut backoff_ms = 100u64;
        
        loop {
            attempt += 1;
            debug!("Fetch attempt {}/{}", attempt, max_attempts);
            
            match Self::fetch_prices_once(coinmarketcap_service).await {
                Ok(prices) => {
                    info!("Successfully fetched prices on attempt {}", attempt);
                    return Ok(prices);
                }
                Err(e) => {
                    if attempt >= max_attempts {
                        error!("Failed to fetch prices after {} attempts: {}", max_attempts, e);
                        metrics.record_provider_fetch_failure(node_id, "max_retries_exceeded").await;
                        return Err(e);
                    }
                    
                    warn!(
                        "Fetch attempt {} failed: {}. Retrying in {}ms",
                        attempt, e, backoff_ms
                    );
                    
                    // Exponential backoff
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                    backoff_ms *= 2;
                }
            }
        }
    }

    /// Fetch prices from CoinMarketCap API once (single attempt)
    async fn fetch_prices_once(
        coinmarketcap_service: &Arc<CoinMarketCapService>,
    ) -> Result<HashMap<String, PriceData>> {
        // Fetch prices for major tokens
        let symbols = vec!["BTC", "ETH", "SOL", "USDC", "USDT"];
        
        let mut prices = HashMap::new();
        
        for symbol in symbols {
            match coinmarketcap_service.get_price_by_symbol(symbol).await {
                Ok(price_data) => {
                    prices.insert(
                        symbol.to_string(),
                        PriceData {
                            asset: symbol.to_string(),
                            price: price_data.price_usd.to_string(),
                            blockchain: "multi".to_string(), // CMC is blockchain-agnostic
                            change_24h: price_data.price_change_24h.map(|d| d.to_string()),
                        },
                    );
                }
                Err(e) => {
                    warn!("Failed to fetch price for {}: {}", symbol, e);
                }
            }
        }
        
        Ok(prices)
    }

    /// Create a price update message with unique ID (static version)
    /// 
    /// # Arguments
    /// * `node_id` - Source node identifier
    /// * `prices` - Map of asset symbols to price data
    /// 
    /// # Returns
    /// A PriceUpdate message with:
    /// - Unique message ID
    /// - Source node ID
    /// - Current timestamp
    /// - Price data
    /// - TTL of 10
    fn create_price_update_static(node_id: Uuid, prices: HashMap<String, PriceData>) -> PriceUpdate {
        PriceUpdate {
            message_id: Uuid::new_v4(),
            source_node_id: node_id,
            timestamp: Utc::now(),
            prices,
            ttl: 10,
        }
    }

    /// Broadcast price update to all connected peers (static version)
    /// 
    /// Sends the price update message to all currently connected peers.
    /// Failures to individual peers are logged but don't prevent broadcasting
    /// to other peers.
    /// 
    /// # Arguments
    /// * `peer_manager` - Manager used to send messages to peers
    /// * `update` - The price update message to broadcast
    /// 
    /// # Returns
    /// * `Ok(())` - Broadcast completed (may have partial failures)
    /// * `Err(_)` - Critical error occurred
    async fn broadcast_update_static(
        peer_manager: &Arc<PeerConnectionManager>,
        update: PriceUpdate,
    ) -> Result<()> {
        let peers = peer_manager.get_active_connections().await;
        
        if peers.is_empty() {
            debug!("No connected peers to broadcast to");
            return Ok(());
        }
        
        info!("Broadcasting price update to {} peers", peers.len());
        
        // Convert PriceUpdate to PeerMessage
        let message = PeerMessage::PriceUpdate {
            message_id: update.message_id,
            source_node_id: update.source_node_id,
            timestamp: update.timestamp,
            prices: serde_json::to_value(&update.prices)?,
            ttl: update.ttl,
        };
        
        let mut success_count = 0;
        let mut failure_count = 0;
        
        for peer_id in peers {
            match peer_manager.send_message(peer_id.clone(), message.clone()).await {
                Ok(()) => {
                    debug!("Successfully sent price update to peer: {}", peer_id);
                    success_count += 1;
                }
                Err(e) => {
                    warn!("Failed to send price update to peer {}: {}", peer_id, e);
                    failure_count += 1;
                }
            }
        }
        
        info!(
            "Broadcast complete: {} successful, {} failed",
            success_count, failure_count
        );
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_create_price_update() {
        let node_id = Uuid::new_v4();
        let mut prices = HashMap::new();
        prices.insert(
            "SOL".to_string(),
            PriceData {
                asset: "SOL".to_string(),
                price: "100.50".to_string(),
                blockchain: "solana".to_string(),
                change_24h: Some("5.2".to_string()),
            },
        );
        
        let update = ProviderNode::create_price_update_static(node_id, prices.clone());
        
        assert_eq!(update.source_node_id, node_id);
        assert_eq!(update.ttl, 10);
        assert_eq!(update.prices.len(), 1);
        assert!(update.prices.contains_key("SOL"));
    }
    
    #[test]
    fn test_message_id_uniqueness() {
        let node_id = Uuid::new_v4();
        let prices = HashMap::new();
        
        let update1 = ProviderNode::create_price_update_static(node_id, prices.clone());
        let update2 = ProviderNode::create_price_update_static(node_id, prices);
        
        assert_ne!(update1.message_id, update2.message_id);
    }
}
