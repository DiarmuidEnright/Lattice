use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use tracing::error;

/// Comprehensive error type for the API
#[derive(Debug)]
pub enum ApiError {
    // Database errors
    DatabaseError(String),
    DatabaseConnectionError(String),
    
    // External API errors
    BirdeyeApiError(String),
    SideShiftApiError(String),
    CoinMarketCapApiError(String),
    BlockchainRpcError(String),
    
    // Circuit breaker errors
    CircuitBreakerOpen(String),
    
    // Validation errors
    ValidationError(String),
    InvalidInput(String),
    
    // Authentication/Authorization errors
    Unauthorized(String),
    Forbidden(String),
    
    // Resource errors
    NotFound(String),
    AlreadyExists(String),
    
    // Business logic errors
    InsufficientBalance(String),
    OrderExpired(String),
    QuoteExpired(String),
    WalletFrozen(String),
    VerificationRequired(String),
    DailyLimitExceeded(String),
    
    // Rate limiting
    RateLimitExceeded(String),
    
    // Internal errors
    InternalError(String),
    ConfigurationError(String),
    
    // Timeout errors
    Timeout(String),
    
    // Not implemented
    NotImplemented(String),
    
    // Mesh network errors
    MeshNetworkError(String),
    InvalidPriceUpdate(String),
    ProviderNotConfigured(String),
    NoProvidersAvailable(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::DatabaseError(msg) => write!(f, "Database error: {}", msg),
            ApiError::DatabaseConnectionError(msg) => write!(f, "Database connection error: {}", msg),
            ApiError::BirdeyeApiError(msg) => write!(f, "Birdeye API error: {}", msg),
            ApiError::SideShiftApiError(msg) => write!(f, "SideShift API error: {}", msg),
            ApiError::CoinMarketCapApiError(msg) => write!(f, "CoinMarketCap API error: {}", msg),
            ApiError::BlockchainRpcError(msg) => write!(f, "Blockchain RPC error: {}", msg),
            ApiError::CircuitBreakerOpen(msg) => write!(f, "Service temporarily unavailable: {}", msg),
            ApiError::ValidationError(msg) => write!(f, "Validation error: {}", msg),
            ApiError::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
            ApiError::Unauthorized(msg) => write!(f, "Unauthorized: {}", msg),
            ApiError::Forbidden(msg) => write!(f, "Forbidden: {}", msg),
            ApiError::NotFound(msg) => write!(f, "Not found: {}", msg),
            ApiError::AlreadyExists(msg) => write!(f, "Already exists: {}", msg),
            ApiError::InsufficientBalance(msg) => write!(f, "Insufficient balance: {}", msg),
            ApiError::OrderExpired(msg) => write!(f, "Order expired: {}", msg),
            ApiError::QuoteExpired(msg) => write!(f, "Quote expired: {}", msg),
            ApiError::WalletFrozen(msg) => write!(f, "Wallet frozen: {}", msg),
            ApiError::VerificationRequired(msg) => write!(f, "Verification required: {}", msg),
            ApiError::DailyLimitExceeded(msg) => write!(f, "Daily limit exceeded: {}", msg),
            ApiError::RateLimitExceeded(msg) => write!(f, "Rate limit exceeded: {}", msg),
            ApiError::InternalError(msg) => write!(f, "Internal error: {}", msg),
            ApiError::ConfigurationError(msg) => write!(f, "Configuration error: {}", msg),
            ApiError::Timeout(msg) => write!(f, "Timeout: {}", msg),
            ApiError::NotImplemented(msg) => write!(f, "Not implemented: {}", msg),
            ApiError::MeshNetworkError(msg) => write!(f, "Mesh network error: {}", msg),
            ApiError::InvalidPriceUpdate(msg) => write!(f, "Invalid price update: {}", msg),
            ApiError::ProviderNotConfigured(msg) => write!(f, "Provider not configured: {}", msg),
            ApiError::NoProvidersAvailable(msg) => write!(f, "No providers available: {}", msg),
        }
    }
}

impl std::error::Error for ApiError {}

