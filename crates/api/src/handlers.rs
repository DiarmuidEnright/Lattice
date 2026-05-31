use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

// Response types
#[derive(Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
        }
    }
}

// Request types
#[derive(Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub wallet_address: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub user: crate::auth::User,
    pub token: String,
}

#[derive(Deserialize)]
pub struct Enable2FARequest {
    pub user_id: String,
}

#[derive(Serialize)]
pub struct Enable2FAResponse {
    pub secret: String,
    pub qr_code: String,
}

#[derive(Deserialize)]
pub struct Verify2FARequest {
    pub user_id: String,
    pub code: String,
}

#[derive(Deserialize)]
pub struct Disable2FARequest {
    pub user_id: String,
    pub password: String,
}

#[derive(Deserialize)]
pub struct ConnectWalletRequest {
    pub wallet_address: String,
}

#[derive(Deserialize)]
pub struct UpdateAutoTraderRequest {
    pub enabled: bool,
}

#[derive(Deserialize)]
pub struct UpdatePositionLimitsRequest {
    pub max_trade_percentage: f64,
    pub max_daily_trades: i32,
}

#[derive(Deserialize)]
pub struct CreateSubscriptionRequest {
    pub tier: String,
}

#[derive(Deserialize)]
pub struct PortfolioPerformanceRequest {
    pub wallet_address: String,
    pub period: String, // "24h", "7d", "30d", or custom date range
}

#[derive(Deserialize)]
pub struct WhaleImpactRequest {
    pub user_id: String, // UUID as string
    pub period: String,  // "24h", "7d", "30d"
}

#[derive(Deserialize)]
pub struct RecommendationAccuracyRequest {
    pub user_id: String, // UUID as string
    pub period: String,  // "24h", "7d", "30d"
}

// User & Wallet Management

/// Register a new user with minimal fields
/// Requirements: 17.1, 17.2, 17.3
pub async fn register_user(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<ApiResponse<crate::auth::User>>, (StatusCode, Json<ApiResponse<crate::auth::User>>)> {
    match state
        .jwt_config
        .register_user(
            &state.db_pool,
            &payload.email,
            &payload.password,
            &payload.wallet_address,
        )
        .await
    {
        Ok(user) => Ok(Json(ApiResponse::success(user))),
        Err(e) => {
            let status = match e {
                crate::auth::AuthError::UserAlreadyExists => StatusCode::CONFLICT,
                crate::auth::AuthError::InvalidEmail => StatusCode::BAD_REQUEST,
                crate::auth::AuthError::WeakPassword(_) => StatusCode::BAD_REQUEST,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::error(e.to_string()))))
        }
    }
}

/// Login user and return JWT token
/// Requirements: 17.6
pub async fn login_user(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, (StatusCode, Json<ApiResponse<LoginResponse>>)> {
    match state
        .jwt_config
        .login_user(&state.db_pool, &payload.email, &payload.password)
        .await
    {
        Ok((user, token)) => {
            let response = LoginResponse { user, token };
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => {
            let status = match e {
                crate::auth::AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::error(e.to_string()))))
        }
    }
}

/// Enable 2FA for a user
/// Requirements: 17.5
pub async fn enable_2fa(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Enable2FARequest>,
) -> Result<Json<ApiResponse<Enable2FAResponse>>, (StatusCode, Json<ApiResponse<Enable2FAResponse>>)> {
    let user_id = match Uuid::parse_str(&payload.user_id) {
        Ok(id) => id,
        Err(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Invalid user_id format".to_string())),
            ));
        }
    };

    match state.jwt_config.enable_2fa(&state.db_pool, user_id).await {
        Ok((secret, qr_code)) => {
            let response = Enable2FAResponse { secret, qr_code };
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(e.to_string())),
        )),
    }
}

/// Verify 2FA code and enable 2FA
/// Requirements: 17.5
pub async fn verify_2fa(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Verify2FARequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let user_id = match Uuid::parse_str(&payload.user_id) {
        Ok(id) => id,
        Err(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Invalid user_id format".to_string())),
            ));
        }
    };

    match state
        .jwt_config
        .verify_and_enable_2fa(&state.db_pool, user_id, &payload.code)
        .await
    {
        Ok(_) => Ok(Json(ApiResponse::success(serde_json::json!({
            "message": "2FA enabled successfully"
        })))),
        Err(e) => {
            let status = match e {
                crate::auth::AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::error(e.to_string()))))
        }
    }
}

/// Disable 2FA for a user
/// Requirements: 17.5
pub async fn disable_2fa(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<Disable2FARequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    let user_id = match Uuid::parse_str(&payload.user_id) {
        Ok(id) => id,
        Err(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Invalid user_id format".to_string())),
            ));
        }
    };

    match state
        .jwt_config
        .disable_2fa(&state.db_pool, user_id, &payload.password)
        .await
    {
        Ok(_) => Ok(Json(ApiResponse::success(serde_json::json!({
            "message": "2FA disabled successfully"
        })))),
        Err(e) => {
            let status = match e {
                crate::auth::AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::error(e.to_string()))))
        }
    }
}

pub async fn connect_wallet(
    State(_state): State<Arc<AppState>>,
    Json(_payload): Json<ConnectWalletRequest>,
) -> impl IntoResponse {
    // TODO: Implement wallet connection
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn get_portfolio(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> Result<Json<ApiResponse<shared::models::Portfolio>>, (StatusCode, Json<ApiResponse<shared::models::Portfolio>>)> {
    tracing::info!("get_portfolio handler called for address: {}", address);

    // Bound the whole operation so a slow/flaky devnet RPC can never exceed the
    // frontend's 10s fetch timeout. If we blow past this we return a fast, clean
    // 504 instead of letting the browser abort the request itself.
    const PORTFOLIO_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);

    let fetch = get_portfolio_inner(state, address.clone());
    match tokio::time::timeout(PORTFOLIO_DEADLINE, fetch).await {
        Ok(result) => result,
        Err(_) => {
            tracing::warn!("get_portfolio timed out for {} after {:?}", address, PORTFOLIO_DEADLINE);
            Err((
                StatusCode::GATEWAY_TIMEOUT,
                Json(ApiResponse::error(format!(
                    "Portfolio lookup timed out for {}. The blockchain RPC may be slow; please retry.",
                    address
                ))),
            ))
        }
    }
}

async fn get_portfolio_inner(
    state: Arc<AppState>,
    address: String,
) -> Result<Json<ApiResponse<shared::models::Portfolio>>, (StatusCode, Json<ApiResponse<shared::models::Portfolio>>)> {
    // Try to get existing portfolio
    match state.wallet_service.get_portfolio(&address).await {
        Ok(portfolio) => {
            tracing::info!("Successfully retrieved portfolio for {}", address);
            Ok(Json(ApiResponse::success(portfolio)))
        },
        Err(shared::Error::WalletNotFound(_)) => {
            tracing::info!("Wallet not found, attempting to connect: {}", address);
            // Wallet not found - try to connect it first with a demo user
            // In production, this should use the authenticated user's ID
            let demo_user_id = uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001")
                .expect("Invalid demo user UUID");
            
            match state.wallet_service.connect_wallet(&address, demo_user_id).await {
                Ok(portfolio) => {
                    tracing::info!("Successfully connected wallet and retrieved portfolio for {}", address);
                    Ok(Json(ApiResponse::success(portfolio)))
                },
                Err(e) => {
                    tracing::error!("Failed to connect wallet {}: {}", address, e);
                    let status = match e {
                        shared::Error::InvalidWalletAddress(_) => StatusCode::BAD_REQUEST,
                        _ => StatusCode::INTERNAL_SERVER_ERROR,
                    };
                    Err((status, Json(ApiResponse::error(e.to_string()))))
                }
            }
        }
        Err(e) => {
            tracing::error!("Error getting portfolio for {}: {}", address, e);
            let status = match e {
                shared::Error::InvalidWalletAddress(_) => StatusCode::BAD_REQUEST,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::error(e.to_string()))))
        }
    }
}

// Whale Tracking
pub async fn get_tracked_whales(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Return the tracked whales ordered by value. The frontend expects each
    // item to expose `address`, `total_value_usd`, `multiplier`, and `rank`.
    // `rank` is the 1-based position by descending value; `multiplier` is the
    // whale's value relative to the smallest tracked whale (>= 1.0).
    let client = match state.db_pool.get().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Database error: {}",
                    e
                ))),
            );
        }
    };

    let rows = match client
        .query(
            "SELECT address, total_value_usd, first_detected, last_checked
             FROM whales
             ORDER BY total_value_usd DESC
             LIMIT 100",
            &[],
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Failed to query whales: {}",
                    e
                ))),
            );
        }
    };

    use rust_decimal::prelude::ToPrimitive;
    // Smallest tracked value (last row, since ordered DESC) is the multiplier base.
    let min_value: f64 = rows
        .last()
        .and_then(|r| r.get::<_, rust_decimal::Decimal>(1).to_f64())
        .filter(|v| *v > 0.0)
        .unwrap_or(1.0);

    let whales: Vec<serde_json::Value> = rows
        .iter()
        .enumerate()
        .map(|(idx, row)| {
            let address: String = row.get(0);
            let total_value_usd: f64 = row
                .get::<_, rust_decimal::Decimal>(1)
                .to_f64()
                .unwrap_or(0.0);
            let multiplier = if min_value > 0.0 {
                (total_value_usd / min_value * 10.0).round() / 10.0
            } else {
                1.0
            };
            serde_json::json!({
                "address": address,
                "total_value_usd": total_value_usd,
                "multiplier": multiplier,
                "rank": idx + 1,
            })
        })
        .collect();

    (StatusCode::OK, Json(ApiResponse::success(serde_json::json!(whales))))
}

/// List a user's wallets with freeze status for the wallet-freeze view.
///
/// Backs `GET /api/privacy/:user_id/wallets`. The frontend expects each item to
/// expose `address`, `blockchain`, `is_primary`, `is_frozen`, and `frozen_at`.
/// Data comes from `multi_chain_wallets` (which carries the freeze columns).
pub async fn get_user_wallets_freeze_status(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> impl IntoResponse {
    let client = match state.db_pool.get().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Database error: {}",
                    e
                ))),
            );
        }
    };

    let rows = match client
        .query(
            "SELECT address, blockchain, is_primary, is_frozen, frozen_at
             FROM multi_chain_wallets
             WHERE user_id = $1
             ORDER BY is_primary DESC, created_at ASC",
            &[&user_id],
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Failed to query wallets: {}",
                    e
                ))),
            );
        }
    };

    let wallets: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let address: String = row.get(0);
            let blockchain: String = row.get(1);
            let is_primary: bool = row.get(2);
            let is_frozen: bool = row.get(3);
            // frozen_at is `timestamp without time zone`; read as NaiveDateTime
            // (reading into DateTime<Utc> would panic) and emit ISO-8601.
            let frozen_at: Option<chrono::NaiveDateTime> = row.get(4);
            serde_json::json!({
                "address": address,
                "blockchain": blockchain,
                "is_primary": is_primary,
                "is_frozen": is_frozen,
                "frozen_at": frozen_at.map(|t| t.and_utc().to_rfc3339()),
            })
        })
        .collect();

    (StatusCode::OK, Json(ApiResponse::success(serde_json::json!(wallets))))
}

