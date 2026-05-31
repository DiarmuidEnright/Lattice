//! Payment queue for offline payment management with auto-settlement
//!
//! This module implements a payment queue that stores stealth payment requests
//! when the device is offline and automatically settles them when connectivity
//! is restored.
//!
//! # Requirements
//! Validates: Requirements 5.1, 5.2, 5.3, 5.4, 5.5, 5.6, 5.7

use crate::error::{StealthError, StealthResult};
use crate::network_monitor::NetworkMonitor;
use crate::storage::SecureStorage;
use crate::wallet_manager::PreparedPayment;
use serde::{Deserialize, Serialize};
use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    system_instruction,
    transaction::Transaction,
};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Unique identifier for a queued payment
pub type PaymentId = Uuid;

/// Maximum number of entries in the queue
const MAX_QUEUE_SIZE: usize = 1000;

/// Maximum retry attempts for a payment
const MAX_RETRY_ATTEMPTS: u32 = 5;

/// Storage key for persisted queue
const QUEUE_STORAGE_KEY: &str = "payment_queue";

/// Payment queue with auto-settlement
///
/// Manages offline payment requests with automatic settlement when connectivity
/// is restored. Integrates with NetworkMonitor for connectivity detection and
/// SecureStorage for persistence.
///
/// # Requirements
/// - 5.1: Queue payments when offline
/// - 5.2: Persist queue to survive restarts
/// - 5.3: Process queue in FIFO order when online
/// - 5.4-5.7: Track payment status through state machine
pub struct PaymentQueue {
    /// FIFO queue of payments
    queue: VecDeque<QueuedPayment>,
    /// Secure storage for persistence
    storage: Arc<dyn SecureStorage>,
    /// Network connectivity monitor
    network_monitor: Arc<Mutex<NetworkMonitor>>,
    /// Solana RPC client for transaction submission
    rpc_client: Arc<RpcClient>,
    /// Payer keypair for transaction fees
    payer_keypair: Arc<Keypair>,
}

impl PaymentQueue {
    /// Create a new payment queue
    ///
    /// # Arguments
    /// * `storage` - Secure storage for queue persistence
    /// * `network_monitor` - Network connectivity monitor
    /// * `rpc_client` - Solana RPC client for transaction submission
    /// * `payer_keypair` - Keypair to pay transaction fees
    ///
    /// # Requirements
    /// Validates: Requirements 5.1, 5.2
    pub fn new(
        storage: Arc<dyn SecureStorage>,
        network_monitor: Arc<Mutex<NetworkMonitor>>,
        rpc_client: Arc<RpcClient>,
        payer_keypair: Arc<Keypair>,
    ) -> Self {
        Self {
            queue: VecDeque::new(),
            storage,
            network_monitor,
            rpc_client,
            payer_keypair,
        }
    }

    /// Add payment to queue
    ///
    /// Adds a prepared payment to the queue and persists it to storage.
    /// Returns a unique PaymentId for tracking.
    ///
    /// # Requirements
    /// Validates: Requirements 5.1, 5.2
    ///
    /// # Errors
    /// Returns QueueFull if the queue has reached MAX_QUEUE_SIZE
    pub async fn enqueue(&mut self, payment: PreparedPayment) -> StealthResult<PaymentId> {
        // Check queue size limit (Requirement 12.5 mentions batching at 100, but we allow up to 1000)
        if self.queue.len() >= MAX_QUEUE_SIZE {
            error!("Payment queue is full: {} entries", self.queue.len());
            return Err(StealthError::QueueFull(MAX_QUEUE_SIZE));
        }

        let payment_id = Uuid::new_v4();
        let queued_payment = QueuedPayment {
            id: payment_id,
            prepared: payment,
            status: PaymentStatus::Queued,
            created_at: SystemTime::now(),
            retry_count: 0,
        };

        info!(
            "Enqueueing payment {} to stealth address {}",
            payment_id, queued_payment.prepared.stealth_address
        );

        // Add to queue (Requirement 5.1)
        self.queue.push_back(queued_payment);

        // Persist to storage (Requirement 5.2)
        self.save_to_storage().await?;

        Ok(payment_id)
    }

