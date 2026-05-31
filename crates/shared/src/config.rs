use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub database: DatabaseConfig,
    pub redis: RedisConfig,
    pub solana: SolanaConfig,
    pub claude: ClaudeConfig,
    pub stripe: StripeConfig,
    pub jwt: JwtConfig,
    pub server: ServerConfig,
    pub monitoring: MonitoringConfig,
    pub rate_limit: RateLimitConfig,
    pub birdeye: BirdeyeConfig,
    pub sideshift: SideShiftConfig,
    pub mesh_network: MeshNetworkConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub pool_size: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SolanaConfig {
    pub rpc_url: String,
    pub rpc_fallback_url: String,
    pub network: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeConfig {
    pub api_key: String,
    pub model: String,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StripeConfig {
    pub secret_key: String,
    pub webhook_secret: String,
    pub basic_price_id: String,
    pub premium_price_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JwtConfig {
    pub secret: String,
    pub expiration_hours: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MonitoringConfig {
    pub whale_check_interval_seconds: u64,
    pub worker_pool_size: usize,
    pub whales_per_worker: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    pub requests_per_minute: u32,
    pub burst: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BirdeyeConfig {
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SideShiftConfig {
    pub affiliate_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MeshNetworkConfig {
    /// Fetch interval for provider nodes in seconds (default: 30)
    pub provider_fetch_interval_secs: u64,
    /// Coordination window in seconds to avoid duplicate fetches (default: 5)
    pub coordination_window_secs: u64,
    /// Initial TTL value for price update messages (default: 10)
    pub message_ttl: u32,
    /// Maximum number of entries in the seen messages cache (default: 10000)
    pub seen_messages_cache_size: usize,
    /// Expiration time for seen messages in seconds (default: 300 = 5 minutes)
    pub seen_messages_expiration_secs: u64,
    /// Maximum number of peer connections per node (default: 10)
    pub max_peer_connections: usize,
    /// Minimum number of peer connections to maintain (default: 3)
    pub min_peer_connections: usize,
    /// Price data staleness threshold in seconds (default: 3600 = 1 hour)
    pub staleness_threshold_secs: u64,
    /// Provider offline indicator threshold in seconds (default: 600 = 10 minutes)
    pub offline_indicator_threshold_secs: u64,
    /// Price discrepancy threshold as percentage (default: 5.0 = 5%)
    pub price_discrepancy_threshold_percent: f64,
}

/// The required string credentials, in the order they are reported. This is the
/// single source of truth for "which environment variables must be present for
/// the backend to start" and is shared by both `from_env` (real startup) and the
/// property test for the fail-fast invariant (Requirements 5.6, 5.8, 5.9).
const REQUIRED_VARS: [&str; 12] = [
    "DATABASE_URL",
    "REDIS_URL",
    "SOLANA_RPC_URL",
    "SOLANA_RPC_FALLBACK_URL",
    "SOLANA_NETWORK",
    "CLAUDE_API_KEY",
    "CLAUDE_MODEL",
    "STRIPE_SECRET_KEY",
    "STRIPE_WEBHOOK_SECRET",
    "STRIPE_BASIC_PRICE_ID",
    "STRIPE_PREMIUM_PRICE_ID",
    "JWT_SECRET",
];

/// Reads a required environment variable through `lookup`. When the variable
/// is absent, its name is recorded in `missing` (so the caller can report every
/// missing credential at once) and an empty placeholder is returned instead of
/// short-circuiting. This is what enables the collect-all-missing-then-error
/// strategy required for fail-fast startup (Requirements 5.6, 5.8, 5.9).
fn require_var<F>(lookup: &F, missing: &mut Vec<String>, key: &str) -> String
where
    F: Fn(&str) -> Option<String>,
{
    match lookup(key) {
        Some(value) => value,
        None => {
            missing.push(key.to_string());
            String::new()
        }
    }
}

/// Returns the names of every required credential that `lookup` reports as
/// absent, preserving the order of `REQUIRED_VARS`. This is the pure core of the
/// fail-fast decision: given a way to read variables, it answers "which required
/// ones are missing?" without touching the process environment, so it can be
/// driven both by real startup (`from_env`) and by the property test.
fn collect_missing<F>(lookup: &F) -> Vec<String>
where
    F: Fn(&str) -> Option<String>,
{
    let mut missing: Vec<String> = Vec::new();
    for key in REQUIRED_VARS {
        let _ = require_var(lookup, &mut missing, key);
    }
    missing
}

/// Builds the single error message that names every missing required credential.
fn missing_vars_message(missing: &[String]) -> String {
    format!(
        "Missing required environment variables: {}",
        missing.join(", ")
    )
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        dotenv::dotenv().ok();

        let lookup = |key: &str| env::var(key).ok();

        // Fail fast: collect the names of every absent required credential
        // instead of failing on the first one with an opaque `VarError`. If any
        // are missing, return a single error naming every one of them before any
        // further work. `main.rs` propagates this via `?`, so the process exits
        // before `TcpListener::bind` and serves no request (Requirements 5.6,
        // 5.8, 5.9). Variables with non-secret defaults below remain optional.
        let missing = collect_missing(&lookup);
        if !missing.is_empty() {
            anyhow::bail!(missing_vars_message(&missing));
        }

        // Every required credential is present, so reading it yields its real
        // value (`unwrap_or_default` never falls back to the empty placeholder).
        let config = Config {
            database: DatabaseConfig {
                url: lookup("DATABASE_URL").unwrap_or_default(),
                max_connections: env::var("DATABASE_MAX_CONNECTIONS")
                    .unwrap_or_else(|_| "10".to_string())
                    .parse()?,
            },
            redis: RedisConfig {
                url: lookup("REDIS_URL").unwrap_or_default(),
                pool_size: env::var("REDIS_POOL_SIZE")
                    .unwrap_or_else(|_| "10".to_string())
                    .parse()?,
            },
            solana: SolanaConfig {
                rpc_url: lookup("SOLANA_RPC_URL").unwrap_or_default(),
                rpc_fallback_url: lookup("SOLANA_RPC_FALLBACK_URL").unwrap_or_default(),
                network: lookup("SOLANA_NETWORK").unwrap_or_default(),
            },
            claude: ClaudeConfig {
                api_key: lookup("CLAUDE_API_KEY").unwrap_or_default(),
                model: lookup("CLAUDE_MODEL").unwrap_or_default(),
                max_tokens: env::var("CLAUDE_MAX_TOKENS")
                    .unwrap_or_else(|_| "4096".to_string())
                    .parse()?,
            },
            stripe: StripeConfig {
                secret_key: lookup("STRIPE_SECRET_KEY").unwrap_or_default(),
                webhook_secret: lookup("STRIPE_WEBHOOK_SECRET").unwrap_or_default(),
                basic_price_id: lookup("STRIPE_BASIC_PRICE_ID").unwrap_or_default(),
                premium_price_id: lookup("STRIPE_PREMIUM_PRICE_ID").unwrap_or_default(),
            },
            jwt: JwtConfig {
                secret: lookup("JWT_SECRET").unwrap_or_default(),
                expiration_hours: env::var("JWT_EXPIRATION_HOURS")
                    .unwrap_or_else(|_| "24".to_string())
                    .parse()?,
            },
            server: ServerConfig {
                host: env::var("SERVER_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
                port: env::var("SERVER_PORT")
                    .unwrap_or_else(|_| "8080".to_string())
                    .parse()?,
            },
            monitoring: MonitoringConfig {
                whale_check_interval_seconds: env::var("WHALE_CHECK_INTERVAL_SECONDS")
                    .unwrap_or_else(|_| "30".to_string())
                    .parse()?,
                worker_pool_size: env::var("WORKER_POOL_SIZE")
                    .unwrap_or_else(|_| "10".to_string())
                    .parse()?,
                whales_per_worker: env::var("WHALES_PER_WORKER")
                    .unwrap_or_else(|_| "100".to_string())
                    .parse()?,
            },
            rate_limit: RateLimitConfig {
                requests_per_minute: env::var("RATE_LIMIT_REQUESTS_PER_MINUTE")
                    .unwrap_or_else(|_| "60".to_string())
                    .parse()?,
                burst: env::var("RATE_LIMIT_BURST")
                    .unwrap_or_else(|_| "10".to_string())
                    .parse()?,
            },
            birdeye: BirdeyeConfig {
                api_key: env::var("BIRDEYE_API_KEY")
                    .unwrap_or_else(|_| "demo_key".to_string()),
            },
            sideshift: SideShiftConfig {
                affiliate_id: env::var("SIDESHIFT_AFFILIATE_ID").ok(),
            },
            mesh_network: MeshNetworkConfig {
                provider_fetch_interval_secs: env::var("MESH_PROVIDER_FETCH_INTERVAL_SECS")
                    .unwrap_or_else(|_| "30".to_string())
                    .parse()?,
                coordination_window_secs: env::var("MESH_COORDINATION_WINDOW_SECS")
                    .unwrap_or_else(|_| "5".to_string())
                    .parse()?,
                message_ttl: env::var("MESH_MESSAGE_TTL")
                    .unwrap_or_else(|_| "10".to_string())
                    .parse()?,
                seen_messages_cache_size: env::var("MESH_SEEN_MESSAGES_CACHE_SIZE")
                    .unwrap_or_else(|_| "10000".to_string())
                    .parse()?,
                seen_messages_expiration_secs: env::var("MESH_SEEN_MESSAGES_EXPIRATION_SECS")
                    .unwrap_or_else(|_| "300".to_string())
                    .parse()?,
                max_peer_connections: env::var("MESH_MAX_PEER_CONNECTIONS")
                    .unwrap_or_else(|_| "10".to_string())
                    .parse()?,
                min_peer_connections: env::var("MESH_MIN_PEER_CONNECTIONS")
                    .unwrap_or_else(|_| "3".to_string())
                    .parse()?,
                staleness_threshold_secs: env::var("MESH_STALENESS_THRESHOLD_SECS")
                    .unwrap_or_else(|_| "3600".to_string())
                    .parse()?,
                offline_indicator_threshold_secs: env::var("MESH_OFFLINE_INDICATOR_THRESHOLD_SECS")
                    .unwrap_or_else(|_| "600".to_string())
                    .parse()?,
                price_discrepancy_threshold_percent: env::var("MESH_PRICE_DISCREPANCY_THRESHOLD_PERCENT")
                    .unwrap_or_else(|_| "5.0".to_string())
                    .parse()?,
            },
        };

        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With a couple of required variables absent, the collected error names
    /// every missing credential. Validates Requirements 5.6, 5.8, 5.9.
    #[test]
    fn missing_required_vars_are_all_named() {
        let present: std::collections::HashSet<&str> =
            ["DATABASE_URL", "REDIS_URL", "SOLANA_RPC_URL"]
                .into_iter()
                .collect();

        // Hermetic lookup: pretends only `present` keys exist. This exercises
        // the same collect-then-error logic `from_env` uses without mutating
        // the real process environment (which would be flaky under parallel
        // test execution and could read the repo `.env`).
        let lookup = |key: &str| {
            if present.contains(key) {
                Some("value".to_string())
            } else {
                None
            }
        };

        let mut missing: Vec<String> = Vec::new();
        // Required string credentials, mirroring `from_env`.
        for key in [
            "DATABASE_URL",
            "REDIS_URL",
            "SOLANA_RPC_URL",
            "SOLANA_RPC_FALLBACK_URL",
            "SOLANA_NETWORK",
            "CLAUDE_API_KEY",
            "CLAUDE_MODEL",
            "STRIPE_SECRET_KEY",
            "STRIPE_WEBHOOK_SECRET",
            "STRIPE_BASIC_PRICE_ID",
            "STRIPE_PREMIUM_PRICE_ID",
            "JWT_SECRET",
        ] {
            let _ = require_var(&lookup, &mut missing, key);
        }

        // Exactly the absent ones are collected (no false positives).
        assert_eq!(
            missing,
            vec![
                "SOLANA_RPC_FALLBACK_URL".to_string(),
                "SOLANA_NETWORK".to_string(),
                "CLAUDE_API_KEY".to_string(),
                "CLAUDE_MODEL".to_string(),
                "STRIPE_SECRET_KEY".to_string(),
                "STRIPE_WEBHOOK_SECRET".to_string(),
                "STRIPE_BASIC_PRICE_ID".to_string(),
                "STRIPE_PREMIUM_PRICE_ID".to_string(),
                "JWT_SECRET".to_string(),
            ]
        );

        let message = missing_vars_message(&missing);
        assert!(message.starts_with("Missing required environment variables: "));
        // Every missing credential is named in the single error message.
        for key in &missing {
            assert!(message.contains(key.as_str()), "message must name {key}");
        }
        // Present credentials are not falsely reported as missing.
        assert!(!message.contains("DATABASE_URL"));
        assert!(!message.contains("REDIS_URL, "));
    }

    /// When every required credential is present, nothing is collected.
    #[test]
    fn no_missing_when_all_present() {
        let lookup = |_key: &str| Some("value".to_string());
        let mut missing: Vec<String> = Vec::new();
        let _ = require_var(&lookup, &mut missing, "DATABASE_URL");
        let _ = require_var(&lookup, &mut missing, "JWT_SECRET");
        assert!(missing.is_empty());
    }

    // Feature: demo-cleanup-restructure, Property 7: Fail-fast config invariant
    //
    // Property 7 (Validates: Requirements 5.6, 5.8, 5.9): for ANY non-empty
    // subset S of the required credentials, when exactly the credentials in S
    // are absent, the fail-fast logic (a) produces a non-empty missing set / an
    // error, and (b) names EVERY credential in S with NO false positives (no
    // name outside S). We drive the SAME pure core (`collect_missing`) that
    // `from_env` uses for its fail-fast decision, via a generated lookup
    // closure, so no real process environment variable is ever mutated (which
    // would be globally shared and unsound under parallel test execution).
    mod property7 {
        use super::super::{collect_missing, missing_vars_message, REQUIRED_VARS};
        use proptest::prelude::*;

        proptest! {
            // 256 cases (proptest default), comfortably above the required
            // minimum of 100 iterations for this property.
            #![proptest_config(ProptestConfig::with_cases(256))]

            #[test]
            fn fail_fast_config_invariant(
                // A random presence flag per required var; `true` means "absent".
                // Filtered to a NON-EMPTY subset S of absent credentials.
                absent_flags in prop::collection::vec(any::<bool>(), REQUIRED_VARS.len())
                    .prop_filter("at least one credential must be absent", |flags| flags.iter().any(|&b| b))
            ) {
                // Build subset S (the absent credentials), preserving REQUIRED_VARS order.
                let subset: Vec<String> = REQUIRED_VARS
                    .iter()
                    .zip(absent_flags.iter())
                    .filter_map(|(key, &absent)| if absent { Some(key.to_string()) } else { None })
                    .collect();

                // Lookup returns None for names in S (absent) and a value otherwise.
                let lookup = |key: &str| {
                    if subset.iter().any(|k| k == key) {
                        None
                    } else {
                        Some("value".to_string())
                    }
                };

                let missing = collect_missing(&lookup);

                // (a) Fail-fast fires: the missing set is non-empty, so `from_env`
                //     would `bail!` before constructing config / binding port 3000.
                prop_assert!(!missing.is_empty());

                // (b) The missing set equals S exactly: every member of S is named
                //     (no omissions) and nothing outside S is named (no false
                //     positives). Equality also covers order since both follow
                //     REQUIRED_VARS order.
                prop_assert_eq!(&missing, &subset);

                // The single fail-fast error message names each absent credential
                // and no credential that was present.
                let message = missing_vars_message(&missing);
                for key in REQUIRED_VARS {
                    let present_in_message = message.contains(key);
                    let should_be_present = subset.iter().any(|k| k == key);
                    prop_assert_eq!(
                        present_in_message,
                        should_be_present,
                        "credential {} presence in error message ({}) must match its membership in the absent subset ({})",
                        key,
                        present_in_message,
                        should_be_present
                    );
                }
            }
        }
    }
}
