// Transfer Service - manages transfer requests and execution

use crate::{ProximityError, Result, TransferRequest, TransferStatus, ErrorContext};
use crate::receipt_helper::ProximityReceiptData;
use blockchain::SolanaClient;
use chrono::{Duration, Utc};
use database::DbPool;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use solana_client::rpc_config::RpcSendTransactionConfig;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    native_token::LAMPORTS_PER_SOL,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    system_instruction,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use spl_token::instruction as token_instruction;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{RwLock, Notify};
use tokio::time::{interval, Duration as TokioDuration};
use tracing::{debug, info, warn, error};
use uuid::Uuid;

const TRANSFER_TIMEOUT_SECS: i64 = 60;
const CLEANUP_INTERVAL_SECS: u64 = 10;
/// How often the background task attempts to settle offline-queued transfers.
const SETTLEMENT_SYNC_INTERVAL_SECS: u64 = 15;

/// Transfer Service manages proximity transfer requests
/// 
/// **Validates: Requirements 5.3, 5.5, 6.4, 6.5, 6.6, 7.1, 7.2, 7.3, 7.4**
pub struct TransferService {
    active_requests: Arc<RwLock<HashMap<Uuid, TransferRequest>>>,
    queued_requests: Arc<RwLock<Vec<TransferRequest>>>,
    db_pool: DbPool,
    solana_client: Arc<SolanaClient>,
    /// Optional backend-custodied signer used to actually submit on-chain
    /// transfers for the demo. When present (devnet only), `execute_sol_transfer`
    /// signs and submits a real SOL transfer from this keypair. When absent, the
    /// transfer execution remains a no-op placeholder (the platform does not hold
    /// users' private keys in production).
    custodial_signer: Option<Arc<Keypair>>,
    shutdown_notify: Arc<Notify>,
    max_concurrent_transfers: usize,
}

impl TransferService {
    /// Create a new transfer service
    pub fn new(db_pool: DbPool, solana_client: Arc<SolanaClient>) -> Self {
        let service = Self {
            active_requests: Arc::new(RwLock::new(HashMap::new())),
            queued_requests: Arc::new(RwLock::new(Vec::new())),
            db_pool,
            solana_client,
            custodial_signer: None,
            shutdown_notify: Arc::new(Notify::new()),
            max_concurrent_transfers: 5, // Default limit per user
        };
        
        // Start background cleanup task for expired requests
        service.start_cleanup_task();
        
        service
    }

    /// Attach a backend-custodied signer so transfers are actually submitted
    /// on-chain. Intended for a devnet demo wallet only.
    pub fn with_custodial_signer(mut self, signer: Arc<Keypair>) -> Self {
        info!(
            "Custodial signer attached for on-chain transfers: {}",
            signer.pubkey()
        );
        self.custodial_signer = Some(signer);
        self
    }

    /// Create a transfer request with balance validation
    /// 
    /// **Validates: Requirements 5.3, 5.5, 14.2, 14.3**
    pub async fn create_transfer_request(
        &self,
        sender_user_id: Uuid,
        sender_wallet: String,
        recipient_user_id: Uuid,
        recipient_wallet: String,
        asset: String,
        amount: Decimal,
    ) -> Result<TransferRequest> {
        info!(
            "Creating transfer request: sender={}, recipient={}, asset={}, amount={}",
            sender_wallet, recipient_wallet, asset, amount
        );

        // Validate amount is positive
        if amount <= Decimal::ZERO {
            return Err(ProximityError::InternalError(
                "Transfer amount must be positive".to_string(),
            ));
        }

        // Validate sender has sufficient balance
        self.validate_sender_balance(&sender_wallet, &asset, amount)
            .await
            .map_err(|e| {
                let context = ErrorContext::new()
                    .with_user_id(sender_user_id)
                    .with_info(format!("asset={}, amount={}", asset, amount));
                e.log_with_context(&context);
                e
            })?;

        // Create transfer request
        let request = TransferRequest {
            id: Uuid::new_v4(),
            sender_user_id,
            sender_wallet: sender_wallet.clone(),
            recipient_user_id,
            recipient_wallet: recipient_wallet.clone(),
            asset: asset.clone(),
            amount,
            status: TransferStatus::Pending,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::seconds(TRANSFER_TIMEOUT_SECS),
        };

        // Check concurrent transfer limit for this user
        let active_requests = self.active_requests.read().await;
        let user_active_count = active_requests
            .values()
            .filter(|r| r.sender_user_id == sender_user_id && 
                       (r.status == TransferStatus::Pending || 
                        r.status == TransferStatus::Accepted ||
                        r.status == TransferStatus::Executing))
            .count();
        drop(active_requests);

        if user_active_count >= self.max_concurrent_transfers {
            // Queue the request
            info!(
                "User {} has {} active transfers (limit: {}), queueing request {}",
                sender_user_id, user_active_count, self.max_concurrent_transfers, request.id
            );
            let mut queued = self.queued_requests.write().await;
            queued.push(request.clone());
            drop(queued);
            
            info!("Transfer request queued: id={}", request.id);
            return Ok(request);
        }

        // Store in active requests
        let mut requests = self.active_requests.write().await;
        requests.insert(request.id, request.clone());
        drop(requests);

        // Also persist the pending request to the DB so a DIFFERENT instance
        // (e.g. the recipient's node in a two-user setup) can look it up and
        // accept it. The proximity transfer state must not live only in the
        // creating instance's memory.
        if let Err(e) = self
            .persist_transfer_to_db(&request, "", TransferStatus::Pending)
            .await
        {
            warn!("Failed to persist pending transfer {}: {}", request.id, e);
        }

        info!("Transfer request created: id={}", request.id);
        Ok(request)
    }

    /// Load a transfer request from the database. Used as a fallback when the
    /// request was created on a different instance (so it's not in this node's
    /// in-memory `active_requests`).
    async fn load_request_from_db(&self, request_id: Uuid) -> Result<Option<TransferRequest>> {
        let record = database::proximity::get_transfer_by_id(&self.db_pool, request_id)
            .await
            .map_err(|e| ProximityError::InternalError(format!("Failed to load transfer: {}", e)))?;

        Ok(record.map(|r| {
            let status = match r.status.as_str() {
                "Pending" => TransferStatus::Pending,
                "Accepted" => TransferStatus::Accepted,
                "Executing" => TransferStatus::Executing,
                "PendingSettlement" => TransferStatus::PendingSettlement,
                "Completed" => TransferStatus::Completed,
                "Rejected" => TransferStatus::Rejected,
                "Failed" => TransferStatus::Failed,
                "Expired" => TransferStatus::Expired,
                _ => TransferStatus::Pending,
            };
            TransferRequest {
                id: r.id,
                sender_user_id: r.sender_user_id,
                sender_wallet: r.sender_wallet,
                recipient_user_id: r.recipient_user_id,
                recipient_wallet: r.recipient_wallet,
                asset: r.asset,
                amount: r.amount,
                status,
                created_at: r.created_at,
                expires_at: r.created_at + Duration::seconds(TRANSFER_TIMEOUT_SECS),
            }
        }))
    }

