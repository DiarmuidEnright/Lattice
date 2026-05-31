use std::collections::HashMap;
use tracing::{debug, info};

/// Token type classification
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenType {
    Native,
    Wrapped,
    ERC20,
    SPL,
}

/// Token metadata information
#[derive(Debug, Clone)]
pub struct TokenMetadata {
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
    pub token_type: TokenType,
    pub blockchain: String,
    pub contract_address: Option<String>,
}

/// Service for managing token metadata across multiple blockchains
/// 
/// Supports native tokens (ETH, BNB, MATIC, SOL) and wrapped tokens (WETH, WBNB, WMATIC)
/// as well as standard ERC-20 and SPL tokens.
/// 
/// **Validates: Requirement 5.1**
pub struct TokenMetadataService {
    metadata_cache: HashMap<String, TokenMetadata>,
}

impl TokenMetadataService {
    /// Create a new token metadata service with pre-populated native and wrapped tokens
    pub fn new() -> Self {
        info!("Initializing token metadata service");
        
        let mut metadata_cache = HashMap::new();
        
        // Ethereum native and wrapped tokens
        metadata_cache.insert(
            "ETH".to_string(),
            TokenMetadata {
                symbol: "ETH".to_string(),
                name: "Ethereum".to_string(),
                decimals: 18,
                token_type: TokenType::Native,
                blockchain: "Ethereum".to_string(),
                contract_address: None,
            },
        );
        
        metadata_cache.insert(
            "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".to_lowercase(),
            TokenMetadata {
                symbol: "WETH".to_string(),
                name: "Wrapped Ether".to_string(),
                decimals: 18,
                token_type: TokenType::Wrapped,
                blockchain: "Ethereum".to_string(),
                contract_address: Some("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2".to_string()),
            },
        );
        
        // Binance Smart Chain native and wrapped tokens
        metadata_cache.insert(
            "BNB".to_string(),
            TokenMetadata {
                symbol: "BNB".to_string(),
                name: "Binance Coin".to_string(),
                decimals: 18,
                token_type: TokenType::Native,
                blockchain: "BinanceSmartChain".to_string(),
                contract_address: None,
            },
        );
        
        metadata_cache.insert(
            "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c".to_lowercase(),
            TokenMetadata {
                symbol: "WBNB".to_string(),
                name: "Wrapped BNB".to_string(),
                decimals: 18,
                token_type: TokenType::Wrapped,
                blockchain: "BinanceSmartChain".to_string(),
                contract_address: Some("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c".to_string()),
            },
        );
        
        // Polygon native and wrapped tokens
        metadata_cache.insert(
            "MATIC".to_string(),
            TokenMetadata {
                symbol: "MATIC".to_string(),
                name: "Polygon".to_string(),
                decimals: 18,
                token_type: TokenType::Native,
                blockchain: "Polygon".to_string(),
                contract_address: None,
            },
        );
        
        metadata_cache.insert(
            "0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270".to_lowercase(),
            TokenMetadata {
                symbol: "WMATIC".to_string(),
                name: "Wrapped Matic".to_string(),
                decimals: 18,
                token_type: TokenType::Wrapped,
                blockchain: "Polygon".to_string(),
                contract_address: Some("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270".to_string()),
            },
        );
        
        // Solana native token
        metadata_cache.insert(
            "SOL".to_string(),
            TokenMetadata {
                symbol: "SOL".to_string(),
                name: "Solana".to_string(),
                decimals: 9,
                token_type: TokenType::Native,
                blockchain: "Solana".to_string(),
                contract_address: None,
            },
        );
        
        metadata_cache.insert(
            "So11111111111111111111111111111111111111112".to_string(),
            TokenMetadata {
                symbol: "SOL".to_string(),
                name: "Solana".to_string(),
                decimals: 9,
                token_type: TokenType::Native,
                blockchain: "Solana".to_string(),
                contract_address: Some("So11111111111111111111111111111111111111112".to_string()),
            },
        );
        
        Self { metadata_cache }
    }
    
