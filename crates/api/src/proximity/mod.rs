//! Proximity-based P2P transfer subsystem (API layer).
//!
//! Groups the API-side proximity service, HTTP handlers, WebSocket event
//! stream, and receipt integration that sit on top of the external
//! `proximity` crate.
//!
//! Note: this module is named `proximity`, the same as the external
//! `proximity` crate dependency. Within these child modules a bare
//! `proximity::` path still resolves to the external crate via the extern
//! prelude; the crate root refers to the external crate as `::proximity`
//! (see `lib.rs`).
//!
//! Child modules are re-exported at the crate root (see `lib.rs`) so the
//! crate's public surface (`api::ProximityWebSocketService`,
//! `api::create_proximity_receipt`, ...) is unchanged by this grouping.

pub mod proximity_receipt_integration;
pub mod proximity_service;
pub mod proximity_handlers;
pub mod proximity_websocket;
