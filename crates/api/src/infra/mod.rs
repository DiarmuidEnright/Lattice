//! Cross-cutting infrastructure for the API crate.
//!
//! Groups error types, metrics/health monitoring, structured logging,
//! rate limiting, security middleware, and the dashboard WebSocket service.
//!
//! Note: this `monitoring` child module shares its name with the external
//! `monitoring` crate dependency; the API crate's `monitoring` has always
//! shadowed the external crate at the crate root, and that behavior is
//! preserved here.
//!
//! Child modules are re-exported at the crate root (see `lib.rs`) so the
//! crate's public surface (`api::ApiError`, `api::MetricsCollector`,
//! `api::WebSocketService`, ...) is unchanged by this grouping.

pub mod error;
pub mod monitoring;
pub mod logging;
pub mod rate_limit;
pub mod security;
pub mod websocket_service;
