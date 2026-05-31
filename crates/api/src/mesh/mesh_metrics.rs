use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Metrics collector for mesh network operations
/// 
/// Tracks:
/// - Message propagation latency
/// - Cache hit/miss rates
/// - Provider fetch success/failure rates
/// - Peer connection counts
/// - Validation failure rates
/// 
/// Requirement: 14.5
#[derive(Clone)]
pub struct MeshMetricsCollector {
    metrics: Arc<RwLock<MeshMetrics>>,
}

impl MeshMetricsCollector {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(MeshMetrics::default())),
        }
    }

    /// Record message propagation latency
    /// 
    /// Tracks how long it takes for a message to propagate through the network.
    /// This helps identify network performance issues.
    pub async fn record_message_propagation(&self, message_id: Uuid, latency_ms: u64) {
        let mut metrics = self.metrics.write().await;
        metrics.message_propagation.total_messages += 1;
        metrics.message_propagation.total_latency_ms += latency_ms;
        metrics.message_propagation.avg_latency_ms = 
            metrics.message_propagation.total_latency_ms / metrics.message_propagation.total_messages;
        
        if latency_ms > metrics.message_propagation.max_latency_ms {
            metrics.message_propagation.max_latency_ms = latency_ms;
        }
        
        if metrics.message_propagation.min_latency_ms == 0 || latency_ms < metrics.message_propagation.min_latency_ms {
            metrics.message_propagation.min_latency_ms = latency_ms;
        }
        
        tracing::debug!(
            message_id = %message_id,
            latency_ms = latency_ms,
            avg_latency_ms = metrics.message_propagation.avg_latency_ms,
            "Recorded message propagation latency"
        );
    }

    /// Record cache hit
    /// 
    /// Tracks successful cache lookups to measure cache effectiveness.
    pub async fn record_cache_hit(&self, asset: &str) {
        let mut metrics = self.metrics.write().await;
        metrics.cache_stats.hits += 1;
        metrics.cache_stats.total_requests += 1;
        metrics.cache_stats.hit_rate = 
            (metrics.cache_stats.hits as f64 / metrics.cache_stats.total_requests as f64) * 100.0;
        
        tracing::trace!(
            asset = %asset,
            hit_rate = metrics.cache_stats.hit_rate,
            "Cache hit"
        );
    }

    /// Record cache miss
    /// 
    /// Tracks failed cache lookups to measure cache effectiveness.
    pub async fn record_cache_miss(&self, asset: &str) {
        let mut metrics = self.metrics.write().await;
        metrics.cache_stats.misses += 1;
        metrics.cache_stats.total_requests += 1;
        metrics.cache_stats.hit_rate = 
            (metrics.cache_stats.hits as f64 / metrics.cache_stats.total_requests as f64) * 100.0;
        
        tracing::trace!(
            asset = %asset,
            hit_rate = metrics.cache_stats.hit_rate,
            "Cache miss"
        );
    }

    /// Record provider fetch success
    /// 
    /// Tracks successful API fetches from provider nodes.
    pub async fn record_provider_fetch_success(&self, provider_id: Uuid, duration_ms: u64, asset_count: usize) {
        let mut metrics = self.metrics.write().await;
        metrics.provider_stats.total_fetches += 1;
        metrics.provider_stats.successful_fetches += 1;
        metrics.provider_stats.total_assets_fetched += asset_count as u64;
        metrics.provider_stats.success_rate = 
            (metrics.provider_stats.successful_fetches as f64 / metrics.provider_stats.total_fetches as f64) * 100.0;
        
        metrics.provider_stats.last_successful_fetch = Some(Utc::now());
        
        tracing::info!(
            provider_id = %provider_id,
            duration_ms = duration_ms,
            asset_count = asset_count,
            success_rate = metrics.provider_stats.success_rate,
            "Provider fetch successful"
        );
    }

    /// Record provider fetch failure
    /// 
    /// Tracks failed API fetches from provider nodes with error categorization.
    pub async fn record_provider_fetch_failure(&self, provider_id: Uuid, error_type: &str) {
        let mut metrics = self.metrics.write().await;
        metrics.provider_stats.total_fetches += 1;
        metrics.provider_stats.failed_fetches += 1;
        metrics.provider_stats.success_rate = 
            (metrics.provider_stats.successful_fetches as f64 / metrics.provider_stats.total_fetches as f64) * 100.0;
        
        *metrics.provider_stats.error_counts.entry(error_type.to_string()).or_insert(0) += 1;
        metrics.provider_stats.last_failed_fetch = Some(Utc::now());
        
        tracing::warn!(
            provider_id = %provider_id,
            error_type = %error_type,
            success_rate = metrics.provider_stats.success_rate,
            "Provider fetch failed"
        );
    }

    /// Record peer connection count
    /// 
    /// Tracks the number of active peer connections over time.
    pub async fn record_peer_count(&self, count: usize) {
        let mut metrics = self.metrics.write().await;
        metrics.peer_stats.current_connections = count;
        
        if count > metrics.peer_stats.max_connections {
            metrics.peer_stats.max_connections = count;
        }
        
        tracing::debug!(
            peer_count = count,
            max_connections = metrics.peer_stats.max_connections,
            "Updated peer connection count"
        );
    }

    /// Record peer connection established
    pub async fn record_peer_connected(&self, peer_id: String) {
        let mut metrics = self.metrics.write().await;
        metrics.peer_stats.total_connections += 1;
        
        tracing::info!(
            peer_id = %peer_id,
            total_connections = metrics.peer_stats.total_connections,
            "Peer connected"
        );
    }

    /// Record peer disconnection
    pub async fn record_peer_disconnected(&self, peer_id: String) {
        let mut metrics = self.metrics.write().await;
        metrics.peer_stats.total_disconnections += 1;
        
        tracing::info!(
            peer_id = %peer_id,
            total_disconnections = metrics.peer_stats.total_disconnections,
            "Peer disconnected"
        );
    }

    /// Record validation failure
    /// 
    /// Tracks price update validation failures with categorization.
    /// This is important for security monitoring.
    pub async fn record_validation_failure(&self, source_node_id: Uuid, reason: &str) {
        let mut metrics = self.metrics.write().await;
        metrics.validation_stats.total_validations += 1;
        metrics.validation_stats.failed_validations += 1;
        metrics.validation_stats.failure_rate = 
            (metrics.validation_stats.failed_validations as f64 / metrics.validation_stats.total_validations as f64) * 100.0;
        
        *metrics.validation_stats.failure_reasons.entry(reason.to_string()).or_insert(0) += 1;
        *metrics.validation_stats.failures_by_node.entry(source_node_id).or_insert(0) += 1;
        
        tracing::warn!(
            source_node_id = %source_node_id,
            reason = %reason,
            failure_rate = metrics.validation_stats.failure_rate,
            "Validation failure"
        );
    }

    /// Record validation success
    pub async fn record_validation_success(&self) {
        let mut metrics = self.metrics.write().await;
        metrics.validation_stats.total_validations += 1;
        metrics.validation_stats.successful_validations += 1;
        metrics.validation_stats.failure_rate = 
            (metrics.validation_stats.failed_validations as f64 / metrics.validation_stats.total_validations as f64) * 100.0;
    }

    /// Get current metrics snapshot
    pub async fn get_metrics(&self) -> MeshMetrics {
        self.metrics.read().await.clone()
    }

    /// Reset all metrics (useful for testing or periodic resets)
    pub async fn reset(&self) {
        let mut metrics = self.metrics.write().await;
        *metrics = MeshMetrics::default();
        tracing::info!("Mesh metrics reset");
    }

    /// Get metrics summary for display
    pub async fn get_summary(&self) -> MeshMetricsSummary {
        let metrics = self.metrics.read().await;
        
        MeshMetricsSummary {
            message_propagation: MessagePropagationSummary {
                total_messages: metrics.message_propagation.total_messages,
                avg_latency_ms: metrics.message_propagation.avg_latency_ms,
                min_latency_ms: metrics.message_propagation.min_latency_ms,
                max_latency_ms: metrics.message_propagation.max_latency_ms,
            },
            cache: CacheSummary {
                hit_rate: metrics.cache_stats.hit_rate,
                total_requests: metrics.cache_stats.total_requests,
                hits: metrics.cache_stats.hits,
                misses: metrics.cache_stats.misses,
            },
            provider: ProviderSummary {
                success_rate: metrics.provider_stats.success_rate,
                total_fetches: metrics.provider_stats.total_fetches,
                successful_fetches: metrics.provider_stats.successful_fetches,
                failed_fetches: metrics.provider_stats.failed_fetches,
                total_assets_fetched: metrics.provider_stats.total_assets_fetched,
            },
            peers: PeerSummary {
                current_connections: metrics.peer_stats.current_connections,
                max_connections: metrics.peer_stats.max_connections,
                total_connections: metrics.peer_stats.total_connections,
                total_disconnections: metrics.peer_stats.total_disconnections,
            },
            validation: ValidationSummary {
                failure_rate: metrics.validation_stats.failure_rate,
                total_validations: metrics.validation_stats.total_validations,
                successful_validations: metrics.validation_stats.successful_validations,
                failed_validations: metrics.validation_stats.failed_validations,
            },
        }
    }
}