/// Recent AI actions for the analytics view.
///
/// Backs `GET /api/analytics/:user_id/ai-actions`. The frontend expects each
/// item to expose `action_type`, `asset`, `description`, `reasoning`,
/// `profit_realized`, and `created_at`. Executed agentic trims (the concrete
/// AI-driven actions the system records) are sourced from `trim_executions`.
pub async fn get_ai_actions(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> impl IntoResponse {
    let client = match state.db_pool.get().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Database error: {}",
                    e
                ))),
            );
        }
    };

    let rows = match client
        .query(
            "SELECT asset, amount_sold, price_usd, profit_realized, confidence,
                    reasoning, executed_at
             FROM trim_executions
             WHERE user_id = $1
             ORDER BY executed_at DESC
             LIMIT 50",
            &[&user_id],
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(format!(
                    "Failed to query AI actions: {}",
                    e
                ))),
            );
        }
    };

    use rust_decimal::prelude::ToPrimitive;
    let actions: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let asset: String = row.get(0);
            let amount_sold = row
                .get::<_, rust_decimal::Decimal>(1)
                .to_f64()
                .unwrap_or(0.0);
            let price_usd = row
                .get::<_, rust_decimal::Decimal>(2)
                .to_f64()
                .unwrap_or(0.0);
            let profit_realized = row
                .get::<_, Option<rust_decimal::Decimal>>(3)
                .and_then(|d| d.to_f64());
            let reasoning: Option<String> = row.get(5);
            // executed_at is `timestamp without time zone`.
            let executed_at: Option<chrono::NaiveDateTime> = row.get(6);
            serde_json::json!({
                "action_type": "trim",
                "asset": asset,
                "description": format!("Trimmed {:.4} {} at ${:.2}", amount_sold, asset, price_usd),
                "reasoning": reasoning,
                "profit_realized": profit_realized,
                "created_at": executed_at.map(|t| t.and_utc().to_rfc3339()),
            })
        })
        .collect();

    (StatusCode::OK, Json(ApiResponse::success(serde_json::json!(actions))))
}

pub async fn get_whale_details(
    State(_state): State<Arc<AppState>>,
    Path(_address): Path<String>,
) -> impl IntoResponse {
    // TODO: Implement get whale details
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn get_whale_movements(
    State(_state): State<Arc<AppState>>,
    Path(_address): Path<String>,
) -> impl IntoResponse {
    // TODO: Implement get whale movements
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn refresh_whales(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // TODO: Implement refresh whales
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

// Notifications
pub async fn get_notifications(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // TODO: Implement get notifications
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn update_notification_preferences(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // TODO: Implement update notification preferences
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn mark_notification_read(
    State(_state): State<Arc<AppState>>,
    Path(_id): Path<Uuid>,
) -> impl IntoResponse {
    // TODO: Implement mark notification as read
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

// Trading & Settings
pub async fn get_trade_history(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // TODO: Implement get trade history
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn get_trade_performance(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // TODO: Implement get trade performance
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn update_auto_trader(
    State(_state): State<Arc<AppState>>,
    Json(_payload): Json<UpdateAutoTraderRequest>,
) -> impl IntoResponse {
    // TODO: Implement update auto-trader
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn update_position_limits(
    State(_state): State<Arc<AppState>>,
    Json(_payload): Json<UpdatePositionLimitsRequest>,
) -> impl IntoResponse {
    // TODO: Implement update position limits
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

// Payments
pub async fn create_subscription(
    State(_state): State<Arc<AppState>>,
    Json(_payload): Json<CreateSubscriptionRequest>,
) -> impl IntoResponse {
    // TODO: Implement create subscription
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn cancel_subscription(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // TODO: Implement cancel subscription
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

pub async fn get_subscription_status(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // TODO: Implement get subscription status
    (StatusCode::NOT_IMPLEMENTED, Json(ApiResponse::<()>::error("Not implemented".to_string())))
}

// Analytics
pub async fn get_portfolio_performance(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<PortfolioPerformanceRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    use chrono::{Duration, Utc};

    // Parse the period parameter
    let (start_date, end_date) = match payload.period.as_str() {
        "24h" => (Utc::now() - Duration::hours(24), Utc::now()),
        "7d" => (Utc::now() - Duration::days(7), Utc::now()),
        "30d" => (Utc::now() - Duration::days(30), Utc::now()),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<serde_json::Value>::error(
                    "Invalid period. Use '24h', '7d', or '30d'".to_string(),
                )),
            ))
        }
    };

    // Get portfolio performance
    match state
        .analytics_service
        .get_portfolio_performance(&payload.wallet_address, start_date, end_date)
        .await
    {
        Ok(performance) => {
            // Serialize performance to JSON value
            let json_value = serde_json::to_value(&performance).unwrap_or_default();
            Ok(Json(ApiResponse::<serde_json::Value>::success(json_value)))
        }
        Err(e) => {
            let status = match e {
                shared::Error::WalletNotFound(_) => StatusCode::NOT_FOUND,
                shared::Error::Internal(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::<serde_json::Value>::error(e.to_string()))))
        }
    }
}

pub async fn get_whale_impact(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<WhaleImpactRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    use chrono::{Duration, Utc};

    // Parse user_id
    let user_id = match Uuid::parse_str(&payload.user_id) {
        Ok(id) => id,
        Err(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<serde_json::Value>::error(
                    "Invalid user_id format".to_string(),
                )),
            ))
        }
    };

    // Parse the period parameter
    let (start_date, end_date) = match payload.period.as_str() {
        "24h" => (Utc::now() - Duration::hours(24), Utc::now()),
        "7d" => (Utc::now() - Duration::days(7), Utc::now()),
        "30d" => (Utc::now() - Duration::days(30), Utc::now()),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<serde_json::Value>::error(
                    "Invalid period. Use '24h', '7d', or '30d'".to_string(),
                )),
            ))
        }
    };

    // Get whale impact analysis
    match state
        .analytics_service
        .get_whale_impact_analysis(user_id, start_date, end_date)
        .await
    {
        Ok(impact) => {
            let json_value = serde_json::to_value(&impact).unwrap_or_default();
            Ok(Json(ApiResponse::<serde_json::Value>::success(json_value)))
        }
        Err(e) => {
            let status = match e {
                shared::Error::Internal(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::<serde_json::Value>::error(e.to_string()))))
        }
    }
}

pub async fn get_recommendation_accuracy(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<RecommendationAccuracyRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    use chrono::{Duration, Utc};

    // Parse user_id
    let user_id = match Uuid::parse_str(&payload.user_id) {
        Ok(id) => id,
        Err(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<serde_json::Value>::error(
                    "Invalid user_id format".to_string(),
                )),
            ))
        }
    };

    // Parse the period parameter
    let (start_date, end_date) = match payload.period.as_str() {
        "24h" => (Utc::now() - Duration::hours(24), Utc::now()),
        "7d" => (Utc::now() - Duration::days(7), Utc::now()),
        "30d" => (Utc::now() - Duration::days(30), Utc::now()),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<serde_json::Value>::error(
                    "Invalid period. Use '24h', '7d', or '30d'".to_string(),
                )),
            ))
        }
    };

    // Get recommendation accuracy
    match state
        .analytics_service
        .get_recommendation_accuracy(user_id, start_date, end_date)
        .await
    {
        Ok(accuracy) => {
            let json_value = serde_json::to_value(&accuracy).unwrap_or_default();
            Ok(Json(ApiResponse::<serde_json::Value>::success(json_value)))
        }
        Err(e) => {
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
            ))
        }
    }
}

// Conversion handlers

#[derive(Deserialize)]
pub struct GetConversionQuoteRequest {
    pub from_asset: String,
    pub to_asset: String,
    pub amount: String, // Decimal as string
    pub amount_type: String, // "from" or "to"
}

#[derive(Deserialize)]
pub struct ExecuteConversionRequest {
    pub quote_id: String,
    pub from_asset: String,
    pub to_asset: String,
    pub from_amount: String,
    pub to_amount: String,
    pub exchange_rate: String,
    pub network_fee: String,
    pub platform_fee: String,
    pub provider_fee: String,
    pub total_fees: String,
    pub provider: String,
    pub expires_at: String,
    pub settle_address: String,
    pub refund_address: Option<String>,
    pub blockchain: Option<String>, // Optional blockchain parameter, defaults to Solana
}

/// Get a conversion quote with fee breakdown
/// Requirements: 6.1, 6.2, 6.3
pub async fn get_conversion_quote(
    State(state): State<Arc<AppState>>,
    Json(request): Json<GetConversionQuoteRequest>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use crate::AmountType;

    // Parse amount
    let amount = match Decimal::from_str(&request.amount) {
        Ok(amt) => amt,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid amount format".to_string())),
            )
                .into_response();
        }
    };

    // Parse amount type
    let amount_type = match request.amount_type.to_lowercase().as_str() {
        "from" => AmountType::From,
        "to" => AmountType::To,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error(
                    "Invalid amount_type. Must be 'from' or 'to'".to_string(),
                )),
            )
                .into_response();
        }
    };

    // Get quote from conversion service
    match state
        .conversion_service
        .get_quote(&request.from_asset, &request.to_asset, amount, amount_type)
        .await
    {
        Ok(quote) => (
            StatusCode::OK,
            Json(ApiResponse::success(quote)),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Failed to get conversion quote: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error(format!(
                    "Failed to get conversion quote: {}",
                    e
                ))),
            )
                .into_response()
        }
    }
}

