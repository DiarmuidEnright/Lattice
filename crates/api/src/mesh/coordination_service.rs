use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use redis::AsyncCommands;
use uuid::Uuid;

/// Coordinates fetch timing between multiple provider nodes to avoid rate limits
/// 
/// This service implements a distributed coordination mechanism using Redis to ensure
/// that when multiple provider nodes exist in the network, they don't all fetch data
/// simultaneously and exceed API rate limits. It uses a 5-second coordination window
/// where only one provider should fetch data.
pub struct CoordinationService {
    /// Redis connection for distributed coordination
    redis: redis::aio::ConnectionManager,
    /// Unique identifier for this node
    node_id: Uuid,
    /// Coordination window duration (5 seconds)
    coordination_window: Duration,
}

impl CoordinationService {
    /// Create a new CoordinationService with the specified Redis connection and node ID
    /// 
    /// # Arguments
    /// * `redis` - Redis connection manager for distributed coordination
    /// * `node_id` - Unique identifier for this provider node
    pub fn new(redis: redis::aio::ConnectionManager, node_id: Uuid) -> Self {
        Self {
            redis,
            node_id,
            coordination_window: Duration::seconds(5),
        }
    }
    
    /// Check if this node should fetch data now
    /// 
    /// Returns true if no other provider has fetched within the coordination window,
    /// or if this node was the last to fetch (allowing it to continue its schedule).
    /// 
    /// # Returns
    /// * `Ok(true)` - This node should proceed with fetching
    /// * `Ok(false)` - Another provider recently fetched, skip this cycle
    /// * `Err(_)` - Redis error occurred
    pub async fn should_fetch(&self) -> Result<bool> {
        let last_fetch_time = self.get_last_fetch_time().await?;
        
        match last_fetch_time {
            Some((last_node_id, timestamp)) => {
                let now = Utc::now();
                let elapsed = now.signed_duration_since(timestamp);
                
                // If this node was the last to fetch, allow it to continue
                if last_node_id == self.node_id {
                    return Ok(true);
                }
                
                // If another node fetched within the coordination window, skip
                if elapsed < self.coordination_window {
                    tracing::debug!(
                        "Skipping fetch - node {} fetched {} seconds ago",
                        last_node_id,
                        elapsed.num_seconds()
                    );
                    return Ok(false);
                }
                
                // Coordination window has passed, this node can fetch
                Ok(true)
            }
            None => {
                // No recent fetch recorded, this node can fetch
                Ok(true)
            }
        }
    }
    
    /// Record that this node is fetching data
    /// 
    /// Stores the current timestamp in Redis to coordinate with other providers.
    /// The record expires after 1 minute to prevent stale coordination data.
    pub async fn record_fetch(&self) -> Result<()> {
        let now = Utc::now();
        let redis_key = "mesh:coordination:last_fetch";
        
        // Store node ID and timestamp as JSON
        let record = serde_json::json!({
            "node_id": self.node_id,
            "timestamp": now.to_rfc3339(),
        });
        let record_str = serde_json::to_string(&record)?;
        
        let mut conn = self.redis.clone();
        // Set with 60-second expiration to prevent stale data
        let _: () = conn.set_ex(redis_key, record_str, 60).await?;
        
        tracing::debug!("Recorded fetch for node {} at {}", self.node_id, now);
        Ok(())
    }
    
    /// Get last fetch time from any provider
    /// 
    /// Retrieves the most recent fetch record from Redis, including which node
    /// performed the fetch and when.
    /// 
    /// # Returns
    /// * `Ok(Some((node_id, timestamp)))` - Last fetch info
    /// * `Ok(None)` - No recent fetch recorded
    /// * `Err(_)` - Redis error occurred
    pub async fn get_last_fetch_time(&self) -> Result<Option<(Uuid, DateTime<Utc>)>> {
        let redis_key = "mesh:coordination:last_fetch";
        let mut conn = self.redis.clone();
        
        match conn.get::<_, Option<String>>(redis_key).await {
            Ok(Some(record_str)) => {
                match serde_json::from_str::<serde_json::Value>(&record_str) {
                    Ok(record) => {
                        let node_id_str = record["node_id"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("Missing node_id in fetch record"))?;
                        let timestamp_str = record["timestamp"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("Missing timestamp in fetch record"))?;
                        
                        let node_id = Uuid::parse_str(node_id_str)?;
                        let timestamp = DateTime::parse_from_rfc3339(timestamp_str)?
                            .with_timezone(&Utc);
                        
                        Ok(Some((node_id, timestamp)))
                    }
                    Err(e) => {
                        tracing::warn!("Failed to parse fetch record: {}", e);
                        Ok(None)
                    }
                }
            }
            Ok(None) => Ok(None),
            Err(e) => {
                tracing::warn!("Redis error getting last fetch time: {}", e);
                // On Redis error, return None to allow fetching
                Ok(None)
            }
        }
    }
    
