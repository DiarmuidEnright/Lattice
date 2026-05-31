//! Trading subsystem (API layer).
//!
//! Groups benchmark/price-trigger monitoring, position evaluation and
//! management, auto-staking, conversion, and the trim configuration/executor
//! services.
//!
//! Note: this module is named `trading`, the same as the external `trading`
//! crate dependency. Child modules refer to the external crate as
//! `::trading` to disambiguate from this local module.
//!
//! Child modules are re-exported at the crate root (see `lib.rs`) so the
//! crate's public surface (`api::BenchmarkService`, `api::ConversionService`,
//! ...) is unchanged by this grouping.

pub mod benchmark_service;
pub mod price_monitor;
pub mod conversion_service;
pub mod staking_service;
pub mod trim_config_service;
pub mod trim_executor;
pub mod position_evaluator;
pub mod position_management_service;
