use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, warn};

use crate::error::{ApiError, ApiResult};

const BIRDEYE_API_BASE: &str = "https://public-api.birdeye.so";
const CACHE_TTL_SECONDS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Blockchain {
    Solana,
    Ethereum,
    BinanceSmartChain,
    Polygon,
}

impl Blockchain {
    pub fn to_birdeye_chain(&self) -> &str {
        match self {
            Blockchain::Solana => "solana",
            Blockchain::Ethereum => "ethereum",
            Blockchain::BinanceSmartChain => "bsc",
            Blockchain::Polygon => "polygon",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletAddress {
    pub blockchain: Blockchain,
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub symbol: String,
    pub name: String,
    pub address: String,
    pub blockchain: Blockchain,
    pub balance: Decimal,
    pub price_usd: Decimal,
    pub value_usd: Decimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiChainPortfolio {
    pub total_value_usd: Decimal,
    pub positions_by_chain: HashMap<String, Vec<Asset>>,
    pub last_updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceData {
    pub price_usd: Decimal,
    pub price_change_24h: Option<Decimal>,
    pub volume_24h: Option<Decimal>,
    pub last_updated: DateTime<Utc>,
}

// Birdeye API response structures
#[derive(Debug, Deserialize)]
struct BirdeyePortfolioResponse {
    success: bool,
    data: Option<BirdeyePortfolioData>,
}

#[derive(Debug, Deserialize)]
struct BirdeyePortfolioData {
    items: Vec<BirdeyeToken>,
}

#[derive(Debug, Deserialize)]
struct BirdeyeToken {
    address: String,
    symbol: String,
    name: String,
    #[serde(rename = "uiAmount")]
    ui_amount: f64,
    #[serde(rename = "priceUsd")]
    price_usd: f64,
    #[serde(rename = "valueUsd")]
    value_usd: f64,
}

#[derive(Debug, Deserialize)]
struct BirdeyePriceResponse {
    success: bool,
    data: Option<BirdeyePriceData>,
}

#[derive(Debug, Deserialize)]
struct BirdeyePriceData {
    value: f64,
    #[allow(dead_code)]
    #[serde(rename = "updateUnixTime")]
    update_unix_time: i64,
}

pub struct BirdeyeService {
    client: Client,
    api_key: String,
    pub redis: redis::aio::ConnectionManager,
    circuit_breaker: Arc<blockchain::circuit_breaker::CircuitBreaker>,
}

impl BirdeyeService {
    pub fn new(api_key: String, redis: redis::aio::ConnectionManager) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        let circuit_breaker_config = blockchain::circuit_breaker::CircuitBreakerConfig {
            failure_threshold: 5,
            success_threshold: 2,
            timeout: Duration::from_secs(60),
        };

        let circuit_breaker = Arc::new(blockchain::circuit_breaker::CircuitBreaker::new(
            "birdeye_api".to_string(),
            circuit_breaker_config,
        ));

        Self {
            client,
            api_key,
            redis,
            circuit_breaker,
        }
    }

    /// Fetch portfolio positions across multiple blockchains
    pub async fn get_multi_chain_portfolio(
        &self,
        wallet_addresses: Vec<WalletAddress>,
    ) -> ApiResult<MultiChainPortfolio> {
        let mut positions_by_chain: HashMap<String, Vec<Asset>> = HashMap::new();
        let mut total_value_usd = Decimal::ZERO;

        for wallet in wallet_addresses {
            // Check cache first
            let cache_key = format!(
                "birdeye:portfolio:{}:{}",
                wallet.address,
                wallet.blockchain.to_birdeye_chain()
            );

            let cached_assets: Option<Vec<Asset>> = self.get_from_cache(&cache_key).await
                .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))?;

            let assets = if let Some(cached) = cached_assets {
                tracing::debug!("Cache hit for portfolio: {}", cache_key);
                cached
            } else {
                tracing::debug!("Cache miss for portfolio: {}", cache_key);
                let fetched = self
                    .fetch_portfolio_from_api(&wallet.address, &wallet.blockchain)
                    .await?;
                self.set_in_cache(&cache_key, &fetched, CACHE_TTL_SECONDS)
                    .await
                    .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))?;
                fetched
            };

            // Aggregate values
            for asset in &assets {
                total_value_usd += asset.value_usd;
            }

            positions_by_chain
                .entry(wallet.blockchain.to_birdeye_chain().to_string())
                .or_default()
                .extend(assets);
        }

        Ok(MultiChainPortfolio {
            total_value_usd,
            positions_by_chain,
            last_updated: Utc::now(),
        })
    }