/// Execute a conversion based on a quote
/// Requirements: 6.5, 6.6
pub async fn execute_conversion(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(request): Json<ExecuteConversionRequest>,
) -> impl IntoResponse {
    use rust_decimal::Decimal;
    use std::str::FromStr;
    use chrono::{DateTime, Utc};
    use crate::{ConversionQuoteWithFees, ConversionProvider};

    // Parse all decimal fields
    let from_amount = match Decimal::from_str(&request.from_amount) {
        Ok(amt) => amt,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid from_amount format".to_string())),
            )
                .into_response();
        }
    };

    let to_amount = match Decimal::from_str(&request.to_amount) {
        Ok(amt) => amt,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid to_amount format".to_string())),
            )
                .into_response();
        }
    };

    let exchange_rate = match Decimal::from_str(&request.exchange_rate) {
        Ok(rate) => rate,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid exchange_rate format".to_string())),
            )
                .into_response();
        }
    };

    let network_fee = match Decimal::from_str(&request.network_fee) {
        Ok(fee) => fee,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid network_fee format".to_string())),
            )
                .into_response();
        }
    };

    let platform_fee = match Decimal::from_str(&request.platform_fee) {
        Ok(fee) => fee,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid platform_fee format".to_string())),
            )
                .into_response();
        }
    };

    let provider_fee = match Decimal::from_str(&request.provider_fee) {
        Ok(fee) => fee,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid provider_fee format".to_string())),
            )
                .into_response();
        }
    };

    let total_fees = match Decimal::from_str(&request.total_fees) {
        Ok(fee) => fee,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid total_fees format".to_string())),
            )
                .into_response();
        }
    };

    let expires_at = match DateTime::<Utc>::from_str(&request.expires_at) {
        Ok(dt) => dt,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid expires_at format".to_string())),
            )
                .into_response();
        }
    };

    let provider = match request.provider.to_lowercase().as_str() {
        "sideshift" => ConversionProvider::SideShift,
        "jupiter" => ConversionProvider::Jupiter,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error("Invalid provider".to_string())),
            )
                .into_response();
        }
    };

    // Parse blockchain (default to Solana if not provided)
    let blockchain = match request.blockchain.as_deref() {
        Some("ethereum") => blockchain::Blockchain::Ethereum,
        Some("binance smart chain") | Some("bsc") => blockchain::Blockchain::BinanceSmartChain,
        Some("polygon") => blockchain::Blockchain::Polygon,
        Some("solana") | None => blockchain::Blockchain::Solana,
        Some(other) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<()>::error(format!("Invalid blockchain: {}", other))),
            )
                .into_response();
        }
    };

    // Reconstruct quote
    let quote = ConversionQuoteWithFees {
        quote_id: request.quote_id,
        from_asset: request.from_asset,
        to_asset: request.to_asset,
        from_amount,
        to_amount,
        exchange_rate,
        network_fee,
        platform_fee,
        provider_fee,
        total_fees,
        provider,
        expires_at,
    };

    // Execute conversion
    match state
        .conversion_service
        .execute_conversion(
            user_id,
            quote,
            &request.settle_address,
            request.refund_address.as_deref(),
            blockchain,
        )
        .await
    {
        Ok(result) => (
            StatusCode::OK,
            Json(ApiResponse::success(result)),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Failed to execute conversion: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error(format!(
                    "Failed to execute conversion: {}",
                    e
                ))),
            )
                .into_response()
        }
    }
}

/// Get conversion history for a user
/// Requirements: 6.6, 16.6
pub async fn get_conversion_history(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> impl IntoResponse {
    match state
        .conversion_service
        .get_conversion_history(user_id, 50, 0)
        .await
    {
        Ok(history) => (
            StatusCode::OK,
            Json(ApiResponse::success(history)),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Failed to get conversion history: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error(format!(
                    "Failed to get conversion history: {}",
                    e
                ))),
            )
                .into_response()
        }
    }
}

// Staking handlers

#[derive(Deserialize)]
pub struct EnableAutoStakingRequest {
    pub asset: String,
    pub enabled: bool,
    pub minimum_idle_amount: Option<rust_decimal::Decimal>,
    pub idle_duration_hours: Option<u32>,
    pub auto_compound: Option<bool>,
}

#[derive(Deserialize)]
pub struct ApproveStakingRequest {
    pub approved: bool,
}

/// Get idle balances eligible for staking
pub async fn get_idle_balances(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> impl IntoResponse {
    // Get auto-staking configs for user
    let client = match state.db_pool.get().await {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Vec<(String, rust_decimal::Decimal)>>::error(
                    format!("Database connection error: {}", e),
                )),
            );
        }
    };

    // Get all enabled auto-staking configs
    let rows = match client
        .query(
            "SELECT asset, minimum_idle_amount, idle_duration_hours, auto_compound
             FROM auto_staking_configs
             WHERE user_id = $1 AND enabled = true",
            &[&user_id],
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<Vec<(String, rust_decimal::Decimal)>>::error(
                    format!("Failed to query configs: {}", e),
                )),
            );
        }
    };

    if rows.is_empty() {
        return (
            StatusCode::OK,
            Json(ApiResponse::success(Vec::<(String, rust_decimal::Decimal)>::new())),
        );
    }

    // For simplicity, use the first config or default
    let config = if let Some(row) = rows.first() {
        crate::StakingConfig {
            minimum_idle_amount: row.get(1),
            idle_duration_hours: row.get::<_, i32>(2) as u32,
            auto_compound: row.get(3),
        }
    } else {
        crate::StakingConfig::default()
    };

    match state
        .staking_service
        .identify_idle_balances(user_id, &config)
        .await
    {
        Ok(balances) => (StatusCode::OK, Json(ApiResponse::success(balances))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<(String, rust_decimal::Decimal)>>::error(e.to_string())),
        ),
    }
}

/// Create a staking approval request
pub async fn create_staking_request(
    State(state): State<Arc<AppState>>,
    Path((user_id, asset)): Path<(Uuid, String)>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    let amount = match payload.get("amount").and_then(|v| v.as_str()) {
        Some(amt) => match rust_decimal::Decimal::from_str_exact(amt) {
            Ok(d) => d,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::<crate::StakingApprovalRequest>::error(
                        format!("Invalid amount: {}", e),
                    )),
                );
            }
        },
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::<crate::StakingApprovalRequest>::error(
                    "Missing amount field".to_string(),
                )),
            );
        }
    };

    match state
        .staking_service
        .create_staking_approval_request(user_id, &asset, amount)
        .await
    {
        Ok(request) => (StatusCode::CREATED, Json(ApiResponse::success(request))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<crate::StakingApprovalRequest>::error(e.to_string())),
        ),
    }
}

/// Approve or reject a staking request
pub async fn approve_staking_request(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<Uuid>,
    Json(payload): Json<ApproveStakingRequest>,
) -> impl IntoResponse {
    match state
        .staking_service
        .initiate_staking(request_id, payload.approved)
        .await
    {
        Ok(result) => (StatusCode::OK, Json(ApiResponse::success(result))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Option<crate::StakingInitiationResult>>::error(e.to_string())),
        ),
    }
}

/// Get all staking positions for a user
pub async fn get_staking_positions(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> impl IntoResponse {
    match state.staking_service.get_staking_positions(user_id).await {
        Ok(positions) => (StatusCode::OK, Json(ApiResponse::success(positions))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<Vec<crate::StakingPosition>>::error(e.to_string())),
        ),
    }
}

/// Get a specific staking position
pub async fn get_staking_position(
    State(state): State<Arc<AppState>>,
    Path(position_id): Path<Uuid>,
) -> impl IntoResponse {
    match state
        .staking_service
        .get_staking_position(position_id)
        .await
    {
        Ok(position) => (StatusCode::OK, Json(ApiResponse::success(position))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<crate::StakingPosition>::error(e.to_string())),
        ),
    }
}

/// Enable or disable auto-staking for an asset
pub async fn set_auto_staking(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<EnableAutoStakingRequest>,
) -> impl IntoResponse {
    let config = if payload.enabled {
        Some(crate::StakingConfig {
            minimum_idle_amount: payload
                .minimum_idle_amount
                .unwrap_or(rust_decimal::Decimal::from(100)),
            idle_duration_hours: payload.idle_duration_hours.unwrap_or(24),
            auto_compound: payload.auto_compound.unwrap_or(false),
        })
    } else {
        None
    };

    match state
        .staking_service
        .set_auto_staking(user_id, &payload.asset, payload.enabled, config)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(ApiResponse::success(serde_json::json!({
                "message": "Auto-staking configuration updated"
            }))),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
        ),
    }
}

/// Get auto-staking configuration for an asset
pub async fn get_auto_staking_config(
    State(state): State<Arc<AppState>>,
    Path((user_id, asset)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    match state
        .staking_service
        .get_auto_staking_config(user_id, &asset)
        .await
    {
        Ok(Some((enabled, config))) => (
            StatusCode::OK,
            Json(ApiResponse::success(serde_json::json!({
                "enabled": enabled,
                "config": config
            }))),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<serde_json::Value>::error(
                "No auto-staking configuration found".to_string(),
            )),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<serde_json::Value>::error(e.to_string())),
        ),
    }
}

// Payment Receipt handlers

/// Request for searching receipts
#[derive(Deserialize)]
pub struct SearchReceiptsRequest {
    /// Filter by transaction type (PAYMENT, TRADE, CONVERSION)
    pub transaction_type: Option<String>,
    /// Filter by asset/currency
    pub asset: Option<String>,
    /// Filter by start date (ISO 8601 format)
    pub start_date: Option<String>,
    /// Filter by end date (ISO 8601 format)
    pub end_date: Option<String>,
    /// Page number (0-indexed)
    pub page: Option<u32>,
    /// Number of items per page
    pub page_size: Option<u32>,
}

/// Search receipts with filters and pagination
pub async fn search_receipts(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SearchReceiptsRequest>,
) -> impl IntoResponse {
    use crate::{Pagination, ReceiptSearchFilters, TransactionType};
    use chrono::DateTime;

    // Parse transaction type
    let transaction_type = payload.transaction_type.and_then(|t| match t.to_uppercase().as_str() {
        "PAYMENT" => Some(TransactionType::Payment),
        "TRADE" => Some(TransactionType::Trade),
        "CONVERSION" => Some(TransactionType::Conversion),
        _ => None,
    });

    // Parse dates
    let start_date = payload
        .start_date
        .and_then(|d| DateTime::parse_from_rfc3339(&d).ok())
        .map(|d| d.with_timezone(&chrono::Utc));

    let end_date = payload
        .end_date
        .and_then(|d| DateTime::parse_from_rfc3339(&d).ok())
        .map(|d| d.with_timezone(&chrono::Utc));

    // Build filters
    let filters = ReceiptSearchFilters {
        transaction_type,
        asset: payload.asset,
        start_date,
        end_date,
    };

    // Build pagination
    let pagination = Pagination {
        page: payload.page.unwrap_or(0),
        page_size: payload.page_size.unwrap_or(20).min(100), // Max 100 per page
    };

    // Search receipts
    match state
        .payment_receipt_service
        .search_receipts(filters, pagination)
        .await
    {
        Ok(results) => (StatusCode::OK, Json(ApiResponse::success(results))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<crate::ReceiptSearchResults>::error(e.to_string())),
        ),
    }
}


/// Export a single receipt as PDF
pub async fn export_receipt_pdf(
    State(state): State<Arc<AppState>>,
    Path(receipt_id): Path<Uuid>,
) -> impl IntoResponse {
    match state
        .payment_receipt_service
        .export_receipt_pdf(receipt_id)
        .await
    {
        Ok(pdf_bytes) => (
            StatusCode::OK,
            [
                ("Content-Type", "application/pdf"),
                ("Content-Disposition", &format!("attachment; filename=\"receipt_{}.pdf\"", receipt_id)),
            ],
            pdf_bytes,
        ).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ).into_response(),
    }
}

