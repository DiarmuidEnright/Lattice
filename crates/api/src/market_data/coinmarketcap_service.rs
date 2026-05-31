use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::warn;

use crate::error::{ApiError, ApiResult};

const CMC_API_BASE: &str = "https://pro-api.coinmarketcap.com/v1";
const CACHE_TTL_SECONDS: u64 = 60;

/// CoinMarketCap API response structures
#[derive(Debug, Deserialize)]
struct CmcResponse<T> {
    data: T,
    status: CmcStatus,
}

#[derive(Debug, Deserialize)]
struct CmcStatus {
    // Parsed from the CoinMarketCap response to document its shape; not read by
    // our logic (serde would ignore it if dropped, but it is kept for clarity).
    #[allow(dead_code)]
    timestamp: String,
    error_code: i32,
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CmcQuote {
    #[serde(rename = "USD")]
    usd: CmcUsdQuote,
}

#[derive(Debug, Deserialize)]
struct CmcUsdQuote {
    price: f64,
    volume_24h: Option<f64>,
    percent_change_24h: Option<f64>,
    market_cap: Option<f64>,
    // Parsed from the CoinMarketCap USD quote to document its shape; our code
    // stamps its own Utc::now() timestamp instead of reading this field.
    #[allow(dead_code)]
    last_updated: String,
}

#[derive(Debug, Deserialize)]
struct CmcCryptocurrency {
    // `id` and `slug` are part of the CoinMarketCap cryptocurrency object and are
    // parsed to document its shape; only `name`, `symbol`, and `quote` are read.
    #[allow(dead_code)]
    id: i64,
    name: String,
    symbol: String,
    #[allow(dead_code)]
    slug: String,
    quote: CmcQuote,
}

#[derive(Debug, Deserialize)]
struct CmcConversionResponse {
    amount: f64,
    // Parsed from the CoinMarketCap conversion response to document its shape;
    // our code stamps its own Utc::now() timestamp instead of reading this field.
    #[allow(dead_code)]
    last_updated: String,
    quote: HashMap<String, CmcConversionQuote>,
}

#[derive(Debug, Deserialize)]
struct CmcConversionQuote {
    price: f64,
}

/// Public types for the service
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmcPriceData {
    pub symbol: String,
    pub name: String,
    pub price_usd: Decimal,
    pub price_change_24h: Option<Decimal>,
    pub volume_24h: Option<Decimal>,
    pub market_cap: Option<Decimal>,
    pub last_updated: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CmcConversionResult {
    pub from_symbol: String,
    pub to_symbol: String,
    pub from_amount: Decimal,
    pub to_amount: Decimal,
    pub rate: Decimal,
    pub last_updated: DateTime<Utc>,
}

pub struct CoinMarketCapService {
    client: Client,
    api_key: String,
    pub redis: redis::aio::ConnectionManager,
    circuit_breaker: Arc<blockchain::circuit_breaker::CircuitBreaker>,
}

impl CoinMarketCapService {
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
            "coinmarketcap_api".to_string(),
            circuit_breaker_config,
        ));

