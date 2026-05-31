use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use redis::AsyncCommands;
use std::sync::Arc;
use tracing::warn;

use crate::AppState;

/// Rate limit configuration
#[derive(Clone)]
pub struct RateLimitConfig {
    pub requests_per_minute: u32,
    pub window_seconds: u64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 60,
            window_seconds: 60,
        }
    }
}

/// Rate limiting middleware using token bucket algorithm
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract user ID from request (simplified - in production, extract from JWT)
    let user_id = req
        .headers()
        .get("X-User-ID")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("anonymous");

    // Get endpoint path for per-endpoint rate limiting
    let path = req.uri().path();
    let rate_limit_key = format!("ratelimit:{}:{}", user_id, path);

    // Check rate limit
    let mut redis_conn = state.redis_pool.clone();
    
    match check_rate_limit(&mut redis_conn, &rate_limit_key).await {
        Ok(allowed) => {
            if allowed {
                Ok(next.run(req).await)
            } else {
                warn!("Rate limit exceeded for user {} on path {}", user_id, path);
                Err(StatusCode::TOO_MANY_REQUESTS)
            }
        }
        Err(e) => {
            warn!("Rate limit check failed: {:?}", e);
            // On Redis error, allow request (fail open)
            Ok(next.run(req).await)
        }
    }
}

/// Check rate limit using Redis
async fn check_rate_limit(
    redis_conn: &mut redis::aio::ConnectionManager,
    key: &str,
) -> Result<bool, redis::RedisError> {
    // Get current count
    let count: Option<u32> = redis_conn.get(key).await?;

    let config = RateLimitConfig::default();

    match count {
        Some(current) if current >= config.requests_per_minute => {
            // Rate limit exceeded
            Ok(false)
        }
        Some(_current) => {
            // Increment counter
            let _: () = redis_conn.incr(key, 1).await?;
            Ok(true)
        }
        None => {
            // First request in window
            let _: () = redis_conn.set_ex(key, 1, config.window_seconds).await?;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limit_config_default() {
        let config = RateLimitConfig::default();
        assert_eq!(config.requests_per_minute, 60);
        assert_eq!(config.window_seconds, 60);
    }

    #[test]
    fn test_rate_limit_key_format() {
        let user_id = "user123";
        let path = "/api/wallets";
        let key = format!("ratelimit:{}:{}", user_id, path);
        assert_eq!(key, "ratelimit:user123:/api/wallets");
    }
}