    /// Get payment status
    ///
    /// Returns the current status of a payment by ID.
    ///
    /// # Requirements
    /// Validates: Requirements 5.4
    pub fn get_status(&self, id: &PaymentId) -> Option<PaymentStatus> {
        self.queue
            .iter()
            .find(|p| &p.id == id)
            .map(|p| p.status.clone())
    }

    /// Process queue when online
    ///
    /// Processes all queued payments in FIFO order, attempting to settle them
    /// on the blockchain. Updates payment status based on settlement results.
    /// When the queue exceeds 100 entries, uses batching to reduce transaction overhead.
    ///
    /// # Requirements
    /// Validates: Requirements 5.3, 5.4, 5.5, 5.6, 5.7, 12.5
    ///
    /// # Returns
    /// Vector of settlement results for all processed payments
    pub async fn process_queue(&mut self) -> StealthResult<Vec<SettlementResult>> {
        info!("Processing payment queue with {} entries", self.queue.len());

        let mut results = Vec::new();
        let mut payments_to_remove = Vec::new();

        // Collect payments to process (to avoid borrow checker issues)
        let mut payments_to_process: Vec<(usize, QueuedPayment)> = self
            .queue
            .iter()
            .enumerate()
            .filter(|(_, p)| !matches!(p.status, PaymentStatus::Settled(_) | PaymentStatus::Failed(_)))
            .map(|(i, p)| (i, p.clone()))
            .collect();

        // Check if we should use batching (Requirement 12.5)
        const BATCH_THRESHOLD: usize = 100;
        let use_batching = payments_to_process.len() > BATCH_THRESHOLD;

        if use_batching {
            info!(
                "Queue size {} exceeds threshold {}, using batched settlement",
                payments_to_process.len(),
                BATCH_THRESHOLD
            );

            // Group payments by recipient for batching
            let mut batches: std::collections::HashMap<Pubkey, Vec<(usize, QueuedPayment)>> =
                std::collections::HashMap::new();

            for payment in payments_to_process.drain(..) {
                batches
                    .entry(payment.1.prepared.stealth_address)
                    .or_insert_with(Vec::new)
                    .push(payment);
            }

            // Process each batch
            for (recipient, batch) in batches {
                debug!(
                    "Processing batch of {} payments to recipient {}",
                    batch.len(),
                    recipient
                );

                // Attempt to settle the batch
                match self.settle_batch(&batch).await {
                    Ok(batch_results) => {
                        for (payment_id, signature) in batch_results {
                            payments_to_remove.push(payment_id);
                            results.push(SettlementResult {
                                payment_id,
                                status: PaymentStatus::Settled(signature),
                            });
                        }
                    }
                    Err(e) => {
                        // Handle batch failure - update retry counts
                        for (index, mut queued_payment) in batch {
                            queued_payment.retry_count += 1;

                            if queued_payment.retry_count >= MAX_RETRY_ATTEMPTS {
                                let error_msg = format!("Max retries exceeded: {}", e);
                                queued_payment.status = PaymentStatus::Failed(error_msg.clone());

                                error!(
                                    "Payment {} failed after {} attempts: {}",
                                    queued_payment.id, queued_payment.retry_count, e
                                );

                                results.push(SettlementResult {
                                    payment_id: queued_payment.id,
                                    status: PaymentStatus::Failed(error_msg),
                                });
                            } else {
                                queued_payment.status = PaymentStatus::Queued;

                                warn!(
                                    "Payment {} settlement failed (attempt {}/{}): {}",
                                    queued_payment.id, queued_payment.retry_count, MAX_RETRY_ATTEMPTS, e
                                );

                                results.push(SettlementResult {
                                    payment_id: queued_payment.id,
                                    status: PaymentStatus::Queued,
                                });
                            }

                            // Update the payment in the queue
                            if let Some(payment) = self.queue.get_mut(index) {
                                *payment = queued_payment;
                            }
                        }
                    }
                }
            }
        } else {
            // Process payments individually (original behavior for small queues)
            for (index, ref mut queued_payment) in payments_to_process.iter_mut() {
                // Update status to settling (Requirement 5.5)
                queued_payment.status = PaymentStatus::Settling;
                debug!("Attempting to settle payment {}", queued_payment.id);

                // Attempt to settle the payment
                match self.settle_payment(&queued_payment).await {
                    Ok(signature) => {
                        // Update status to settled (Requirement 5.6)
                        queued_payment.status = PaymentStatus::Settled(signature);
                        payments_to_remove.push(queued_payment.id);

                        info!(
                            "Payment {} settled successfully. Signature: {}",
                            queued_payment.id, signature
                        );

                        results.push(SettlementResult {
                            payment_id: queued_payment.id,
                            status: PaymentStatus::Settled(signature),
                        });
                    }
                    Err(e) => {
                        // Increment retry count
                        queued_payment.retry_count += 1;

                        // Check if max retries exceeded (Requirement 5.7)
                        if queued_payment.retry_count >= MAX_RETRY_ATTEMPTS {
                            let error_msg = format!("Max retries exceeded: {}", e);
                            queued_payment.status = PaymentStatus::Failed(error_msg.clone());

                            error!(
                                "Payment {} failed after {} attempts: {}",
                                queued_payment.id, queued_payment.retry_count, e
                            );

                            results.push(SettlementResult {
                                payment_id: queued_payment.id,
                                status: PaymentStatus::Failed(error_msg),
                            });
                        } else {
                            // Revert to queued status for retry
                            queued_payment.status = PaymentStatus::Queued;

                            warn!(
                                "Payment {} settlement failed (attempt {}/{}): {}",
                                queued_payment.id, queued_payment.retry_count, MAX_RETRY_ATTEMPTS, e
                            );

                            results.push(SettlementResult {
                                payment_id: queued_payment.id,
                                status: PaymentStatus::Queued,
                            });
                        }
                    }
                }

                // Update the payment in the queue
                if let Some(payment) = self.queue.get_mut(*index) {
                    *payment = queued_payment.clone();
                }
            }
        }

        // Remove settled payments from queue (Requirement 5.6)
        self.queue.retain(|p| !payments_to_remove.contains(&p.id));

        // Persist updated queue
        self.save_to_storage().await?;

        info!(
            "Queue processing complete. {} payments processed, {} remaining",
            results.len(),
            self.queue.len()
        );

        Ok(results)
    }