    /// Get token metadata by symbol or contract address
    /// 
    /// **Validates: Requirement 5.1**
    pub fn get_metadata(&self, identifier: &str) -> Option<&TokenMetadata> {
        debug!("Looking up token metadata for: {}", identifier);
        
        // Try exact match first
        if let Some(metadata) = self.metadata_cache.get(identifier) {
            return Some(metadata);
        }
        
        // Try lowercase for contract addresses
        let lowercase_id = identifier.to_lowercase();
        self.metadata_cache.get(&lowercase_id)
    }
    
    /// Check if a token is a native token
    /// 
    /// **Validates: Requirement 5.1**
    pub fn is_native_token(&self, identifier: &str) -> bool {
        self.get_metadata(identifier)
            .map(|m| m.token_type == TokenType::Native)
            .unwrap_or(false)
    }
    
    /// Check if a token is a wrapped token
    /// 
    /// **Validates: Requirement 5.1**
    pub fn is_wrapped_token(&self, identifier: &str) -> bool {
        self.get_metadata(identifier)
            .map(|m| m.token_type == TokenType::Wrapped)
            .unwrap_or(false)
    }
    
    /// Add custom token metadata (for ERC-20 or SPL tokens)
    /// 
    /// **Validates: Requirement 5.1**
    pub fn add_token_metadata(&mut self, identifier: String, metadata: TokenMetadata) {
        info!("Adding custom token metadata for: {}", identifier);
        self.metadata_cache.insert(identifier, metadata);
    }
    
    /// Get native token symbol for a blockchain
    /// 
    /// **Validates: Requirement 5.1**
    pub fn get_native_token_symbol(blockchain: &str) -> &'static str {
        match blockchain {
            "Ethereum" => "ETH",
            "BinanceSmartChain" => "BNB",
            "Polygon" => "MATIC",
            "Solana" => "SOL",
            _ => "UNKNOWN",
        }
    }
    
    /// Get wrapped token address for a blockchain
    /// 
    /// **Validates: Requirement 5.1**
    pub fn get_wrapped_token_address(blockchain: &str) -> Option<&'static str> {
        match blockchain {
            "Ethereum" => Some("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
            "BinanceSmartChain" => Some("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c"),
            "Polygon" => Some("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270"),
            _ => None,
        }
    }
    
    /// Normalize token identifier (lowercase for EVM addresses)
    pub fn normalize_identifier(identifier: &str, blockchain: &str) -> String {
        if blockchain == "Solana" {
            // Solana addresses are case-sensitive
            identifier.to_string()
        } else {
            // EVM addresses are case-insensitive
            identifier.to_lowercase()
        }
    }
}

impl Default for TokenMetadataService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_native_token_metadata() {
        let service = TokenMetadataService::new();
        
        // Test Ethereum native token
        let eth_metadata = service.get_metadata("ETH");
        assert!(eth_metadata.is_some());
        let eth = eth_metadata.unwrap();
        assert_eq!(eth.symbol, "ETH");
        assert_eq!(eth.decimals, 18);
        assert_eq!(eth.token_type, TokenType::Native);
        assert_eq!(eth.blockchain, "Ethereum");
        
        // Test BSC native token
        let bnb_metadata = service.get_metadata("BNB");
        assert!(bnb_metadata.is_some());
        let bnb = bnb_metadata.unwrap();
        assert_eq!(bnb.symbol, "BNB");
        assert_eq!(bnb.token_type, TokenType::Native);
        
        // Test Polygon native token
        let matic_metadata = service.get_metadata("MATIC");
        assert!(matic_metadata.is_some());
        let matic = matic_metadata.unwrap();
        assert_eq!(matic.symbol, "MATIC");
        assert_eq!(matic.token_type, TokenType::Native);
        