impl Default for MeshMetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete mesh network metrics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MeshMetrics {
    pub message_propagation: MessagePropagationMetrics,
    pub cache_stats: CacheMetrics,
    pub provider_stats: ProviderMetrics,
    pub peer_stats: PeerMetrics,
    pub validation_stats: ValidationMetrics,
}

/// Message propagation metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePropagationMetrics {
    pub total_messages: u64,
    pub total_latency_ms: u64,
    pub avg_latency_ms: u64,
    pub min_latency_ms: u64,
    pub max_latency_ms: u64,
}

impl Default for MessagePropagationMetrics {
    fn default() -> Self {
        Self {
            total_messages: 0,
            total_latency_ms: 0,
            avg_latency_ms: 0,
            min_latency_ms: 0,
            max_latency_ms: 0,
        }
    }
}

/// Cache performance metrics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CacheMetrics {
    pub total_requests: u64,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
}

/// Provider fetch metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderMetrics {
    pub total_fetches: u64,
    pub successful_fetches: u64,
    pub failed_fetches: u64,
    pub success_rate: f64,
    pub total_assets_fetched: u64,
    pub error_counts: HashMap<String, u64>,
    pub last_successful_fetch: Option<DateTime<Utc>>,
    pub last_failed_fetch: Option<DateTime<Utc>>,
}