    /// Clean up stale coordination records
    /// 
    /// Removes old coordination records from Redis. This is primarily for maintenance
    /// as records have TTL, but can be called explicitly if needed.
    /// 
    /// Note: With the current implementation using a single key with TTL,
    /// this method doesn't need to do much. It's here for future extensibility
    /// if we add more coordination keys.
    pub async fn cleanup_stale_records(&self) -> Result<()> {
        // With the current single-key implementation with TTL, Redis handles cleanup
        // This method is here for future extensibility
        tracing::debug!("Coordination cleanup called (Redis TTL handles expiration)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio;
    
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
    async fn test_should_fetch_no_previous_fetch() {
        let redis = create_test_redis().await;
        let node_id = Uuid::new_v4();
        let service = CoordinationService::new(redis, node_id);
        
        // Clean up any previous test data
        let mut conn = service.redis.clone();
        let _: () = conn.del("mesh:coordination:last_fetch").await.unwrap();
        
        // Should return true when no previous fetch exists
        let result = service.should_fetch().await.unwrap();
        assert!(result, "Should allow fetch when no previous fetch exists");
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_record_and_retrieve_fetch() {
        let redis = create_test_redis().await;
        let node_id = Uuid::new_v4();
        let service = CoordinationService::new(redis, node_id);
        
        // Record a fetch
        service.record_fetch().await.unwrap();
        
        // Retrieve the fetch time
        let result = service.get_last_fetch_time().await.unwrap();
        assert!(result.is_some(), "Should retrieve recorded fetch");
        
        let (retrieved_node_id, timestamp) = result.unwrap();
        assert_eq!(retrieved_node_id, node_id, "Node ID should match");
        
        // Timestamp should be recent (within last second)
        let now = Utc::now();
        let elapsed = now.signed_duration_since(timestamp);
        assert!(
            elapsed.num_seconds() < 2,
            "Timestamp should be recent"
        );
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_coordination_window_same_node() {
        let redis = create_test_redis().await;
        let node_id = Uuid::new_v4();
        let service = CoordinationService::new(redis, node_id);
        
        // Record a fetch
        service.record_fetch().await.unwrap();
        
        // Same node should be allowed to fetch again immediately
        let result = service.should_fetch().await.unwrap();
        assert!(result, "Same node should be allowed to fetch again");
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_coordination_window_different_node() {
        let redis = create_test_redis().await;
        let node1_id = Uuid::new_v4();
        let node2_id = Uuid::new_v4();
        
        let service1 = CoordinationService::new(redis.clone(), node1_id);
        let service2 = CoordinationService::new(redis, node2_id);
        
        // Node 1 records a fetch
        service1.record_fetch().await.unwrap();
        
        // Node 2 should not be allowed to fetch within coordination window
        let result = service2.should_fetch().await.unwrap();
        assert!(!result, "Different node should not fetch within coordination window");
    }
    
    #[tokio::test]
    #[ignore] // Requires Redis connection
    async fn test_coordination_window_expires() {
        let redis = create_test_redis().await;
        let node1_id = Uuid::new_v4();
        let node2_id = Uuid::new_v4();
        
        let service1 = CoordinationService::new(redis.clone(), node1_id);
        let service2 = CoordinationService::new(redis, node2_id);
        
        // Node 1 records a fetch
        service1.record_fetch().await.unwrap();
        
        // Wait for coordination window to expire (6 seconds > 5 second window)
        tokio::time::sleep(tokio::time::Duration::from_secs(6)).await;
        
        // Node 2 should now be allowed to fetch
        let result = service2.should_fetch().await.unwrap();
        assert!(result, "Different node should fetch after coordination window expires");
    }
}