/// Export receipt history as CSV
pub async fn export_receipts_csv(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SearchReceiptsRequest>,
) -> impl IntoResponse {
    use crate::{ReceiptSearchFilters, TransactionType};
    use chrono::DateTime;

    // Parse transaction type
    let transaction_type = payload.transaction_type.and_then(|t| match t.to_uppercase().as_str() {
        "PAYMENT" => Some(TransactionType::Payment),
        "TRADE" => Some(TransactionType::Trade),
        "CONVERSION" => Some(TransactionType::Conversion),
        _ => None,
    });

    // Parse dates
    let start_date = payload
        .start_date
        .and_then(|d| DateTime::parse_from_rfc3339(&d).ok())
        .map(|d| d.with_timezone(&chrono::Utc));

    let end_date = payload
        .end_date
        .and_then(|d| DateTime::parse_from_rfc3339(&d).ok())
        .map(|d| d.with_timezone(&chrono::Utc));

    // Build filters
    let filters = ReceiptSearchFilters {
        transaction_type,
        asset: payload.asset,
        start_date,
        end_date,
    };

    // Export as CSV
    match state
        .payment_receipt_service
        .export_receipts_csv(filters)
        .await
    {
        Ok(csv_bytes) => {
            let filename = format!("receipts_{}.csv", chrono::Utc::now().format("%Y%m%d_%H%M%S"));
            (
                StatusCode::OK,
                [
                    ("Content-Type", "text/csv"),
                    ("Content-Disposition", &format!("attachment; filename=\"{}\"", filename)),
                ],
                csv_bytes,
            ).into_response()
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        ).into_response(),
    }
}

// Chat handlers
#[derive(Deserialize)]
pub struct SendMessageRequest {
    pub to_user_id: Uuid,
    pub content: String,
    pub encryption_key: String, // Hex-encoded 32-byte key
    pub verify_on_chain: bool,
    pub blockchain: Option<String>, // "Solana", "Ethereum", etc.
}

#[derive(Deserialize)]
pub struct GetMessagesRequest {
    pub other_user_id: Uuid,
    pub encryption_key: String, // Hex-encoded 32-byte key
    pub limit: Option<i64>,
}

#[derive(Serialize)]
pub struct ChatMessageResponse {
    pub id: Uuid,
    pub from_user_id: Uuid,
    pub to_user_id: Uuid,
    pub content: String,
    pub blockchain_hash: Option<String>,
    pub verification_status: Option<String>,
    pub read: bool,
    pub created_at: String,
}

pub async fn send_message(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<SendMessageRequest>,
) -> Result<Json<ApiResponse<ChatMessageResponse>>, (StatusCode, Json<ApiResponse<ChatMessageResponse>>)> {
    // Decode encryption key
    let key_bytes = match hex::decode(&req.encryption_key) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            key
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error(
                    "Invalid encryption key format".to_string(),
                )),
            ));
        }
    };

    // Parse blockchain if provided
    let blockchain = if req.verify_on_chain {
        match req.blockchain.as_deref() {
            Some("Solana") => Some(blockchain::Blockchain::Solana),
            Some("Ethereum") => Some(blockchain::Blockchain::Ethereum),
            Some("BinanceSmartChain") | Some("BSC") => Some(blockchain::Blockchain::BinanceSmartChain),
            Some("Polygon") => Some(blockchain::Blockchain::Polygon),
            _ => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::error(
                        "Invalid blockchain specified".to_string(),
                    )),
                ));
            }
        }
    } else {
        None
    };

    match state
        .chat_service
        .send_message(
            user_id,
            req.to_user_id,
            req.content,
            &key_bytes,
            req.verify_on_chain,
            blockchain,
        )
        .await
    {
        Ok(message) => {
            let response = ChatMessageResponse {
                id: message.id,
                from_user_id: message.from_user_id,
                to_user_id: message.to_user_id,
                content: message.content,
                blockchain_hash: message.blockchain_hash,
                verification_status: message.verification_status,
                read: message.read,
                created_at: message.created_at.to_rfc3339(),
            };
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to send message: {}", e))),
        )),
    }
}

pub async fn get_messages(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<GetMessagesRequest>,
) -> Result<Json<ApiResponse<Vec<ChatMessageResponse>>>, (StatusCode, Json<ApiResponse<Vec<ChatMessageResponse>>>)> {
    // Decode encryption key
    let key_bytes = match hex::decode(&req.encryption_key) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            key
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error(
                    "Invalid encryption key format".to_string(),
                )),
            ));
        }
    };

    let limit = req.limit.unwrap_or(50);

    match state
        .chat_service
        .get_messages(user_id, req.other_user_id, &key_bytes, limit)
        .await
    {
        Ok(messages) => {
            let response: Vec<ChatMessageResponse> = messages
                .into_iter()
                .map(|m| ChatMessageResponse {
                    id: m.id,
                    from_user_id: m.from_user_id,
                    to_user_id: m.to_user_id,
                    content: m.content,
                    blockchain_hash: m.blockchain_hash,
                    verification_status: m.verification_status,
                    read: m.read,
                    created_at: m.created_at.to_rfc3339(),
                })
                .collect();
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to get messages: {}",
                e
            ))),
        )),
    }
}

pub async fn mark_message_read(
    State(state): State<Arc<AppState>>,
    Path((user_id, message_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    match state.chat_service.mark_as_read(message_id, user_id).await {
        Ok(_) => Ok(Json(ApiResponse::success("Message marked as read".to_string()))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to mark message as read: {}",
                e
            ))),
        )),
    }
}

#[derive(Serialize)]
pub struct VerificationResponse {
    pub verified: bool,
}

pub async fn verify_message(
    State(state): State<Arc<AppState>>,
    Path(message_id): Path<Uuid>,
) -> Result<Json<ApiResponse<VerificationResponse>>, (StatusCode, Json<ApiResponse<VerificationResponse>>)> {
    match state.chat_service.verify_message(message_id).await {
        Ok(verified) => {
            Ok(Json(ApiResponse::success(VerificationResponse { verified })))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to verify message: {}",
                e
            ))),
        )),
    }
}

#[derive(Deserialize)]
pub struct ReportMessageRequest {
    pub reason: String,
}

pub async fn report_message(
    State(state): State<Arc<AppState>>,
    Path((user_id, message_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<ReportMessageRequest>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    match state
        .chat_service
        .report_message(message_id, user_id, req.reason)
        .await
    {
        Ok(_) => Ok(Json(ApiResponse::success("Message reported successfully".to_string()))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to report message: {}", e))),
        )),
    }
}

/// Get available chat contacts from proximity network
pub async fn get_proximity_contacts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<crate::chat_service::ProximityContact>>>, (StatusCode, Json<ApiResponse<Vec<crate::chat_service::ProximityContact>>>)> {
    match state
        .chat_service
        .get_proximity_contacts(&state.proximity_discovery_service)
        .await
    {
        Ok(contacts) => Ok(Json(ApiResponse::success(contacts))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to get proximity contacts: {}",
                e
            ))),
        )),
    }
}

#[derive(Deserialize)]
pub struct CreateConversationFromOfferRequest {
    pub offer_id: Uuid,
    pub creator_id: Uuid,
    pub acceptor_id: Uuid,
}

#[derive(Serialize)]
pub struct ConversationResponse {
    pub conversation_id: Uuid,
}

/// Create a chat conversation from a P2P offer
pub async fn create_conversation_from_offer(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateConversationFromOfferRequest>,
) -> Result<Json<ApiResponse<ConversationResponse>>, (StatusCode, Json<ApiResponse<ConversationResponse>>)> {
    match state
        .chat_service
        .create_conversation_from_offer(req.offer_id, req.creator_id, req.acceptor_id)
        .await
    {
        Ok(conversation_id) => Ok(Json(ApiResponse::success(ConversationResponse {
            conversation_id,
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to create conversation from offer: {}",
                e
            ))),
        )),
    }
}

// ============================================================================
// Position Management Handlers
// ============================================================================

#[derive(Deserialize)]
pub struct SetPositionModeRequest {
    pub asset: String,
    pub blockchain: Option<String>,
    pub mode: String, // "manual" or "automatic"
}

/// Set position mode for an asset (manual or automatic)
pub async fn set_position_mode(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<SetPositionModeRequest>,
) -> Result<Json<ApiResponse<crate::PositionModeConfig>>, (StatusCode, Json<ApiResponse<crate::PositionModeConfig>>)> {
    let blockchain = req.blockchain.unwrap_or_else(|| "Solana".to_string());
    
    let mode = match crate::PositionMode::from_str(&req.mode) {
        Ok(m) => m,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error(format!("Invalid mode: {}", e))),
            ));
        }
    };

    match state
        .position_management_service
        .set_position_mode(user_id, &req.asset, &blockchain, mode)
        .await
    {
        Ok(config) => Ok(Json(ApiResponse::success(config))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to set position mode: {}",
                e
            ))),
        )),
    }
}

/// Get position mode for an asset
pub async fn get_position_mode(
    State(state): State<Arc<AppState>>,
    Path((user_id, asset)): Path<(Uuid, String)>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ApiResponse<crate::PositionMode>>, (StatusCode, Json<ApiResponse<crate::PositionMode>>)> {
    let blockchain = params.get("blockchain").map(|s| s.as_str()).unwrap_or("Solana");

    match state
        .position_management_service
        .get_position_mode(user_id, &asset, blockchain)
        .await
    {
        Ok(mode) => Ok(Json(ApiResponse::success(mode))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to get position mode: {}",
                e
            ))),
        )),
    }
}

/// Get all position modes for a user
pub async fn get_user_position_modes(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<Vec<crate::PositionModeConfig>>>, (StatusCode, Json<ApiResponse<Vec<crate::PositionModeConfig>>>)> {
    match state
        .position_management_service
        .get_user_position_modes(user_id)
        .await
    {
        Ok(modes) => Ok(Json(ApiResponse::success(modes))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to get position modes: {}",
                e
            ))),
        )),
    }
}

/// Create a manual order
pub async fn create_manual_order(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<crate::ManualOrderRequest>,
) -> Result<Json<ApiResponse<crate::ManualOrder>>, (StatusCode, Json<ApiResponse<crate::ManualOrder>>)> {
    match state
        .position_management_service
        .create_manual_order(user_id, req)
        .await
    {
        Ok(order) => Ok(Json(ApiResponse::success(order))),
        Err(e) => {
            let status_code = if e.to_string().contains("Insufficient balance") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            
            Err((
                status_code,
                Json(ApiResponse::error(format!("Failed to create manual order: {}", e))),
            ))
        }
    }
}