    /// Ensure a transfer request is present in this node's in-memory map,
    /// loading it from the DB if it was created on another instance. Returns the
    /// request, or TransferNotFound if it exists nowhere.
    async fn ensure_request_loaded(&self, request_id: Uuid) -> Result<TransferRequest> {
        {
            let requests = self.active_requests.read().await;
            if let Some(r) = requests.get(&request_id) {
                return Ok(r.clone());
            }
        }
        match self.load_request_from_db(request_id).await? {
            Some(req) => {
                let mut requests = self.active_requests.write().await;
                let entry = requests.entry(request_id).or_insert_with(|| req.clone());
                Ok(entry.clone())
            }
            None => Err(ProximityError::TransferNotFound(request_id.to_string())),
        }
    }

    /// Create a transfer request with token account validation
    /// 
    /// This method checks if the recipient needs a token account for SPL tokens
    /// and returns information about token account creation requirements.
    /// 
    /// Returns (TransferRequest, needs_token_account, creation_fee)
    /// 
    /// **Validates: Requirements 5.3, 5.5, 12.4, 12.5**
    pub async fn create_transfer_request_with_validation(
        &self,
        sender_user_id: Uuid,
        sender_wallet: String,
        recipient_user_id: Uuid,
        recipient_wallet: String,
        asset: String,
        amount: Decimal,
    ) -> Result<(TransferRequest, bool, Option<Decimal>)> {
        info!(
            "Creating transfer request with validation: sender={}, recipient={}, asset={}, amount={}",
            sender_wallet, recipient_wallet, asset, amount
        );

        // Check token account requirements before creating the request
        let (_is_valid, needs_token_account, creation_fee) = self
            .validate_transfer_requirements(&sender_wallet, &recipient_wallet, &asset, amount)
            .await?;

        // Create the transfer request
        let request = self
            .create_transfer_request(
                sender_user_id,
                sender_wallet,
                recipient_user_id,
                recipient_wallet,
                asset,
                amount,
            )
            .await?;

        Ok((request, needs_token_account, creation_fee))
    }

    /// Accept a transfer request
    /// 
    /// **Validates: Requirements 6.4**
    pub async fn accept_transfer(&self, request_id: Uuid) -> Result<()> {
        info!("Accepting transfer request: {}", request_id);

        // Ensure the request is in our in-memory map (loading from the DB if it
        // was created on another instance — e.g. the sender's node).
        self.ensure_request_loaded(request_id).await?;

        let mut requests = self.active_requests.write().await;
        
        let request = requests
            .get_mut(&request_id)
            .ok_or_else(|| {
                let err = ProximityError::TransferNotFound(request_id.to_string());
                let context = ErrorContext::new()
                    .with_transfer_id(request_id)
                    .with_info("Transfer not found during acceptance".to_string());
                err.log_with_context(&context);
                err
            })?;

        // Check if request is still pending
        if request.status != TransferStatus::Pending {
            return Err(ProximityError::InternalError(format!(
                "Transfer request {} is not pending (status: {})",
                request_id, request.status
            )));
        }

        // Check if request has expired
        if Utc::now() > request.expires_at {
            request.status = TransferStatus::Expired;
            return Err(ProximityError::Timeout(format!(
                "Transfer request {} has expired",
                request_id
            )));
        }

        // Update status to accepted
        request.status = TransferStatus::Accepted;
        
        info!("Transfer request accepted: {}", request_id);
        Ok(())
    }

    /// Reject a transfer request
    /// 
    /// **Validates: Requirements 6.5**
    pub async fn reject_transfer(&self, request_id: Uuid, reason: Option<String>) -> Result<()> {
        info!("Rejecting transfer request: {} (reason: {:?})", request_id, reason);

        let mut requests = self.active_requests.write().await;
        
        let request = requests
            .get_mut(&request_id)
            .ok_or_else(|| {
                let err = ProximityError::TransferNotFound(request_id.to_string());
                let context = ErrorContext::new()
                    .with_transfer_id(request_id)
                    .with_info("Transfer not found during rejection".to_string());
                err.log_with_context(&context);
                err
            })?;

        // Check if request is still pending
        if request.status != TransferStatus::Pending {
            return Err(ProximityError::InternalError(format!(
                "Transfer request {} is not pending (status: {})",
                request_id, request.status
            )));
        }

        // Update status to rejected
        request.status = TransferStatus::Rejected;
        let user_id = request.sender_user_id;
        drop(requests);
        
        // Process queued requests for this user
        let _ = self.process_queued_requests(user_id).await;
        
        info!("Transfer request rejected: {}", request_id);
        Ok(())
    }

    /// Execute a transfer by submitting blockchain transaction
    /// 
    /// **Validates: Requirements 7.1, 7.2, 7.3, 7.4**
    pub async fn execute_transfer(&self, request_id: Uuid) -> Result<String> {
        info!("Executing transfer request: {}", request_id);

        // Get the transfer request
        let mut requests = self.active_requests.write().await;
        let request = requests
            .get_mut(&request_id)
            .ok_or_else(|| ProximityError::TransferNotFound(request_id.to_string()))?;

        // Verify request is in Accepted status
        if request.status != TransferStatus::Accepted {
            return Err(ProximityError::InternalError(format!(
                "Transfer request {} is not accepted (status: {})",
                request_id, request.status
            )));
        }

        // Update status to Executing
        request.status = TransferStatus::Executing;
        let request_clone = request.clone();
        drop(requests);

        // Execute the blockchain transaction
        let tx_hash = self
            .execute_blockchain_transaction(&request_clone)
            .await
            .map_err(|e| {
                let context = ErrorContext::new()
                    .with_user_id(request_clone.sender_user_id)
                    .with_transfer_id(request_id)
                    .with_info(format!(
                        "Blockchain transaction failed: asset={}, amount={}",
                        request_clone.asset, request_clone.amount
                    ));
                e.log_with_context(&context);
                error!("Blockchain transaction failed for request {}: {}", request_id, e);
                e
            })?;

        // Persist to database BEFORE marking the transfer as Completed so that a
        // persistence failure never leaves the transfer in a success state
        // (persist-then-success). The error propagates via `?` before any success
        // status is produced, so the caller never observes a successful completion
        // for a transfer that was not recorded.
        self.persist_transfer_to_db(&request_clone, &tx_hash, TransferStatus::Completed)
            .await?;

        // Update status to Completed only after both blockchain execution and
        // persistence have succeeded.
        let mut requests = self.active_requests.write().await;
        if let Some(request) = requests.get_mut(&request_id) {
            request.status = TransferStatus::Completed;
        }
        drop(requests);

        // Process queued requests for this user
        let _ = self.process_queued_requests(request_clone.sender_user_id).await;

        info!("Transfer executed successfully: {} (tx: {})", request_id, tx_hash);
        Ok(tx_hash)
    }

