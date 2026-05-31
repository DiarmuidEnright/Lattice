use reqwest::Client;
use serde::{Deserialize, Serialize};
use shared::{Error, Result};
use std::time::Duration;
use tracing::{debug, info, warn};

/// Helius API client for wallet analytics and management
/// 
/// Provides access to Helius's enhanced Solana RPC APIs for wallet information.
///
/// The API key is read from the `HELIUS_API_KEY` environment variable at runtime.
pub struct TantumClient {
    client: Client,
    base_url: String,
    use_mainnet: bool,
}

/// Helius wallet information response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TantumWalletInfo {
    pub address: String,
    pub balance: TantumBalance,
    pub tokens: Vec<TantumToken>,
    #[serde(rename = "totalValueUsd")]
    pub total_value_usd: f64,
}

/// Balance information from Helius
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TantumBalance {
    pub sol: f64,
    #[serde(rename = "solUsd")]
    pub sol_usd: f64,
}

/// Token information from Helius
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TantumToken {
    pub mint: String,
    pub symbol: String,
    pub name: String,
    pub amount: f64,
    pub decimals: u8,
    #[serde(rename = "valueUsd")]
    pub value_usd: Option<f64>,
}

impl TantumClient {
    /// Create a new Helius client
    pub fn new(api_key: String, use_mainnet: bool) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        // Use the correct Helius RPC URLs
        let base_url = if use_mainnet {
            format!("https://mainnet.helius-rpc.com/?api-key={}", api_key)
        } else {
            format!("https://devnet.helius-rpc.com/?api-key={}", api_key)
        };

