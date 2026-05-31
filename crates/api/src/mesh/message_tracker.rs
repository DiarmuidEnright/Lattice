use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use lru::LruCache;
use redis::AsyncCommands;
use std::num::NonZeroUsize;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

/// Tracks seen messages to prevent duplicate processing in the mesh network
pub struct MessageTracker {
    /// In-memory LRU cache for fast lookups
    seen_messages: Arc<RwLock<LruCache<Uuid, DateTime<Utc>>>>,
    /// Redis connection for persistence
    redis: redis::aio::ConnectionManager,
    /// Expiration time for seen messages (5 minutes)
    expiration: Duration,
}

impl MessageTracker {
    /// Create a new MessageTracker with the specified Redis connection
    pub fn new(redis: redis::aio::ConnectionManager) -> Self {
        // Create LRU cache with 10,000 entry limit
        let cache_size = NonZeroUsize::new(10_000).unwrap();
        let seen_messages = Arc::new(RwLock::new(LruCache::new(cache_size)));
        
        Self {
            seen_messages,
            redis,
            expiration: Duration::minutes(5),
        }
    }
    
    /// Check if a message has been seen before
    /// 
    /// Returns true if the message ID exists in the cache or Redis
    pub async fn has_seen(&self, message_id: &Uuid) -> bool {
        // First check in-memory cache for fast lookup
        {
            let cache = self.seen_messages.read().await;
            if cache.contains(message_id) {
                return true;
            }
        }
        
        // If not in memory, check Redis
        let redis_key = format!("mesh:seen:{}", message_id);
        match self.redis.clone().exists::<_, bool>(&redis_key).await {
            Ok(exists) => {
                if exists {
                    // Add to in-memory cache for future fast lookups
                    let mut cache = self.seen_messages.write().await;
                    cache.put(*message_id, Utc::now());
                    true
                } else {
                    false
                }
            }
            Err(e) => {
                tracing::warn!("Redis error checking message {}: {}", message_id, e);
                // On Redis error, assume not seen to avoid blocking messages
                false
            }
        }
    }
    
    /// Mark a message as seen with 5-minute expiration
    /// 
    /// Stores the message ID in both the in-memory cache and Redis
    pub async fn mark_seen(&self, message_id: Uuid) -> Result<()> {
        let now = Utc::now();
        
        // Add to in-memory cache
        {
            let mut cache = self.seen_messages.write().await;
            cache.put(message_id, now);
        }
        
        // Add to Redis with expiration
        let redis_key = format!("mesh:seen:{}", message_id);
        let expiration_secs = self.expiration.num_seconds() as u64;
        
        let mut conn = self.redis.clone();
        let _: () = conn.set_ex(&redis_key, now.to_rfc3339(), expiration_secs).await?;
        
        Ok(())
    }
    
    /// Load seen messages from Redis cache on startup
    /// 
    /// Restores the in-memory cache from Redis to maintain state across restarts
    pub async fn load_from_cache(&self) -> Result<()> {
        let mut conn = self.redis.clone();
        
        // Scan for all mesh:seen:* keys
        let pattern = "mesh:seen:*";
        let mut cursor = 0u64;
        let mut loaded_count = 0;
        
        loop {
            let (new_cursor, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(100)
                .query_async(&mut conn)
                .await?;
            
            // Load each key's value and add to in-memory cache
            for key in keys {
                // Extract UUID from key (format: mesh:seen:{uuid})
                if let Some(uuid_str) = key.strip_prefix("mesh:seen:") {
                    if let Ok(message_id) = Uuid::parse_str(uuid_str) {
                        // Get the timestamp value
                        match conn.get::<_, Option<String>>(&key).await {
                            Ok(Some(timestamp_str)) => {
                                if let Ok(timestamp) = DateTime::parse_from_rfc3339(&timestamp_str) {
                                    let mut cache = self.seen_messages.write().await;
                                    cache.put(message_id, timestamp.with_timezone(&Utc));
                                    loaded_count += 1;
                                }
                            }
                            Ok(None) => {
                                // Key expired between scan and get, ignore
                            }
                            Err(e) => {
                                tracing::warn!("Error loading message {}: {}", message_id, e);
                            }
                        }
                    }
                }
            }
            
            cursor = new_cursor;
            if cursor == 0 {
                break;
            }
        }
        
        tracing::info!("Loaded {} seen messages from Redis cache", loaded_count);
        Ok(())
    }
    
    /// Persist seen messages to Redis cache
    /// 
    /// Saves the current in-memory cache state to Redis for durability
    pub async fn persist_to_cache(&self) -> Result<()> {
        let cache = self.seen_messages.read().await;
        let mut conn = self.redis.clone();
        
        let mut persisted_count = 0;
        
        // Iterate through all entries in the LRU cache
        for (message_id, timestamp) in cache.iter() {
            let redis_key = format!("mesh:seen:{}", message_id);
            
            // Calculate remaining TTL based on when the message was seen
            let now = Utc::now();
            let age = now.signed_duration_since(*timestamp);
            let remaining_ttl = self.expiration - age;
            
            // Only persist if there's still time left
            if remaining_ttl.num_seconds() > 0 {
                let ttl_secs = remaining_ttl.num_seconds() as u64;
                let _: () = conn.set_ex(&redis_key, timestamp.to_rfc3339(), ttl_secs).await?;
                persisted_count += 1;
            }
        }
        
        tracing::debug!("Persisted {} seen messages to Redis", persisted_count);
        Ok(())
    }
    
    /// Clean up expired entries from the in-memory cache
    /// 
    /// Removes entries older than the expiration time to free memory
    /// Returns the number of entries removed
    pub async fn cleanup_expired(&self) -> Result<usize> {
        let mut cache = self.seen_messages.write().await;
        let now = Utc::now();
        let mut removed_count = 0;
        
        // Collect expired message IDs
        let expired_ids: Vec<Uuid> = cache
            .iter()
            .filter_map(|(id, timestamp)| {
                let age = now.signed_duration_since(*timestamp);
                if age > self.expiration {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect();
        
        // Remove expired entries
        for id in expired_ids {
            cache.pop(&id);
            removed_count += 1;
        }
        
        if removed_count > 0 {
            tracing::debug!("Cleaned up {} expired message entries", removed_count);
        }
        
        Ok(removed_count)
    }
}