    /// Check whether the blockchain (Solana RPC) is currently reachable.
    pub async fn is_blockchain_online(&self) -> bool {
        self.solana_client.health_check().await.is_ok()
    }

    /// Accept-and-settle a transfer with offline tolerance.
    ///
    /// If the blockchain is reachable, this behaves like `execute_transfer`:
    /// the transfer is submitted on-chain and marked `Completed`, returning the
    /// real transaction hash.
    ///
    /// If the blockchain is NOT reachable (both peers effectively offline w.r.t.
    /// the chain), the transfer is durably recorded as `PendingSettlement` in the
    /// database with no transaction hash and `Ok(None)` is returned. A background
    /// sync task (`sync_pending_settlements`) submits it on-chain automatically
    /// once connectivity returns, transitioning it to `Completed`. This is the
    /// offline create-now / settle-later path.
    pub async fn execute_or_queue_transfer(&self, request_id: Uuid) -> Result<Option<String>> {
        // Snapshot the accepted request.
        let request_clone = {
            let mut requests = self.active_requests.write().await;
            let request = requests
                .get_mut(&request_id)
                .ok_or_else(|| ProximityError::TransferNotFound(request_id.to_string()))?;
            if request.status != TransferStatus::Accepted {
                return Err(ProximityError::InternalError(format!(
                    "Transfer request {} is not accepted (status: {})",
                    request_id, request.status
                )));
            }
            request.clone()
        };

        if self.is_blockchain_online().await {
            // Online: settle immediately on-chain.
            let tx_hash = self.execute_transfer(request_id).await?;
            return Ok(Some(tx_hash));
        }

        // Offline: durably record as PendingSettlement (no tx hash yet).
        warn!(
            "Blockchain unreachable; queuing transfer {} for later settlement",
            request_id
        );
        self.persist_transfer_to_db(&request_clone, "", TransferStatus::PendingSettlement)
            .await?;

        let mut requests = self.active_requests.write().await;
        if let Some(request) = requests.get_mut(&request_id) {
            request.status = TransferStatus::PendingSettlement;
        }
        drop(requests);

        info!(
            "Transfer {} recorded as PendingSettlement (will sync when online)",
            request_id
        );
        Ok(None)
    }

    /// Settle all transfers that were recorded offline.
    ///
    /// Scans the database for `PendingSettlement` transfers, submits each on-chain
    /// (when the blockchain is reachable), and updates the row to `Completed` with
    /// the real transaction hash (or `Failed` if the on-chain submission errors).
    /// Returns the number of transfers successfully settled. Safe to call
    /// repeatedly (idempotent: only acts on rows still in `PendingSettlement`).
    pub async fn sync_pending_settlements(&self) -> Result<usize> {
        // Nothing to do if we still cannot reach the chain.
        if !self.is_blockchain_online().await {
            return Ok(0);
        }

        let client = self.db_pool.get().await.map_err(|e| {
            ProximityError::InternalError(format!("Database connection error: {}", e))
        })?;

        let rows = client
            .query(
                "SELECT id, sender_user_id, sender_wallet, recipient_user_id,
                        recipient_wallet, asset, amount, created_at
                 FROM proximity_transfers
                 WHERE status = 'PendingSettlement'
                 ORDER BY created_at ASC",
                &[],
            )
            .await
            .map_err(|e| ProximityError::InternalError(format!("Failed to query pending settlements: {}", e)))?;

        if rows.is_empty() {
            return Ok(0);
        }

        info!("Syncing {} pending offline transfer(s) to the blockchain", rows.len());
        let mut settled = 0usize;

        for row in rows {
            let request = TransferRequest {
                id: row.get(0),
                sender_user_id: row.get(1),
                sender_wallet: row.get(2),
                recipient_user_id: row.get(3),
                recipient_wallet: row.get(4),
                asset: row.get(5),
                amount: row.get(6),
                status: TransferStatus::PendingSettlement,
                created_at: row.get::<_, chrono::NaiveDateTime>(7).and_utc(),
                expires_at: Utc::now(),
            };

            match self.execute_blockchain_transaction(&request).await {
                Ok(tx_hash) => {
                    // Mark Completed with the real on-chain signature.
                    if let Err(e) = client
                        .execute(
                            "UPDATE proximity_transfers
                             SET status = 'Completed', transaction_hash = $2, completed_at = NOW()
                             WHERE id = $1 AND status = 'PendingSettlement'",
                            &[&request.id, &tx_hash],
                        )
                        .await
                    {
                        error!("Settled on-chain but failed to update row {}: {}", request.id, e);
                        continue;
                    }
                    info!("Synced offline transfer {} -> on-chain tx {}", request.id, tx_hash);
                    settled += 1;
                }
                Err(e) => {
                    warn!("Failed to settle pending transfer {} (will retry): {}", request.id, e);
                    // Leave as PendingSettlement so a future sync retries it.
                }
            }
        }

        info!("Pending-settlement sync complete: {} settled", settled);
        Ok(settled)
    }