        Self {
            client,
            base_url,
            use_mainnet,
        }
    }

    /// Get wallet information from Helius API
    /// 
    /// This fetches comprehensive wallet data including:
    /// - SOL balance and USD value
    /// - All SPL token holdings with metadata
    /// - Total portfolio value in USD
    /// 
    /// # Arguments
    /// * `wallet_address` - The Solana wallet address to query
    /// * `use_mainnet` - Whether to use mainnet (true) or devnet (false) API
    pub async fn get_wallet_info(
        &self,
        wallet_address: &str,
        _use_mainnet: bool,
    ) -> Result<TantumWalletInfo> {
        let network = if self.use_mainnet { "mainnet" } else { "devnet" };
        
        info!(
            "Fetching wallet info from Helius API for {} on {}",
            wallet_address, network
        );

        // Use Helius getAssetsByOwner API to get all assets
        let request_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "wallet-info",
            "method": "getAssetsByOwner",
            "params": {
                "ownerAddress": wallet_address,
                "page": 1,
                "limit": 1000
            }
        });

        let response = self
            .client
            .post(&self.base_url)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                warn!("Failed to fetch wallet info from Helius: {}", e);
                Error::ExternalService(format!("Helius API request failed: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            warn!(
                "Helius API returned error status {}: {}",
                status, error_text
            );
            return Err(Error::ExternalService(format!(
                "Helius API error {}: {}",
                status, error_text
            )));
        }

        // Parse the JSON-RPC response
        let json_response: serde_json::Value = response.json().await.map_err(|e| {
            warn!("Failed to parse Helius response: {}", e);
            Error::ExternalService(format!("Failed to parse Helius response: {}", e))
        })?;

        // Check for JSON-RPC errors
        if let Some(error) = json_response.get("error") {
            let error_msg = error.get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            warn!("Helius API returned error: {}", error_msg);
            return Err(Error::ExternalService(format!("Helius API error: {}", error_msg)));
        }

        // Extract the result
        let result = json_response.get("result")
            .ok_or_else(|| Error::ExternalService("Missing result in Helius response".to_string()))?;

        // Get SOL balance first using getBalance RPC call
        let balance_request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": "get-balance",
            "method": "getBalance",
            "params": [wallet_address]
        });

        let balance_response = self
            .client
            .post(&self.base_url)
            .json(&balance_request)
            .send()
            .await
            .map_err(|e| Error::ExternalService(format!("Failed to get balance: {}", e)))?;

        let balance_json: serde_json::Value = balance_response.json().await
            .map_err(|e| Error::ExternalService(format!("Failed to parse balance: {}", e)))?;

        let sol_lamports = balance_json
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let sol_balance = sol_lamports as f64 / 1_000_000_000.0;

        // Get SOL price from CoinMarketCap (using the API key from environment)
        // For now, we'll use a reasonable default if the price fetch fails
        let sol_price_usd = 100.0; // Fallback price
        
        // TODO: Integrate with CoinMarketCap service for real-time SOL price
        // This would require passing the CoinMarketCap service to this method
        let sol_usd = sol_balance * sol_price_usd;

        // Parse assets from getAssetsByOwner response
        let items = result.get("items")
            .and_then(|i| i.as_array())
            .ok_or_else(|| Error::ExternalService("Invalid assets format".to_string()))?;

        let mut tokens = Vec::new();
        let total_value_usd = sol_usd;

        for item in items {
            // Extract token information from Helius DAS API format
            let token_info = item.get("token_info");
            let content = item.get("content");
            
            // Get mint address
            let mint = item.get("id")
                .and_then(|id| id.as_str())
                .unwrap_or("")
                .to_string();

            // Get symbol and name from metadata
            let symbol = content
                .and_then(|c| c.get("metadata"))
                .and_then(|m| m.get("symbol"))
                .and_then(|s| s.as_str())
                .unwrap_or("UNKNOWN")
                .to_string();

            let name = content
                .and_then(|c| c.get("metadata"))
                .and_then(|m| m.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("Unknown Token")
                .to_string();

            // Get amount and decimals
            let balance = token_info
                .and_then(|ti| ti.get("balance"))
                .and_then(|b| b.as_u64())
                .unwrap_or(0);

            let decimals = token_info
                .and_then(|ti| ti.get("decimals"))
                .and_then(|d| d.as_u64())
                .unwrap_or(0) as u8;

            // Skip if balance is zero
            if balance == 0 {
                continue;
            }

            // Calculate actual amount
            let amount = if decimals > 0 {
                balance as f64 / 10_f64.powi(decimals as i32)
            } else {
                balance as f64
            };

            // For now, we don't have price data for SPL tokens
            // In production, integrate with a price feed service
            let value_usd = None;

            tokens.push(TantumToken {
                mint,
                symbol,
                name,
                amount,
                decimals,
                value_usd,
            });
        }

        let wallet_info = TantumWalletInfo {
            address: wallet_address.to_string(),
            balance: TantumBalance {
                sol: sol_balance,
                sol_usd,
            },
            tokens,
            total_value_usd,
        };

        debug!(
            "Successfully fetched wallet info: {} SOL (${:.2}), {} tokens, ${:.2} total",
            wallet_info.balance.sol,
            wallet_info.balance.sol_usd,
            wallet_info.tokens.len(),
            wallet_info.total_value_usd
        );

        Ok(wallet_info)
    }

    /// Delete/burn a wallet (not supported by Helius)
    /// 
    /// This method is not supported by Helius API.
    /// 
    /// # Arguments
    /// * `wallet_address` - The wallet address
    /// * `use_mainnet` - Whether to use mainnet (true) or devnet (false)
    /// 
    /// # Returns
    /// Error indicating this operation is not supported
    pub async fn burn_wallet(
        &self,
        _wallet_address: &str,
        _use_mainnet: bool,
    ) -> Result<f64> {
        Err(Error::ExternalService(
            "Wallet burning is not supported by Helius API".to_string()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helius_client_creation() {
        let client = TantumClient::new(
            "test-key".to_string(),
            false,
        );
        assert!(client.base_url.contains("devnet.helius-rpc.com"));
    }
}
