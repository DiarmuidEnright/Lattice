use anyhow::Result;
use chrono::{DateTime, Utc};
use redis::AsyncCommands;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::mesh_types::{CachedPriceData, DataFreshness};
use crate::mesh_metrics::MeshMetricsCollector;
use database::DbPool;

/// Manages local caching of price data with freshness tracking
pub struct PriceCache {
    /// In-memory cache for fast lookups
    cache: Arc<RwLock<HashMap<String, CachedPriceData>>>,
    /// Redis connection for distributed caching
    redis: redis::aio::ConnectionManager,
    /// Database pool for persistent storage
    db: DbPool,
    /// Metrics collector for tracking cache operations
    metrics: Arc<MeshMetricsCollector>,
}

impl PriceCache {
    /// Create a new PriceCache with the specified Redis connection and database pool
    pub fn new(redis: redis::aio::ConnectionManager, db: DbPool, metrics: Arc<MeshMetricsCollector>) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            redis,
            db,
            metrics,
        }
    }
    
    /// Store price data in cache
    /// 
    /// Compares timestamps and only stores if the new data is fresher than existing data
    /// Stores in memory, Redis, and database for durability
    /// Detects price discrepancies >5% between providers and logs them
    pub async fn store(&self, asset: String, data: CachedPriceData) -> Result<()> {
        // Check if we should replace existing data and detect price discrepancies
        let (should_store, existing_data) = {
            let cache = self.cache.read().await;
            match cache.get(&asset) {
                Some(existing) => {
                    let should_replace = self.should_replace(existing, &data);
                    (should_replace, Some(existing.clone()))
                }
                None => (true, None),
            }
        };
        
        // Detect price discrepancies between providers (Requirement 8.2, 8.4)
        if let Some(existing) = &existing_data {
            // Only check discrepancies if data is from different providers
            if existing.source_node_id != data.source_node_id {
                if let Some(discrepancy_pct) = self.calculate_price_discrepancy(existing, &data) {
                    if discrepancy_pct > 5.0 {
                        tracing::warn!(
                            "Price discrepancy detected for asset {}: {:.2}% difference between provider {} (price: {}) and provider {} (price: {})",
                            asset,
                            discrepancy_pct,
                            existing.source_node_id,
                            existing.price,
                            data.source_node_id,
                            data.price
                        );
                    }
                }
            }
        }
        
        if !should_store {
            tracing::debug!(
                "Skipping store for asset {} - existing data is newer (existing: {}, new: {})",
                asset,
                existing_data.as_ref().map(|d| d.timestamp.to_rfc3339()).unwrap_or_default(),
                data.timestamp.to_rfc3339()
            );
            return Ok(());
        }
        
        // Store in memory cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(asset.clone(), data.clone());
        }
        
        // Store in Redis with 1-hour TTL
        let redis_key = format!("mesh:price:{}", asset);
        let data_json = serde_json::to_string(&data)?;
        let mut conn = self.redis.clone();
        let _: () = conn.set_ex(&redis_key, data_json, 3600).await?;
        
        tracing::debug!(
            "Stored price data for asset {} from node {} (timestamp: {})",
            asset,
            data.source_node_id,
            data.timestamp.to_rfc3339()
        );
        
        Ok(())
    }
    
    /// Get price data from cache for a specific asset
    /// 
    /// Checks memory first, then Redis if not found in memory
    pub async fn get(&self, asset: &str) -> Result<Option<CachedPriceData>> {
        // First check in-memory cache
        {
            let cache = self.cache.read().await;
            if let Some(data) = cache.get(asset) {
                self.metrics.record_cache_hit(asset).await;
                return Ok(Some(data.clone()));
            }
        }
        
        // If not in memory, check Redis
        let redis_key = format!("mesh:price:{}", asset);
        let mut conn = self.redis.clone();
        
        match conn.get::<_, Option<String>>(&redis_key).await {
            Ok(Some(data_json)) => {
                match serde_json::from_str::<CachedPriceData>(&data_json) {
                    Ok(data) => {
                        // Add to in-memory cache for future fast lookups
                        let mut cache = self.cache.write().await;
                        cache.insert(asset.to_string(), data.clone());
                        self.metrics.record_cache_hit(asset).await;
                        Ok(Some(data))
                    }
                    Err(e) => {
                        tracing::warn!("Failed to deserialize cached price data: {}", e);
                        self.metrics.record_cache_miss(asset).await;
                        Ok(None)
                    }
                }
            }
            Ok(None) => {
                self.metrics.record_cache_miss(asset).await;
                Ok(None)
            }
            Err(e) => {
                tracing::warn!("Redis error getting price for {}: {}", asset, e);
                self.metrics.record_cache_miss(asset).await;
                Ok(None)
            }
        }
    }
    
    /// Get all cached prices
    /// 
    /// Returns all price data currently in the in-memory cache
    pub async fn get_all(&self) -> Result<HashMap<String, CachedPriceData>> {
        let cache = self.cache.read().await;
        Ok(cache.clone())
    }
}