/// Get a manual order by ID
pub async fn get_manual_order(
    State(state): State<Arc<AppState>>,
    Path((user_id, order_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<crate::ManualOrder>>, (StatusCode, Json<ApiResponse<crate::ManualOrder>>)> {
    match state
        .position_management_service
        .get_manual_order(order_id, user_id)
        .await
    {
        Ok(order) => Ok(Json(ApiResponse::success(order))),
        Err(e) => Err((
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!("Manual order not found: {}", e))),
        )),
    }
}

/// Get all manual orders for a user
pub async fn get_user_manual_orders(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ApiResponse<Vec<crate::ManualOrder>>>, (StatusCode, Json<ApiResponse<Vec<crate::ManualOrder>>>)> {
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok());

    match state
        .position_management_service
        .get_user_manual_orders(user_id, limit)
        .await
    {
        Ok(orders) => Ok(Json(ApiResponse::success(orders))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!(
                "Failed to get manual orders: {}",
                e
            ))),
        )),
    }
}

/// Cancel a manual order
pub async fn cancel_manual_order(
    State(state): State<Arc<AppState>>,
    Path((user_id, order_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    match state
        .position_management_service
        .cancel_manual_order(order_id, user_id)
        .await
    {
        Ok(_) => Ok(Json(ApiResponse::success("Order cancelled successfully".to_string()))),
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error(format!("Failed to cancel order: {}", e))),
        )),
    }
}

// ============================================================================
// Benchmark Handlers
// ============================================================================

/// Get all benchmarks for a user
pub async fn get_user_benchmarks(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<Vec<crate::Benchmark>>>, (StatusCode, Json<ApiResponse<Vec<crate::Benchmark>>>)> {
    match state.benchmark_service.get_user_benchmarks(user_id).await {
        Ok(benchmarks) => Ok(Json(ApiResponse::success(benchmarks))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get benchmarks: {}", e))),
        )),
    }
}

/// Create a new benchmark
pub async fn create_benchmark(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<crate::CreateBenchmarkRequest>,
) -> Result<Json<ApiResponse<crate::Benchmark>>, (StatusCode, Json<ApiResponse<crate::Benchmark>>)> {
    match state.benchmark_service.create_benchmark(user_id, req).await {
        Ok(benchmark) => Ok(Json(ApiResponse::success(benchmark))),
        Err(e) => {
            let status = if e.to_string().contains("validation") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            Err((status, Json(ApiResponse::error(format!("Failed to create benchmark: {}", e)))))
        }
    }
}

/// Get a specific benchmark
pub async fn get_benchmark(
    State(state): State<Arc<AppState>>,
    Path((user_id, benchmark_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<crate::Benchmark>>, (StatusCode, Json<ApiResponse<crate::Benchmark>>)> {
    match state.benchmark_service.get_benchmark(benchmark_id, user_id).await {
        Ok(benchmark) => Ok(Json(ApiResponse::success(benchmark))),
        Err(e) => Err((
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!("Benchmark not found: {}", e))),
        )),
    }
}

/// Update a benchmark
pub async fn update_benchmark(
    State(state): State<Arc<AppState>>,
    Path((user_id, benchmark_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<crate::UpdateBenchmarkRequest>,
) -> Result<Json<ApiResponse<crate::Benchmark>>, (StatusCode, Json<ApiResponse<crate::Benchmark>>)> {
    match state.benchmark_service.update_benchmark(benchmark_id, user_id, req).await {
        Ok(benchmark) => Ok(Json(ApiResponse::success(benchmark))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to update benchmark: {}", e))),
        )),
    }
}

/// Delete a benchmark
pub async fn delete_benchmark(
    State(state): State<Arc<AppState>>,
    Path((user_id, benchmark_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    match state.benchmark_service.delete_benchmark(benchmark_id, user_id).await {
        Ok(_) => Ok(Json(ApiResponse::success("Benchmark deleted successfully".to_string()))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to delete benchmark: {}", e))),
        )),
    }
}

// ============================================================================
// Trim Configuration Handlers
// ============================================================================

/// Get trim configuration for a user
pub async fn get_trim_config(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<crate::TrimConfig>>, (StatusCode, Json<ApiResponse<crate::TrimConfig>>)> {
    match state.trim_config_service.get_trim_config(user_id).await {
        Ok(config) => Ok(Json(ApiResponse::success(config))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get trim config: {}", e))),
        )),
    }
}

/// Update trim configuration
pub async fn update_trim_config(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<crate::UpdateTrimConfigRequest>,
) -> Result<Json<ApiResponse<crate::TrimConfig>>, (StatusCode, Json<ApiResponse<crate::TrimConfig>>)> {
    match state.trim_config_service.upsert_trim_config(user_id, req).await {
        Ok(config) => Ok(Json(ApiResponse::success(config))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to update trim config: {}", e))),
        )),
    }
}

/// Get trim execution history
pub async fn get_trim_executions(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<Vec<crate::TrimExecution>>>, (StatusCode, Json<ApiResponse<Vec<crate::TrimExecution>>>)> {
    match state.trim_executor.get_trim_history(user_id).await {
        Ok(executions) => Ok(Json(ApiResponse::success(executions))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get trim executions: {}", e))),
        )),
    }
}

// ============================================================================
// Dashboard Handler
// ============================================================================

#[derive(Serialize)]
pub struct DashboardData {
    pub total_portfolio_value: rust_decimal::Decimal,
    pub change_24h: rust_decimal::Decimal,
    pub change_7d: rust_decimal::Decimal,
    pub all_time_pnl: rust_decimal::Decimal,
    pub positions_by_chain: std::collections::HashMap<String, Vec<serde_json::Value>>,
    pub active_benchmarks: Vec<crate::Benchmark>,
    pub recent_trims: Vec<crate::TrimExecution>,
    pub position_modes: Vec<crate::PositionModeConfig>,
}

/// Get comprehensive dashboard data
pub async fn get_dashboard_data(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<DashboardData>>, (StatusCode, Json<ApiResponse<DashboardData>>)> {
    use rust_decimal::Decimal;
    use std::collections::HashMap;

    // Get multi-chain portfolio (placeholder - would integrate with Birdeye)
    let total_portfolio_value = Decimal::ZERO;
    let change_24h = Decimal::ZERO;
    let change_7d = Decimal::ZERO;
    let all_time_pnl = Decimal::ZERO;
    let positions_by_chain = HashMap::new();

    // Get active benchmarks
    let active_benchmarks = state
        .benchmark_service
        .get_user_benchmarks(user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|b| b.is_active)
        .collect();

    // Get recent trims
    let recent_trims = state
        .trim_executor
        .get_trim_history(user_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .take(10)
        .collect();

    // Get position modes
    let position_modes = state
        .position_management_service
        .get_user_position_modes(user_id)
        .await
        .unwrap_or_default();

    let dashboard = DashboardData {
        total_portfolio_value,
        change_24h,
        change_7d,
        all_time_pnl,
        positions_by_chain,
        active_benchmarks,
        recent_trims,
        position_modes,
    };

    Ok(Json(ApiResponse::success(dashboard)))
}

// ============================================================================
// Multi-Chain Portfolio Handler
// ============================================================================

/// Get multi-chain portfolio for a user
/// NOTE: This handler is currently disabled as it requires Birdeye API which has been removed.
/// CoinMarketCap does not provide multi-chain portfolio aggregation.
/// Consider implementing this using individual blockchain clients instead.
pub async fn get_multi_chain_portfolio(
    State(_state): State<Arc<AppState>>,
    Path(_user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(ApiResponse::error(
            "Multi-chain portfolio aggregation is not currently available. Birdeye API has been removed in favor of CoinMarketCap.".to_string()
        )),
    ))
}

// ============================================================================
// P2P Exchange Handlers
// ============================================================================

#[derive(Deserialize)]
pub struct CreateP2POfferRequest {
    pub offer_type: String, // "buy" or "sell"
    pub from_asset: String,
    pub to_asset: String,
    pub from_amount: String,
    pub to_amount: String,
    pub price: String,
}

#[derive(Serialize)]
pub struct OfferStatusResponse {
    pub offer_id: Uuid,
    pub status: String,
    pub creator_id: Uuid,
    pub acceptor_id: Option<Uuid>,
    pub acceptor_wallet: Option<String>,
    pub acceptor_user_tag: Option<String>,
    pub accepted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub conversation_id: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub from_asset: String,
    pub to_asset: String,
    pub from_amount: String,
    pub to_amount: String,
    pub offer_type: String,
}

/// Create a P2P offer
pub async fn create_p2p_offer(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<CreateP2POfferRequest>,
) -> Result<Json<ApiResponse<crate::P2POffer>>, (StatusCode, Json<ApiResponse<crate::P2POffer>>)> {
    use rust_decimal::Decimal;
    use std::str::FromStr;

    let offer_type = match req.offer_type.to_lowercase().as_str() {
        "buy" => crate::OfferType::Buy,
        "sell" => crate::OfferType::Sell,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Invalid offer_type. Must be 'buy' or 'sell'".to_string())),
            ));
        }
    };

    let from_amount = Decimal::from_str(&req.from_amount).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Invalid from_amount".to_string())),
        )
    })?;

    let to_amount = Decimal::from_str(&req.to_amount).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Invalid to_amount".to_string())),
        )
    })?;

    let price = Decimal::from_str(&req.price).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Invalid price".to_string())),
        )
    })?;

    match state
        .p2p_service
        .create_offer(
            user_id,
            offer_type,
            req.from_asset,
            req.to_asset,
            from_amount,
            to_amount,
            price,
            false, // Regular P2P offers are not proximity offers by default
        )
        .await
    {
        Ok(offer) => Ok(Json(ApiResponse::success(offer))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to create offer: {}", e))),
        )),
    }
}

/// Get user's P2P offers
pub async fn get_user_offers(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ApiResponse<Vec<crate::P2POffer>>>, (StatusCode, Json<ApiResponse<Vec<crate::P2POffer>>>)> {
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50);
        
    match state.p2p_service.get_user_offers(user_id, limit).await {
        Ok(offers) => Ok(Json(ApiResponse::success(offers))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get offers: {}", e))),
        )),
    }
}

/// Get all active P2P offers
pub async fn get_all_offers(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ApiResponse<Vec<crate::P2POffer>>>, (StatusCode, Json<ApiResponse<Vec<crate::P2POffer>>>)> {
    let from_asset = params.get("from_asset").cloned();
    let to_asset = params.get("to_asset").cloned();
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50);
    
    match state.p2p_service.get_active_offers(from_asset, to_asset, limit).await {
        Ok(offers) => Ok(Json(ApiResponse::success(offers))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get offers: {}", e))),
        )),
    }
}

/// Get marketplace offers (excludes user's own offers)
pub async fn get_marketplace_offers(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ApiResponse<Vec<crate::P2POffer>>>, (StatusCode, Json<ApiResponse<Vec<crate::P2POffer>>>)> {
    let from_asset = params.get("from_asset").cloned();
    let to_asset = params.get("to_asset").cloned();
    let limit = params
        .get("limit")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(50);
    
    match state.p2p_service.get_marketplace_offers(user_id, from_asset, to_asset, limit).await {
        Ok(offers) => Ok(Json(ApiResponse::success(offers))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get marketplace offers: {}", e))),
        )),
    }
}