impl Default for ProviderMetrics {
    fn default() -> Self {
        Self {
            total_fetches: 0,
            successful_fetches: 0,
            failed_fetches: 0,
            success_rate: 0.0,
            total_assets_fetched: 0,
            error_counts: HashMap::new(),
            last_successful_fetch: None,
            last_failed_fetch: None,
        }
    }
}

/// Peer connection metrics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PeerMetrics {
    pub current_connections: usize,
    pub max_connections: usize,
    pub total_connections: u64,
    pub total_disconnections: u64,
}

/// Validation metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationMetrics {
    pub total_validations: u64,
    pub successful_validations: u64,
    pub failed_validations: u64,
    pub failure_rate: f64,
    pub failure_reasons: HashMap<String, u64>,
    pub failures_by_node: HashMap<Uuid, u64>,
}

impl Default for ValidationMetrics {
    fn default() -> Self {
        Self {
            total_validations: 0,
            successful_validations: 0,
            failed_validations: 0,
            failure_rate: 0.0,
            failure_reasons: HashMap::new(),
            failures_by_node: HashMap::new(),
        }
    }
}

/// Simplified metrics summary for API responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshMetricsSummary {
    pub message_propagation: MessagePropagationSummary,
    pub cache: CacheSummary,
    pub provider: ProviderSummary,
    pub peers: PeerSummary,
    pub validation: ValidationSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePropagationSummary {
    pub total_messages: u64,
    pub avg_latency_ms: u64,
    pub min_latency_ms: u64,
    pub max_latency_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheSummary {
    pub hit_rate: f64,
    pub total_requests: u64,
    pub hits: u64,
    pub misses: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSummary {
    pub success_rate: f64,
    pub total_fetches: u64,
    pub successful_fetches: u64,
    pub failed_fetches: u64,
    pub total_assets_fetched: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerSummary {
    pub current_connections: usize,
    pub max_connections: usize,
    pub total_connections: u64,
    pub total_disconnections: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationSummary {
    pub failure_rate: f64,
    pub total_validations: u64,
    pub successful_validations: u64,
    pub failed_validations: u64,
}

/// Timer for measuring operation duration
pub struct MeshOperationTimer {
    start: Instant,
    operation: String,
    metrics: MeshMetricsCollector,
}

impl MeshOperationTimer {
    pub fn new(operation: String, metrics: MeshMetricsCollector) -> Self {
        Self {
            start: Instant::now(),
            operation,
            metrics,
        }
    }

    /// Get elapsed time in milliseconds
    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    /// Complete the timer and record message propagation
    pub async fn complete_propagation(self, message_id: Uuid) {
        let duration_ms = self.elapsed_ms();
        tracing::debug!(
            operation = %self.operation,
            message_id = %message_id,
            duration_ms = duration_ms,
            "Completed mesh operation, recording propagation latency"
        );
        self.metrics.record_message_propagation(message_id, duration_ms).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_message_propagation_metrics() {
        let collector = MeshMetricsCollector::new();
        let message_id = Uuid::new_v4();
        
        collector.record_message_propagation(message_id, 100).await;
        collector.record_message_propagation(Uuid::new_v4(), 200).await;
        
        let metrics = collector.get_metrics().await;
        assert_eq!(metrics.message_propagation.total_messages, 2);
        assert_eq!(metrics.message_propagation.avg_latency_ms, 150);
        assert_eq!(metrics.message_propagation.min_latency_ms, 100);
        assert_eq!(metrics.message_propagation.max_latency_ms, 200);
    }

    #[tokio::test]
    async fn test_cache_metrics() {
        let collector = MeshMetricsCollector::new();
        
        collector.record_cache_hit("SOL").await;
        collector.record_cache_hit("BTC").await;
        collector.record_cache_miss("ETH").await;
        
        let metrics = collector.get_metrics().await;
        assert_eq!(metrics.cache_stats.total_requests, 3);
        assert_eq!(metrics.cache_stats.hits, 2);
        assert_eq!(metrics.cache_stats.misses, 1);
        assert!((metrics.cache_stats.hit_rate - 66.67).abs() < 0.1);
    }

    #[tokio::test]
    async fn test_provider_metrics() {
        let collector = MeshMetricsCollector::new();
        let provider_id = Uuid::new_v4();
        
        collector.record_provider_fetch_success(provider_id, 500, 10).await;
        collector.record_provider_fetch_failure(provider_id, "timeout").await;
        
        let metrics = collector.get_metrics().await;
        assert_eq!(metrics.provider_stats.total_fetches, 2);
        assert_eq!(metrics.provider_stats.successful_fetches, 1);
        assert_eq!(metrics.provider_stats.failed_fetches, 1);
        assert_eq!(metrics.provider_stats.success_rate, 50.0);
        assert_eq!(metrics.provider_stats.total_assets_fetched, 10);
    }

    #[tokio::test]
    async fn test_peer_metrics() {
        let collector = MeshMetricsCollector::new();
        
        collector.record_peer_connected("peer1".to_string()).await;
        collector.record_peer_connected("peer2".to_string()).await;
        collector.record_peer_count(2).await;
        collector.record_peer_disconnected("peer1".to_string()).await;
        collector.record_peer_count(1).await;
        
        let metrics = collector.get_metrics().await;
        assert_eq!(metrics.peer_stats.current_connections, 1);
        assert_eq!(metrics.peer_stats.max_connections, 2);
        assert_eq!(metrics.peer_stats.total_connections, 2);
        assert_eq!(metrics.peer_stats.total_disconnections, 1);
    }

    #[tokio::test]
    async fn test_validation_metrics() {
        let collector = MeshMetricsCollector::new();
        let node_id = Uuid::new_v4();
        
        collector.record_validation_success().await;
        collector.record_validation_failure(node_id, "invalid_price").await;
        
        let metrics = collector.get_metrics().await;
        assert_eq!(metrics.validation_stats.total_validations, 2);
        assert_eq!(metrics.validation_stats.successful_validations, 1);
        assert_eq!(metrics.validation_stats.failed_validations, 1);
        assert_eq!(metrics.validation_stats.failure_rate, 50.0);
    }

    #[tokio::test]
    async fn test_metrics_summary() {
        let collector = MeshMetricsCollector::new();
        
        collector.record_message_propagation(Uuid::new_v4(), 100).await;
        collector.record_cache_hit("SOL").await;
        collector.record_provider_fetch_success(Uuid::new_v4(), 500, 5).await;
        collector.record_peer_count(3).await;
        collector.record_validation_success().await;
        
        let summary = collector.get_summary().await;
        assert_eq!(summary.message_propagation.total_messages, 1);
        assert_eq!(summary.cache.hits, 1);
        assert_eq!(summary.provider.successful_fetches, 1);
        assert_eq!(summary.peers.current_connections, 3);
        assert_eq!(summary.validation.successful_validations, 1);
    }
}