impl PriceCache {
    /// Load cache from persistent storage on startup
    /// 
    /// Restores the in-memory cache from the database to maintain state across restarts
    pub async fn load_from_storage(&self) -> Result<()> {
        let client = self.db.get().await?;
        
        let rows = client
            .query(
                "SELECT asset, price, blockchain, timestamp, source_node_id, change_24h 
                 FROM mesh_price_cache 
                 ORDER BY timestamp DESC",
                &[],
            )
            .await?;
        
        let mut cache = self.cache.write().await;
        let mut loaded_count = 0;
        
        for row in rows {
            let asset: String = row.get(0);
            let price: String = row.get(1);
            let blockchain: String = row.get(2);
            let timestamp: DateTime<Utc> = row.get(3);
            let source_node_id: uuid::Uuid = row.get(4);
            let change_24h: Option<String> = row.get(5);
            
            let data = CachedPriceData {
                asset: asset.clone(),
                price,
                timestamp,
                source_node_id,
                blockchain,
                change_24h,
            };
            
            cache.insert(asset, data);
            loaded_count += 1;
        }
        
        tracing::info!("Loaded {} price entries from database", loaded_count);
        Ok(())
    }
    
    /// Persist cache to storage
    /// 
    /// Saves the current in-memory cache state to the database for durability
    pub async fn persist_to_storage(&self) -> Result<()> {
        let cache = self.cache.read().await;
        let client = self.db.get().await?;
        
        let mut persisted_count = 0;
        
        for (_asset, data) in cache.iter() {
            client
                .execute(
                    "INSERT INTO mesh_price_cache 
                     (asset, price, blockchain, timestamp, source_node_id, change_24h, updated_at)
                     VALUES ($1, $2, $3, $4, $5, $6, NOW())
                     ON CONFLICT (asset) 
                     DO UPDATE SET 
                         price = EXCLUDED.price,
                         blockchain = EXCLUDED.blockchain,
                         timestamp = EXCLUDED.timestamp,
                         source_node_id = EXCLUDED.source_node_id,
                         change_24h = EXCLUDED.change_24h,
                         updated_at = NOW()",
                    &[
                        &data.asset,
                        &data.price,
                        &data.blockchain,
                        &data.timestamp,
                        &data.source_node_id,
                        &data.change_24h,
                    ],
                )
                .await?;
            
            persisted_count += 1;
        }
        
        tracing::debug!("Persisted {} price entries to database", persisted_count);
        Ok(())
    }
    
    /// Calculate data freshness
    /// 
    /// Determines how old the price data is based on its timestamp
    pub fn calculate_freshness(&self, data: &CachedPriceData) -> DataFreshness {
        DataFreshness::from_timestamp(data.timestamp)
    }
    
    /// Check if data should be replaced with newer data
    /// 
    /// Compares timestamps to determine if new data is fresher than existing data
    pub fn should_replace(&self, existing: &CachedPriceData, new: &CachedPriceData) -> bool {
        new.timestamp > existing.timestamp
    }
    
    /// Calculate price discrepancy percentage between two price data points
    /// 
    /// Returns None if prices cannot be parsed as numbers
    /// Returns Some(percentage) representing the absolute percentage difference
    fn calculate_price_discrepancy(
        &self,
        existing: &CachedPriceData,
        new: &CachedPriceData,
    ) -> Option<f64> {
        // Parse prices as f64
        let existing_price = existing.price.parse::<f64>().ok()?;
        let new_price = new.price.parse::<f64>().ok()?;
        
        // Avoid division by zero
        if existing_price == 0.0 {
            return None;
        }
        
        // Calculate absolute percentage difference
        let difference = (new_price - existing_price).abs();
        let percentage = (difference / existing_price) * 100.0;
        
        Some(percentage)
    }
}
