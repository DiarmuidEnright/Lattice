//! Mesh networking and distributed price-data subsystem.
//!
//! Groups the peer-to-peer mesh price service together with its supporting
//! components: gossip relay, coordination, provider nodes, network-status
//! tracking, message deduplication, price caching/validation, and metrics.
//!
//! Child modules are re-exported at the crate root (see `lib.rs`) so the
//! crate's public surface (`api::MeshPriceService`, `api::mesh_types`, ...)
//! is unchanged by this grouping.

pub mod mesh_types;
pub mod mesh_metrics;
pub mod message_tracker;
pub mod price_cache;
pub mod price_update_validator;
pub mod coordination_service;
pub mod gossip_protocol;
pub mod provider_node;
pub mod network_status_tracker;
pub mod mesh_price_service;