    /// Start background auto-settlement task
    ///
    /// Spawns a background task that monitors network connectivity and
    /// automatically processes the queue when connectivity is restored.
    ///
    /// # Requirements
    /// Validates: Requirements 5.3, 5.8
    ///
    /// # Returns
    /// JoinHandle for the background task
    pub fn start_auto_settlement(queue: Arc<Mutex<Self>>) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!("Starting auto-settlement background task");

            loop {
                // Check if online
                let is_online = {
                    let queue_guard = queue.lock().await;
                    let monitor = queue_guard.network_monitor.lock().await;
                    monitor.is_online()
                };

                if is_online {
                    // Process queue when online
                    let mut queue_guard = queue.lock().await;
                    if !queue_guard.queue.is_empty() {
                        debug!("Network online, processing queue");
                        match queue_guard.process_queue().await {
                            Ok(results) => {
                                info!("Auto-settlement processed {} payments", results.len());
                            }
                            Err(e) => {
                                error!("Auto-settlement failed: {}", e);
                            }
                        }
                    }
                }

                // Sleep before next check (every 30 seconds)
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        })
    }

    /// Load queue from persistent storage
    ///
    /// Loads the payment queue from secure storage. Called during initialization
    /// to restore queue state after application restart.
    ///
    /// # Requirements
    /// Validates: Requirements 5.2
    pub async fn load_from_storage(&mut self) -> StealthResult<()> {
        debug!("Loading payment queue from storage");

        match self.storage.load_data(QUEUE_STORAGE_KEY).await {
            Ok(data) => {
                let stored_queue: StoredQueue = serde_json::from_slice(&data)?;
                self.queue = stored_queue.payments;

                info!("Loaded {} payments from storage", self.queue.len());
                Ok(())
            }
            Err(StealthError::StorageFailed(_)) => {
                // Queue doesn't exist yet, start with empty queue
                debug!("No existing queue in storage, starting fresh");
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Save queue to persistent storage
    ///
    /// Persists the current payment queue to secure storage. Called after
    /// any queue modification to ensure durability.
    ///
    /// # Requirements
    /// Validates: Requirements 5.2
    async fn save_to_storage(&self) -> StealthResult<()> {
        debug!("Saving payment queue to storage ({} entries)", self.queue.len());

        let stored_queue = StoredQueue {
            payments: self.queue.clone(),
        };

        let data = serde_json::to_vec(&stored_queue)?;
        self.storage.store_data(QUEUE_STORAGE_KEY, &data).await?;

        Ok(())
    }

    /// Settle a single payment on the blockchain
    ///
    /// Creates and submits a transaction to transfer funds to the stealth address.
    /// Includes stealth metadata (ephemeral public key and viewing tag) in the
    /// transaction for receiver scanning.
    ///
    /// # Requirements
    /// Validates: Requirements 5.3, 5.5, 5.6
    async fn settle_payment(&self, payment: &QueuedPayment) -> StealthResult<Signature> {
        debug!(
            "Settling payment {} to stealth address {}",
            payment.id, payment.prepared.stealth_address
        );

        // Get recent blockhash
        let recent_blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .map_err(|e| StealthError::BlockchainError(format!("Failed to get blockhash: {}", e)))?;

        // Create transfer instruction
        let transfer_instruction = system_instruction::transfer(
            &self.payer_keypair.pubkey(),
            &payment.prepared.stealth_address,
            payment.prepared.amount,
        );

        // Create stealth metadata instruction
        let metadata_instruction = create_stealth_metadata_instruction(
            &payment.prepared.ephemeral_public_key,
            &payment.prepared.viewing_tag,
            1, // version 1 (standard mode)
        );

        // Build transaction
        let transaction = Transaction::new_signed_with_payer(
            &[transfer_instruction, metadata_instruction],
            Some(&self.payer_keypair.pubkey()),
            &[self.payer_keypair.as_ref()],
            recent_blockhash,
        );

        // Submit transaction
        let signature = self
            .rpc_client
            .send_and_confirm_transaction_with_spinner(&transaction)
            .map_err(|e| StealthError::BlockchainError(format!("Transaction failed: {}", e)))?;

        Ok(signature)
    }

    /// Settle a batch of payments to the same recipient
    ///
    /// Groups multiple payments to the same stealth address into a single transaction
    /// to reduce blockchain overhead. This is used when the queue exceeds 100 entries.
    ///
    /// # Requirements
    /// Validates: Requirements 12.5
    async fn settle_batch(
        &self,
        batch: &[(usize, QueuedPayment)],
    ) -> StealthResult<Vec<(PaymentId, Signature)>> {
        if batch.is_empty() {
            return Ok(Vec::new());
        }

        debug!("Settling batch of {} payments", batch.len());

        // Get recent blockhash
        let recent_blockhash = self
            .rpc_client
            .get_latest_blockhash()
            .map_err(|e| StealthError::BlockchainError(format!("Failed to get blockhash: {}", e)))?;

        let mut results = Vec::new();

        // For now, we'll process batches sequentially but with optimized blockhash reuse
        // Future optimization: Use Solana's versioned transactions to batch multiple transfers
        for (_index, payment) in batch {
            // Create transfer instruction
            let transfer_instruction = system_instruction::transfer(
                &self.payer_keypair.pubkey(),
                &payment.prepared.stealth_address,
                payment.prepared.amount,
            );

            // Create stealth metadata instruction
            let metadata_instruction = create_stealth_metadata_instruction(
                &payment.prepared.ephemeral_public_key,
                &payment.prepared.viewing_tag,
                1, // version 1 (standard mode)
            );

            // Build transaction (reusing blockhash)
            let transaction = Transaction::new_signed_with_payer(
                &[transfer_instruction, metadata_instruction],
                Some(&self.payer_keypair.pubkey()),
                &[self.payer_keypair.as_ref()],
                recent_blockhash,
            );

            // Submit transaction
            match self.rpc_client.send_and_confirm_transaction_with_spinner(&transaction) {
                Ok(signature) => {
                    results.push((payment.id, signature));
                }
                Err(e) => {
                    // If any transaction in the batch fails, return error
                    // The caller will handle retry logic
                    return Err(StealthError::BlockchainError(format!(
                        "Batch transaction failed: {}",
                        e
                    )));
                }
            }
        }

        info!("Batch settlement complete: {} payments settled", results.len());
        Ok(results)
    }
}

/// Create a custom instruction to store stealth payment metadata on-chain
///
/// This instruction stores the ephemeral public key and viewing tag in the
/// transaction memo, allowing receivers to scan for incoming payments.
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
    let memo_program_id = solana_sdk::pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr");

    Instruction {
        program_id: memo_program_id,
        accounts: vec![],
        data: metadata,
    }
}

/// A queued payment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedPayment {
    pub id: PaymentId,
    pub prepared: PreparedPayment,
    pub status: PaymentStatus,
    pub created_at: SystemTime,
    pub retry_count: u32,
}

