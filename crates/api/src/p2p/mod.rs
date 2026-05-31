//! Peer-to-peer exchange subsystem.
//!
//! Groups the P2P offer/exchange service together with the closely related
//! chat, identity-verification, and privacy (temporary-wallet) services.
//!
//! Child modules are re-exported at the crate root (see `lib.rs`) so the
//! crate's public surface (`api::P2PService`, `api::ChatService`, ...) is
//! unchanged by this grouping.

pub mod p2p_service;
pub mod chat_service;
pub mod verification_service;
pub mod privacy_service;
