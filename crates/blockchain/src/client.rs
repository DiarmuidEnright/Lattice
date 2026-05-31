use anyhow::Context;
use shared::{Error, Result};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::Signature,
};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::retry::{retry_with_backoff, RetryConfig};
use crate::types::{TokenAccount, WalletBalance};

/// Per-request timeout for the (blocking) Solana RPC client. Devnet can be
/// flaky; without an explicit timeout the underlying client waits ~30s, which
/// blocks the calling worker thread and starves the async runtime. Keep this
/// well under the frontend's 10s fetch timeout so a slow RPC degrades to a
/// fast, clean error instead of hanging the whole dashboard.
const RPC_REQUEST_TIMEOUT: Duration = Duration::from_secs(4);

/// Run a blocking closure without starving the async runtime.
///
/// The Solana `RpcClient` used here is synchronous/blocking. Calling it
/// directly on a tokio worker thread blocks that worker; enough concurrent
/// blocking calls starve the runtime and make *unrelated* endpoints time out.
/// On the multi-threaded runtime (the API server) we hand the work to
/// `block_in_place` so tokio can relocate other tasks. On a current-thread
/// runtime or outside any runtime (unit tests) we just run it inline, since
/// `block_in_place` would panic there.
fn run_blocking<T>(f: impl FnOnce() -> T) -> T {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// Stealth payment metadata found on-chain
#[derive(Debug, Clone)]
pub struct StealthMetadata {
    pub slot: u64,
    pub signature: Signature,
    pub ephemeral_public_key: Pubkey,
    pub viewing_tag: [u8; 4],
    pub version: u8,
}

/// Parsed stealth metadata from instruction data
#[derive(Debug, Clone)]
struct ParsedStealthMetadata {
    pub ephemeral_public_key: Pubkey,
    pub viewing_tag: [u8; 4],
    pub version: u8,
}

/// Create a custom instruction to store stealth payment metadata on-chain
/// 
/// This instruction stores the ephemeral public key and viewing tag in the
/// transaction memo, allowing receivers to scan for incoming payments.
/// 
/// # Arguments
/// * `ephemeral_public_key` - The ephemeral public key used for ECDH
/// * `viewing_tag` - The 4-byte viewing tag for efficient scanning
/// * `version` - Stealth address version (1 for standard, 2 for hybrid)
fn create_stealth_metadata_instruction(
    ephemeral_public_key: &Pubkey,
    viewing_tag: &[u8; 4],
    version: u8,
) -> Instruction {
    // Encode metadata as: version (1 byte) + viewing_tag (4 bytes) + ephemeral_pk (32 bytes)
    let mut metadata = Vec::with_capacity(37);
    metadata.push(version);
    metadata.extend_from_slice(viewing_tag);
    metadata.extend_from_slice(&ephemeral_public_key.to_bytes());
    
    // Use SPL Memo program to store metadata on-chain
    // Program ID for SPL Memo: MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr
    let memo_program_id = solana_sdk::pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");
    
    Instruction {
        program_id: memo_program_id,
        accounts: vec![],
        data: metadata,
    }
}

/// Parse stealth metadata from instruction data
/// 
/// Returns Some(ParsedStealthMetadata) if the data contains valid stealth metadata,
/// None otherwise.
fn parse_stealth_metadata(data: &[u8]) -> Option<ParsedStealthMetadata> {
    // Stealth metadata format: version (1) + viewing_tag (4) + ephemeral_pk (32) = 37 bytes
    if data.len() != 37 {
        return None;
    }
    
    let version = data[0];
    
    // Only support version 1 (standard) and version 2 (hybrid) for now
    if version != 1 && version != 2 {
        return None;
    }
    
    let mut viewing_tag = [0u8; 4];
    viewing_tag.copy_from_slice(&data[1..5]);
    
    let mut ephemeral_pk_bytes = [0u8; 32];
    ephemeral_pk_bytes.copy_from_slice(&data[5..37]);
    let ephemeral_public_key = Pubkey::new_from_array(ephemeral_pk_bytes);
    
    Some(ParsedStealthMetadata {
        ephemeral_public_key,
        viewing_tag,
        version,
    })
}

/// Solana client wrapper for blockchain interactions
pub struct SolanaClient {
    primary_client: RpcClient,
    fallback_client: Option<RpcClient>,
    primary_circuit_breaker: Arc<CircuitBreaker>,
    fallback_circuit_breaker: Option<Arc<CircuitBreaker>>,
    retry_config: RetryConfig,
}

impl SolanaClient {
    /// Create a new Solana client with primary and optional fallback RPC endpoints
    pub fn new(rpc_url: String, fallback_url: Option<String>) -> Self {
        info!("Initializing Solana client with primary RPC: {}", rpc_url);
        
        let primary_client = RpcClient::new_with_timeout_and_commitment(
            rpc_url.clone(),
            RPC_REQUEST_TIMEOUT,
            CommitmentConfig::confirmed(),
        );

        let fallback_client = fallback_url.as_ref().map(|url| {
            info!("Configuring fallback RPC: {}", url);
            RpcClient::new_with_timeout_and_commitment(
                url.clone(),
                RPC_REQUEST_TIMEOUT,
                CommitmentConfig::confirmed(),
            )
        });

        // Create circuit breakers for primary and fallback
        let circuit_breaker_config = CircuitBreakerConfig::default();
        let primary_circuit_breaker = Arc::new(CircuitBreaker::new(
            format!("primary-rpc-{}", rpc_url),
            circuit_breaker_config.clone(),
        ));

        let fallback_circuit_breaker = fallback_url.map(|url| {
            Arc::new(CircuitBreaker::new(
                format!("fallback-rpc-{}", url),
                circuit_breaker_config,
            ))
        });

        Self {
            primary_client,
            fallback_client,
            primary_circuit_breaker,
            fallback_circuit_breaker,
            retry_config: RetryConfig::default(),
        }
    }

    /// Create a new Solana client with custom retry and circuit breaker configurations
    pub fn new_with_config(
        rpc_url: String,
        fallback_url: Option<String>,
        retry_config: RetryConfig,
        circuit_breaker_config: CircuitBreakerConfig,
    ) -> Self {
        info!("Initializing Solana client with custom config");
        
        let primary_client = RpcClient::new_with_timeout_and_commitment(
            rpc_url.clone(),
            RPC_REQUEST_TIMEOUT,
            CommitmentConfig::confirmed(),
        );

        let fallback_client = fallback_url.as_ref().map(|url| {
            RpcClient::new_with_timeout_and_commitment(
                url.clone(),
                RPC_REQUEST_TIMEOUT,
                CommitmentConfig::confirmed(),
            )
        });

        let primary_circuit_breaker = Arc::new(CircuitBreaker::new(
            format!("primary-rpc-{}", rpc_url),
            circuit_breaker_config.clone(),
        ));

        let fallback_circuit_breaker = fallback_url.map(|url| {
            Arc::new(CircuitBreaker::new(
                format!("fallback-rpc-{}", url),
                circuit_breaker_config,
            ))
        });

        Self {
            primary_client,
            fallback_client,
            primary_circuit_breaker,
            fallback_circuit_breaker,
            retry_config,
        }
    }

    /// Validate a Solana wallet address format
    pub fn validate_address(&self, address: &str) -> Result<Pubkey> {
        Pubkey::from_str(address).map_err(|e| {
            warn!("Invalid wallet address format: {} - {}", address, e);
            Error::InvalidWalletAddress(format!("Invalid Solana address format: {}", e))
        })
    }

    /// Get SOL balance for a wallet address (in lamports)
    pub async fn get_sol_balance(&self, address: &str) -> Result<u64> {
        let pubkey = self.validate_address(address)?;
        
        debug!("Fetching SOL balance for address: {}", address);
        
        // Try primary client with circuit breaker and retry
        let primary_result = self.execute_with_circuit_breaker(
            &self.primary_circuit_breaker,
            "get_sol_balance_primary",
            || {
                let client = &self.primary_client;
                let pk = pubkey;
                async move {
                    run_blocking(|| client.get_balance(&pk)).map_err(|e| {
                        Error::SolanaRpc(format!("Primary RPC failed: {}", e))
                    })
                }
            },
        ).await;

        match primary_result {
            Ok(balance) => {
                debug!("Retrieved SOL balance from primary: {} lamports", balance);
                Ok(balance)
            }
            Err(e) => {
                warn!("Primary RPC failed for get_balance: {}", e);
                
                // Try fallback if available
                if let (Some(fallback), Some(fallback_cb)) = 
                    (&self.fallback_client, &self.fallback_circuit_breaker) 
                {
                    debug!("Attempting fallback RPC for get_balance");
                    
                    let fallback_result = self.execute_with_circuit_breaker(
                        fallback_cb,
                        "get_sol_balance_fallback",
                        || {
                            let client = fallback;
                            let pk = pubkey;
                            async move {
                                run_blocking(|| client.get_balance(&pk)).map_err(|e| {
                                    Error::SolanaRpc(format!("Fallback RPC failed: {}", e))
                                })
                            }
                        },
                    ).await;

                    match fallback_result {
                        Ok(balance) => {
                            debug!("Retrieved SOL balance from fallback: {} lamports", balance);
                            Ok(balance)
                        }
                        Err(fallback_err) => {
                            error!("Both primary and fallback RPC failed: {}", fallback_err);
                            Err(fallback_err)
                        }
                    }
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Get all SPL token accounts for a wallet address
    pub async fn get_token_accounts(&self, address: &str) -> Result<Vec<TokenAccount>> {
        let pubkey = self.validate_address(address)?;
        
        debug!("Fetching token accounts for address: {}", address);
        
        // Try primary client with circuit breaker and retry
        let primary_result = self.execute_with_circuit_breaker(
            &self.primary_circuit_breaker,
            "get_token_accounts_primary",
            || {
                let client = &self.primary_client;
                let pk = pubkey;
                async move {
                    run_blocking(|| {
                        client.get_token_accounts_by_owner(
                            &pk,
                            solana_client::rpc_request::TokenAccountsFilter::ProgramId(
                                spl_token::id(),
                            ),
                        )
                    })
                    .map_err(|e| {
                        Error::SolanaRpc(format!("Primary RPC failed: {}", e))
                    })
                }
            },
        ).await;

        let accounts = match primary_result {
            Ok(accounts) => accounts,
            Err(e) => {
                warn!("Primary RPC failed for get_token_accounts: {}", e);
                
                // Try fallback if available
                if let (Some(fallback), Some(fallback_cb)) = 
                    (&self.fallback_client, &self.fallback_circuit_breaker) 
                {
                    debug!("Attempting fallback RPC for get_token_accounts");
                    
                    self.execute_with_circuit_breaker(
                        fallback_cb,
                        "get_token_accounts_fallback",
                        || {
                            let client = fallback;
                            let pk = pubkey;
                            async move {
                                run_blocking(|| {
                                    client.get_token_accounts_by_owner(
                                        &pk,
                                        solana_client::rpc_request::TokenAccountsFilter::ProgramId(
                                            spl_token::id(),
                                        ),
                                    )
                                })
                                .map_err(|e| {
                                    Error::SolanaRpc(format!("Fallback RPC failed: {}", e))
                                })
                            }
                        },
                    ).await?
                } else {
                    return Err(e);
                }
            }
        };

        let mut token_accounts = Vec::new();

        for account in accounts {
            // Parse token account data using the decoded info
            match self.parse_token_account_from_ui(address, &account) {
                Ok(token_account) => token_accounts.push(token_account),
                Err(e) => {
                    warn!("Failed to parse token account: {}", e);
                    continue;
                }
            }
        }

        debug!("Retrieved {} token accounts", token_accounts.len());
        Ok(token_accounts)
    }

    /// Get complete wallet balance including SOL and all SPL tokens
    pub async fn get_wallet_balance(&self, address: &str) -> Result<WalletBalance> {
        info!("Fetching complete wallet balance for: {}", address);
        
        // Validate address first
        self.validate_address(address)?;

        // Get SOL balance
        let sol_balance = self.get_sol_balance(address).await?;

        // Get token accounts
        let token_accounts = self.get_token_accounts(address).await?;

        Ok(WalletBalance {
            address: address.to_string(),
            sol_balance,
            token_accounts,
        })
    }

    /// Parse token account from RPC UI response
    fn parse_token_account_from_ui(
        &self,
        owner: &str,
        account: &solana_client::rpc_response::RpcKeyedAccount,
    ) -> anyhow::Result<TokenAccount> {
        use solana_account_decoder::UiAccountData;
        
        // Extract token account info from the UI account data
        match &account.account.data {
            UiAccountData::Json(parsed_account) => {
                // The parsed account should have token account info
                let info = parsed_account
                    .parsed
                    .get("info")
                    .ok_or_else(|| anyhow::anyhow!("Missing info field"))?;

                let mint = info
                    .get("mint")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing mint field"))?
                    .to_string();

                let token_amount = info
                    .get("tokenAmount")
                    .ok_or_else(|| anyhow::anyhow!("Missing tokenAmount field"))?;

                let amount_str = token_amount
                    .get("amount")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing amount field"))?;

                let amount = amount_str
                    .parse::<u64>()
                    .context("Failed to parse amount")?;

                let decimals = token_amount
                    .get("decimals")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow::anyhow!("Missing decimals field"))?
                    as u8;

                Ok(TokenAccount {
                    mint,
                    owner: owner.to_string(),
                    amount,
                    decimals,
                })
            }
            _ => Err(anyhow::anyhow!("Expected JSON parsed account data")),
        }
    }

    /// Get the primary RPC client (for advanced operations)
    pub fn primary_client(&self) -> &RpcClient {
        &self.primary_client
    }

    /// Get the fallback RPC client if configured
    pub fn fallback_client(&self) -> Option<&RpcClient> {
        self.fallback_client.as_ref()
    }

    /// Health check for Solana RPC connectivity
    /// 
    /// **Validates: Requirements 11.1, 11.2**
    pub async fn health_check(&self) -> Result<()> {
        // Try to get the latest blockhash as a simple health check
        self.execute_with_circuit_breaker(
            &self.primary_circuit_breaker,
            "health_check",
            || async {
                self.primary_client
                    .get_latest_blockhash()
                    .map_err(|e| Error::SolanaRpc(format!("Health check failed: {}", e)))
            },
        )
        .await?;

        Ok(())
    }

    /// Submit a stealth payment transaction to the blockchain
    /// 
    /// This method submits a transaction that transfers funds to a stealth address
    /// and includes stealth metadata (ephemeral public key and viewing tag) in a memo instruction.
    /// 
    /// # Arguments
    /// * `payer` - The keypair paying for and signing the transaction
    /// * `stealth_address` - The destination stealth address
    /// * `amount` - Amount in lamports to transfer
    /// * `ephemeral_public_key` - The ephemeral public key for ECDH
    /// * `viewing_tag` - The 4-byte viewing tag for efficient scanning
    /// * `version` - Stealth address version (1 for standard, 2 for hybrid)
    /// 
    /// # Requirements
    /// Validates: Requirements 10.1, 11.2
    /// 
    /// # Returns
    /// Transaction signature on success
    pub async fn submit_stealth_payment(
        &self,
        payer: &solana_sdk::signature::Keypair,
        stealth_address: &Pubkey,
        amount: u64,
        ephemeral_public_key: &Pubkey,
        viewing_tag: &[u8; 4],
        version: u8,
    ) -> Result<solana_sdk::signature::Signature> {
        use solana_sdk::{system_instruction, transaction::Transaction, signature::Signer};
        
        info!(
            "Submitting stealth payment: {} lamports to {}",
            amount, stealth_address
        );
        
        // Execute with circuit breaker and retry logic
        self.execute_with_circuit_breaker(
            &self.primary_circuit_breaker,
            "submit_stealth_payment",
            || {
                let client = &self.primary_client;
                let payer_pubkey = payer.pubkey();
                let stealth_addr = *stealth_address;
                let eph_pk = *ephemeral_public_key;
                let tag = *viewing_tag;
                
                async move {
                    // Get recent blockhash
                    let recent_blockhash = client
                        .get_latest_blockhash()
                        .map_err(|e| Error::SolanaRpc(format!("Failed to get blockhash: {}", e)))?;
                    
                    // Create transfer instruction
                    let transfer_ix = system_instruction::transfer(&payer_pubkey, &stealth_addr, amount);
                    
                    // Create stealth metadata instruction
                    let metadata_ix = create_stealth_metadata_instruction(&eph_pk, &tag, version);
                    
                    // Build transaction
                    let transaction = Transaction::new_signed_with_payer(
                        &[transfer_ix, metadata_ix],
                        Some(&payer_pubkey),
                        &[payer],
                        recent_blockhash,
                    );
                    
                    // Submit transaction
                    client
                        .send_and_confirm_transaction_with_spinner(&transaction)
                        .map_err(|e| Error::SolanaRpc(format!("Transaction failed: {}", e)))
                }
            },
        )
        .await
    }

    /// Scan blockchain for stealth payment metadata
    /// 
    /// This method scans transactions in a slot range for stealth payment metadata.
    /// It looks for memo instructions containing stealth metadata (version, viewing tag, ephemeral key).
    /// 
    /// # Arguments
    /// * `from_slot` - Starting slot (inclusive)
    /// * `to_slot` - Ending slot (inclusive)
    /// 
    /// # Requirements
    /// Validates: Requirements 10.1
    /// 
    /// # Returns
    /// Vector of stealth metadata found in the slot range
    pub async fn scan_stealth_metadata(
        &self,
        from_slot: u64,
        to_slot: u64,
    ) -> Result<Vec<StealthMetadata>> {
        debug!("Scanning stealth metadata from slot {} to {}", from_slot, to_slot);
        
        // Execute with circuit breaker and retry logic
        self.execute_with_circuit_breaker(
            &self.primary_circuit_breaker,
            "scan_stealth_metadata",
            || {
                let client = &self.primary_client;
                
                async move {
                    let mut metadata_list = Vec::new();
                    
                    // Scan each slot in the range
                    for slot in from_slot..=to_slot {
                        // Get block for this slot
                        let block = match client.get_block(slot) {
                            Ok(block) => block,
                            Err(e) => {
                                // Skip missing blocks (common on devnet/testnet)
                                debug!("Skipping slot {}: {}", slot, e);
                                continue;
                            }
                        };
                        
                        // Scan transactions in this block
                        for tx in block.transactions {
                            if let Some(meta) = tx.meta {
                                // Check if transaction succeeded
                                if meta.err.is_some() {
                                    continue;
                                }
                            }
                            
                            // Parse transaction to find stealth metadata
                            if let Some(transaction) = tx.transaction.decode() {
                                // Get signature from the transaction
                                let signature = if let Some(sig) = transaction.signatures.first() {
                                    *sig
                                } else {
                                    continue;
                                };
                                
                                // Iterate through instructions
                                for instruction in transaction.message.instructions() {
                                    // Check if this is a memo instruction with stealth metadata
                                    if let Some(stealth_meta) = parse_stealth_metadata(&instruction.data) {
                                        metadata_list.push(StealthMetadata {
                                            slot,
                                            signature,
                                            ephemeral_public_key: stealth_meta.ephemeral_public_key,
                                            viewing_tag: stealth_meta.viewing_tag,
                                            version: stealth_meta.version,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    
                    debug!("Found {} stealth metadata entries", metadata_list.len());
                    Ok(metadata_list)
                }
            },
        )
        .await
    }

    /// Execute an operation with circuit breaker and retry logic
    async fn execute_with_circuit_breaker<F, Fut, T>(
        &self,
        circuit_breaker: &CircuitBreaker,
        operation_name: &str,
        operation: F,
    ) -> Result<T>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        // Check if circuit breaker allows the request
        if !circuit_breaker.is_request_allowed().await {
            let state = circuit_breaker.get_state().await;
            error!(
                "Circuit breaker is {:?} for operation: {}",
                state, operation_name
            );
            return Err(Error::CircuitBreakerOpen(format!(
                "Circuit breaker is open for {}",
                operation_name
            )));
        }

        // Execute with retry logic
        let result = retry_with_backoff(
            operation_name,
            &self.retry_config,
            operation,
        )
        .await;

        // Record success or failure in circuit breaker
        match &result {
            Ok(_) => {
                circuit_breaker.record_success().await;
            }
            Err(_) => {
                circuit_breaker.record_failure().await;
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_address_valid() {
        let client = SolanaClient::new(
            "https://api.mainnet-beta.solana.com".to_string(),
            None,
        );

        // Valid Solana address
        let result = client.validate_address("11111111111111111111111111111111");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_address_invalid() {
        let client = SolanaClient::new(
            "https://api.mainnet-beta.solana.com".to_string(),
            None,
        );

        // Invalid address
        let result = client.validate_address("invalid_address");
        assert!(result.is_err());
        
        if let Err(Error::InvalidWalletAddress(msg)) = result {
            assert!(msg.contains("Invalid Solana address format"));
        } else {
            panic!("Expected InvalidWalletAddress error");
        }
    }

    #[test]
    fn test_validate_address_empty() {
        let client = SolanaClient::new(
            "https://api.mainnet-beta.solana.com".to_string(),
            None,
        );

        let result = client.validate_address("");
        assert!(result.is_err());
    }

    // Task 21.3: Unit tests for blockchain integration

    #[test]
    fn test_create_stealth_metadata_instruction() {
        let ephemeral_pk = Pubkey::new_unique();
        let viewing_tag = [0x01, 0x02, 0x03, 0x04];
        let version = 1u8;
        
        let instruction = create_stealth_metadata_instruction(&ephemeral_pk, &viewing_tag, version);
        
        // Verify instruction structure
        assert_eq!(
            instruction.program_id,
            solana_sdk::pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
            "Should use SPL Memo program"
        );
        assert_eq!(instruction.accounts.len(), 0, "Memo instruction should have no accounts");
        
        // Verify data format: version (1) + viewing_tag (4) + ephemeral_pk (32) = 37 bytes
        assert_eq!(instruction.data.len(), 37, "Metadata should be 37 bytes");
        assert_eq!(instruction.data[0], version, "First byte should be version");
        assert_eq!(&instruction.data[1..5], &viewing_tag, "Bytes 1-4 should be viewing tag");
        assert_eq!(
            &instruction.data[5..37],
            &ephemeral_pk.to_bytes(),
            "Bytes 5-36 should be ephemeral public key"
        );
    }

    #[test]
    fn test_create_stealth_metadata_instruction_different_versions() {
        let ephemeral_pk = Pubkey::new_unique();
        let viewing_tag = [0xAA, 0xBB, 0xCC, 0xDD];
        
        // Test version 1 (standard)
        let instruction_v1 = create_stealth_metadata_instruction(&ephemeral_pk, &viewing_tag, 1);
        assert_eq!(instruction_v1.data[0], 1, "Version 1 should be encoded");
        
        // Test version 2 (hybrid)
        let instruction_v2 = create_stealth_metadata_instruction(&ephemeral_pk, &viewing_tag, 2);
        assert_eq!(instruction_v2.data[0], 2, "Version 2 should be encoded");
        
        // Rest of the data should be the same
        assert_eq!(
            &instruction_v1.data[1..],
            &instruction_v2.data[1..],
            "Viewing tag and ephemeral key should be the same"
        );
    }

    #[test]
    fn test_parse_stealth_metadata_valid() {
        let ephemeral_pk = Pubkey::new_unique();
        let viewing_tag = [0x11, 0x22, 0x33, 0x44];
        let version = 1u8;
        
        // Create metadata
        let mut metadata = Vec::with_capacity(37);
        metadata.push(version);
        metadata.extend_from_slice(&viewing_tag);
        metadata.extend_from_slice(&ephemeral_pk.to_bytes());
        
        // Parse it back
        let parsed = parse_stealth_metadata(&metadata);
        assert!(parsed.is_some(), "Should parse valid metadata");
        
        let parsed = parsed.unwrap();
        assert_eq!(parsed.version, version, "Version should match");
        assert_eq!(parsed.viewing_tag, viewing_tag, "Viewing tag should match");
        assert_eq!(parsed.ephemeral_public_key, ephemeral_pk, "Ephemeral key should match");
    }

    #[test]
    fn test_parse_stealth_metadata_invalid_length() {
        // Too short
        let short_data = vec![1, 2, 3];
        assert!(parse_stealth_metadata(&short_data).is_none(), "Should reject short data");
        
        // Too long
        let long_data = vec![0u8; 50];
        assert!(parse_stealth_metadata(&long_data).is_none(), "Should reject long data");
    }

    #[test]
    fn test_parse_stealth_metadata_invalid_version() {
        let ephemeral_pk = Pubkey::new_unique();
        let viewing_tag = [0x11, 0x22, 0x33, 0x44];
        let invalid_version = 99u8; // Unsupported version
        
        // Create metadata with invalid version
        let mut metadata = Vec::with_capacity(37);
        metadata.push(invalid_version);
        metadata.extend_from_slice(&viewing_tag);
        metadata.extend_from_slice(&ephemeral_pk.to_bytes());
        
        // Should reject invalid version
        assert!(parse_stealth_metadata(&metadata).is_none(), "Should reject invalid version");
    }

    #[test]
    fn test_parse_stealth_metadata_version_2() {
        let ephemeral_pk = Pubkey::new_unique();
        let viewing_tag = [0xAA, 0xBB, 0xCC, 0xDD];
        let version = 2u8; // Hybrid mode
        
        // Create metadata
        let mut metadata = Vec::with_capacity(37);
        metadata.push(version);
        metadata.extend_from_slice(&viewing_tag);
        metadata.extend_from_slice(&ephemeral_pk.to_bytes());
        
        // Parse it back
        let parsed = parse_stealth_metadata(&metadata);
        assert!(parsed.is_some(), "Should parse version 2 metadata");
        
        let parsed = parsed.unwrap();
        assert_eq!(parsed.version, 2, "Version should be 2");
    }

    #[test]
    fn test_stealth_metadata_round_trip() {
        let ephemeral_pk = Pubkey::new_unique();
        let viewing_tag = [0x12, 0x34, 0x56, 0x78];
        let version = 1u8;
        
        // Create instruction
        let instruction = create_stealth_metadata_instruction(&ephemeral_pk, &viewing_tag, version);
        
        // Parse the instruction data
        let parsed = parse_stealth_metadata(&instruction.data);
        assert!(parsed.is_some(), "Should parse instruction data");
        
        let parsed = parsed.unwrap();
        assert_eq!(parsed.version, version, "Version should match");
        assert_eq!(parsed.viewing_tag, viewing_tag, "Viewing tag should match");
        assert_eq!(parsed.ephemeral_public_key, ephemeral_pk, "Ephemeral key should match");
    }

    #[test]
    fn test_stealth_metadata_struct() {
        use solana_sdk::signature::Signature;
        
        let metadata = StealthMetadata {
            slot: 12345,
            signature: Signature::new_unique(),
            ephemeral_public_key: Pubkey::new_unique(),
            viewing_tag: [0x01, 0x02, 0x03, 0x04],
            version: 1,
        };
        
        assert_eq!(metadata.slot, 12345);
        assert_eq!(metadata.version, 1);
        assert_eq!(metadata.viewing_tag.len(), 4);
    }

    #[test]
    fn test_parsed_stealth_metadata_struct() {
        let parsed = ParsedStealthMetadata {
            ephemeral_public_key: Pubkey::new_unique(),
            viewing_tag: [0xAA, 0xBB, 0xCC, 0xDD],
            version: 2,
        };
        
        assert_eq!(parsed.version, 2);
        assert_eq!(parsed.viewing_tag, [0xAA, 0xBB, 0xCC, 0xDD]);
    }

    // Note: Integration tests for submit_stealth_payment and scan_stealth_metadata
    // that interact with the Solana blockchain will be implemented in task 27
    // (comprehensive integration tests). These tests require a running Solana
    // test validator or devnet connection.
}
