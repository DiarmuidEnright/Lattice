use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
};
use futures::{sink::SinkExt, stream::StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::AppState;

/// Maximum number of messages to buffer in the broadcast channel
const CHANNEL_CAPACITY: usize = 100;

/// WebSocket update types that can be pushed to clients
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardUpdate {
    /// Price change for an asset
    PriceUpdate {
        asset: String,
        blockchain: String,
        price: String,
        change_24h: String,
        timestamp: i64,
    },
    /// Trade execution notification
    TradeExecuted {
        trade_id: String,
        asset: String,
        action: String,
        amount: String,
        price: String,
        timestamp: i64,
    },
    /// Agentic trim execution
    TrimExecuted {
        trim_id: String,
        asset: String,
        amount_sold: String,
        profit_realized: String,
        reasoning: String,
        timestamp: i64,
    },
    /// Benchmark triggered
    BenchmarkTriggered {
        benchmark_id: String,
        asset: String,
        target_price: String,
        current_price: String,
        action: String,
        timestamp: i64,
    },
    /// Portfolio value update
    PortfolioUpdate {
        total_value_usd: String,
        change_24h: String,
        timestamp: i64,
    },
    /// Conversion completed
    ConversionCompleted {
        conversion_id: String,
        from_asset: String,
        to_asset: String,
        from_amount: String,
        to_amount: String,
        timestamp: i64,
    },
    /// Price update from mesh network
    PriceMeshUpdate {
        asset: String,
        blockchain: String,
        price: String,
        change_24h: Option<String>,
        timestamp: i64,
        source_node_id: String,
        freshness: String,
    },
    /// Stealth payment detected during blockchain scan
    StealthPaymentDetected {
        payment_id: String,
        stealth_address: String,
        amount: u64,
        ephemeral_public_key: String,
        viewing_tag: String,
        signature: String,
        slot: u64,
        timestamp: i64,
    },
    /// Payment added to offline queue
    PaymentQueued {
        payment_id: String,
        stealth_address: String,
        amount: u64,
        timestamp: i64,
    },
    /// Payment successfully settled on-chain
    PaymentSettled {
        payment_id: String,
        stealth_address: String,
        amount: u64,
        signature: String,
        timestamp: i64,
    },
    /// Payment failed after retries
    PaymentFailed {
        payment_id: String,
        stealth_address: String,
        amount: u64,
        error: String,
        retry_count: u32,
        timestamp: i64,
    },
}

/// WebSocket service for managing real-time dashboard updates
#[derive(Clone)]
pub struct WebSocketService {
    tx: broadcast::Sender<DashboardUpdate>,
}

impl WebSocketService {
    /// Create a new WebSocket service
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx }
    }

    /// Get a receiver for dashboard updates
    pub fn subscribe(&self) -> broadcast::Receiver<DashboardUpdate> {
        self.tx.subscribe()
    }

    /// Broadcast a price update to all connected clients
    pub fn broadcast_price_update(
        &self,
        asset: String,
        blockchain: String,
        price: String,
        change_24h: String,
    ) {
        let update = DashboardUpdate::PriceUpdate {
            asset,
            blockchain,
            price,
            change_24h,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast price update: {}", e);
        }
    }

    /// Broadcast a trade execution to all connected clients
    pub fn broadcast_trade_executed(
        &self,
        trade_id: Uuid,
        asset: String,
        action: String,
        amount: String,
        price: String,
    ) {
        let update = DashboardUpdate::TradeExecuted {
            trade_id: trade_id.to_string(),
            asset,
            action,
            amount,
            price,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast trade execution: {}", e);
        }
    }

    /// Broadcast a trim execution to all connected clients
    pub fn broadcast_trim_executed(
        &self,
        trim_id: Uuid,
        asset: String,
        amount_sold: String,
        profit_realized: String,
        reasoning: String,
    ) {
        let update = DashboardUpdate::TrimExecuted {
            trim_id: trim_id.to_string(),
            asset,
            amount_sold,
            profit_realized,
            reasoning,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast trim execution: {}", e);
        }
    }

    /// Broadcast a benchmark trigger to all connected clients
    pub fn broadcast_benchmark_triggered(
        &self,
        benchmark_id: Uuid,
        asset: String,
        target_price: String,
        current_price: String,
        action: String,
    ) {
        let update = DashboardUpdate::BenchmarkTriggered {
            benchmark_id: benchmark_id.to_string(),
            asset,
            target_price,
            current_price,
            action,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast benchmark trigger: {}", e);
        }
    }

    /// Broadcast a portfolio value update to all connected clients
    pub fn broadcast_portfolio_update(&self, total_value_usd: String, change_24h: String) {
        let update = DashboardUpdate::PortfolioUpdate {
            total_value_usd,
            change_24h,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast portfolio update: {}", e);
        }
    }

    /// Broadcast a conversion completion to all connected clients
    pub fn broadcast_conversion_completed(
        &self,
        conversion_id: Uuid,
        from_asset: String,
        to_asset: String,
        from_amount: String,
        to_amount: String,
    ) {
        let update = DashboardUpdate::ConversionCompleted {
            conversion_id: conversion_id.to_string(),
            from_asset,
            to_asset,
            from_amount,
            to_amount,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast conversion completion: {}", e);
        }
    }

    /// Broadcast a mesh network price update to all connected clients
    /// 
    /// This method is used by the gossip protocol to push price updates
    /// received from the mesh network to WebSocket clients.
    /// 
    /// Requirements: 12.1
    pub fn broadcast_mesh_price_update(
        &self,
        asset: String,
        blockchain: String,
        price: String,
        change_24h: Option<String>,
        timestamp: chrono::DateTime<chrono::Utc>,
        source_node_id: Uuid,
        freshness: String,
    ) {
        let update = DashboardUpdate::PriceMeshUpdate {
            asset,
            blockchain,
            price,
            change_24h,
            timestamp: timestamp.timestamp(),
            source_node_id: source_node_id.to_string(),
            freshness,
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast mesh price update: {}", e);
        }
    }

    /// Broadcast a stealth payment detection to all connected clients
    /// 
    /// This method is called when the stealth scanner detects an incoming
    /// payment during blockchain scanning.
    /// 
    /// Requirements: 10.4, 10.7
    pub fn broadcast_stealth_payment_detected(
        &self,
        payment_id: Uuid,
        stealth_address: String,
        amount: u64,
        ephemeral_public_key: String,
        viewing_tag: [u8; 4],
        signature: String,
        slot: u64,
    ) {
        let update = DashboardUpdate::StealthPaymentDetected {
            payment_id: payment_id.to_string(),
            stealth_address,
            amount,
            ephemeral_public_key,
            viewing_tag: hex::encode(viewing_tag),
            signature,
            slot,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast stealth payment detection: {}", e);
        }
    }

    /// Broadcast a payment queued event to all connected clients
    /// 
    /// This method is called when a stealth payment is added to the offline
    /// payment queue.
    /// 
    /// Requirements: 10.4, 10.7
    pub fn broadcast_payment_queued(
        &self,
        payment_id: Uuid,
        stealth_address: String,
        amount: u64,
    ) {
        let update = DashboardUpdate::PaymentQueued {
            payment_id: payment_id.to_string(),
            stealth_address,
            amount,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast payment queued: {}", e);
        }
    }

    /// Broadcast a payment settled event to all connected clients
    /// 
    /// This method is called when a queued payment successfully settles
    /// on the blockchain.
    /// 
    /// Requirements: 10.4, 10.7
    pub fn broadcast_payment_settled(
        &self,
        payment_id: Uuid,
        stealth_address: String,
        amount: u64,
        signature: String,
    ) {
        let update = DashboardUpdate::PaymentSettled {
            payment_id: payment_id.to_string(),
            stealth_address,
            amount,
            signature,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast payment settled: {}", e);
        }
    }

    /// Broadcast a payment failed event to all connected clients
    /// 
    /// This method is called when a queued payment fails after maximum
    /// retry attempts.
    /// 
    /// Requirements: 10.4, 10.7
    pub fn broadcast_payment_failed(
        &self,
        payment_id: Uuid,
        stealth_address: String,
        amount: u64,
        error: String,
        retry_count: u32,
    ) {
        let update = DashboardUpdate::PaymentFailed {
            payment_id: payment_id.to_string(),
            stealth_address,
            amount,
            error,
            retry_count,
            timestamp: chrono::Utc::now().timestamp(),
        };
        
        if let Err(e) = self.tx.send(update) {
            warn!("Failed to broadcast payment failed: {}", e);
        }
    }
}