/// Get a specific offer
pub async fn get_offer(
    State(state): State<Arc<AppState>>,
    Path(offer_id): Path<Uuid>,
) -> Result<Json<ApiResponse<crate::P2POffer>>, (StatusCode, Json<ApiResponse<crate::P2POffer>>)> {
    // Delegate to the P2P service rather than hand-rolling the row mapping here.
    // The previous inline version read `created_at`/`expires_at` (Postgres
    // `timestamp without time zone`) directly into `DateTime<Utc>`, which
    // panics at runtime, and it dropped the acceptor/accepted_at/conversation
    // fields. `P2PService::get_offer` maps every column correctly via
    // `row_to_offer` (reading the timestamps as `SystemTime` first).
    match state.p2p_service.get_offer(offer_id).await {
        Ok(offer) => Ok(Json(ApiResponse::success(offer))),
        Err(e) => {
            // A missing offer is a 404; anything else is a 500.
            let msg = e.to_string();
            if msg.contains("not found") {
                Err((
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::error("Offer not found".to_string())),
                ))
            } else {
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::error(format!("Database error: {}", msg))),
                ))
            }
        }
    }
}

/// Accept a P2P offer
pub async fn accept_p2p_offer(
    State(state): State<Arc<AppState>>,
    Path((user_id, offer_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<crate::P2POffer>>, (StatusCode, Json<ApiResponse<crate::P2POffer>>)> {
    match state.p2p_service.accept_offer(offer_id, user_id, &state.chat_service).await {
        Ok(offer) => Ok(Json(ApiResponse::success(offer))),
        Err(e) => {
            let error_msg = e.to_string();
            let status_code = if error_msg.contains("Cannot accept your own offer") {
                StatusCode::BAD_REQUEST
            } else if error_msg.contains("not available") || error_msg.contains("not in ACTIVE status") {
                StatusCode::BAD_REQUEST
            } else if error_msg.contains("expired") {
                StatusCode::BAD_REQUEST
            } else if error_msg.contains("already been accepted") {
                StatusCode::CONFLICT
            } else if error_msg.contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            
            Err((
                status_code,
                Json(ApiResponse::error(format!("Failed to accept offer: {}", e))),
            ))
        }
    }
}

/// Get P2P offer status with acceptor details
pub async fn get_p2p_offer_status(
    State(state): State<Arc<AppState>>,
    Path((_user_id, offer_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<OfferStatusResponse>>, (StatusCode, Json<ApiResponse<OfferStatusResponse>>)> {
    match state.p2p_service.get_offer(offer_id).await {
        Ok(offer) => {
            // Fetch acceptor details if offer is accepted
            let (acceptor_wallet, acceptor_user_tag) = if let Some(acceptor_id) = offer.acceptor_id {
                // Try to get acceptor's wallet and user tag
                let wallet = state.db_pool.get()
                    .await
                    .ok()
                    .and_then(|client| {
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                client.query_opt(
                                    "SELECT address FROM wallets WHERE user_id = $1 LIMIT 1",
                                    &[&acceptor_id]
                                ).await.ok().flatten()
                            })
                        })
                    })
                    .and_then(|row| row.get::<_, Option<String>>(0));
                
                let user_tag = state.db_pool.get()
                    .await
                    .ok()
                    .and_then(|client| {
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                client.query_opt(
                                    "SELECT user_tag FROM users WHERE id = $1",
                                    &[&acceptor_id]
                                ).await.ok().flatten()
                            })
                        })
                    })
                    .and_then(|row| row.get::<_, Option<String>>(0));
                
                (wallet, user_tag)
            } else {
                (None, None)
            };

            let response = OfferStatusResponse {
                offer_id: offer.id,
                status: offer.status.as_str().to_string(),
                creator_id: offer.user_id,
                acceptor_id: offer.acceptor_id,
                acceptor_wallet,
                acceptor_user_tag,
                accepted_at: offer.accepted_at,
                conversation_id: offer.conversation_id,
                created_at: offer.created_at,
                expires_at: offer.expires_at,
                from_asset: offer.from_asset,
                to_asset: offer.to_asset,
                from_amount: offer.from_amount.to_string(),
                to_amount: offer.to_amount.to_string(),
                offer_type: offer.offer_type.as_str().to_string(),
            };

            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => {
            let error_msg = e.to_string();
            let status_code = if error_msg.contains("not found") {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            
            Err((
                status_code,
                Json(ApiResponse::error(format!("Failed to get offer status: {}", e))),
            ))
        }
    }
}

/// Cancel a P2P offer
pub async fn cancel_p2p_offer(
    State(state): State<Arc<AppState>>,
    Path((user_id, offer_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    match state.p2p_service.cancel_offer(offer_id, user_id).await {
        Ok(_) => Ok(Json(ApiResponse::success("Offer cancelled successfully".to_string()))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to cancel offer: {}", e))),
        )),
    }
}

/// Get user's P2P exchanges
pub async fn get_user_exchanges(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<Vec<crate::P2PExchange>>>, (StatusCode, Json<ApiResponse<Vec<crate::P2PExchange>>>)> {
    match state.p2p_service.get_user_exchanges(user_id).await {
        Ok(exchanges) => Ok(Json(ApiResponse::success(exchanges))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get exchanges: {}", e))),
        )),
    }
}

// ============================================================================
// Verification Handlers
// ============================================================================

#[derive(Deserialize)]
pub struct SubmitIdentityVerificationRequest {
    pub document_type: String,
    pub document_data: String, // Base64 encoded
}

#[derive(Deserialize)]
pub struct VerifyWalletRequest {
    pub wallet_address: String,
    pub blockchain: String,
    pub signature: String,
}

/// Get verification status
pub async fn get_verification_status(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    match state.verification_service.get_verification_status(user_id).await {
        Ok(status) => {
            let json_value = serde_json::json!({
                "level": status.level,
                "status": status.status,
            });
            Ok(Json(ApiResponse::success(json_value)))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get verification status: {}", e))),
        )),
    }
}

/// Submit identity verification
pub async fn submit_identity_verification(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(_req): Json<SubmitIdentityVerificationRequest>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    // This would integrate with a KYC provider like Onfido
    match state.verification_service.submit_identity_verification(user_id).await {
        Ok(request_id) => Ok(Json(ApiResponse::success(format!("Verification request submitted: {}", request_id)))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to submit verification: {}", e))),
        )),
    }
}

/// Verify wallet ownership
pub async fn verify_wallet_ownership(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<VerifyWalletRequest>,
) -> Result<Json<ApiResponse<crate::WalletVerification>>, (StatusCode, Json<ApiResponse<crate::WalletVerification>>)> {
    let blockchain = match req.blockchain.as_str() {
        "Solana" => blockchain::Blockchain::Solana,
        "Ethereum" => blockchain::Blockchain::Ethereum,
        "BinanceSmartChain" | "BSC" => blockchain::Blockchain::BinanceSmartChain,
        "Polygon" => blockchain::Blockchain::Polygon,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Invalid blockchain".to_string())),
            ));
        }
    };

    match state
        .verification_service
        .verify_wallet_ownership(user_id, req.wallet_address, blockchain, req.signature)
        .await
    {
        Ok(verification) => Ok(Json(ApiResponse::success(verification))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to verify wallet: {}", e))),
        )),
    }
}

/// Get verified wallets
pub async fn get_verified_wallets(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<Vec<crate::WalletVerification>>>, (StatusCode, Json<ApiResponse<Vec<crate::WalletVerification>>>)> {
    match state.verification_service.get_verified_wallets(user_id).await {
        Ok(wallets) => Ok(Json(ApiResponse::success(wallets))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get verified wallets: {}", e))),
        )),
    }
}

// ============================================================================
// Privacy & Wallet Management Handlers
// ============================================================================

#[derive(Deserialize)]
pub struct CreateTemporaryWalletRequest {
    pub tag: String,
    pub blockchain: String,
    pub expires_at: Option<String>, // ISO 8601 format
}

#[derive(Deserialize)]
pub struct FreezeWalletRequest {
    pub two_fa_code: Option<String>,
}

#[derive(Deserialize)]
pub struct UnfreezeWalletRequest {
    pub password: String,
    pub two_fa_code: Option<String>,
}

/// Get temporary wallets
pub async fn get_temporary_wallets(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<Vec<crate::TemporaryWallet>>>, (StatusCode, Json<ApiResponse<Vec<crate::TemporaryWallet>>>)> {
    match state.privacy_service.get_temporary_wallets(user_id).await {
        Ok(wallets) => Ok(Json(ApiResponse::success(wallets))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get temporary wallets: {}", e))),
        )),
    }
}

/// Create temporary wallet
pub async fn create_temporary_wallet(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<CreateTemporaryWalletRequest>,
) -> Result<Json<ApiResponse<crate::TemporaryWallet>>, (StatusCode, Json<ApiResponse<crate::TemporaryWallet>>)> {
    use chrono::DateTime;

    let blockchain = match req.blockchain.to_lowercase().as_str() {
        "solana" => blockchain::Blockchain::Solana,
        "ethereum" => blockchain::Blockchain::Ethereum,
        "binancesmartchain" | "bsc" => blockchain::Blockchain::BinanceSmartChain,
        "polygon" => blockchain::Blockchain::Polygon,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error("Invalid blockchain".to_string())),
            ));
        }
    };

    let expires_at = req
        .expires_at
        .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    match state
        .privacy_service
        .create_temporary_wallet_with_blockchain(user_id, req.tag, blockchain, expires_at)
        .await
    {
        Ok(wallet) => Ok(Json(ApiResponse::success(wallet))),
        Err(e) => {
            let status = if e.to_string().contains("limit") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            Err((status, Json(ApiResponse::error(format!("Failed to create temporary wallet: {}", e)))))
        }
    }
}

/// Freeze wallet
pub async fn freeze_wallet(
    State(state): State<Arc<AppState>>,
    Path((user_id, wallet_address)): Path<(Uuid, String)>,
    Json(_req): Json<FreezeWalletRequest>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    match state.privacy_service.freeze_wallet_by_address(user_id, wallet_address).await {
        Ok(_) => Ok(Json(ApiResponse::success("Wallet frozen successfully".to_string()))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to freeze wallet: {}", e))),
        )),
    }
}

/// Unfreeze wallet
pub async fn unfreeze_wallet(
    State(state): State<Arc<AppState>>,
    Path((user_id, wallet_address)): Path<(Uuid, String)>,
    Json(req): Json<UnfreezeWalletRequest>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    // Verify password
    match state
        .jwt_config
        .verify_user_password(&state.db_pool, user_id, &req.password)
        .await
    {
        Ok(true) => {
            // Password verified, proceed with unfreeze
            match state.privacy_service.unfreeze_wallet_by_address(user_id, wallet_address).await {
                Ok(_) => Ok(Json(ApiResponse::success("Wallet unfrozen successfully".to_string()))),
                Err(e) => Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::error(format!("Failed to unfreeze wallet: {}", e))),
                )),
            }
        }
        Ok(false) => Err((
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::error("Invalid password".to_string())),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Authentication error: {}", e))),
        )),
    }
}