    /// Get real-time price data for an asset
    pub async fn get_asset_price(
        &self,
        chain: &Blockchain,
        token_address: &str,
    ) -> ApiResult<PriceData> {
        let cache_key = format!(
            "birdeye:price:{}:{}",
            chain.to_birdeye_chain(),
            token_address
        );

        // Check cache first (10 second TTL for prices)
        if let Some(cached) = self.get_from_cache::<PriceData>(&cache_key).await
            .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))? {
            tracing::debug!("Cache hit for price: {}", cache_key);
            return Ok(cached);
        }

        tracing::debug!("Cache miss for price: {}", cache_key);
        let price_data = self.fetch_price_from_api(chain, token_address).await?;
        self.set_in_cache(&cache_key, &price_data, 10).await
            .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))?;

        Ok(price_data)
    }

    /// Fetch portfolio from Birdeye API with retry logic and circuit breaker
    async fn fetch_portfolio_from_api(
        &self,
        wallet_address: &str,
        blockchain: &Blockchain,
    ) -> ApiResult<Vec<Asset>> {
        // Check circuit breaker
        if !self.circuit_breaker.is_request_allowed().await {
            warn!("Birdeye API circuit breaker is open");
            return Err(ApiError::CircuitBreakerOpen(
                "Birdeye API is temporarily unavailable. Please try again later.".to_string()
            ));
        }

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            attempts += 1;

            match self
                .fetch_portfolio_from_api_once(wallet_address, blockchain)
                .await
            {
                Ok(assets) => {
                    self.circuit_breaker.record_success().await;
                    return Ok(assets);
                }
                Err(e) if attempts < max_attempts => {
                    let backoff = Duration::from_millis(100 * 2_u64.pow(attempts - 1));
                    warn!(
                        "Birdeye API request failed (attempt {}/{}): {}. Retrying in {:?}",
                        attempts,
                        max_attempts,
                        e,
                        backoff
                    );
                    tokio::time::sleep(backoff).await;
                }
                Err(e) => {
                    error!(
                        "Birdeye API request failed after {} attempts: {}",
                        max_attempts,
                        e
                    );
                    self.circuit_breaker.record_failure().await;
                    return Err(ApiError::BirdeyeApiError(e.to_string()));
                }
            }
        }
    }

    async fn fetch_portfolio_from_api_once(
        &self,
        wallet_address: &str,
        blockchain: &Blockchain,
    ) -> Result<Vec<Asset>> {
        let url = format!(
            "{}/v1/wallet/token_list?wallet={}",
            BIRDEYE_API_BASE, wallet_address
        );

        let response = self
            .client
            .get(&url)
            .header("X-API-KEY", &self.api_key)
            .header("x-chain", blockchain.to_birdeye_chain())
            .send()
            .await
            .context("Failed to send request to Birdeye API")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Birdeye API returned error status: {}",
                response.status()
            );
        }

        let birdeye_response: BirdeyePortfolioResponse = response
            .json()
            .await
            .context("Failed to parse Birdeye API response")?;

        if !birdeye_response.success {
            anyhow::bail!("Birdeye API returned success=false");
        }

        let data = birdeye_response
            .data
            .context("Birdeye API returned no data")?;

        Ok(self.normalize_portfolio(data, blockchain.clone()))
    }

    async fn fetch_price_from_api(
        &self,
        blockchain: &Blockchain,
        token_address: &str,
    ) -> ApiResult<PriceData> {
        // Check circuit breaker
        if !self.circuit_breaker.is_request_allowed().await {
            warn!("Birdeye API circuit breaker is open");
            return Err(ApiError::CircuitBreakerOpen(
                "Birdeye API is temporarily unavailable. Please try again later.".to_string()
            ));
        }

        let url = format!("{}/defi/price?address={}", BIRDEYE_API_BASE, token_address);

        let response = self
            .client
            .get(&url)
            .header("X-API-KEY", &self.api_key)
            .header("x-chain", blockchain.to_birdeye_chain())
            .send()
            .await
            .map_err(|e| {
                ApiError::BirdeyeApiError(format!("Failed to send price request: {}", e))
            })?;

        if !response.status().is_success() {
            self.circuit_breaker.record_failure().await;
            return Err(ApiError::BirdeyeApiError(format!(
                "Birdeye price API returned error status: {}",
                response.status()
            )));
        }

        let birdeye_response: BirdeyePriceResponse = response
            .json()
            .await
            .map_err(|e| {
                ApiError::BirdeyeApiError(format!("Failed to parse price response: {}", e))
            })?;

        if !birdeye_response.success {
            self.circuit_breaker.record_failure().await;
            return Err(ApiError::BirdeyeApiError("Birdeye price API returned success=false".to_string()));
        }

        let data = birdeye_response
            .data
            .ok_or_else(|| ApiError::BirdeyeApiError("Birdeye price API returned no data".to_string()))?;

        self.circuit_breaker.record_success().await;

        Ok(PriceData {
            price_usd: Decimal::from_f64_retain(data.value)
                .unwrap_or(Decimal::ZERO),
            price_change_24h: None,
            volume_24h: None,
            last_updated: Utc::now(),
        })
    }

    /// Normalize Birdeye response to internal Portfolio format
    fn normalize_portfolio(
        &self,
        data: BirdeyePortfolioData,
        blockchain: Blockchain,
    ) -> Vec<Asset> {
        data.items
            .into_iter()
            .map(|token| Asset {
                symbol: token.symbol,
                name: token.name,
                address: token.address,
                blockchain: blockchain.clone(),
                balance: Decimal::from_f64_retain(token.ui_amount).unwrap_or(Decimal::ZERO),
                price_usd: Decimal::from_f64_retain(token.price_usd).unwrap_or(Decimal::ZERO),
                value_usd: Decimal::from_f64_retain(token.value_usd).unwrap_or(Decimal::ZERO),
            })
            .collect()
    }

    async fn get_from_cache<T: for<'de> Deserialize<'de>>(
        &self,
        key: &str,
    ) -> Result<Option<T>> {
        let mut conn = self.redis.clone();
        let value: Option<String> = redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn)
            .await
            .context("Failed to get from Redis")?;

        match value {
            Some(json) => {
                let data: T = serde_json::from_str(&json)
                    .context("Failed to deserialize cached data")?;
                Ok(Some(data))
            }
            None => Ok(None),
        }
    }

    async fn set_in_cache<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl_seconds: u64,
    ) -> Result<()> {
        let json = serde_json::to_string(value).context("Failed to serialize data")?;
        let mut conn = self.redis.clone();
        let _: () = redis::cmd("SETEX")
            .arg(key)
            .arg(ttl_seconds)
            .arg(json)
            .query_async(&mut conn)
            .await
            .context("Failed to set in Redis")?;
        Ok(())
    }
}