        Self {
            client,
            api_key,
            redis,
            circuit_breaker,
        }
    }

    /// Get latest price data for a cryptocurrency by symbol
    pub async fn get_price_by_symbol(&self, symbol: &str) -> ApiResult<CmcPriceData> {
        let cache_key = format!("cmc:price:symbol:{}", symbol.to_uppercase());

        // Check cache first
        if let Some(cached) = self.get_from_cache::<CmcPriceData>(&cache_key).await
            .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))? {
            tracing::debug!("Cache hit for CMC price: {}", cache_key);
            return Ok(cached);
        }

        tracing::debug!("Cache miss for CMC price: {}", cache_key);
        let price_data = self.fetch_price_by_symbol(symbol).await?;
        
        self.set_in_cache(&cache_key, &price_data, CACHE_TTL_SECONDS).await
            .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))?;

        Ok(price_data)
    }

    /// Get latest prices for multiple cryptocurrencies by symbols
    pub async fn get_prices_by_symbols(&self, symbols: &[String]) -> ApiResult<Vec<CmcPriceData>> {
        if symbols.is_empty() {
            return Ok(Vec::new());
        }

        let symbols_str = symbols.join(",");
        let cache_key = format!("cmc:prices:symbols:{}", symbols_str);

        // Check cache first
        if let Some(cached) = self.get_from_cache::<Vec<CmcPriceData>>(&cache_key).await
            .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))? {
            tracing::debug!("Cache hit for CMC prices: {}", cache_key);
            return Ok(cached);
        }

        tracing::debug!("Cache miss for CMC prices: {}", cache_key);
        let price_data = self.fetch_prices_by_symbols(&symbols_str).await?;
        
        self.set_in_cache(&cache_key, &price_data, CACHE_TTL_SECONDS).await
            .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))?;

        Ok(price_data)
    }

    /// Convert between two cryptocurrencies
    pub async fn convert(
        &self,
        from_symbol: &str,
        to_symbol: &str,
        amount: Decimal,
    ) -> ApiResult<CmcConversionResult> {
        let cache_key = format!(
            "cmc:convert:{}:{}:{}",
            from_symbol.to_uppercase(),
            to_symbol.to_uppercase(),
            amount
        );

        // Check cache first (shorter TTL for conversions)
        if let Some(cached) = self.get_from_cache::<CmcConversionResult>(&cache_key).await
            .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))? {
            tracing::debug!("Cache hit for CMC conversion: {}", cache_key);
            return Ok(cached);
        }

        tracing::debug!("Cache miss for CMC conversion: {}", cache_key);
        let conversion = self.fetch_conversion(from_symbol, to_symbol, amount).await?;
        
        self.set_in_cache(&cache_key, &conversion, 30).await // 30 second TTL for conversions
            .map_err(|e| ApiError::InternalError(format!("Cache error: {}", e)))?;

        Ok(conversion)
    }

    /// Fetch price data from CoinMarketCap API
    async fn fetch_price_by_symbol(&self, symbol: &str) -> ApiResult<CmcPriceData> {
        // Check circuit breaker
        if !self.circuit_breaker.is_request_allowed().await {
            warn!("CoinMarketCap API circuit breaker is open");
            return Err(ApiError::CircuitBreakerOpen(
                "CoinMarketCap API is temporarily unavailable. Please try again later.".to_string()
            ));
        }

        let url = format!(
            "{}/cryptocurrency/quotes/latest?symbol={}",
            CMC_API_BASE,
            symbol.to_uppercase()
        );

        let response = match self
            .client
            .get(&url)
            .header("X-CMC_PRO_API_KEY", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                self.circuit_breaker.record_failure().await;
                return Err(ApiError::CoinMarketCapApiError(format!("Failed to send CMC request: {}", e)));
            }
        };

        if !response.status().is_success() {
            self.circuit_breaker.record_failure().await;
            return Err(ApiError::CoinMarketCapApiError(format!(
                "CoinMarketCap API returned error status: {}",
                response.status()
            )));
        }

        let cmc_response: CmcResponse<HashMap<String, CmcCryptocurrency>> = match response
            .json()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                self.circuit_breaker.record_failure().await;
                return Err(ApiError::CoinMarketCapApiError(format!("Failed to parse CMC response: {}", e)));
            }
        };

        if cmc_response.status.error_code != 0 {
            self.circuit_breaker.record_failure().await;
            return Err(ApiError::CoinMarketCapApiError(format!(
                "CoinMarketCap API error: {}",
                cmc_response.status.error_message.unwrap_or_default()
            )));
        }

        let crypto = cmc_response
            .data
            .get(symbol.to_uppercase().as_str())
            .ok_or_else(|| {
                ApiError::CoinMarketCapApiError(format!("Symbol {} not found", symbol))
            })?;

        self.circuit_breaker.record_success().await;

        Ok(self.normalize_price_data(crypto))
    }

    /// Fetch multiple prices from CoinMarketCap API
    async fn fetch_prices_by_symbols(&self, symbols: &str) -> ApiResult<Vec<CmcPriceData>> {
        // Check circuit breaker
        if !self.circuit_breaker.is_request_allowed().await {
            warn!("CoinMarketCap API circuit breaker is open");
            return Err(ApiError::CircuitBreakerOpen(
                "CoinMarketCap API is temporarily unavailable. Please try again later.".to_string()
            ));
        }

        let url = format!(
            "{}/cryptocurrency/quotes/latest?symbol={}",
            CMC_API_BASE,
            symbols.to_uppercase()
        );

        let response = match self
            .client
            .get(&url)
            .header("X-CMC_PRO_API_KEY", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                self.circuit_breaker.record_failure().await;
                return Err(ApiError::CoinMarketCapApiError(format!("Failed to send CMC request: {}", e)));
            }
        };

        if !response.status().is_success() {
            self.circuit_breaker.record_failure().await;
            return Err(ApiError::CoinMarketCapApiError(format!(
                "CoinMarketCap API returned error status: {}",
                response.status()
            )));
        }

        let cmc_response: CmcResponse<HashMap<String, CmcCryptocurrency>> = match response
            .json()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                self.circuit_breaker.record_failure().await;
                return Err(ApiError::CoinMarketCapApiError(format!("Failed to parse CMC response: {}", e)));
            }
        };

        if cmc_response.status.error_code != 0 {
            self.circuit_breaker.record_failure().await;
            return Err(ApiError::CoinMarketCapApiError(format!(
                "CoinMarketCap API error: {}",
                cmc_response.status.error_message.unwrap_or_default()
            )));
        }

        self.circuit_breaker.record_success().await;

        let mut results = Vec::new();
        for crypto in cmc_response.data.values() {
            results.push(self.normalize_price_data(crypto));
        }

        Ok(results)
    }

    /// Fetch conversion rate from CoinMarketCap API
    async fn fetch_conversion(
        &self,
        from_symbol: &str,
        to_symbol: &str,
        amount: Decimal,
    ) -> ApiResult<CmcConversionResult> {
        // Check circuit breaker
        if !self.circuit_breaker.is_request_allowed().await {
            warn!("CoinMarketCap API circuit breaker is open");
            return Err(ApiError::CircuitBreakerOpen(
                "CoinMarketCap API is temporarily unavailable. Please try again later.".to_string()
            ));
        }

        let amount_f64 = amount.to_string().parse::<f64>()
            .map_err(|e| ApiError::InternalError(format!("Invalid amount: {}", e)))?;

        let url = format!(
            "{}/tools/price-conversion?amount={}&symbol={}&convert={}",
            CMC_API_BASE,
            amount_f64,
            from_symbol.to_uppercase(),
            to_symbol.to_uppercase()
        );

        let response = match self
            .client
            .get(&url)
            .header("X-CMC_PRO_API_KEY", &self.api_key)
            .header("Accept", "application/json")
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                self.circuit_breaker.record_failure().await;
                return Err(ApiError::CoinMarketCapApiError(format!("Failed to send CMC conversion request: {}", e)));
            }
        };

        if !response.status().is_success() {
            self.circuit_breaker.record_failure().await;
            return Err(ApiError::CoinMarketCapApiError(format!(
                "CoinMarketCap conversion API returned error status: {}",
                response.status()
            )));
        }

        let cmc_response: CmcResponse<CmcConversionResponse> = match response
            .json()
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                self.circuit_breaker.record_failure().await;
                return Err(ApiError::CoinMarketCapApiError(format!("Failed to parse CMC conversion response: {}", e)));
            }
        };

        if cmc_response.status.error_code != 0 {
            self.circuit_breaker.record_failure().await;
            return Err(ApiError::CoinMarketCapApiError(format!(
                "CoinMarketCap conversion API error: {}",
                cmc_response.status.error_message.unwrap_or_default()
            )));
        }

        self.circuit_breaker.record_success().await;

        let conversion_data = cmc_response.data;
        let to_quote = conversion_data.quote.get(to_symbol.to_uppercase().as_str())
            .ok_or_else(|| ApiError::CoinMarketCapApiError(format!("Conversion to {} not found", to_symbol)))?;

        let to_amount = Decimal::from_f64_retain(conversion_data.amount)
            .unwrap_or(Decimal::ZERO);
        let rate = Decimal::from_f64_retain(to_quote.price)
            .unwrap_or(Decimal::ZERO);

        Ok(CmcConversionResult {
            from_symbol: from_symbol.to_uppercase(),
            to_symbol: to_symbol.to_uppercase(),
            from_amount: amount,
            to_amount,
            rate,
            last_updated: Utc::now(),
        })
    }

    /// Normalize CoinMarketCap data to internal format
    fn normalize_price_data(&self, crypto: &CmcCryptocurrency) -> CmcPriceData {
        CmcPriceData {
            symbol: crypto.symbol.clone(),
            name: crypto.name.clone(),
            price_usd: Decimal::from_f64_retain(crypto.quote.usd.price)
                .unwrap_or(Decimal::ZERO),
            price_change_24h: crypto.quote.usd.percent_change_24h
                .and_then(|v| Decimal::from_f64_retain(v)),
            volume_24h: crypto.quote.usd.volume_24h
                .and_then(|v| Decimal::from_f64_retain(v)),
            market_cap: crypto.quote.usd.market_cap
                .and_then(|v| Decimal::from_f64_retain(v)),
            last_updated: Utc::now(),
        }
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