impl Default for WebSocketService {
    fn default() -> Self {
        Self::new()
    }
}

/// WebSocket handler for dashboard updates
pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// Handle an individual WebSocket connection
/// 
/// Requirements: 12.2 - Send initial cached data on WebSocket connection
/// Requirements: 12.4 - Send only changed assets in updates (delta updates)
async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();
    
    // Subscribe to dashboard updates
    let mut rx = state.websocket_service.subscribe();
    
    info!("New WebSocket connection established");
    
    // Track last sent prices for delta updates (Requirement 12.4)
    let mut last_sent_prices: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    
    // Send initial cached mesh price data if available
    // Note: This will be fully integrated when mesh_price_service is added to AppState
    // For now, we prepare the infrastructure for sending initial data
    
    // Spawn a task to send updates to the client
    let mut send_task = tokio::spawn(async move {
        // TODO: When mesh_price_service is added to AppState, send initial cached data here:
        // if let Ok(cached_prices) = state.mesh_price_service.get_all_price_data().await {
        //     for (asset, data) in cached_prices {
        //         let freshness = DataFreshness::from_timestamp(data.timestamp);
        //         let freshness_str = match freshness { ... };
        //         let initial_update = DashboardUpdate::PriceMeshUpdate { ... };
        //         if let Ok(json) = serde_json::to_string(&initial_update) {
        //             let _ = sender.send(Message::Text(json)).await;
        //         }
        //         // Track initial prices for delta updates
        //         last_sent_prices.insert(asset, data.price);
        //     }
        // }
        
        while let Ok(update) = rx.recv().await {
            // For mesh price updates, check if the price has changed (delta update)
            let should_send = match &update {
                DashboardUpdate::PriceMeshUpdate { asset, price, .. } => {
                    // Check if price has changed since last send
                    let has_changed = last_sent_prices.get(asset)
                        .map(|last_price| last_price != price)
                        .unwrap_or(true); // Send if we haven't sent this asset before
                    
                    if has_changed {
                        // Update tracking
                        last_sent_prices.insert(asset.clone(), price.clone());
                        true
                    } else {
                        // Price hasn't changed, skip sending
                        false
                    }
                }
                // For non-mesh updates, always send
                _ => true,
            };
            
            if should_send {
                // Serialize the update to JSON
                match serde_json::to_string(&update) {
                    Ok(json) => {
                        if sender.send(Message::Text(json)).await.is_err() {
                            // Client disconnected
                            break;
                        }
                    }
                    Err(e) => {
                        error!("Failed to serialize dashboard update: {}", e);
                    }
                }
            }
        }
    });
    
    // Spawn a task to receive messages from the client (for ping/pong)
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Close(_) => {
                    info!("WebSocket client requested close");
                    break;
                }
                Message::Ping(_data) => {
                    // Echo back pong (axum handles this automatically)
                    info!("Received ping from client");
                }
                Message::Pong(_) => {
                    // Client responded to our ping
                }
                Message::Text(text) => {
                    // Could handle client commands here if needed
                    info!("Received text message from client: {}", text);
                }
                _ => {}
            }
        }
    });
    
    // Wait for either task to finish
    tokio::select! {
        _ = (&mut send_task) => {
            recv_task.abort();
            info!("WebSocket send task completed");
        }
        _ = (&mut recv_task) => {
            send_task.abort();
            info!("WebSocket receive task completed");
        }
    }
    
    info!("WebSocket connection closed");
}
