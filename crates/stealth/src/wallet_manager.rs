//! High-level stealth wallet management

use crate::error::{StealthError, StealthResult};
use crate::generator::StealthAddressGenerator;
use crate::keypair::StealthKeyPair;
use crate::payment_queue::PaymentStatus;
use crate::scanner::{DetectedPayment, StealthScanner};
use serde::{Deserialize, Serialize};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    system_instruction,
    transaction::Transaction,
};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// High-level stealth wallet manager
/// 
/// Integrates all stealth address components:
/// - Key pair management
/// - Stealth address generation
/// - Blockchain scanning
/// - Payment queue (optional)
/// 
/// # Requirements
/// Validates: Requirements 2.1, 2.2, 2.3, 3.1, 3.6, 5.1, 5.3
pub struct StealthWalletManager {
    keypair: StealthKeyPair,
    scanner: StealthScanner,
    rpc_client: Arc<RpcClient>,
}

impl StealthWalletManager {
    /// Create a new stealth wallet manager
    /// 
    /// # Arguments
    /// * `keypair` - The stealth key pair for this wallet
    /// * `rpc_url` - Solana RPC endpoint URL
    /// 
    /// # Requirements
    /// Validates: Requirements 2.1, 3.1
    pub fn new(keypair: StealthKeyPair, rpc_url: &str) -> Self {
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            rpc_url.to_string(),
            CommitmentConfig::confirmed(),
        ));
        
        let scanner = StealthScanner::new(&keypair, rpc_url);
        
        info!("Initialized StealthWalletManager with meta-address: {}", keypair.to_meta_address());
        
        Self {
            keypair,
            scanner,
            rpc_client,
        }
    }

    /// Get meta-address for receiving payments
    /// 
    /// Returns the formatted meta-address that can be shared with senders.
    /// Format: `stealth:1:<spending_pk>:<viewing_pk>`
    /// 
    /// # Requirements
    /// Validates: Requirements 2.1, 2.2
    pub fn get_meta_address(&self) -> String {
        self.keypair.to_meta_address()
    }

    /// Generate stealth address for sending to receiver
    /// 
    /// Prepares a payment by generating a one-time stealth address for the receiver.
    /// This method performs the sender-side stealth address derivation using ECDH.
    /// 
    /// # Arguments
    /// * `receiver_meta_address` - The receiver's meta-address
    /// * `amount` - Amount in lamports to send
    /// 
    /// # Returns
    /// A PreparedPayment containing the stealth address, ephemeral key, and viewing tag
    /// 
    /// # Requirements
    /// Validates: Requirements 2.3
    pub fn prepare_payment(
        &self,
        receiver_meta_address: &str,
        amount: u64,
    ) -> StealthResult<PreparedPayment> {
        debug!(
            "Preparing payment of {} lamports to meta-address: {}",
            amount, receiver_meta_address
        );
        
        // Generate stealth address using the generator
        let stealth_output = StealthAddressGenerator::generate_stealth_address_uncached(
            receiver_meta_address,
            None, // Generate random ephemeral key
        )?;
        
        info!(
            "Prepared payment: stealth_address={}, ephemeral_key={}, viewing_tag={:?}",
            stealth_output.stealth_address,
            stealth_output.ephemeral_public_key,
            stealth_output.viewing_tag
        );
        
        Ok(PreparedPayment {
            stealth_address: stealth_output.stealth_address,
            amount,
            ephemeral_public_key: stealth_output.ephemeral_public_key,
            viewing_tag: stealth_output.viewing_tag,
        })
    }

    /// Send payment (online) or queue (offline)
    /// 
    /// This is a simplified implementation that attempts to send the payment immediately.
    /// In a full implementation with payment queue integration, this would:
    /// 1. Check network connectivity
    /// 2. If online: submit transaction to blockchain
    /// 3. If offline: add to payment queue for later settlement
    /// 
    /// # Requirements
    /// Validates: Requirements 5.1, 5.3
    /// 
    /// # Note
    /// Full payment queue integration will be completed when PaymentQueue is integrated
    /// with the wallet manager (requires payer keypair and network monitor).
    pub async fn send_payment(&mut self, prepared: PreparedPayment) -> StealthResult<PaymentStatus> {
        info!(
            "Sending payment of {} lamports to stealth address: {}",
            prepared.amount, prepared.stealth_address
        );
        
        // For now, this is a simplified implementation that attempts immediate settlement
        // Full implementation with payment queue will be added when integrating with
        // the payment queue component (requires payer keypair and network monitor)
        
        // Get recent blockhash
        let _recent_blockhash = self.rpc_client
            .get_latest_blockhash()
            .map_err(|e| {
                error!("Failed to get recent blockhash: {}", e);
                StealthError::BlockchainError(format!("Failed to get recent blockhash: {}", e))
            })?;
        
        // Note: In a real implementation, we would need a payer keypair to sign the transaction
        // For now, we return Queued status to indicate the payment is ready but needs
        // a payer keypair to be submitted
        
        warn!("Payment prepared but requires payer keypair for submission. Returning Queued status.");
        
        Ok(PaymentStatus::Queued)
    }

    /// Scan for incoming payments
    /// 
    /// Scans the blockchain for stealth payments sent to this wallet.
    /// Uses the viewing key to detect payments without exposing the spending key.
    /// 
    /// # Requirements
    /// Validates: Requirements 3.1, 3.6
    pub async fn scan_incoming(&mut self) -> StealthResult<Vec<DetectedPayment>> {
        info!("Scanning blockchain for incoming stealth payments");
        
        // Scan using the scanner component
        let detected = self.scanner.scan_for_payments(None, None).await?;
        
        info!("Scan complete. Found {} stealth payments", detected.len());
        
        Ok(detected)
    }

    /// Shield: convert regular funds to stealth address
    /// 
    /// This method transfers funds from a regular address to a newly generated
    /// stealth address, breaking on-chain transaction graph linkage.
    /// 
    /// # Arguments
    /// * `amount` - Amount in lamports to shield
    /// * `source_keypair` - Keypair controlling the source funds
    /// 
    /// # Requirements
    /// Validates: Requirements 7.1, 7.2, 7.3, 7.4, 7.5
    /// 
    /// # Returns
    /// Transaction signature on success
    pub async fn shield(
        &mut self,
        amount: u64,
        source_keypair: &Keypair,
    ) -> StealthResult<Signature> {
        info!("Initiating shield operation for {} lamports", amount);
        
        // Generate a new stealth address for this wallet (Requirement 7.1, 7.4)
        let meta_address = self.keypair.to_meta_address();
        let stealth_output = StealthAddressGenerator::generate_stealth_address_uncached(&meta_address, None)
            .map_err(|e| {
                error!("Failed to generate stealth address for shield: {}", e);
                e
            })?;
        
        debug!(
            "Generated stealth address: {}, ephemeral key: {}",
            stealth_output.stealth_address, stealth_output.ephemeral_public_key
        );
        
        // Get recent blockhash
        let recent_blockhash = self.rpc_client
            .get_latest_blockhash()
            .map_err(|e| {
                error!("Failed to get recent blockhash: {}", e);
                StealthError::BlockchainError(format!("Failed to get recent blockhash: {}", e))
            })?;
        
        // Create transfer instruction from source to stealth address (Requirement 7.1)
        let transfer_instruction = system_instruction::transfer(
            &source_keypair.pubkey(),
            &stealth_output.stealth_address,
            amount,
        );
        
        // Create custom instruction to store stealth metadata on-chain (Requirement 7.3)
        // This includes the ephemeral public key and viewing tag for scanning
        let metadata_instruction = create_stealth_metadata_instruction(
            &stealth_output.ephemeral_public_key,
            &stealth_output.viewing_tag,
            1, // version 1 (standard mode)
        );
        
        // Build transaction with both instructions
        let transaction = Transaction::new_signed_with_payer(
            &[transfer_instruction, metadata_instruction],
            Some(&source_keypair.pubkey()),
            &[source_keypair],
            recent_blockhash,
        );
        
        // Submit transaction to blockchain
        let signature = self.rpc_client
            .send_and_confirm_transaction_with_spinner(&transaction)
            .map_err(|e| {
                error!("Shield transaction failed: {}", e);
                StealthError::BlockchainError(format!("Shield transaction failed: {}", e))
            })?;
        
        info!(
            "Shield operation successful. Signature: {}, stealth address: {}",
            signature, stealth_output.stealth_address
        );
        
        Ok(signature)
    }

    /// Unshield: convert stealth funds to regular address
    /// 
    /// This method transfers funds from a stealth address back to a regular address.
    /// The stealth private key is derived from the detected payment information.
    /// 
    /// # Arguments
    /// * `detected_payment` - Information about the detected stealth payment
    /// * `destination` - Regular address to receive the unshielded funds
    /// 
    /// # Requirements
    /// Validates: Requirements 7.2, 7.5
    /// 
    /// # Returns
    /// Transaction signature on success
    pub async fn unshield(
        &mut self,
        detected_payment: &DetectedPayment,
        destination: &Pubkey,
    ) -> StealthResult<Signature> {
        info!(
            "Initiating unshield operation from stealth address: {} to destination: {}",
            detected_payment.stealth_address, destination
        );
        
        // Derive the spending key for this stealth address (Requirement 7.5)
        let spending_secret = self.keypair.spending_secret_key();
        let stealth_keypair = self.scanner.derive_spending_key(
            &detected_payment.ephemeral_public_key,
            &spending_secret,
        )?;
        
        let derived_pubkey = Pubkey::new_from_array(stealth_keypair.public.to_bytes());
        debug!(
            "Derived stealth keypair. Public key: {}",
            derived_pubkey
        );
        
        // Verify the derived public key matches the stealth address
        if derived_pubkey != detected_payment.stealth_address {
            error!(
                "Derived public key {} does not match stealth address {}",
                derived_pubkey, detected_payment.stealth_address
            );
            return Err(StealthError::KeyDerivationFailed(
                "Derived key does not match stealth address".to_string(),
            ));
        }
        
        // Get the balance at the stealth address
        let balance = self.rpc_client
            .get_balance(&detected_payment.stealth_address)
            .map_err(|e| {
                error!("Failed to get stealth address balance: {}", e);
                StealthError::BlockchainError(format!("Failed to get balance: {}", e))
            })?;
        
        if balance == 0 {
            return Err(StealthError::InsufficientBalance(
                "Stealth address has zero balance".to_string(),
            ));
        }
        
        // Calculate transfer amount (leave some for transaction fee)
        // Estimate fee at 5000 lamports (typical for simple transfer)
        const ESTIMATED_FEE: u64 = 5000;
        if balance <= ESTIMATED_FEE {
            return Err(StealthError::InsufficientBalance(
                format!("Balance {} too low to cover fee", balance),
            ));
        }
        let transfer_amount = balance - ESTIMATED_FEE;
        
        debug!(
            "Unshielding {} lamports (balance: {}, fee: {})",
            transfer_amount, balance, ESTIMATED_FEE
        );
        
        // Get recent blockhash
        let recent_blockhash = self.rpc_client
            .get_latest_blockhash()
            .map_err(|e| {
                error!("Failed to get recent blockhash: {}", e);
                StealthError::BlockchainError(format!("Failed to get recent blockhash: {}", e))
            })?;
        
        // Create transfer instruction from stealth address to destination (Requirement 7.2)
        let transfer_instruction = system_instruction::transfer(
            &detected_payment.stealth_address,
            destination,
            transfer_amount,
        );
        
        // Convert Ed25519 keypair to Solana Keypair for signing
        let stealth_signer = ed25519_to_solana_keypair(&stealth_keypair)?;
        
        // Build and sign transaction
        let transaction = Transaction::new_signed_with_payer(
            &[transfer_instruction],
            Some(&detected_payment.stealth_address),
            &[&stealth_signer],
            recent_blockhash,
        );
        
        // Submit transaction to blockchain
        let signature = self.rpc_client
            .send_and_confirm_transaction_with_spinner(&transaction)
            .map_err(|e| {
                error!("Unshield transaction failed: {}", e);
                StealthError::BlockchainError(format!("Unshield transaction failed: {}", e))
            })?;
        
        info!(
            "Unshield operation successful. Signature: {}, transferred {} lamports",
            signature, transfer_amount
        );
        
        Ok(signature)
    }
}