    /// Start a background task that periodically settles offline transfers once
    /// blockchain connectivity is available.
    pub fn start_settlement_sync_task(self: &Arc<Self>) {
        let service = Arc::clone(self);
        let shutdown = Arc::clone(&self.shutdown_notify);
        tokio::spawn(async move {
            let mut ticker = interval(TokioDuration::from_secs(SETTLEMENT_SYNC_INTERVAL_SECS));
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        match service.sync_pending_settlements().await {
                            Ok(n) if n > 0 => info!("Settlement sync task settled {} offline transfer(s)", n),
                            Ok(_) => {}
                            Err(e) => debug!("Settlement sync skipped/failed: {}", e),
                        }
                    }
                    _ = shutdown.notified() => {
                        debug!("Settlement sync task received shutdown");
                        break;
                    }
                }
            }
        });
    }

    /// Execute blockchain transaction for SOL or SPL token transfer
    /// 
    /// **Validates: Requirements 7.1, 7.2, 7.3, 7.4**
    async fn execute_blockchain_transaction(&self, request: &TransferRequest) -> Result<String> {
        debug!(
            "Executing blockchain transaction: asset={}, amount={}",
            request.asset, request.amount
        );

        // Parse wallet addresses
        let sender_pubkey = Pubkey::from_str(&request.sender_wallet).map_err(|e| {
            let err = ProximityError::InvalidWalletAddress(format!("Invalid sender address: {}", e));
            let context = ErrorContext::new()
                .with_user_id(request.sender_user_id)
                .with_transfer_id(request.id)
                .with_info(format!("sender_wallet={}", request.sender_wallet));
            err.log_with_context(&context);
            err
        })?;

        let recipient_pubkey = Pubkey::from_str(&request.recipient_wallet).map_err(|e| {
            let err = ProximityError::InvalidWalletAddress(format!("Invalid recipient address: {}", e));
            let context = ErrorContext::new()
                .with_user_id(request.sender_user_id)
                .with_transfer_id(request.id)
                .with_info(format!("recipient_wallet={}", request.recipient_wallet));
            err.log_with_context(&context);
            err
        })?;

        // Determine if this is SOL or SPL token transfer
        let is_sol_transfer = request.asset == "SOL" || request.asset == "So11111111111111111111111111111111111111112";

        if is_sol_transfer {
            self.execute_sol_transfer(&sender_pubkey, &recipient_pubkey, request.amount)
                .await
        } else {
            self.execute_spl_token_transfer(
                &sender_pubkey,
                &recipient_pubkey,
                &request.asset,
                request.amount,
            )
            .await
        }
    }

    /// Execute SOL transfer
    /// 
    /// **Validates: Requirements 7.2**
    async fn execute_sol_transfer(
        &self,
        sender: &Pubkey,
        recipient: &Pubkey,
        amount: Decimal,
    ) -> Result<String> {
        debug!("Executing SOL transfer: {} SOL", amount);

        // Convert amount to lamports
        let lamports = (amount * Decimal::from(LAMPORTS_PER_SOL))
            .to_u64()
            .ok_or_else(|| {
                ProximityError::InternalError("Amount conversion overflow".to_string())
            })?;

        // If a custodial signer is configured (devnet demo wallet), submit a
        // REAL on-chain transfer signed by that keypair. The funds move from the
        // custodial wallet to the recipient on-chain and we return the real
        // transaction signature (verifiable on a devnet explorer).
        //
        // The `sender` pubkey from the request is informational here; the actual
        // on-chain source is the custodial signer (the demo wallet that holds
        // devnet SOL). In production a non-custodial design would sign on the
        // client instead.
        if let Some(signer) = &self.custodial_signer {
            let from = signer.pubkey();
            info!(
                "Submitting on-chain SOL transfer: from(custodial)={}, to={}, lamports={}",
                from, recipient, lamports
            );

            let recipient = *recipient;
            let signer = Arc::clone(signer);
            let client = Arc::clone(&self.solana_client);

            // The solana RpcClient is blocking; run it on a blocking thread.
            let signature = tokio::task::spawn_blocking(move || {
                let rpc = client.primary_client();
                let recent_blockhash = rpc
                    .get_latest_blockhash()
                    .map_err(|e| ProximityError::NetworkError(format!("blockhash: {}", e)))?;
                let instruction = system_instruction::transfer(&from, &recipient, lamports);
                let transaction = Transaction::new_signed_with_payer(
                    &[instruction],
                    Some(&from),
                    &[signer.as_ref()],
                    recent_blockhash,
                );
                rpc.send_and_confirm_transaction_with_spinner_and_config(
                    &transaction,
                    CommitmentConfig::confirmed(),
                    RpcSendTransactionConfig {
                        skip_preflight: false,
                        preflight_commitment: Some(CommitmentConfig::confirmed().commitment),
                        ..Default::default()
                    },
                )
                .map_err(|e| ProximityError::TransactionFailed(format!("SOL transfer failed: {}", e)))
            })
            .await
            .map_err(|e| ProximityError::InternalError(format!("join error: {}", e)))??;

            info!("On-chain SOL transfer confirmed: {}", signature);
            return Ok(signature.to_string());
        }

        // No custodial signer configured: the platform does not hold the
        // sender's key, so we cannot sign a real transfer here. Return an error
        // rather than pretending success.
        let _ = (sender, recipient);
        Err(ProximityError::TransactionFailed(
            "No signer configured for on-chain transfer (set DEMO_SOLANA_KEYPAIR for the devnet demo wallet)".to_string(),
        ))
    }

    /// Execute SPL token transfer
    /// 
    /// This method automatically checks if the recipient has a token account
    /// and creates one if needed before executing the transfer.
    /// 
    /// **Validates: Requirements 7.2, 7.3, 12.4, 12.5**
    async fn execute_spl_token_transfer(
        &self,
        sender: &Pubkey,
        recipient: &Pubkey,
        token_mint: &str,
        amount: Decimal,
    ) -> Result<String> {
        debug!("Executing SPL token transfer: {} of {}", amount, token_mint);

        // Parse token mint address
        let mint_pubkey = Pubkey::from_str(token_mint).map_err(|e| {
            ProximityError::InvalidWalletAddress(format!("Invalid token mint: {}", e))
        })?;

        // Get token decimals (typically 9 for most SPL tokens, but should be queried)
        let decimals = 9u8; // Placeholder - should query from token mint

        // Convert amount to token units
        let token_amount = (amount * Decimal::from(10u64.pow(decimals as u32)))
            .to_u64()
            .ok_or_else(|| {
                ProximityError::InternalError("Token amount conversion overflow".to_string())
            })?;

        // Get associated token accounts
        let sender_token_account = get_associated_token_address(sender, &mint_pubkey);
        let recipient_token_account = get_associated_token_address(recipient, &mint_pubkey);

        // Check if recipient token account exists
        let recipient_account_exists = match self
            .solana_client
            .primary_client()
            .get_account(&recipient_token_account)
        {
            Ok(_) => true,
            Err(e) => {
                let error_str = e.to_string();
                if error_str.contains("AccountNotFound") || error_str.contains("could not find account") {
                    false
                } else {
                    return Err(ProximityError::NetworkError(format!(
                        "Failed to check recipient token account: {}",
                        e
                    )));
                }
            }
        };

        // Get recent blockhash
        let recent_blockhash = self
            .solana_client
            .primary_client()
            .get_latest_blockhash()
            .map_err(|e| {
                ProximityError::NetworkError(format!("Failed to get recent blockhash: {}", e))
            })?;

        // Build instructions
        let mut instructions = Vec::new();

        // If recipient token account doesn't exist, create it first
        if !recipient_account_exists {
            info!(
                "Recipient token account does not exist, creating: {}",
                recipient_token_account
            );
            
            let create_account_instruction = spl_associated_token_account::instruction::create_associated_token_account(
                sender, // Payer (sender pays for account creation)
                recipient,
                &mint_pubkey,
                &spl_token::id(),
            );
            instructions.push(create_account_instruction);
        }

        // Create transfer instruction
        let transfer_instruction = token_instruction::transfer(
            &spl_token::id(),
            &sender_token_account,
            &recipient_token_account,
            sender,
            &[],
            token_amount,
        )
        .map_err(|e| {
            ProximityError::InternalError(format!("Failed to create transfer instruction: {}", e))
        })?;
        instructions.push(transfer_instruction);

        // Create a temporary keypair for demonstration
        let payer = Keypair::new();

        // Create transaction with all instructions
        let transaction = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &[&payer],
            recent_blockhash,
        );

        // Send transaction
        let signature = self
            .solana_client
            .primary_client()
            .send_and_confirm_transaction_with_spinner_and_config(
                &transaction,
                CommitmentConfig::confirmed(),
                RpcSendTransactionConfig {
                    skip_preflight: false,
                    preflight_commitment: Some(CommitmentConfig::confirmed().commitment),
                    ..Default::default()
                },
            )
            .map_err(|e| {
                ProximityError::TransactionFailed(format!("SPL token transfer failed: {}", e))
            })?;

        Ok(signature.to_string())
    }

    /// Monitor transaction confirmation and update status
    /// 
    /// Returns the final status and optional receipt data if completed successfully
    /// 
    /// **Validates: Requirements 7.5, 7.6, 10.3, 10.4**
    pub async fn monitor_transaction(
        &self,
        request_id: Uuid,
        tx_hash: String,
    ) -> Result<(TransferStatus, Option<ProximityReceiptData>)> {
        info!("Monitoring transaction: {} for request {}", tx_hash, request_id);

        // Parse transaction signature
        let signature = solana_sdk::signature::Signature::from_str(&tx_hash).map_err(|e| {
            ProximityError::InternalError(format!("Invalid transaction signature: {}", e))
        })?;

        // Poll for confirmation with timeout
        let max_attempts = 30; // 30 attempts with 2 second intervals = 60 seconds max
        let poll_interval = TokioDuration::from_secs(2);

        for attempt in 1..=max_attempts {
            debug!("Polling transaction status (attempt {}/{})", attempt, max_attempts);

            match self.check_transaction_status(&signature).await {
                Ok(confirmed) => {
                    if confirmed {
                        info!("Transaction confirmed: {}", tx_hash);
                        
                        // Get transfer request for receipt generation
                        let requests = self.active_requests.read().await;
                        let request = requests
                            .get(&request_id)
                            .ok_or_else(|| ProximityError::TransferNotFound(request_id.to_string()))?
                            .clone();
                        drop(requests);
                        
                        // Update request status to Completed
                        let mut requests = self.active_requests.write().await;
                        if let Some(req) = requests.get_mut(&request_id) {
                            req.status = TransferStatus::Completed;
                        }
                        drop(requests);
                        
                        // Notify both parties
                        self.notify_transfer_completion(request_id, &tx_hash, true)
                            .await?;
                        
                        // Generate receipt data for API layer to create blockchain receipt
                        let receipt_data = ProximityReceiptData::from_transfer(&request, tx_hash);
                        
                        return Ok((TransferStatus::Completed, Some(receipt_data)));
                    }
                }
                Err(e) => {
                    warn!("Error checking transaction status: {}", e);
                }
            }

            // Wait before next poll
            tokio::time::sleep(poll_interval).await;
        }

        // Transaction not confirmed within timeout
        error!("Transaction confirmation timeout: {}", tx_hash);
        
        // Update request status to Failed
        let mut requests = self.active_requests.write().await;
        let user_id = requests.get(&request_id).map(|r| r.sender_user_id);
        if let Some(request) = requests.get_mut(&request_id) {
            request.status = TransferStatus::Failed;
        }
        drop(requests);
        
        // Process queued requests for this user
        if let Some(uid) = user_id {
            let _ = self.process_queued_requests(uid).await;
        }
        
        // Notify both parties of failure
        self.notify_transfer_completion(request_id, &tx_hash, false)
            .await?;
        
        Ok((TransferStatus::Failed, None))
    }

    /// Check if a transaction is confirmed on the blockchain
    /// 
    /// **Validates: Requirements 7.5**
    async fn check_transaction_status(
        &self,
        signature: &solana_sdk::signature::Signature,
    ) -> Result<bool> {
        // Get transaction status
        let result = self
            .solana_client
            .primary_client()
            .get_signature_status_with_commitment(signature, CommitmentConfig::confirmed())
            .map_err(|e| {
                ProximityError::NetworkError(format!("Failed to get transaction status: {}", e))
            })?;

        // Check if transaction is confirmed
        match result {
            Some(Ok(())) => Ok(true),
            Some(Err(e)) => {
                error!("Transaction failed on blockchain: {}", e);
                Err(ProximityError::TransactionFailed(format!(
                    "Transaction failed: {}",
                    e
                )))
            }
            None => Ok(false), // Not yet confirmed
        }
    }

    /// Notify both parties about transfer completion or failure
    /// 
    /// **Validates: Requirements 7.6**
    async fn notify_transfer_completion(
        &self,
        request_id: Uuid,
        tx_hash: &str,
        success: bool,
    ) -> Result<()> {
        debug!(
            "Notifying parties about transfer {}: success={}",
            request_id, success
        );

        // Get transfer request details
        let requests = self.active_requests.read().await;
        let request = requests
            .get(&request_id)
            .ok_or_else(|| ProximityError::TransferNotFound(request_id.to_string()))?;

        let sender_id = request.sender_user_id;
        let recipient_id = request.recipient_user_id;
        drop(requests);

        // Create notification records in database
        let client = self.db_pool.get().await.map_err(|e| {
            ProximityError::InternalError(format!("Database connection error: {}", e))
        })?;

        let notification_type = if success {
            "proximity_transfer_completed"
        } else {
            "proximity_transfer_failed"
        };

        let message = if success {
            format!("Transfer completed successfully. Transaction: {}", tx_hash)
        } else {
            format!("Transfer failed. Transaction: {}", tx_hash)
        };

        // Title is NOT NULL in the notifications schema, so provide one.
        let title = if success {
            "Proximity Transfer Completed"
        } else {
            "Proximity Transfer Failed"
        };

        // Notify sender. Column is `notification_type` (not `type`), and
        // `title`/`priority` are NOT NULL in the schema, so they must be set.
        client
            .execute(
                "INSERT INTO notifications (user_id, notification_type, title, message, priority, created_at, read)
                 VALUES ($1, $2, $3, $4, $5, NOW(), false)",
                &[&sender_id, &notification_type, &title, &message, &"MEDIUM"],
            )
            .await
            .map_err(|e| {
                ProximityError::InternalError(format!("Failed to create sender notification: {}", e))
            })?;

        // Notify recipient
        client
            .execute(
                "INSERT INTO notifications (user_id, notification_type, title, message, priority, created_at, read)
                 VALUES ($1, $2, $3, $4, $5, NOW(), false)",
                &[&recipient_id, &notification_type, &title, &message, &"MEDIUM"],
            )
            .await
            .map_err(|e| {
                ProximityError::InternalError(format!(
                    "Failed to create recipient notification: {}",
                    e
                ))
            })?;

        info!(
            "Notifications sent to sender {} and recipient {}",
            sender_id, recipient_id
        );

        Ok(())
    }

    /// Execute transfer with automatic monitoring
    /// 
    /// This is a convenience method that executes the transfer and monitors it to completion.
    /// Returns the transaction hash, final status, and optional receipt data.
    /// 
    /// **Validates: Requirements 7.1, 7.5, 7.6, 10.3, 10.4**
    pub async fn execute_and_monitor_transfer(&self, request_id: Uuid) -> Result<(String, TransferStatus, Option<ProximityReceiptData>)> {
        // Execute the transfer
        let tx_hash = self.execute_transfer(request_id).await?;
        
        // Monitor the transaction
        let (status, receipt_data) = self.monitor_transaction(request_id, tx_hash.clone()).await?;
        
        Ok((tx_hash, status, receipt_data))
    }

    /// Persist transfer to database
    async fn persist_transfer_to_db(
        &self,
        request: &TransferRequest,
        tx_hash: &str,
        status: TransferStatus,
    ) -> Result<()> {
        let client = self.db_pool.get().await.map_err(|e| {
            ProximityError::InternalError(format!("Database connection error: {}", e))
        })?;

        client
            .execute(
                "INSERT INTO proximity_transfers (
                    id, sender_user_id, sender_wallet, recipient_user_id, recipient_wallet,
                    asset, amount, transaction_hash, status, discovery_method, created_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                ON CONFLICT (id) DO UPDATE SET
                    transaction_hash = EXCLUDED.transaction_hash,
                    status = EXCLUDED.status",
                &[
                    &request.id,
                    &request.sender_user_id,
                    &request.sender_wallet,
                    &request.recipient_user_id,
                    &request.recipient_wallet,
                    &request.asset,
                    // amount column is NUMERIC; pass the Decimal directly (a
                    // String fails to serialize into NUMERIC).
                    &request.amount,
                    &tx_hash,
                    &status.to_string(),
                    &"WiFi", // Placeholder - should track actual discovery method
                    // created_at column is `timestamp without time zone`; pass a
                    // NaiveDateTime (a DateTime<Utc> fails to serialize into it).
                    &request.created_at.naive_utc(),
                ],
            )
            .await
            .map_err(|e| {
                ProximityError::InternalError(format!("Failed to persist transfer: {}", e))
            })?;

        Ok(())
    }

    /// Get transfer status
    pub async fn get_transfer_status(&self, request_id: Uuid) -> Result<TransferStatus> {
        let requests = self.active_requests.read().await;
        
        let request = requests
            .get(&request_id)
            .ok_or_else(|| ProximityError::TransferNotFound(request_id.to_string()))?;

        Ok(request.status)
    }

    /// Get a transfer request by ID
    pub async fn get_transfer_request(&self, request_id: Uuid) -> Result<TransferRequest> {
        let requests = self.active_requests.read().await;
        
        requests
            .get(&request_id)
            .cloned()
            .ok_or_else(|| ProximityError::TransferNotFound(request_id.to_string()))
    }

    /// Get all active transfer requests
    pub async fn get_active_requests(&self) -> Result<Vec<TransferRequest>> {
        let requests = self.active_requests.read().await;
        Ok(requests.values().cloned().collect())
    }

    /// Get queued transfer requests
    pub async fn get_queued_requests(&self) -> Result<Vec<TransferRequest>> {
        let queued = self.queued_requests.read().await;
        Ok(queued.clone())
    }

    /// Process queued requests when slots become available
    /// 
    /// **Validates: Requirements 14.3**
    async fn process_queued_requests(&self, user_id: Uuid) -> Result<()> {
        let mut queued = self.queued_requests.write().await;
        
        // Find queued requests for this user
        let user_queued: Vec<_> = queued
            .iter()
            .enumerate()
            .filter(|(_, r)| r.sender_user_id == user_id)
            .map(|(i, _)| i)
            .collect();
        
        if user_queued.is_empty() {
            return Ok(());
        }
        
        // Check how many active transfers this user has
        let active_requests = self.active_requests.read().await;
        let user_active_count = active_requests
            .values()
            .filter(|r| r.sender_user_id == user_id && 
                       (r.status == TransferStatus::Pending || 
                        r.status == TransferStatus::Accepted ||
                        r.status == TransferStatus::Executing))
            .count();
        drop(active_requests);
        
        // Calculate available slots
        let available_slots = self.max_concurrent_transfers.saturating_sub(user_active_count);
        
        if available_slots == 0 {
            return Ok(());
        }
        
        // Move queued requests to active (up to available slots)
        let mut moved_count = 0;
        let mut active_requests = self.active_requests.write().await;
        
        for idx in user_queued.iter().take(available_slots) {
            if let Some(request) = queued.get(*idx - moved_count) {
                info!(
                    "Moving queued request {} to active for user {}",
                    request.id, user_id
                );
                active_requests.insert(request.id, request.clone());
                queued.remove(*idx - moved_count);
                moved_count += 1;
            }
        }
        
        drop(active_requests);
        drop(queued);
        
        if moved_count > 0 {
            info!(
                "Processed {} queued request(s) for user {}",
                moved_count, user_id
            );
        }
        
        Ok(())
    }

    /// Validate sender has sufficient balance for the transfer
    /// 
    /// **Validates: Requirements 5.5**
    async fn validate_sender_balance(
        &self,
        sender_wallet: &str,
        asset: &str,
        amount: Decimal,
    ) -> Result<()> {
        debug!(
            "Validating balance for wallet={}, asset={}, amount={}",
            sender_wallet, asset, amount
        );

        let is_sol = asset == "SOL"
            || asset == "So11111111111111111111111111111111111111112";

        // In the devnet demo the on-chain source of funds is the backend's
        // custodial signer (the funded demo wallet), NOT the wallet the user
        // happened to connect on the dashboard. The platform does not hold the
        // connected wallet's private key, so it can never sign from it. For SOL
        // transfers, validate against the custodial wallet's REAL on-chain
        // balance so the check reflects the wallet that actually pays.
        if is_sol {
            if let Some(signer) = &self.custodial_signer {
                let custodial_pubkey = signer.pubkey().to_string();
                match self.solana_client.get_sol_balance(&custodial_pubkey).await {
                    Ok(lamports) => {
                        let balance =
                            Decimal::from(lamports) / Decimal::from(LAMPORTS_PER_SOL);
                        let fee_estimate = amount * Decimal::new(1, 2); // ~1%
                        let required = amount + fee_estimate;
                        if balance < required {
                            return Err(ProximityError::InsufficientBalance {
                                required: required.to_string(),
                                available: balance.to_string(),
                            });
                        }
                        debug!(
                            "Custodial balance validation passed: balance={}, required={}",
                            balance, required
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        // If the RPC is unreachable we don't want to hard-block
                        // the demo on a flaky devnet. Log and allow the request;
                        // the actual on-chain submit at accept-time will surface
                        // any real funding problem.
                        warn!(
                            "Could not fetch custodial wallet balance ({}); skipping pre-check",
                            e
                        );
                        return Ok(());
                    }
                }
            }
        }

        // Query portfolio assets from database
        let client = self.db_pool.get().await.map_err(|e| {
            ProximityError::InternalError(format!("Database connection error: {}", e))
        })?;

        // Get wallet ID
        let wallet_row = client
            .query_one(
                "SELECT id FROM wallets WHERE address = $1",
                &[&sender_wallet],
            )
            .await
            .map_err(|_| {
                ProximityError::InvalidWalletAddress(format!(
                    "Wallet not found: {}",
                    sender_wallet
                ))
            })?;

        let wallet_id: Uuid = wallet_row.get(0);

        // Get asset balance. The `asset` identifier from the transfer request
        // may be either a token mint address (e.g. "So111...112") or a symbol
        // (e.g. "SOL"); portfolio_assets stores both, so match on either to
        // avoid spurious "insufficient balance" when a symbol is supplied.
        let asset_row = client
            .query_opt(
                "SELECT amount FROM portfolio_assets
                 WHERE wallet_id = $1 AND (token_mint = $2 OR token_symbol = $2)",
                &[&wallet_id, &asset],
            )
            .await?;

        if let Some(row) = asset_row {
            // `amount` is a Postgres NUMERIC column; read it as Decimal directly
            // (reading into String panics at runtime). Convert to string only
            // for the existing downstream parsing/logging.
            let balance: Decimal = row.get(0);
            let balance_str = balance.to_string();
            let _ = &balance_str;

            // Check if balance is sufficient (including estimated fees)
            // For now, we use a simple 1% fee estimate
            let fee_estimate = amount * Decimal::new(1, 2); // 1% = 0.01
            let required = amount + fee_estimate;

            if balance < required {
                return Err(ProximityError::InsufficientBalance {
                    required: required.to_string(),
                    available: balance.to_string(),
                });
            }

            debug!(
                "Balance validation passed: balance={}, required={}",
                balance, required
            );
            Ok(())
        } else {
            // Asset not found in portfolio
            Err(ProximityError::InsufficientBalance {
                required: amount.to_string(),
                available: "0".to_string(),
            })
        }
    }

    /// Start background task to clean up expired requests
    /// 
    /// **Validates: Requirements 6.6**
    fn start_cleanup_task(&self) {
        let requests = Arc::clone(&self.active_requests);
        let queued_requests = Arc::clone(&self.queued_requests);
        let shutdown = Arc::clone(&self.shutdown_notify);
        let max_concurrent = self.max_concurrent_transfers;

        tokio::spawn(async move {
            let mut cleanup_interval = interval(TokioDuration::from_secs(CLEANUP_INTERVAL_SECS));

            loop {
                tokio::select! {
                    _ = cleanup_interval.tick() => {
                        let now = Utc::now();
                        let mut request_map = requests.write().await;
                        let initial_count = request_map.len();
                        let mut users_to_process = std::collections::HashSet::new();

                        // Auto-reject expired pending requests
                        for (id, request) in request_map.iter_mut() {
                            if request.status == TransferStatus::Pending && now > request.expires_at {
                                request.status = TransferStatus::Expired;
                                users_to_process.insert(request.sender_user_id);
                                info!("Auto-expired transfer request: {}", id);
                            }
                        }

                        // Remove completed, failed, rejected, and expired requests older than 5 minutes
                        let cleanup_threshold = now - Duration::minutes(5);
                        request_map.retain(|id, request| {
                            let should_keep = match request.status {
                                TransferStatus::Pending | TransferStatus::Accepted | TransferStatus::Executing => true,
                                _ => request.created_at > cleanup_threshold,
                            };
                            
                            if !should_keep {
                                users_to_process.insert(request.sender_user_id);
                                debug!("Removing old transfer request: {}", id);
                            }
                            
                            should_keep
                        });

                        let removed_count = initial_count - request_map.len();
                        if removed_count > 0 {
                            debug!("Cleanup task removed {} old request(s)", removed_count);
                        }
                        
                        // Process queued requests for affected users
                        for user_id in users_to_process {
                            let mut queued = queued_requests.write().await;
                            
                            // Find queued requests for this user
                            let user_queued: Vec<_> = queued
                                .iter()
                                .enumerate()
                                .filter(|(_, r)| r.sender_user_id == user_id)
                                .map(|(i, _)| i)
                                .collect();
                            
                            if user_queued.is_empty() {
                                continue;
                            }
                            
                            // Check how many active transfers this user has
                            let user_active_count = request_map
                                .values()
                                .filter(|r| r.sender_user_id == user_id && 
                                           (r.status == TransferStatus::Pending || 
                                            r.status == TransferStatus::Accepted ||
                                            r.status == TransferStatus::Executing))
                                .count();
                            
                            // Calculate available slots
                            let available_slots = max_concurrent.saturating_sub(user_active_count);
                            
                            if available_slots == 0 {
                                continue;
                            }
                            
                            // Move queued requests to active (up to available slots)
                            let mut moved_count = 0;
                            
                            for idx in user_queued.iter().take(available_slots) {
                                if let Some(request) = queued.get(*idx - moved_count) {
                                    info!(
                                        "Cleanup task moving queued request {} to active for user {}",
                                        request.id, user_id
                                    );
                                    request_map.insert(request.id, request.clone());
                                    queued.remove(*idx - moved_count);
                                    moved_count += 1;
                                }
                            }
                            
                            if moved_count > 0 {
                                info!(
                                    "Cleanup task processed {} queued request(s) for user {}",
                                    moved_count, user_id
                                );
                            }
                        }
                    }
                    _ = shutdown.notified() => {
                        debug!("Cleanup task received shutdown signal");
                        break;
                    }
                }
            }

            debug!("Transfer cleanup task terminated");
        });
    }

    /// Stop the transfer service and cleanup tasks
    pub async fn shutdown(&self) {
        info!("Shutting down transfer service");
        self.shutdown_notify.notify_waiters();
    }

    /// Check if recipient has an associated token account for SPL token
    /// 
    /// **Validates: Requirements 12.4**
    pub async fn check_token_account_exists(
        &self,
        recipient_wallet: &str,
        token_mint: &str,
    ) -> Result<bool> {
        debug!(
            "Checking token account existence: recipient={}, mint={}",
            recipient_wallet, token_mint
        );

        // Parse addresses
        let recipient_pubkey = Pubkey::from_str(recipient_wallet).map_err(|e| {
            ProximityError::InvalidWalletAddress(format!("Invalid recipient address: {}", e))
        })?;

        let mint_pubkey = Pubkey::from_str(token_mint).map_err(|e| {
            ProximityError::InvalidWalletAddress(format!("Invalid token mint: {}", e))
        })?;

        // Get associated token account address
        let token_account = get_associated_token_address(&recipient_pubkey, &mint_pubkey);

        // Query Solana blockchain for account existence
        match self
            .solana_client
            .primary_client()
            .get_account(&token_account)
        {
            Ok(_account) => {
                debug!("Token account exists: {}", token_account);
                Ok(true)
            }
            Err(e) => {
                // Check if error is "account not found" vs other errors
                let error_str = e.to_string();
                if error_str.contains("AccountNotFound") || error_str.contains("could not find account") {
                    debug!("Token account does not exist: {}", token_account);
                    Ok(false)
                } else {
                    // Other error occurred
                    warn!("Error checking token account: {}", e);
                    Err(ProximityError::NetworkError(format!(
                        "Failed to check token account: {}",
                        e
                    )))
                }
            }
        }
    }

    /// Calculate the fee required to create an associated token account
    /// 
    /// **Validates: Requirements 12.5**
    pub async fn calculate_token_account_creation_fee(&self) -> Result<Decimal> {
        debug!("Calculating token account creation fee");

        // Get rent exemption for token account
        // Token accounts are 165 bytes
        const TOKEN_ACCOUNT_SIZE: usize = 165;

        let rent_exemption = self
            .solana_client
            .primary_client()
            .get_minimum_balance_for_rent_exemption(TOKEN_ACCOUNT_SIZE)
            .map_err(|e| {
                ProximityError::NetworkError(format!(
                    "Failed to get rent exemption: {}",
                    e
                ))
            })?;

        // Convert lamports to SOL
        let fee_sol = Decimal::from(rent_exemption) / Decimal::from(LAMPORTS_PER_SOL);

        debug!("Token account creation fee: {} SOL ({} lamports)", fee_sol, rent_exemption);
        Ok(fee_sol)
    }

    /// Create an associated token account for the recipient
    /// 
    /// This method creates the token account and returns the transaction hash.
    /// The payer (sender) will pay for the account creation.
    /// 
    /// **Validates: Requirements 12.5**
    pub async fn create_token_account(
        &self,
        payer_wallet: &str,
        recipient_wallet: &str,
        token_mint: &str,
    ) -> Result<String> {
        info!(
            "Creating token account: payer={}, recipient={}, mint={}",
            payer_wallet, recipient_wallet, token_mint
        );

        // Parse addresses
        let payer_pubkey = Pubkey::from_str(payer_wallet).map_err(|e| {
            ProximityError::InvalidWalletAddress(format!("Invalid payer address: {}", e))
        })?;

        let recipient_pubkey = Pubkey::from_str(recipient_wallet).map_err(|e| {
            ProximityError::InvalidWalletAddress(format!("Invalid recipient address: {}", e))
        })?;

        let mint_pubkey = Pubkey::from_str(token_mint).map_err(|e| {
            ProximityError::InvalidWalletAddress(format!("Invalid token mint: {}", e))
        })?;

        // Get recent blockhash
        let recent_blockhash = self
            .solana_client
            .primary_client()
            .get_latest_blockhash()
            .map_err(|e| {
                ProximityError::NetworkError(format!("Failed to get recent blockhash: {}", e))
            })?;

        // Create associated token account instruction
        let instruction = spl_associated_token_account::instruction::create_associated_token_account(
            &payer_pubkey,
            &recipient_pubkey,
            &mint_pubkey,
            &spl_token::id(),
        );

        // Create a temporary keypair for demonstration
        // In production, this would be handled by the wallet/signer
        let payer_keypair = Keypair::new();

        // Create transaction
        let transaction = Transaction::new_signed_with_payer(
            &[instruction],
            Some(&payer_keypair.pubkey()),
            &[&payer_keypair],
            recent_blockhash,
        );

        // Send transaction
        let signature = self
            .solana_client
            .primary_client()
            .send_and_confirm_transaction_with_spinner_and_config(
                &transaction,
                CommitmentConfig::confirmed(),
                RpcSendTransactionConfig {
                    skip_preflight: false,
                    preflight_commitment: Some(CommitmentConfig::confirmed().commitment),
                    ..Default::default()
                },
            )
            .map_err(|e| {
                ProximityError::TransactionFailed(format!("Token account creation failed: {}", e))
            })?;

        let tx_hash = signature.to_string();
        info!("Token account created successfully: {}", tx_hash);
        Ok(tx_hash)
    }

    /// Validate transfer and check for token account requirements
    /// 
    /// This is a convenience method that checks if the transfer is valid and
    /// whether a token account needs to be created for SPL token transfers.
    /// 
    /// Returns (is_valid, needs_token_account, estimated_fee)
    /// 
    /// **Validates: Requirements 12.4, 12.5**
    pub async fn validate_transfer_requirements(
        &self,
        sender_wallet: &str,
        recipient_wallet: &str,
        asset: &str,
        amount: Decimal,
    ) -> Result<(bool, bool, Option<Decimal>)> {
        debug!(
            "Validating transfer requirements: sender={}, recipient={}, asset={}, amount={}",
            sender_wallet, recipient_wallet, asset, amount
        );

        // Check if this is a SOL transfer
        let is_sol_transfer = asset == "SOL" || asset == "So11111111111111111111111111111111111111112";

        if is_sol_transfer {
            // SOL transfers don't need token accounts
            return Ok((true, false, None));
        }

        // For SPL tokens, check if recipient has token account
        let has_token_account = self
            .check_token_account_exists(recipient_wallet, asset)
            .await?;

        if has_token_account {
            // Token account exists, no creation needed
            Ok((true, false, None))
        } else {
            // Token account doesn't exist, calculate creation fee
            let creation_fee = self.calculate_token_account_creation_fee().await?;
            Ok((true, true, Some(creation_fee)))
        }
    }
}