/// Check if wallet is frozen
pub async fn check_wallet_frozen(
    State(state): State<Arc<AppState>>,
    Path(wallet_address): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    match state.privacy_service.is_wallet_frozen_by_address(wallet_address).await {
        Ok(is_frozen) => {
            let response = serde_json::json!({ "frozen": is_frozen });
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to check wallet status: {}", e))),
        )),
    }
}

/// Set primary temporary wallet
pub async fn set_primary_temporary_wallet(
    State(state): State<Arc<AppState>>,
    Path((user_id, wallet_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<String>>, (StatusCode, Json<ApiResponse<String>>)> {
    match state.privacy_service.set_primary_wallet(user_id, wallet_id).await {
        Ok(_) => Ok(Json(ApiResponse::success("Wallet set as primary successfully".to_string()))),
        Err(e) => {
            let status = match e {
                shared::Error::WalletNotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::error(format!("Failed to set primary wallet: {}", e)))))
        }
    }
}

/// Get user tag
pub async fn get_user_tag(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    match state.privacy_service.get_user_tag(user_id).await {
        Ok(tag) => {
            let response = serde_json::json!({ "user_tag": tag });
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to get user tag: {}", e))),
        )),
    }
}

#[derive(Deserialize)]
pub struct UpdateUserTagRequest {
    pub new_tag: String,
}

/// Update user tag
pub async fn update_user_tag(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<Uuid>,
    Json(req): Json<UpdateUserTagRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    match state.privacy_service.set_user_tag(user_id, req.new_tag).await {
        Ok(tag) => {
            let response = serde_json::json!({ "user_tag": tag });
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => {
            let status = if e.to_string().contains("already exists") {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            Err((status, Json(ApiResponse::error(format!("Failed to update user tag: {}", e)))))
        }
    }
}

// Health check and monitoring endpoints

/// Health check endpoint
pub async fn health_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut services_health = std::collections::HashMap::new();
    
    // Check each service
    let services = vec![
        "birdeye_api",
        "sideshift_api",
        "blockchain_rpc",
        "database",
        "redis",
    ];
    
    for service in services {
        let health = state.metrics.check_health(service).await;
        services_health.insert(service.to_string(), health);
    }
    
    // Determine overall health. A freshly-started backend has not yet served
    // traffic to every dependency, so those services report `Unknown` rather
    // than `Healthy`. `Unknown` means "no signal yet", not "failing", so it must
    // not force a 503 on an otherwise-healthy startup. Overall health is only
    // unavailable when a service is actively `Degraded` or `Unhealthy`.
    let any_unhealthy = services_health.values().any(|h| {
        matches!(
            h,
            crate::monitoring::HealthStatus::Degraded { .. }
                | crate::monitoring::HealthStatus::Unhealthy { .. }
        )
    });

    let status = if any_unhealthy {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };

    (status, Json(serde_json::json!({
        "status": if any_unhealthy { "unhealthy" } else { "healthy" },
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "services": services_health,
    })))
}

/// Get metrics for all services
pub async fn get_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let metrics = state.metrics.get_metrics().await;
    Json(ApiResponse::success(metrics))
}

/// Get metrics for a specific service
pub async fn get_service_metrics(
    State(state): State<Arc<AppState>>,
    Path(service): Path<String>,
) -> impl IntoResponse {
    match state.metrics.get_service_metrics(&service).await {
        Some(metrics) => (StatusCode::OK, Json(ApiResponse::success(metrics))).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error(format!("Service '{}' not found", service))),
        ).into_response(),
    }
}

/// Readiness probe (for Kubernetes/container orchestration)
pub async fn readiness_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Check if critical services are available
    let db_health = state.metrics.check_health("database").await;
    let redis_health = state.metrics.check_health("redis").await;
    
    let ready = matches!(db_health, crate::monitoring::HealthStatus::Healthy) &&
                matches!(redis_health, crate::monitoring::HealthStatus::Healthy);
    
    if ready {
        (StatusCode::OK, Json(serde_json::json!({
            "ready": true,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        })))
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, Json(serde_json::json!({
            "ready": false,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "database": db_health,
            "redis": redis_health,
        })))
    }
}

/// Liveness probe (for Kubernetes/container orchestration)
pub async fn liveness_check() -> impl IntoResponse {
    // Simple check that the service is running
    (StatusCode::OK, Json(serde_json::json!({
        "alive": true,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    })))
}

// Mesh Price Service handlers

/// Request to enable provider mode
#[derive(Deserialize)]
pub struct EnableProviderRequest {
    pub api_key: String,
}

/// Response for provider status
#[derive(Serialize)]
pub struct ProviderStatusResponse {
    pub enabled: bool,
    pub node_id: String,
}

/// Enable provider mode with API key validation
/// Requirements: 1.1, 1.2
pub async fn enable_mesh_provider(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<EnableProviderRequest>,
) -> Result<Json<ApiResponse<ProviderStatusResponse>>, (StatusCode, Json<ApiResponse<ProviderStatusResponse>>)> {
    match state
        .mesh_price_service
        .enable_provider_mode(payload.api_key)
        .await
    {
        Ok(_) => {
            let node_id = state.mesh_price_service.node_id().to_string();
            let response = ProviderStatusResponse {
                enabled: true,
                node_id,
            };
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => {
            let status = match e.to_string().as_str() {
                s if s.contains("Invalid API key") || s.contains("validation failed") => {
                    StatusCode::UNAUTHORIZED
                }
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            };
            Err((status, Json(ApiResponse::error(e.to_string()))))
        }
    }
}

/// Disable provider mode
/// Requirements: 1.2, 1.5
pub async fn disable_mesh_provider(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ProviderStatusResponse>>, (StatusCode, Json<ApiResponse<ProviderStatusResponse>>)> {
    match state.mesh_price_service.disable_provider_mode().await {
        Ok(_) => {
            let node_id = state.mesh_price_service.node_id().to_string();
            let response = ProviderStatusResponse {
                enabled: false,
                node_id,
            };
            Ok(Json(ApiResponse::success(response)))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(e.to_string())),
        )),
    }
}

/// Get current provider status
/// Requirements: 1.4, 1.5
pub async fn get_mesh_provider_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ProviderStatusResponse>>, (StatusCode, Json<ApiResponse<ProviderStatusResponse>>)> {
    let enabled = state.mesh_price_service.is_provider().await;
    let node_id = state.mesh_price_service.node_id().to_string();
    
    let response = ProviderStatusResponse {
        enabled,
        node_id,
    };
    
    Ok(Json(ApiResponse::success(response)))
}

/// Get all cached prices
/// Requirements: 6.4
pub async fn get_mesh_prices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    match state.mesh_price_service.get_all_price_data().await {
        Ok(prices) => {
            let json_value = serde_json::to_value(&prices).unwrap_or_default();
            Ok(Json(ApiResponse::success(json_value)))
        }
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(e.to_string())),
        )),
    }
}

/// Get price for specific asset
/// Requirements: 6.4
pub async fn get_mesh_price_by_asset(
    State(state): State<Arc<AppState>>,
    Path(asset): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    match state.mesh_price_service.get_price_data(&asset).await {
        Ok(Some(price_data)) => {
            let json_value = serde_json::to_value(&price_data).unwrap_or_default();
            Ok(Json(ApiResponse::success(json_value)))
        }
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(format!("Price data not found for asset: {}", asset))),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(e.to_string())),
        )),
    }
}

/// Get network status and topology
/// Requirements: 8.5
pub async fn get_mesh_network_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    match state.mesh_price_service.get_network_status().await {
        Ok(status) => {
            tracing::debug!(
                "Returning network status: {} providers, {} peers",
                status.active_providers.len(),
                status.connected_peers
            );
            let json_value = serde_json::to_value(&status).unwrap_or_default();
            Ok(Json(ApiResponse::success(json_value)))
        }
        Err(e) => {
            tracing::error!("Failed to get network status: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::error(e.to_string())),
            ))
        }
    }
}

// ============================================================================
// Stealth Address Handlers
// ============================================================================

/// Request to generate a stealth meta-address
#[derive(Deserialize)]
pub struct GenerateStealthAddressRequest {
    pub version: Option<u8>, // 1 for standard, 2 for hybrid post-quantum
}

/// Response containing the generated meta-address
#[derive(Serialize)]
pub struct GenerateStealthAddressResponse {
    pub meta_address: String,
    pub version: u8,
}

/// Request to prepare a stealth payment
#[derive(Deserialize)]
pub struct PrepareStealthPaymentRequest {
    pub receiver_meta_address: String,
    pub amount: u64,
}

/// Response containing prepared stealth payment details
#[derive(Serialize)]
pub struct PrepareStealthPaymentResponse {
    pub stealth_address: String,
    pub amount: u64,
    pub ephemeral_public_key: String,
    pub viewing_tag: String, // Hex-encoded 4 bytes
}

/// Request to send a stealth payment
#[derive(Deserialize)]
pub struct SendStealthPaymentRequest {
    pub stealth_address: String,
    pub amount: u64,
    pub ephemeral_public_key: String,
    pub viewing_tag: String,
    pub via_mesh: Option<bool>, // If true, send via BLE mesh network
}

/// Response for stealth payment send operation
#[derive(Serialize)]
pub struct SendStealthPaymentResponse {
    pub status: String, // "queued", "settling", "settled"
    pub payment_id: Option<String>,
    pub signature: Option<String>,
}

/// Response for stealth payment scanning
#[derive(Serialize)]
pub struct ScanStealthPaymentsResponse {
    pub payments: Vec<DetectedStealthPayment>,
    pub last_scanned_slot: u64,
}

#[derive(Serialize)]
pub struct DetectedStealthPayment {
    pub stealth_address: String,
    pub amount: u64,
    pub ephemeral_public_key: String,
    pub viewing_tag: String,
    pub slot: u64,
    pub signature: String,
}

/// Request to shield funds to a stealth address
#[derive(Deserialize)]
pub struct ShieldFundsRequest {
    pub amount: u64,
    pub source_keypair: String, // Base58-encoded secret key
}

/// Response for shield operation
#[derive(Serialize)]
pub struct ShieldFundsResponse {
    pub stealth_address: String,
    pub signature: String,
}

/// Request to unshield funds from a stealth address
#[derive(Deserialize)]
pub struct UnshieldFundsRequest {
    pub stealth_address: String,
    pub ephemeral_public_key: String,
    pub amount: u64,
    pub destination_address: String,
}

/// Response for unshield operation
#[derive(Serialize)]
pub struct UnshieldFundsResponse {
    pub destination_address: String,
    pub signature: String,
}

/// Response for payment queue status
#[derive(Serialize)]
pub struct PaymentQueueStatusResponse {
    pub queued_payments: Vec<QueuedPaymentInfo>,
    pub total_count: usize,
}

