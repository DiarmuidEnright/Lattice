//! Market-data provider integrations.
//!
//! Groups the external price/market-data clients (CoinMarketCap, Birdeye,
//! Tantum/Helius, SideShift) together with token-metadata lookup and the
//! portfolio price cache.
//!
//! Child modules are re-exported at the crate root (see `lib.rs`) so the
//! crate's public surface (`api::CoinMarketCapService`, `api::SideShiftClient`,
//! ...) is unchanged by this grouping.

pub mod coinmarketcap_service;
pub mod birdeye_service;
pub mod tantum_client;
pub mod sideshift_client;
pub mod token_metadata_service;
pub mod portfolio_cache;