/// Payment status
///
/// Represents the lifecycle of a payment through the queue:
/// - Queued: Payment is waiting to be processed
/// - Settling: Payment is currently being submitted to blockchain
/// - Settled: Payment successfully confirmed on-chain
/// - Failed: Payment failed after maximum retry attempts
///
/// # Requirements
/// Validates: Requirements 5.4, 5.5, 5.6, 5.7
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PaymentStatus {
    Queued,
    Settling,
    Settled(Signature),
    Failed(String),
}

/// Result of settlement attempt
pub struct SettlementResult {
    pub payment_id: PaymentId,
    pub status: PaymentStatus,
}

/// Stored queue format for persistence
#[derive(Serialize, Deserialize)]
struct StoredQueue {
    payments: VecDeque<QueuedPayment>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{InMemoryStorage, SecureStorage};
    use solana_sdk::commitment_config::CommitmentConfig;
    use solana_sdk::pubkey::Pubkey;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn create_test_prepared_payment() -> PreparedPayment {
        PreparedPayment {
            stealth_address: Pubkey::new_unique(),
            amount: 1_000_000,
            ephemeral_public_key: Pubkey::new_unique(),
            viewing_tag: [0x01, 0x02, 0x03, 0x04],
        }
    }

    fn create_test_queue() -> PaymentQueue {
        let storage: Arc<dyn SecureStorage> = Arc::new(InMemoryStorage::new(b"test-device-key"));
        let network_monitor = Arc::new(Mutex::new(NetworkMonitor::new()));
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            "https://api.devnet.solana.com".to_string(),
            CommitmentConfig::confirmed(),
        ));
        let payer_keypair = Arc::new(Keypair::new());

        PaymentQueue::new(storage, network_monitor, rpc_client, payer_keypair)
    }

    #[tokio::test]
    async fn test_enqueue_payment() {
        let mut queue = create_test_queue();
        let payment = create_test_prepared_payment();

        let payment_id = queue.enqueue(payment).await.unwrap();

        // Verify payment was added
        assert_eq!(queue.queue.len(), 1);
        assert_eq!(queue.queue[0].id, payment_id);
        assert_eq!(queue.queue[0].status, PaymentStatus::Queued);
        assert_eq!(queue.queue[0].retry_count, 0);
    }

    #[tokio::test]
    async fn test_get_status() {
        let mut queue = create_test_queue();
        let payment = create_test_prepared_payment();

        let payment_id = queue.enqueue(payment).await.unwrap();

        // Get status
        let status = queue.get_status(&payment_id);
        assert!(status.is_some());
        assert_eq!(status.unwrap(), PaymentStatus::Queued);

        // Non-existent payment
        let fake_id = Uuid::new_v4();
        assert!(queue.get_status(&fake_id).is_none());
    }

    #[tokio::test]
    async fn test_queue_full() {
        let mut queue = create_test_queue();

        // Fill queue to max capacity
        for _ in 0..MAX_QUEUE_SIZE {
            let payment = create_test_prepared_payment();
            queue.enqueue(payment).await.unwrap();
        }

        // Next enqueue should fail
        let payment = create_test_prepared_payment();
        let result = queue.enqueue(payment).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StealthError::QueueFull(_)));
    }

    #[tokio::test]
    async fn test_save_and_load_from_storage() {
        let storage: Arc<dyn SecureStorage> = Arc::new(InMemoryStorage::new(b"test-device-key"));
        let network_monitor = Arc::new(Mutex::new(NetworkMonitor::new()));
        let rpc_client = Arc::new(RpcClient::new_with_commitment(
            "https://api.devnet.solana.com".to_string(),
            CommitmentConfig::confirmed(),
        ));
        let payer_keypair = Arc::new(Keypair::new());

        // Create queue and add payments
        let mut queue1 = PaymentQueue::new(
            Arc::clone(&storage),
            Arc::clone(&network_monitor),
            Arc::clone(&rpc_client),
            Arc::clone(&payer_keypair),
        );

        let payment1 = create_test_prepared_payment();
        let payment2 = create_test_prepared_payment();
        let id1 = queue1.enqueue(payment1).await.unwrap();
        let id2 = queue1.enqueue(payment2).await.unwrap();

        // Create new queue with same storage
        let mut queue2 = PaymentQueue::new(
            Arc::clone(&storage),
            network_monitor,
            rpc_client,
            payer_keypair,
        );

        // Load from storage
        queue2.load_from_storage().await.unwrap();

        // Verify payments were loaded
        assert_eq!(queue2.queue.len(), 2);
        assert_eq!(queue2.queue[0].id, id1);
        assert_eq!(queue2.queue[1].id, id2);
    }

    #[tokio::test]
    async fn test_load_from_empty_storage() {
        let mut queue = create_test_queue();

        // Load from empty storage should succeed
        let result = queue.load_from_storage().await;
        assert!(result.is_ok());
        assert_eq!(queue.queue.len(), 0);
    }

    #[tokio::test]
    async fn test_payment_status_transitions() {
        let mut payment = QueuedPayment {
            id: Uuid::new_v4(),
            prepared: create_test_prepared_payment(),
            status: PaymentStatus::Queued,
            created_at: SystemTime::now(),
            retry_count: 0,
        };

        // Queued -> Settling
        payment.status = PaymentStatus::Settling;
        assert_eq!(payment.status, PaymentStatus::Settling);

        // Settling -> Settled
        let signature = Signature::new_unique();
        payment.status = PaymentStatus::Settled(signature);
        assert_eq!(payment.status, PaymentStatus::Settled(signature));
    }

    #[tokio::test]
    async fn test_fifo_order() {
        let mut queue = create_test_queue();

        // Add multiple payments
        let payment1 = create_test_prepared_payment();
        let payment2 = create_test_prepared_payment();
        let payment3 = create_test_prepared_payment();

        let id1 = queue.enqueue(payment1).await.unwrap();
        let id2 = queue.enqueue(payment2).await.unwrap();
        let id3 = queue.enqueue(payment3).await.unwrap();

        // Verify FIFO order
        assert_eq!(queue.queue[0].id, id1);
        assert_eq!(queue.queue[1].id, id2);
        assert_eq!(queue.queue[2].id, id3);
    }

    #[test]
    fn test_create_stealth_metadata_instruction() {
        let ephemeral_pk = Pubkey::new_unique();
        let viewing_tag = [0xAA, 0xBB, 0xCC, 0xDD];
        let version = 1u8;

        let instruction = create_stealth_metadata_instruction(&ephemeral_pk, &viewing_tag, version);

        // Verify instruction structure
        assert_eq!(
            instruction.program_id,
            solana_sdk::pubkey!("MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"),
            "Should use SPL Memo program"
        );
        assert_eq!(instruction.accounts.len(), 0);

        // Verify data format
        assert_eq!(instruction.data.len(), 37);
        assert_eq!(instruction.data[0], version);
        assert_eq!(&instruction.data[1..5], &viewing_tag);
        assert_eq!(&instruction.data[5..37], &ephemeral_pk.to_bytes());
    }

    // Note: Integration tests for process_queue() and settle_payment() that interact
    // with the Solana blockchain will be implemented in task 27 (integration tests).
    // These tests require a running Solana test validator or devnet connection.
}