#[derive(Serialize)]
pub struct QueuedPaymentInfo {
    pub payment_id: String,
    pub stealth_address: String,
    pub amount: u64,
    pub status: String,
    pub created_at: String,
    pub retry_count: u32,
}

/// Generate a stealth meta-address
/// Requirements: 10.3 (2.1, 2.2)
pub async fn generate_stealth_address(
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<GenerateStealthAddressRequest>,
) -> Result<Json<ApiResponse<GenerateStealthAddressResponse>>, (StatusCode, Json<ApiResponse<GenerateStealthAddressResponse>>)> {
    let version = payload.version.unwrap_or(1);
    
    if version != 1 && version != 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Invalid version. Must be 1 (standard) or 2 (hybrid)".to_string())),
        ));
    }
    
    // Mock implementation for demo
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer;
    
    let spending_keypair = Keypair::new();
    let viewing_keypair = Keypair::new();
    
    let meta_address = format!(
        "stealth:{}:{}:{}",
        version,
        spending_keypair.pubkey(),
        viewing_keypair.pubkey()
    );
    
    let response = GenerateStealthAddressResponse {
        meta_address,
        version,
    };
    
    Ok(Json(ApiResponse::success(response)))
}

/// Request to encode a stealth meta-address as a QR code.
#[derive(Deserialize)]
pub struct StealthQrEncodeRequest {
    pub meta_address: String,
}

/// Encode a stealth meta-address into a scannable QR code (SVG image).
///
/// Backs `POST /api/stealth/qr-encode`. The frontend consumes the response as an
/// image blob and renders it in an `<img>`, so we return an SVG body with an
/// `image/svg+xml` content type. The meta-address is encoded verbatim; nothing
/// is persisted.
pub async fn qr_encode_stealth(
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<StealthQrEncodeRequest>,
) -> impl IntoResponse {
    if payload.meta_address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [("Content-Type", "application/json")],
            b"{\"success\":false,\"error\":\"meta_address is required\"}".to_vec(),
        )
            .into_response();
    }

    let code = match qrcode::QrCode::new(payload.meta_address.as_bytes()) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                [("Content-Type", "application/json")],
                format!("{{\"success\":false,\"error\":\"Failed to generate QR code: {}\"}}", e)
                    .into_bytes(),
            )
                .into_response();
        }
    };

    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(300, 300)
        .build();

    (
        StatusCode::OK,
        [("Content-Type", "image/svg+xml")],
        svg.into_bytes(),
    )
        .into_response()
}


/// Requirements: 10.3 (2.3, 2.4, 2.5)
pub async fn prepare_stealth_payment(
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<PrepareStealthPaymentRequest>,
) -> Result<Json<ApiResponse<PrepareStealthPaymentResponse>>, (StatusCode, Json<ApiResponse<PrepareStealthPaymentResponse>>)> {
    // Validate meta-address format
    if !payload.receiver_meta_address.starts_with("stealth:") {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Invalid meta-address format. Must start with 'stealth:'".to_string())),
        ));
    }
    
    if payload.amount == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Amount must be greater than 0".to_string())),
        ));
    }
    
    // Mock implementation for demo
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer;
    
    let ephemeral_keypair = Keypair::new();
    let stealth_keypair = Keypair::new();
    
    let response = PrepareStealthPaymentResponse {
        stealth_address: stealth_keypair.pubkey().to_string(),
        ephemeral_public_key: ephemeral_keypair.pubkey().to_string(),
        viewing_tag: "12345678".to_string(),
        amount: payload.amount,
    };
    
    Ok(Json(ApiResponse::success(response)))
}

/// Send or queue a stealth payment
/// Requirements: 10.3 (5.1, 5.3, 8.1)
pub async fn send_stealth_payment(
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<SendStealthPaymentRequest>,
) -> Result<Json<ApiResponse<SendStealthPaymentResponse>>, (StatusCode, Json<ApiResponse<SendStealthPaymentResponse>>)> {
    // Validate inputs
    if payload.amount == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Amount must be greater than 0".to_string())),
        ));
    }
    
    if payload.viewing_tag.len() != 8 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Viewing tag must be 4 bytes (8 hex characters)".to_string())),
        ));
    }
    
    // Mock implementation for demo
    let response = SendStealthPaymentResponse {
        status: if payload.via_mesh.unwrap_or(false) {
            "queued".to_string()
        } else {
            "settled".to_string()
        },
        signature: Some("mock_signature_abc123".to_string()),
        payment_id: Some("payment_001".to_string()),
    };
    
    Ok(Json(ApiResponse::success(response)))
}

/// Scan blockchain for incoming stealth payments
/// Requirements: 10.3 (3.1, 3.2, 3.3)
pub async fn scan_stealth_payments(
    State(_state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<ApiResponse<ScanStealthPaymentsResponse>>, (StatusCode, Json<ApiResponse<ScanStealthPaymentsResponse>>)> {
    // Parse optional slot range parameters
    let _from_slot = params
        .get("from_slot")
        .and_then(|s| s.parse::<u64>().ok());
    
    let _to_slot = params
        .get("to_slot")
        .and_then(|s| s.parse::<u64>().ok());
    
    // Mock implementation for demo - return empty payments list
    let response = ScanStealthPaymentsResponse {
        payments: vec![],
        last_scanned_slot: 100,
    };
    
    Ok(Json(ApiResponse::success(response)))
}

/// Shield funds to a stealth address
/// Requirements: 10.3 (7.1, 7.2, 7.3)
pub async fn shield_funds(
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<ShieldFundsRequest>,
) -> Result<Json<ApiResponse<ShieldFundsResponse>>, (StatusCode, Json<ApiResponse<ShieldFundsResponse>>)> {
    // Validate amount
    if payload.amount == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Amount must be greater than 0".to_string())),
        ));
    }
    
    // Validate source keypair format (should be base58-encoded)
    if payload.source_keypair.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Source keypair is required".to_string())),
        ));
    }
    
    // Mock implementation for demo
    use solana_sdk::signature::Keypair;
    use solana_sdk::signer::Signer;
    
    let stealth_keypair = Keypair::new();
    
    let response = ShieldFundsResponse {
        signature: "mock_shield_signature_xyz789".to_string(),
        stealth_address: stealth_keypair.pubkey().to_string(),
    };
    
    Ok(Json(ApiResponse::success(response)))
}

/// Unshield funds from a stealth address
/// Requirements: 10.3 (7.2, 7.5)
pub async fn unshield_funds(
    State(_state): State<Arc<AppState>>,
    Json(payload): Json<UnshieldFundsRequest>,
) -> Result<Json<ApiResponse<UnshieldFundsResponse>>, (StatusCode, Json<ApiResponse<UnshieldFundsResponse>>)> {
    // Validate amount
    if payload.amount == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Amount must be greater than 0".to_string())),
        ));
    }
    
    // Validate destination address
    if payload.destination_address.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error("Destination address is required".to_string())),
        ));
    }
    
    // Mock implementation for demo
    let response = UnshieldFundsResponse {
        signature: "mock_unshield_signature_123".to_string(),
        destination_address: payload.destination_address.clone(),
    };
    
    Ok(Json(ApiResponse::success(response)))
}

/// Get payment queue status
/// Requirements: 10.3 (5.2, 5.4)
pub async fn get_payment_queue(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<PaymentQueueStatusResponse>>, (StatusCode, Json<ApiResponse<PaymentQueueStatusResponse>>)> {
    // Mock implementation for demo
    let response = PaymentQueueStatusResponse {
        queued_payments: vec![],
        total_count: 0,
    };
    
    Ok(Json(ApiResponse::success(response)))
}

/// Get BLE mesh network status
pub async fn get_ble_mesh_status(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    // Mock implementation for demo
    let status = serde_json::json!({
        "connected": false,
        "node_count": 0,
        "active_routes": 0,
        "status": "disconnected"
    });
    
    Ok(Json(ApiResponse::success(status)))
}

/// Connect to BLE mesh network
pub async fn connect_ble_mesh(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    // Mock implementation for demo
    let response = serde_json::json!({
        "status": "connected",
        "message": "Successfully connected to BLE mesh network"
    });
    
    Ok(Json(ApiResponse::success(response)))
}

/// Disconnect from BLE mesh network
pub async fn disconnect_ble_mesh(
    State(_state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<serde_json::Value>>, (StatusCode, Json<ApiResponse<serde_json::Value>>)> {
    // Mock implementation for demo
    let response = serde_json::json!({
        "status": "disconnected",
        "message": "Successfully disconnected from BLE mesh network"
    });
    
    Ok(Json(ApiResponse::success(response)))
}


// ============================================
// CoinMarketCap Price Handlers
// ============================================

#[derive(Deserialize)]
pub struct GetPriceQuery {
    pub symbol: String,
}

#[derive(Deserialize)]
pub struct GetPricesQuery {
    pub symbols: String, // Comma-separated list
}

#[derive(Deserialize)]
pub struct ConvertCryptoQuery {
    pub from: String,
    pub to: String,
    pub amount: String,
}

/// Get real-time price for a cryptocurrency
pub async fn get_crypto_price(
    State(state): State<Arc<AppState>>,
    Query(params): Query<GetPriceQuery>,
) -> Result<Json<ApiResponse<crate::CmcPriceData>>, (StatusCode, Json<ApiResponse<crate::CmcPriceData>>)> {
    match state.coinmarketcap_service.get_price_by_symbol(&params.symbol).await {
        Ok(price_data) => Ok(Json(ApiResponse::success(price_data))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to fetch price: {}", e))),
        )),
    }
}

/// Get real-time prices for multiple cryptocurrencies
pub async fn get_crypto_prices(
    State(state): State<Arc<AppState>>,
    Query(params): Query<GetPricesQuery>,
) -> Result<Json<ApiResponse<Vec<crate::CmcPriceData>>>, (StatusCode, Json<ApiResponse<Vec<crate::CmcPriceData>>>)> {
    let symbols: Vec<String> = params
        .symbols
        .split(',')
        .map(|s| s.trim().to_uppercase())
        .collect();

    match state.coinmarketcap_service.get_prices_by_symbols(&symbols).await {
        Ok(prices) => Ok(Json(ApiResponse::success(prices))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to fetch prices: {}", e))),
        )),
    }
}

/// Convert between two cryptocurrencies
pub async fn convert_crypto(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ConvertCryptoQuery>,
) -> Result<Json<ApiResponse<crate::CmcConversionResult>>, (StatusCode, Json<ApiResponse<crate::CmcConversionResult>>)> {
    use rust_decimal::Decimal;
    use std::str::FromStr;

    let amount = match Decimal::from_str(&params.amount) {
        Ok(amt) => amt,
        Err(e) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ApiResponse::error(format!("Invalid amount: {}", e))),
            ))
        }
    };

    match state
        .coinmarketcap_service
        .convert(&params.from, &params.to, amount)
        .await
    {
        Ok(conversion) => Ok(Json(ApiResponse::success(conversion))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(format!("Failed to convert: {}", e))),
        )),
    }
}