        // Test Solana native token
        let sol_metadata = service.get_metadata("SOL");
        assert!(sol_metadata.is_some());
        let sol = sol_metadata.unwrap();
        assert_eq!(sol.symbol, "SOL");
        assert_eq!(sol.decimals, 9);
        assert_eq!(sol.token_type, TokenType::Native);
    }
    
    #[test]
    fn test_wrapped_token_metadata() {
        let service = TokenMetadataService::new();
        
        // Test WETH
        let weth_metadata = service.get_metadata("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
        assert!(weth_metadata.is_some());
        let weth = weth_metadata.unwrap();
        assert_eq!(weth.symbol, "WETH");
        assert_eq!(weth.token_type, TokenType::Wrapped);
        assert!(weth.contract_address.is_some());
        
        // Test WBNB
        let wbnb_metadata = service.get_metadata("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c");
        assert!(wbnb_metadata.is_some());
        let wbnb = wbnb_metadata.unwrap();
        assert_eq!(wbnb.symbol, "WBNB");
        assert_eq!(wbnb.token_type, TokenType::Wrapped);
        
        // Test WMATIC
        let wmatic_metadata = service.get_metadata("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270");
        assert!(wmatic_metadata.is_some());
        let wmatic = wmatic_metadata.unwrap();
        assert_eq!(wmatic.symbol, "WMATIC");
        assert_eq!(wmatic.token_type, TokenType::Wrapped);
    }
    
    #[test]
    fn test_case_insensitive_lookup() {
        let service = TokenMetadataService::new();
        
        // Test lowercase lookup
        let weth_lower = service.get_metadata("0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
        assert!(weth_lower.is_some());
        assert_eq!(weth_lower.unwrap().symbol, "WETH");
        
        // Test mixed case lookup
        let weth_mixed = service.get_metadata("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2");
        assert!(weth_mixed.is_some());
        assert_eq!(weth_mixed.unwrap().symbol, "WETH");
    }
    
    #[test]
    fn test_is_native_token() {
        let service = TokenMetadataService::new();
        
        assert!(service.is_native_token("ETH"));
        assert!(service.is_native_token("BNB"));
        assert!(service.is_native_token("MATIC"));
        assert!(service.is_native_token("SOL"));
        
        assert!(!service.is_native_token("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2")); // WETH
        assert!(!service.is_native_token("UNKNOWN"));
    }
    
    #[test]
    fn test_is_wrapped_token() {
        let service = TokenMetadataService::new();
        
        assert!(service.is_wrapped_token("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2")); // WETH
        assert!(service.is_wrapped_token("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c")); // WBNB
        assert!(service.is_wrapped_token("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270")); // WMATIC
        
        assert!(!service.is_wrapped_token("ETH"));
        assert!(!service.is_wrapped_token("UNKNOWN"));
    }
    
    #[test]
    fn test_get_native_token_symbol() {
        assert_eq!(TokenMetadataService::get_native_token_symbol("Ethereum"), "ETH");
        assert_eq!(TokenMetadataService::get_native_token_symbol("BinanceSmartChain"), "BNB");
        assert_eq!(TokenMetadataService::get_native_token_symbol("Polygon"), "MATIC");
        assert_eq!(TokenMetadataService::get_native_token_symbol("Solana"), "SOL");
        assert_eq!(TokenMetadataService::get_native_token_symbol("Unknown"), "UNKNOWN");
    }
    
    #[test]
    fn test_get_wrapped_token_address() {
        assert_eq!(
            TokenMetadataService::get_wrapped_token_address("Ethereum"),
            Some("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2")
        );
        assert_eq!(
            TokenMetadataService::get_wrapped_token_address("BinanceSmartChain"),
            Some("0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c")
        );
        assert_eq!(
            TokenMetadataService::get_wrapped_token_address("Polygon"),
            Some("0x0d500B1d8E8eF31E21C99d1Db9A6444d3ADf1270")
        );
        assert_eq!(TokenMetadataService::get_wrapped_token_address("Solana"), None);
    }
    
    #[test]
    fn test_add_custom_token() {
        let mut service = TokenMetadataService::new();
        
        let custom_token = TokenMetadata {
            symbol: "USDC".to_string(),
            name: "USD Coin".to_string(),
            decimals: 6,
            token_type: TokenType::ERC20,
            blockchain: "Ethereum".to_string(),
            contract_address: Some("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string()),
        };
        
        service.add_token_metadata("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(), custom_token);
        
        let usdc = service.get_metadata("0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");
        assert!(usdc.is_some());
        assert_eq!(usdc.unwrap().symbol, "USDC");
    }
    
    #[test]
    fn test_normalize_identifier() {
        // EVM addresses should be lowercase
        assert_eq!(
            TokenMetadataService::normalize_identifier("0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2", "Ethereum"),
            "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"
        );
        
        // Solana addresses should remain unchanged
        assert_eq!(
            TokenMetadataService::normalize_identifier("So11111111111111111111111111111111111111112", "Solana"),
            "So11111111111111111111111111111111111111112"
        );
    }
}
