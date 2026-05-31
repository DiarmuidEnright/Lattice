use database::RedisPool;
use redis::AsyncCommands;
use shared::{models::Portfolio, Error, Result};
use tracing::{debug, warn};

/// Portfolio cache service using Redis
/// 
/// Implements caching with 60-second TTL as per Requirements 1.5
pub struct PortfolioCache {
    redis_pool: RedisPool,
}

impl PortfolioCache {
    /// Create a new portfolio cache
    pub fn new(redis_pool: RedisPool) -> Self {
        Self { redis_pool }
    }
    
    /// Get cached portfolio for a wallet address
    /// 
    /// **Validates: Requirements 1.5**
    pub async fn get(&self, wallet_address: &str) -> Result<Option<Portfolio>> {
        let key = Self::cache_key(wallet_address);
        debug!("Fetching portfolio from cache: {}", key);
        
        let mut conn = self.redis_pool.clone();
        
        match conn.get::<_, String>(&key).await {
            Ok(json) => {
                match serde_json::from_str::<Portfolio>(&json) {
                    Ok(portfolio) => {
                        debug!("Cache hit for wallet: {}", wallet_address);
                        Ok(Some(portfolio))
                    }
                    Err(e) => {
                        warn!("Failed to deserialize cached portfolio: {}", e);
                        Ok(None)
                    }
                }
            }
            Err(e) => {
                if e.kind() == redis::ErrorKind::TypeError {
                    // Key doesn't exist
                    debug!("Cache miss for wallet: {}", wallet_address);
                    Ok(None)
                } else {
                    warn!("Redis error fetching portfolio: {}", e);
                    // Don't fail on cache errors, just return None
                    Ok(None)
                }
            }
        }
    }
    
    /// Set cached portfolio for a wallet address with 60-second TTL
    /// 
    /// **Validates: Requirements 1.5**
    pub async fn set(&self, wallet_address: &str, portfolio: &Portfolio) -> Result<()> {
        let key = Self::cache_key(wallet_address);
        debug!("Caching portfolio: {}", key);
        
        let json = serde_json::to_string(portfolio)
            .map_err(|e| Error::Internal(format!("Failed to serialize portfolio: {}", e)))?;
        
        let mut conn = self.redis_pool.clone();
        
        // Set with 60-second TTL as per requirements
        match conn.set_ex::<_, _, ()>(&key, json, 60).await {
            Ok(_) => {
                debug!("Successfully cached portfolio for wallet: {}", wallet_address);
                Ok(())
            }
            Err(e) => {
                warn!("Failed to cache portfolio: {}", e);
                // Don't fail on cache errors, just log
                Ok(())
            }
        }
    }
    
    /// Invalidate cached portfolio for a wallet address
    /// 
    /// **Validates: Requirements 1.5**
    pub async fn invalidate(&self, wallet_address: &str) -> Result<()> {
        let key = Self::cache_key(wallet_address);
        debug!("Invalidating portfolio cache: {}", key);
        
        let mut conn = self.redis_pool.clone();
        
        match conn.del::<_, ()>(&key).await {
            Ok(_) => {
                debug!("Successfully invalidated cache for wallet: {}", wallet_address);
                Ok(())
            }
            Err(e) => {
                warn!("Failed to invalidate cache: {}", e);
                // Don't fail on cache errors
                Ok(())
            }
        }
    }
    
    /// Generate Redis cache key for a wallet address
    fn cache_key(wallet_address: &str) -> String {
        format!("portfolio:{}", wallet_address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use shared::models::Asset;
    
    #[test]
    fn test_cache_key_generation() {
        let key = PortfolioCache::cache_key("test_wallet_123");
        assert_eq!(key, "portfolio:test_wallet_123");
    }
    
    #[tokio::test]
    #[ignore] // Only run with a real Redis instance
    async fn test_cache_set_and_get() {
        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://localhost:6379".to_string());
        
        let client = database::create_redis_client(&redis_url).await.unwrap();
        let pool = database::create_redis_pool(client).await.unwrap();
        let cache = PortfolioCache::new(pool);
        
        let portfolio = Portfolio {
            wallet_address: "test_wallet".to_string(),
            assets: vec![
                Asset {
                    token_mint: "SOL".to_string(),
                    token_symbol: "SOL".to_string(),
                    amount: "10.5".to_string(),
                    value_usd: Some(1050.0),
                }
            ],
            total_value_usd: 1050.0,
            last_updated: Utc::now(),
        };
        
        // Set cache
        let result = cache.set("test_wallet", &portfolio).await;
        assert!(result.is_ok());
        
        // Get cache
        let cached = cache.get("test_wallet").await;
        assert!(cached.is_ok());
        assert!(cached.unwrap().is_some());
    }
    
    #[tokio::test]
    #[ignore] // Only run with a real Redis instance
    async fn test_cache_miss() {
        let redis_url = std::env::var("REDIS_URL")
            .unwrap_or_else(|_| "redis://localhost:6379".to_string());
        
        let client = database::create_redis_client(&redis_url).await.unwrap();
        let pool = database::create_redis_pool(client).await.unwrap();
        let cache = PortfolioCache::new(pool);
        
        let cached = cache.get("nonexistent_wallet").await;
        assert!(cached.is_ok());
        assert!(cached.unwrap().is_none());
    }
}
