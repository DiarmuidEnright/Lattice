use axum::{
    routing::{get, post, put},
    Router,
};
use std::sync::Arc;
use tower_http::services::ServeDir;

use crate::{handlers, proximity_handlers, proximity_websocket, AppState};

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        // Health check and monitoring
        .route("/health", get(handlers::health_check))
        .route("/health/ready", get(handlers::readiness_check))
        .route("/health/live", get(handlers::liveness_check))
        .route("/metrics", get(handlers::get_metrics))
        .route("/metrics/:service", get(handlers::get_service_metrics))
        
        // WebSocket endpoint for real-time dashboard updates
        .route("/ws/dashboard", get(crate::websocket_handler))
        
        // WebSocket endpoint for proximity events
        .route("/api/proximity/events", get(proximity_websocket::proximity_websocket_handler))
        
        // User & Wallet Management
        .route("/api/users/register", post(handlers::register_user))
        .route("/api/users/login", post(handlers::login_user))
        .route("/api/users/2fa/enable", post(handlers::enable_2fa))
        .route("/api/users/2fa/verify", post(handlers::verify_2fa))
        .route("/api/users/2fa/disable", post(handlers::disable_2fa))
        .route("/api/users/:user_id/tag", get(handlers::get_user_tag))
        .route("/api/users/:user_id/tag", put(handlers::update_user_tag))
        .route("/api/wallets/connect", post(handlers::connect_wallet))
        .route("/api/wallets/:address/portfolio", get(handlers::get_portfolio))
        .route("/api/wallets/:address/multi-chain-portfolio", get(handlers::get_portfolio)) // Alias for frontend compatibility
        .route("/api/wallets/multi-chain/:user_id", get(handlers::get_multi_chain_portfolio))
        
        // Whale Tracking
        .route("/api/whales/tracked", get(handlers::get_tracked_whales))
        .route("/api/whales/:address/details", get(handlers::get_whale_details))
        .route("/api/whales/:address/movements", get(handlers::get_whale_movements))
        .route("/api/whales/refresh", post(handlers::refresh_whales))
        
        // Notifications
        .route("/api/notifications", get(handlers::get_notifications))
        .route("/api/notifications/preferences", put(handlers::update_notification_preferences))
        .route("/api/notifications/:id/read", post(handlers::mark_notification_read))
        
        // Trading & Settings
        .route("/api/trades/history", get(handlers::get_trade_history))
        .route("/api/trades/performance", get(handlers::get_trade_performance))
        .route("/api/settings/auto-trader", put(handlers::update_auto_trader))
        .route("/api/settings/position-limits", put(handlers::update_position_limits))
        
        // Payments
        .route("/api/subscriptions/create", post(handlers::create_subscription))
        .route("/api/subscriptions/cancel", post(handlers::cancel_subscription))
        .route("/api/subscriptions/status", get(handlers::get_subscription_status))
        
        // Analytics & Dashboard
        .route("/api/analytics/portfolio-performance", post(handlers::get_portfolio_performance))
        .route("/api/analytics/whale-impact", post(handlers::get_whale_impact))
        .route("/api/analytics/recommendation-accuracy", post(handlers::get_recommendation_accuracy))
        .route("/api/analytics/:user_id/ai-actions", get(handlers::get_ai_actions))
        .route("/api/dashboard/:user_id", get(handlers::get_dashboard_data))
        
        // Benchmarks
        .route("/api/benchmarks/:user_id", get(handlers::get_user_benchmarks))
        .route("/api/benchmarks/:user_id", post(handlers::create_benchmark))
        .route("/api/benchmarks/:user_id/:benchmark_id", get(handlers::get_benchmark))
        .route("/api/benchmarks/:user_id/:benchmark_id", put(handlers::update_benchmark))
        .route("/api/benchmarks/:user_id/:benchmark_id", axum::routing::delete(handlers::delete_benchmark))
        
        // Conversions
        .route("/api/conversions/quote", post(handlers::get_conversion_quote))
        .route("/api/conversions/:user_id/execute", post(handlers::execute_conversion))
        .route("/api/conversions/:user_id/history", get(handlers::get_conversion_history))
        
        // Staking
        .route("/api/staking/:user_id/idle-balances", get(handlers::get_idle_balances))
        .route("/api/staking/:user_id/:asset/request", post(handlers::create_staking_request))
        .route("/api/staking/requests/:request_id/approve", post(handlers::approve_staking_request))
        .route("/api/staking/:user_id/positions", get(handlers::get_staking_positions))
        .route("/api/staking/positions/:position_id", get(handlers::get_staking_position))
        .route("/api/staking/:user_id/auto-staking", post(handlers::set_auto_staking))
        .route("/api/staking/:user_id/:asset/config", get(handlers::get_auto_staking_config))
        
        // Agentic Trimming
        .route("/api/trim/:user_id/config", get(handlers::get_trim_config))
        .route("/api/trim/:user_id/config", put(handlers::update_trim_config))
        .route("/api/trim/:user_id/executions", get(handlers::get_trim_executions))
        
        // Position Management
        .route("/api/positions/:user_id/modes", get(handlers::get_user_position_modes))
        .route("/api/positions/:user_id/mode", post(handlers::set_position_mode))
        .route("/api/positions/:user_id/:asset/mode", get(handlers::get_position_mode))
        .route("/api/positions/:user_id/orders/manual", post(handlers::create_manual_order))
        .route("/api/positions/:user_id/orders/manual", get(handlers::get_user_manual_orders))
        .route("/api/positions/:user_id/orders/:order_id", get(handlers::get_manual_order))
        .route("/api/positions/:user_id/orders/:order_id/cancel", post(handlers::cancel_manual_order))
        
        // Payment Receipts
        .route("/api/receipts/search", post(handlers::search_receipts))
        .route("/api/receipts/:receipt_id/pdf", get(handlers::export_receipt_pdf))
        .route("/api/receipts/export/csv", post(handlers::export_receipts_csv))
        
        // Chat
        .route("/api/chat/proximity-contacts", get(handlers::get_proximity_contacts))
        .route("/api/chat/:user_id/send", post(handlers::send_message))
        .route("/api/chat/:user_id/messages", post(handlers::get_messages))
        .route("/api/chat/:user_id/messages/:message_id/read", post(handlers::mark_message_read))
        .route("/api/chat/messages/:message_id/verify", get(handlers::verify_message))
        .route("/api/chat/:user_id/messages/:message_id/report", post(handlers::report_message))
        .route("/api/chat/conversations/from-offer", post(handlers::create_conversation_from_offer))
        
        // P2P Exchange
        .route("/api/p2p/:user_id/offers", get(handlers::get_user_offers))
        .route("/api/p2p/:user_id/offers", post(handlers::create_p2p_offer))
        .route("/api/p2p/offers", get(handlers::get_all_offers))
        .route("/api/p2p/offers/:offer_id", get(handlers::get_offer))
        .route("/api/p2p/:user_id/offers/:offer_id/cancel", post(handlers::cancel_p2p_offer))
        .route("/api/p2p/:user_id/offers/:offer_id/accept", post(handlers::accept_p2p_offer))
        .route("/api/p2p/:user_id/offers/:offer_id/status", get(handlers::get_p2p_offer_status))
        .route("/api/p2p/:user_id/marketplace", get(handlers::get_marketplace_offers))
        .route("/api/p2p/:user_id/exchanges", get(handlers::get_user_exchanges))
        
        // Verification
        .route("/api/verification/:user_id/status", get(handlers::get_verification_status))
        .route("/api/verification/:user_id/identity", post(handlers::submit_identity_verification))
        .route("/api/verification/:user_id/wallet", post(handlers::verify_wallet_ownership))
        .route("/api/verification/:user_id/wallets", get(handlers::get_verified_wallets))
        
        // Privacy & Wallet Management
        .route("/api/privacy/:user_id/temporary-wallets", get(handlers::get_temporary_wallets))
        .route("/api/privacy/:user_id/temporary-wallets", post(handlers::create_temporary_wallet))
        .route("/api/privacy/:user_id/temporary-wallets/:wallet_id/primary", put(handlers::set_primary_temporary_wallet))
        .route("/api/privacy/:user_id/wallets", get(handlers::get_user_wallets_freeze_status))
        .route("/api/privacy/:user_id/wallets/:wallet_address/freeze", post(handlers::freeze_wallet))
        .route("/api/privacy/:user_id/wallets/:wallet_address/unfreeze", post(handlers::unfreeze_wallet))
        .route("/api/privacy/wallets/:wallet_address/frozen", get(handlers::check_wallet_frozen))
        
        // Proximity P2P Transfers - Discovery
        .route("/api/proximity/discovery/start", post(proximity_handlers::start_discovery))
        .route("/api/proximity/discovery/stop", post(proximity_handlers::stop_discovery))
        .route("/api/proximity/peers", get(proximity_handlers::get_discovered_peers))
        .route("/api/proximity/peers/:peer_id/block", post(proximity_handlers::block_peer))
        
        // Proximity P2P Transfers - Transfers
        .route("/api/proximity/transfers", post(proximity_handlers::create_transfer))
        .route("/api/proximity/transfers/sync", post(proximity_handlers::sync_pending_transfers))
        .route("/api/proximity/transfers/:id/accept", post(proximity_handlers::accept_transfer))
        .route("/api/proximity/transfers/:id/reject", post(proximity_handlers::reject_transfer))
        .route("/api/proximity/transfers/:id", get(proximity_handlers::get_transfer_status))
        .route("/api/proximity/transfers/history", get(proximity_handlers::get_transfer_history))
        
        // Stealth Transfers - Address Management
        .route("/api/stealth/generate", post(handlers::generate_stealth_address))
        .route("/api/stealth/prepare-payment", post(handlers::prepare_stealth_payment))
        .route("/api/stealth/send", post(handlers::send_stealth_payment))
        .route("/api/stealth/scan", post(handlers::scan_stealth_payments))
        .route("/api/stealth/shield", post(handlers::shield_funds))
        .route("/api/stealth/unshield", post(handlers::unshield_funds))
        .route("/api/stealth/queue", get(handlers::get_payment_queue))
        .route("/api/stealth/qr-encode", post(handlers::qr_encode_stealth))
        
        // BLE Mesh Network
        .route("/api/mesh/status", get(handlers::get_ble_mesh_status))
        .route("/api/mesh/connect", post(handlers::connect_ble_mesh))
        .route("/api/mesh/disconnect", post(handlers::disconnect_ble_mesh))
        
        // Mesh Price Service - Provider Management
        .route("/api/mesh/provider/enable", post(handlers::enable_mesh_provider))
        .route("/api/mesh/provider/disable", post(handlers::disable_mesh_provider))
        .route("/api/mesh/provider/status", get(handlers::get_mesh_provider_status))
        
        // Mesh Price Service - Price Data Access
        .route("/api/mesh/prices", get(handlers::get_mesh_prices))
        .route("/api/mesh/prices/:asset", get(handlers::get_mesh_price_by_asset))
        .route("/api/mesh/network/status", get(handlers::get_mesh_network_status))
        
        // CoinMarketCap Price Data
        .route("/api/cmc/price", get(handlers::get_crypto_price))
        .route("/api/cmc/prices", get(handlers::get_crypto_prices))
        .route("/api/cmc/convert", get(handlers::convert_crypto))
        
        .with_state(state)
        // Serve static frontend files
        .nest_service("/", ServeDir::new("frontend"))
}