/// A prepared payment ready to send
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparedPayment {
    pub stealth_address: Pubkey,
    pub amount: u64,
    pub ephemeral_public_key: Pubkey,
    pub viewing_tag: [u8; 4],
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

/// Convert Ed25519 Keypair to Solana SDK Keypair
/// 
/// This is needed because the stealth scanner uses ed25519-dalek keypairs,
/// but Solana transaction signing requires solana_sdk::signature::Keypair.
fn ed25519_to_solana_keypair(ed_keypair: &ed25519_dalek::Keypair) -> StealthResult<Keypair> {
    // Combine secret and public key bytes (Solana format: 64 bytes = 32 secret + 32 public)
    let mut keypair_bytes = [0u8; 64];
    keypair_bytes[..32].copy_from_slice(&ed_keypair.secret.to_bytes());
    keypair_bytes[32..].copy_from_slice(&ed_keypair.public.to_bytes());
    
    Keypair::from_bytes(&keypair_bytes).map_err(|e| {
        StealthError::KeyDerivationFailed(format!("Failed to convert Ed25519 keypair: {}", e))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keypair::StealthKeyPair;

    /// Probe the Solana RPC endpoint with a short timeout.
    ///
    /// Returns `Err` (with a human-readable reason) when the endpoint is
    /// unreachable so callers can skip-with-named-reason rather than fail
    /// (Requirement 6.5: skipped distinct from failed for unreachable
    /// dependencies). The blocking RPC call runs on a blocking thread and is
    /// bounded by an outer timeout so it cannot hang the suite.
    async fn probe_solana_rpc(rpc_url: &str) -> Result<(), String> {
        let url = rpc_url.to_string();
        let probe = tokio::task::spawn_blocking(move || {
            let client = RpcClient::new_with_timeout_and_commitment(
                url,
                std::time::Duration::from_secs(3),
                CommitmentConfig::confirmed(),
            );
            // `get_latest_blockhash` is the exact RPC the dependent tests rely
            // on; if it succeeds the endpoint is reachable.
            client.get_latest_blockhash()
        });

        match tokio::time::timeout(std::time::Duration::from_secs(5), probe).await {
            Ok(Ok(Ok(_))) => Ok(()),
            Ok(Ok(Err(e))) => Err(format!("RPC request failed: {}", e)),
            Ok(Err(e)) => Err(format!("probe task failed: {}", e)),
            Err(_) => Err("RPC probe timed out after 5s".to_string()),
        }
    }

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
    fn test_ed25519_to_solana_keypair_conversion() {
        // Generate an Ed25519 keypair
        let mut secret_bytes = [0u8; 32];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut secret_bytes);
        
        let ed_secret = ed25519_dalek::SecretKey::from_bytes(&secret_bytes).unwrap();
        let ed_public: ed25519_dalek::PublicKey = (&ed_secret).into();
        let ed_keypair = ed25519_dalek::Keypair {
            secret: ed_secret,
            public: ed_public,
        };
        
        // Convert to Solana keypair
        let result = ed25519_to_solana_keypair(&ed_keypair);
        assert!(result.is_ok(), "Conversion should succeed");
        
        let solana_keypair = result.unwrap();
        
        // Verify the public keys match
        let ed_pubkey = Pubkey::new_from_array(ed_keypair.public.to_bytes());
        assert_eq!(
            solana_keypair.pubkey(),
            ed_pubkey,
            "Public keys should match after conversion"
        );
    }

    #[test]
    fn test_ed25519_to_solana_keypair_deterministic() {
        // Create the same Ed25519 keypair twice
        let secret_bytes = [42u8; 32];
        
        let ed_secret1 = ed25519_dalek::SecretKey::from_bytes(&secret_bytes).unwrap();
        let ed_public1: ed25519_dalek::PublicKey = (&ed_secret1).into();
        let ed_keypair1 = ed25519_dalek::Keypair {
            secret: ed_secret1,
            public: ed_public1,
        };
        
        let ed_secret2 = ed25519_dalek::SecretKey::from_bytes(&secret_bytes).unwrap();
        let ed_public2: ed25519_dalek::PublicKey = (&ed_secret2).into();
        let ed_keypair2 = ed25519_dalek::Keypair {
            secret: ed_secret2,
            public: ed_public2,
        };
        
        // Convert both to Solana keypairs
        let solana_keypair1 = ed25519_to_solana_keypair(&ed_keypair1).unwrap();
        let solana_keypair2 = ed25519_to_solana_keypair(&ed_keypair2).unwrap();
        
        // They should produce the same result
        assert_eq!(
            solana_keypair1.pubkey(),
            solana_keypair2.pubkey(),
            "Same Ed25519 keypair should produce same Solana keypair"
        );
    }

    #[test]
    fn test_metadata_instruction_different_versions() {
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

    // Note: Integration tests for shield() and unshield() that interact with
    // the Solana blockchain will be implemented in task 27 (integration tests).
    // These tests require a running Solana test validator or devnet connection.

    // Task 16.2: Unit tests for wallet manager

    #[test]
    fn test_wallet_manager_new() {
        let keypair = StealthKeyPair::generate_standard().unwrap();
        let wallet = StealthWalletManager::new(keypair, "https://api.devnet.solana.com");
        
        // Verify wallet was created successfully
        let meta_address = wallet.get_meta_address();
        assert!(meta_address.starts_with("stealth:1:"), "Meta-address should have correct format");
    }

    #[test]
    fn test_get_meta_address() {
        let keypair = StealthKeyPair::generate_standard().unwrap();
        let expected_meta = keypair.to_meta_address();
        
        let wallet = StealthWalletManager::new(keypair, "https://api.devnet.solana.com");
        let actual_meta = wallet.get_meta_address();
        
        assert_eq!(actual_meta, expected_meta, "Meta-address should match keypair's meta-address");
    }

    #[test]
    fn test_get_meta_address_format() {
        let keypair = StealthKeyPair::generate_standard().unwrap();
        let wallet = StealthWalletManager::new(keypair, "https://api.devnet.solana.com");
        
        let meta_address = wallet.get_meta_address();
        
        // Verify format: stealth:1:<spending_pk>:<viewing_pk>
        let parts: Vec<&str> = meta_address.split(':').collect();
        assert_eq!(parts.len(), 4, "Meta-address should have 4 parts");
        assert_eq!(parts[0], "stealth", "First part should be 'stealth'");
        assert_eq!(parts[1], "1", "Version should be 1");
        
        // Verify public keys are valid
        assert!(parts[2].parse::<Pubkey>().is_ok(), "Spending key should be valid");
        assert!(parts[3].parse::<Pubkey>().is_ok(), "Viewing key should be valid");
    }

    #[test]
    fn test_prepare_payment_basic() {
        let sender_keypair = StealthKeyPair::generate_standard().unwrap();
        let receiver_keypair = StealthKeyPair::generate_standard().unwrap();
        
        let wallet = StealthWalletManager::new(sender_keypair, "https://api.devnet.solana.com");
        let receiver_meta = receiver_keypair.to_meta_address();
        
        let amount = 1_000_000u64;
        let result = wallet.prepare_payment(&receiver_meta, amount);
        
        assert!(result.is_ok(), "Payment preparation should succeed");
        
        let prepared = result.unwrap();
        assert_eq!(prepared.amount, amount, "Amount should match");
        assert_ne!(prepared.stealth_address, Pubkey::default(), "Stealth address should be generated");
        assert_ne!(prepared.ephemeral_public_key, Pubkey::default(), "Ephemeral key should be generated");
        assert_ne!(prepared.viewing_tag, [0u8; 4], "Viewing tag should be generated");
    }

    #[test]
    fn test_prepare_payment_generates_unique_addresses() {
        let sender_keypair = StealthKeyPair::generate_standard().unwrap();
        let receiver_keypair = StealthKeyPair::generate_standard().unwrap();
        
        let wallet = StealthWalletManager::new(sender_keypair, "https://api.devnet.solana.com");
        let receiver_meta = receiver_keypair.to_meta_address();
        
        // Prepare two payments to the same receiver
        let prepared1 = wallet.prepare_payment(&receiver_meta, 1_000_000).unwrap();
        let prepared2 = wallet.prepare_payment(&receiver_meta, 1_000_000).unwrap();
        
        // Each payment should have unique stealth address and ephemeral key
        assert_ne!(
            prepared1.stealth_address, prepared2.stealth_address,
            "Each payment should have unique stealth address"
        );
        assert_ne!(
            prepared1.ephemeral_public_key, prepared2.ephemeral_public_key,
            "Each payment should have unique ephemeral key"
        );
        assert_ne!(
            prepared1.viewing_tag, prepared2.viewing_tag,
            "Each payment should have unique viewing tag"
        );
    }

    #[test]
    fn test_prepare_payment_invalid_meta_address() {
        let sender_keypair = StealthKeyPair::generate_standard().unwrap();
        let wallet = StealthWalletManager::new(sender_keypair, "https://api.devnet.solana.com");
        
        let invalid_cases = vec![
            "invalid",
            "stealth:1:invalid:invalid",
            "",
            "stealth:2:key1:key2", // Wrong version
        ];
        
        for invalid in invalid_cases {
            let result = wallet.prepare_payment(invalid, 1_000_000);
            assert!(result.is_err(), "Should reject invalid meta-address: {}", invalid);
        }
    }

    #[test]
    fn test_prepare_payment_different_amounts() {
        let sender_keypair = StealthKeyPair::generate_standard().unwrap();
        let receiver_keypair = StealthKeyPair::generate_standard().unwrap();
        
        let wallet = StealthWalletManager::new(sender_keypair, "https://api.devnet.solana.com");
        let receiver_meta = receiver_keypair.to_meta_address();
        
        let amounts = vec![1_000, 1_000_000, 1_000_000_000];
        
        for amount in amounts {
            let prepared = wallet.prepare_payment(&receiver_meta, amount).unwrap();
            assert_eq!(prepared.amount, amount, "Amount should be preserved");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_payment_returns_queued_status() {
        // Note: This is a simplified test since full send_payment implementation
        // requires payer keypair and network monitor integration

        const RPC_URL: &str = "https://api.devnet.solana.com";

        // send_payment calls get_latest_blockhash, so it requires a reachable
        // Solana RPC endpoint. Skip-with-named-reason when it is unreachable
        // instead of panicking (Requirement 6.5).
        if let Err(reason) = probe_solana_rpc(RPC_URL).await {
            eprintln!("SKIPPED: Solana RPC unavailable — {}", reason);
            return;
        }

        let sender_keypair = StealthKeyPair::generate_standard().unwrap();
        let receiver_keypair = StealthKeyPair::generate_standard().unwrap();

        let mut wallet = StealthWalletManager::new(sender_keypair, RPC_URL);
        let receiver_meta = receiver_keypair.to_meta_address();
        
        let prepared = wallet.prepare_payment(&receiver_meta, 1_000_000).unwrap();
        
        // Send payment (should return Queued status in current implementation)
        let result = wallet.send_payment(prepared).await;
        
        assert!(result.is_ok(), "Send payment should not error");
        
        let status = result.unwrap();
        assert_eq!(status, PaymentStatus::Queued, "Should return Queued status");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_scan_incoming_empty_blockchain() {
        // Note: This test uses devnet which may have no stealth payments for our test wallet

        const RPC_URL: &str = "https://api.devnet.solana.com";

        // scan_incoming queries the blockchain, so it requires a reachable
        // Solana RPC endpoint. Skip-with-named-reason when it is unreachable
        // instead of panicking (Requirement 6.5).
        if let Err(reason) = probe_solana_rpc(RPC_URL).await {
            eprintln!("SKIPPED: Solana RPC unavailable — {}", reason);
            return;
        }

        let keypair = StealthKeyPair::generate_standard().unwrap();
        let mut wallet = StealthWalletManager::new(keypair, RPC_URL);
        
        // Scan for incoming payments
        let result = wallet.scan_incoming().await;
        
        // Should succeed even if no payments found
        assert!(result.is_ok(), "Scan should succeed");
        
        let detected = result.unwrap();
        // Likely empty since this is a fresh test wallet
        assert_eq!(detected.len(), 0, "Test wallet should have no payments");
    }

    #[test]
    fn test_wallet_manager_with_different_rpc_urls() {
        let urls = vec![
            "https://api.devnet.solana.com",
            "https://api.testnet.solana.com",
            "https://api.mainnet-beta.solana.com",
        ];
        
        for url in urls {
            let keypair = StealthKeyPair::generate_standard().unwrap();
            let wallet = StealthWalletManager::new(keypair, url);
            let meta = wallet.get_meta_address();
            assert!(meta.starts_with("stealth:1:"), "Should work with URL: {}", url);
        }
    }

    #[test]
    fn test_prepared_payment_structure() {
        let sender_keypair = StealthKeyPair::generate_standard().unwrap();
        let receiver_keypair = StealthKeyPair::generate_standard().unwrap();
        
        let wallet = StealthWalletManager::new(sender_keypair, "https://api.devnet.solana.com");
        let receiver_meta = receiver_keypair.to_meta_address();
        
        let prepared = wallet.prepare_payment(&receiver_meta, 5_000_000).unwrap();
        
        // Verify PreparedPayment has all required fields
        assert_eq!(prepared.amount, 5_000_000);
        assert_ne!(prepared.stealth_address.to_bytes(), [0u8; 32]);
        assert_ne!(prepared.ephemeral_public_key.to_bytes(), [0u8; 32]);
        assert_eq!(prepared.viewing_tag.len(), 4);
    }

    #[test]
    fn test_wallet_manager_keypair_independence() {
        // Create two different wallets
        let keypair1 = StealthKeyPair::generate_standard().unwrap();
        let keypair2 = StealthKeyPair::generate_standard().unwrap();
        
        let wallet1 = StealthWalletManager::new(keypair1, "https://api.devnet.solana.com");
        let wallet2 = StealthWalletManager::new(keypair2, "https://api.devnet.solana.com");
        
        // They should have different meta-addresses
        assert_ne!(
            wallet1.get_meta_address(),
            wallet2.get_meta_address(),
            "Different wallets should have different meta-addresses"
        );
    }
}
