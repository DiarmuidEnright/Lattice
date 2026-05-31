use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metrics collector for monitoring service health
#[derive(Clone)]
pub struct MetricsCollector {
    metrics: Arc<RwLock<ServiceMetrics>>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(RwLock::new(ServiceMetrics::default())),
        }
    }

    /// Record a successful API call
    pub async fn record_success(&self, service: &str, duration_ms: u64) {
        let mut metrics = self.metrics.write().await;
        let service_metrics = metrics.services.entry(service.to_string()).or_default();
        
        service_metrics.total_requests += 1;
        service_metrics.successful_requests += 1;
        service_metrics.total_duration_ms += duration_ms;
        service_metrics.last_success = Some(chrono::Utc::now());
        
        // Update average response time
        service_metrics.avg_response_time_ms = 
            service_metrics.total_duration_ms / service_metrics.successful_requests;
    }

    /// Record a failed API call
    pub async fn record_failure(&self, service: &str, error_type: &str) {
        let mut metrics = self.metrics.write().await;
        let service_metrics = metrics.services.entry(service.to_string()).or_default();
        
        service_metrics.total_requests += 1;
        service_metrics.failed_requests += 1;
        service_metrics.last_failure = Some(chrono::Utc::now());
        
        *service_metrics.error_counts.entry(error_type.to_string()).or_insert(0) += 1;
        
        // Calculate error rate
        service_metrics.error_rate = 
            (service_metrics.failed_requests as f64 / service_metrics.total_requests as f64) * 100.0;
        
        // Alert if error rate exceeds threshold
        if service_metrics.error_rate > 10.0 {
            warn!(
                "High error rate detected for {}: {:.2}% ({}/{})",
                service,
                service_metrics.error_rate,
                service_metrics.failed_requests,
                service_metrics.total_requests
            );
        }
    }

    /// Record circuit breaker state change
    pub async fn record_circuit_breaker_state(&self, service: &str, state: &str) {
        let mut metrics = self.metrics.write().await;
        let service_metrics = metrics.services.entry(service.to_string()).or_default();
        
        service_metrics.circuit_breaker_state = state.to_string();
        
        if state == "open" {
            error!("Circuit breaker OPEN for service: {}", service);
        } else if state == "closed" {
            info!("Circuit breaker CLOSED for service: {}", service);
        }
    }

    /// Get current metrics snapshot
    pub async fn get_metrics(&self) -> ServiceMetrics {
        self.metrics.read().await.clone()
    }

    /// Get metrics for a specific service
    pub async fn get_service_metrics(&self, service: &str) -> Option<ServiceMetric> {
        let metrics = self.metrics.read().await;
        metrics.services.get(service).cloned()
    }

    /// Reset metrics (useful for testing or periodic resets)
    pub async fn reset(&self) {
        let mut metrics = self.metrics.write().await;
        *metrics = ServiceMetrics::default();
    }

    /// Check service health
    pub async fn check_health(&self, service: &str) -> HealthStatus {
        let metrics = self.metrics.read().await;
        
        if let Some(service_metrics) = metrics.services.get(service) {
            // Check if circuit breaker is open
            if service_metrics.circuit_breaker_state == "open" {
                return HealthStatus::Unhealthy {
                    reason: "Circuit breaker is open".to_string(),
                };
            }
            
            // Check error rate
            if service_metrics.error_rate > 50.0 {
                return HealthStatus::Degraded {
                    reason: format!("High error rate: {:.2}%", service_metrics.error_rate),
                };
            }
            
            // Check if service has been responding
            if let Some(last_success) = service_metrics.last_success {
                let elapsed = chrono::Utc::now() - last_success;
                if elapsed.num_minutes() > 5 {
                    return HealthStatus::Degraded {
                        reason: format!("No successful requests in {} minutes", elapsed.num_minutes()),
                    };
                }
            }
            
            HealthStatus::Healthy
        } else {
            HealthStatus::Unknown
        }
    }
}

impl Default for MetricsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Overall service metrics
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceMetrics {
    pub services: HashMap<String, ServiceMetric>,
}

/// Metrics for a single service
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMetric {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub error_rate: f64,
    pub avg_response_time_ms: u64,
    pub total_duration_ms: u64,
    pub circuit_breaker_state: String,
    pub error_counts: HashMap<String, u64>,
    pub last_success: Option<chrono::DateTime<chrono::Utc>>,
    pub last_failure: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for ServiceMetric {
    fn default() -> Self {
        Self {
            total_requests: 0,
            successful_requests: 0,
            failed_requests: 0,
            error_rate: 0.0,
            avg_response_time_ms: 0,
            total_duration_ms: 0,
            circuit_breaker_state: "closed".to_string(),
            error_counts: HashMap::new(),
            last_success: None,
            last_failure: None,
        }
    }
}

/// Health status for a service
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
    Unknown,
}