/// Error response structure for API responses
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    pub timestamp: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_type, message) = match &self {
            ApiError::DatabaseError(msg) | ApiError::DatabaseConnectionError(msg) => {
                error!("Database error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "database_error", msg.clone())
            }
            ApiError::BirdeyeApiError(msg) => {
                error!("Birdeye API error: {}", msg);
                (StatusCode::BAD_GATEWAY, "external_api_error", msg.clone())
            }
            ApiError::SideShiftApiError(msg) => {
                error!("SideShift API error: {}", msg);
                (StatusCode::BAD_GATEWAY, "external_api_error", msg.clone())
            }
            ApiError::CoinMarketCapApiError(msg) => {
                error!("CoinMarketCap API error: {}", msg);
                (StatusCode::BAD_GATEWAY, "external_api_error", msg.clone())
            }
            ApiError::BlockchainRpcError(msg) => {
                error!("Blockchain RPC error: {}", msg);
                (StatusCode::BAD_GATEWAY, "blockchain_error", msg.clone())
            }
            ApiError::CircuitBreakerOpen(msg) => {
                error!("Circuit breaker open: {}", msg);
                (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable", msg.clone())
            }
            ApiError::ValidationError(msg) | ApiError::InvalidInput(msg) => {
                (StatusCode::BAD_REQUEST, "validation_error", msg.clone())
            }
            ApiError::Unauthorized(msg) => {
                (StatusCode::UNAUTHORIZED, "unauthorized", msg.clone())
            }
            ApiError::Forbidden(msg) => {
                (StatusCode::FORBIDDEN, "forbidden", msg.clone())
            }
            ApiError::NotFound(msg) => {
                (StatusCode::NOT_FOUND, "not_found", msg.clone())
            }
            ApiError::AlreadyExists(msg) => {
                (StatusCode::CONFLICT, "already_exists", msg.clone())
            }
            ApiError::InsufficientBalance(msg) => {
                (StatusCode::BAD_REQUEST, "insufficient_balance", msg.clone())
            }
            ApiError::OrderExpired(msg) | ApiError::QuoteExpired(msg) => {
                (StatusCode::BAD_REQUEST, "expired", msg.clone())
            }
            ApiError::WalletFrozen(msg) => {
                (StatusCode::FORBIDDEN, "wallet_frozen", msg.clone())
            }
            ApiError::VerificationRequired(msg) => {
                (StatusCode::FORBIDDEN, "verification_required", msg.clone())
            }
            ApiError::DailyLimitExceeded(msg) => {
                (StatusCode::TOO_MANY_REQUESTS, "limit_exceeded", msg.clone())
            }
            ApiError::RateLimitExceeded(msg) => {
                (StatusCode::TOO_MANY_REQUESTS, "rate_limit_exceeded", msg.clone())
            }
            ApiError::InternalError(msg) => {
                error!("Internal error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "An internal error occurred".to_string())
            }
            ApiError::ConfigurationError(msg) => {
                error!("Configuration error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "configuration_error", "Service misconfigured".to_string())
            }
            ApiError::Timeout(msg) => {
                error!("Timeout: {}", msg);
                (StatusCode::GATEWAY_TIMEOUT, "timeout", msg.clone())
            }
            ApiError::NotImplemented(msg) => {
                (StatusCode::NOT_IMPLEMENTED, "not_implemented", msg.clone())
            }
            ApiError::MeshNetworkError(msg) => {
                error!("Mesh network error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "mesh_network_error", msg.clone())
            }
            ApiError::InvalidPriceUpdate(msg) => {
                (StatusCode::BAD_REQUEST, "invalid_price_update", msg.clone())
            }
            ApiError::ProviderNotConfigured(msg) => {
                (StatusCode::BAD_REQUEST, "provider_not_configured", msg.clone())
            }
            ApiError::NoProvidersAvailable(msg) => {
                (StatusCode::SERVICE_UNAVAILABLE, "no_providers_available", msg.clone())
            }
        };

        let error_response = ErrorResponse {
            error: error_type.to_string(),
            message,
            details: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };

        (status, Json(error_response)).into_response()
    }
}

// Conversion implementations for common error types

impl From<tokio_postgres::Error> for ApiError {
    fn from(err: tokio_postgres::Error) -> Self {
        ApiError::DatabaseError(err.to_string())
    }
}

impl From<deadpool_postgres::PoolError> for ApiError {
    fn from(err: deadpool_postgres::PoolError) -> Self {
        ApiError::DatabaseConnectionError(err.to_string())
    }
}

impl From<redis::RedisError> for ApiError {
    fn from(err: redis::RedisError) -> Self {
        ApiError::InternalError(format!("Redis error: {}", err))
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            ApiError::Timeout(format!("External API request timeout: {}", err))
        } else if err.is_connect() {
            ApiError::InternalError(format!("Failed to connect to external API: {}", err))
        } else {
            ApiError::InternalError(format!("External API error: {}", err))
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::InternalError(err.to_string())
    }
}

/// Result type alias for API operations
pub type ApiResult<T> = Result<T, ApiError>;