/// Request timer for measuring operation duration
pub struct RequestTimer {
    start: Instant,
    service: String,
    metrics: MetricsCollector,
}

impl RequestTimer {
    pub fn new(service: String, metrics: MetricsCollector) -> Self {
        Self {
            start: Instant::now(),
            service,
            metrics,
        }
    }

    /// Complete the timer and record success
    pub async fn success(self) {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        self.metrics.record_success(&self.service, duration_ms).await;
    }

    /// Complete the timer and record failure
    pub async fn failure(self, error_type: &str) {
        self.metrics.record_failure(&self.service, error_type).await;
    }
}

/// Alert manager for sending notifications about service issues
pub struct AlertManager {
    alert_threshold: f64,
    metrics: MetricsCollector,
}

impl AlertManager {
    pub fn new(metrics: MetricsCollector) -> Self {
        Self {
            alert_threshold: 10.0, // Alert if error rate exceeds 10%
            metrics,
        }
    }

    /// Check all services and send alerts if needed
    pub async fn check_and_alert(&self) {
        let metrics = self.metrics.get_metrics().await;
        
        for (service_name, service_metrics) in metrics.services.iter() {
            // Check error rate
            if service_metrics.error_rate > self.alert_threshold {
                self.send_alert(
                    service_name,
                    AlertLevel::Warning,
                    &format!(
                        "High error rate: {:.2}% ({}/{})",
                        service_metrics.error_rate,
                        service_metrics.failed_requests,
                        service_metrics.total_requests
                    ),
                ).await;
            }
            
            // Check circuit breaker state
            if service_metrics.circuit_breaker_state == "open" {
                self.send_alert(
                    service_name,
                    AlertLevel::Critical,
                    "Circuit breaker is OPEN - service unavailable",
                ).await;
            }
            
            // Check response time
            if service_metrics.avg_response_time_ms > 5000 {
                self.send_alert(
                    service_name,
                    AlertLevel::Warning,
                    &format!(
                        "Slow response time: {}ms average",
                        service_metrics.avg_response_time_ms
                    ),
                ).await;
            }
        }
    }

    /// Send an alert (in production, this would integrate with PagerDuty, Slack, etc.)
    async fn send_alert(&self, service: &str, level: AlertLevel, message: &str) {
        match level {
            AlertLevel::Critical => {
                error!("🚨 CRITICAL ALERT [{}]: {}", service, message);
                // In production: send to PagerDuty, Slack, email, etc.
            }
            AlertLevel::Warning => {
                warn!("⚠️  WARNING ALERT [{}]: {}", service, message);
                // In production: send to monitoring dashboard, Slack, etc.
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum AlertLevel {
    Critical,
    Warning,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_metrics_collector_success() {
        let collector = MetricsCollector::new();
        
        collector.record_success("test_service", 100).await;
        collector.record_success("test_service", 200).await;
        
        let metrics = collector.get_service_metrics("test_service").await.unwrap();
        assert_eq!(metrics.total_requests, 2);
        assert_eq!(metrics.successful_requests, 2);
        assert_eq!(metrics.failed_requests, 0);
        assert_eq!(metrics.avg_response_time_ms, 150);
    }

    #[tokio::test]
    async fn test_metrics_collector_failure() {
        let collector = MetricsCollector::new();
        
        collector.record_success("test_service", 100).await;
        collector.record_failure("test_service", "timeout").await;
        
        let metrics = collector.get_service_metrics("test_service").await.unwrap();
        assert_eq!(metrics.total_requests, 2);
        assert_eq!(metrics.successful_requests, 1);
        assert_eq!(metrics.failed_requests, 1);
        assert_eq!(metrics.error_rate, 50.0);
    }

    #[tokio::test]
    async fn test_health_check_healthy() {
        let collector = MetricsCollector::new();
        
        collector.record_success("test_service", 100).await;
        
        let health = collector.check_health("test_service").await;
        assert!(matches!(health, HealthStatus::Healthy));
    }

    #[tokio::test]
    async fn test_health_check_circuit_breaker_open() {
        let collector = MetricsCollector::new();
        
        collector.record_circuit_breaker_state("test_service", "open").await;
        
        let health = collector.check_health("test_service").await;
        assert!(matches!(health, HealthStatus::Unhealthy { .. }));
    }

    #[tokio::test]
    async fn test_request_timer() {
        let collector = MetricsCollector::new();
        
        let timer = RequestTimer::new("test_service".to_string(), collector.clone());
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        timer.success().await;
        
        let metrics = collector.get_service_metrics("test_service").await.unwrap();
        assert_eq!(metrics.successful_requests, 1);
        assert!(metrics.avg_response_time_ms >= 10);
    }
}
