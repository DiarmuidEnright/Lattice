use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use chrono::{DateTime, Utc};
use database::DbPool;
use rand::RngCore;
use shared::{Error, Result};
use std::sync::Arc;
use tracing::{debug, error, info};
use uuid::Uuid;

use crate::receipt_service::{ReceiptService, ReceiptData};
use blockchain::Blockchain;

/// Proximity contact structure for chat
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProximityContact {
    pub peer_id: String,
    pub user_tag: String,
    pub wallet_address: String,
    pub discovery_method: String,
    pub signal_strength: Option<i8>,
    pub verified: bool,
    pub last_seen: DateTime<Utc>,
}

/// Chat message structure
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub id: Uuid,
    pub from_user_id: Uuid,
    pub to_user_id: Uuid,
    pub content: String,
    pub encrypted: bool,
    pub blockchain_hash: Option<String>,
    pub verification_status: Option<String>,
    pub read: bool,
    pub created_at: DateTime<Utc>,
}

/// Chat service for encrypted peer-to-peer messaging
pub struct ChatService {
    db: DbPool,
    receipt_service: Arc<ReceiptService>,
}

impl ChatService {
    pub fn new(db: DbPool, receipt_service: Arc<ReceiptService>) -> Self {
        info!("Initializing chat service");
        Self {
            db,
            receipt_service,
        }
    }

    /// Get available chat contacts from proximity network
    /// Only returns users who are currently discovered via proximity
    pub async fn get_proximity_contacts(
        &self,
        discovery_service: &proximity::DiscoveryService,
    ) -> Result<Vec<ProximityContact>> {
        // Get discovered peers from proximity network
        let peers = discovery_service.get_discovered_peers().await
            .map_err(|e| Error::Internal(format!("Failed to get discovered peers: {}", e)))?;
        
        // Convert to contact format
        let contacts: Vec<ProximityContact> = peers
            .into_iter()
            .map(|peer| ProximityContact {
                peer_id: peer.peer_id.to_string(),
                user_tag: peer.user_tag,
                wallet_address: peer.wallet_address,
                discovery_method: format!("{:?}", peer.discovery_method),
                signal_strength: peer.signal_strength,
                verified: peer.verified,
                last_seen: peer.last_seen,
            })
            .collect();
        
        Ok(contacts)
    }

    /// Check if a user is in the proximity network
    pub async fn is_user_in_proximity(
        &self,
        wallet_address: &str,
        discovery_service: &proximity::DiscoveryService,
    ) -> Result<bool> {
        let peers = discovery_service.get_discovered_peers().await
            .map_err(|e| Error::Internal(format!("Failed to get discovered peers: {}", e)))?;
        
        Ok(peers.iter().any(|p| p.wallet_address == wallet_address))
    }

    /// Send an encrypted message between users
    /// 
    /// Messages are encrypted using AES-256-GCM with a shared key derived from wallet signatures.
    /// Optionally, a hash of the message can be submitted to the blockchain for verification.
    pub async fn send_message(
        &self,
        from_user_id: Uuid,
        to_user_id: Uuid,
        content: String,
        encryption_key: &[u8; 32],
        verify_on_chain: bool,
        blockchain: Option<Blockchain>,
    ) -> Result<ChatMessage> {
        info!(
            "Sending message from user {} to user {}",
            from_user_id, to_user_id
        );

        // Encrypt the message content
        let encrypted_content = self.encrypt_message(&content, encryption_key)?;
        debug!("Message encrypted successfully");

        // Optionally submit hash to blockchain
        let blockchain_hash = if verify_on_chain {
            let blockchain = blockchain.ok_or_else(|| {
                Error::Internal("Blockchain must be specified for on-chain verification".to_string())
            })?;
            
            Some(self.submit_message_hash(&content, blockchain).await?)
        } else {
            None
        };

        // Store encrypted message in database
        let message = self
            .store_message(
                from_user_id,
                to_user_id,
                encrypted_content,
                blockchain_hash.clone(),
            )
            .await?;

        info!("Message sent successfully: {}", message.id);
        Ok(message)
    }

    /// Retrieve and decrypt messages for a user
    pub async fn get_messages(
        &self,
        user_id: Uuid,
        other_user_id: Uuid,
        encryption_key: &[u8; 32],
        limit: i64,
    ) -> Result<Vec<ChatMessage>> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                r#"
                SELECT id, from_user_id, to_user_id, content, encrypted,
                       blockchain_hash, verification_status, read, created_at
                FROM chat_messages
                WHERE (from_user_id = $1 AND to_user_id = $2)
                   OR (from_user_id = $2 AND to_user_id = $1)
                ORDER BY created_at DESC
                LIMIT $3
                "#,
                &[&user_id, &other_user_id, &limit],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch messages: {}", e)))?;

        let mut messages = Vec::new();
        for row in rows {
            let mut message = self.row_to_message(&row)?;
            
            // Decrypt content if encrypted
            if message.encrypted {
                match self.decrypt_message(&message.content, encryption_key) {
                    Ok(decrypted) => message.content = decrypted,
                    Err(e) => {
                        error!("Failed to decrypt message {}: {}", message.id, e);
                        message.content = "[Decryption failed]".to_string();
                    }
                }
            }
            
            messages.push(message);
        }

        Ok(messages)
    }

    /// Mark a message as read
    pub async fn mark_as_read(&self, message_id: Uuid, user_id: Uuid) -> Result<()> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        client
            .execute(
                r#"
                UPDATE chat_messages
                SET read = TRUE
                WHERE id = $1 AND to_user_id = $2
                "#,
                &[&message_id, &user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to mark message as read: {}", e)))?;

        Ok(())
    }

    /// Verify a message's authenticity by checking blockchain
    pub async fn verify_message(&self, message_id: Uuid) -> Result<bool> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let row = client
            .query_one(
                r#"
                SELECT blockchain_hash, verification_status
                FROM chat_messages
                WHERE id = $1
                "#,
                &[&message_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch message: {}", e)))?;

        let blockchain_hash: Option<String> = row.get("blockchain_hash");
        
        if blockchain_hash.is_none() {
            return Ok(false); // Not verified on-chain
        }

        // Check if already verified
        let status: Option<String> = row.get("verification_status");
        if status.as_deref() == Some("CONFIRMED") {
            return Ok(true);
        }

        // TODO: Implement actual blockchain verification
        // For now, mark as pending
        client
            .execute(
                r#"
                UPDATE chat_messages
                SET verification_status = 'PENDING'
                WHERE id = $1
                "#,
                &[&message_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to update verification status: {}", e)))?;

        Ok(false)
    }

    /// Report a message (only processes if message has on-chain verification)
    pub async fn report_message(&self, message_id: Uuid, reporter_id: Uuid, reason: String) -> Result<()> {
        info!("User {} reporting message {}", reporter_id, message_id);

        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Check if message has blockchain verification
        let row = client
            .query_one(
                "SELECT blockchain_hash FROM chat_messages WHERE id = $1",
                &[&message_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch message: {}", e)))?;

        let blockchain_hash: Option<String> = row.get("blockchain_hash");
        
        if blockchain_hash.is_none() {
            return Err(Error::Internal(
                "Cannot report message without blockchain verification".to_string(),
            ));
        }

        // TODO: Store report in a reports table
        info!("Message {} reported by user {}: {}", message_id, reporter_id, reason);

        Ok(())
    }

    /// Create a chat conversation from a P2P offer
    /// 
    /// This method:
    /// 1. Creates a conversation record
    /// 2. Links conversation to the offer
    /// 3. Adds both users as participants
    /// 4. Allows messaging regardless of proximity
    /// 
    /// Returns the conversation ID
    pub async fn create_conversation_from_offer(
        &self,
        offer_id: Uuid,
        creator_id: Uuid,
        acceptor_id: Uuid,
    ) -> Result<Uuid> {
        tracing::info!(
            offer_id = %offer_id,
            creator_id = %creator_id,
            acceptor_id = %acceptor_id,
            "Attempting to create conversation from offer"
        );

        let mut client = self.db.get().await.map_err(|e| {
            tracing::error!(
                offer_id = %offer_id,
                creator_id = %creator_id,
                acceptor_id = %acceptor_id,
                error = %e,
                "Failed to get database connection for conversation creation"
            );
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Start transaction for atomicity
        let transaction = client.transaction().await.map_err(|e| {
            tracing::error!(
                offer_id = %offer_id,
                creator_id = %creator_id,
                acceptor_id = %acceptor_id,
                error = %e,
                "Failed to start transaction for conversation creation"
            );
            Error::Database(format!("Failed to start transaction: {}", e))
        })?;

        // Create conversation record with offer_id reference
        let conversation_id = Uuid::new_v4();
        transaction
            .execute(
                r#"
                INSERT INTO chat_conversations (id, offer_id, created_at, updated_at)
                VALUES ($1, $2, NOW(), NOW())
                "#,
                &[&conversation_id, &offer_id],
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    offer_id = %offer_id,
                    conversation_id = %conversation_id,
                    creator_id = %creator_id,
                    acceptor_id = %acceptor_id,
                    error = %e,
                    "Failed to create conversation record"
                );
                Error::Database(format!("Failed to create conversation: {}", e))
            })?;

        // Add creator as participant
        transaction
            .execute(
                r#"
                INSERT INTO chat_participants (conversation_id, user_id, joined_at)
                VALUES ($1, $2, NOW())
                "#,
                &[&conversation_id, &creator_id],
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    offer_id = %offer_id,
                    conversation_id = %conversation_id,
                    creator_id = %creator_id,
                    error = %e,
                    "Failed to add creator as conversation participant"
                );
                Error::Database(format!("Failed to add creator as participant: {}", e))
            })?;

        // Add acceptor as participant
        transaction
            .execute(
                r#"
                INSERT INTO chat_participants (conversation_id, user_id, joined_at)
                VALUES ($1, $2, NOW())
                "#,
                &[&conversation_id, &acceptor_id],
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    offer_id = %offer_id,
                    conversation_id = %conversation_id,
                    acceptor_id = %acceptor_id,
                    error = %e,
                    "Failed to add acceptor as conversation participant"
                );
                Error::Database(format!("Failed to add acceptor as participant: {}", e))
            })?;

        // Update offer with conversation_id
        transaction
            .execute(
                r#"
                UPDATE p2p_offers
                SET conversation_id = $1
                WHERE id = $2
                "#,
                &[&conversation_id, &offer_id],
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    offer_id = %offer_id,
                    conversation_id = %conversation_id,
                    error = %e,
                    "Failed to update offer with conversation_id"
                );
                Error::Database(format!("Failed to update offer with conversation_id: {}", e))
            })?;

        // Commit transaction
        transaction.commit().await.map_err(|e| {
            tracing::error!(
                offer_id = %offer_id,
                conversation_id = %conversation_id,
                creator_id = %creator_id,
                acceptor_id = %acceptor_id,
                error = %e,
                "Failed to commit transaction for conversation creation"
            );
            Error::Database(format!("Failed to commit transaction: {}", e))
        })?;

        tracing::info!(
            offer_id = %offer_id,
            conversation_id = %conversation_id,
            creator_id = %creator_id,
            acceptor_id = %acceptor_id,
            "Successfully created conversation from offer"
        );

        Ok(conversation_id)
    }

    /// Send a system notification about offer acceptance
    pub async fn send_offer_notification(
        &self,
        to_user_id: Uuid,
        from_user_id: Uuid,
        offer_id: Uuid,
        message: String,
    ) -> Result<()> {
        tracing::info!(
            to_user_id = %to_user_id,
            from_user_id = %from_user_id,
            offer_id = %offer_id,
            "Attempting to send offer acceptance notification"
        );

        // Store notification as a system message
        let notification_id = Uuid::new_v4();
        
        let client = self.db.get().await.map_err(|e| {
            tracing::error!(
                to_user_id = %to_user_id,
                from_user_id = %from_user_id,
                offer_id = %offer_id,
                error = %e,
                "Failed to get database connection for notification"
            );
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        client
            .execute(
                r#"
                INSERT INTO chat_messages (
                    id, from_user_id, to_user_id, content, encrypted,
                    blockchain_hash, verification_status, read, created_at
                )
                VALUES ($1, $2, $3, $4, FALSE, NULL, NULL, FALSE, NOW())
                "#,
                &[
                    &notification_id,
                    &from_user_id,
                    &to_user_id,
                    &message,
                ],
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    notification_id = %notification_id,
                    to_user_id = %to_user_id,
                    from_user_id = %from_user_id,
                    offer_id = %offer_id,
                    error = %e,
                    "Failed to store notification message"
                );
                Error::Database(format!("Failed to store notification: {}", e))
            })?;

        tracing::info!(
            notification_id = %notification_id,
            to_user_id = %to_user_id,
            from_user_id = %from_user_id,
            offer_id = %offer_id,
            "Successfully sent offer acceptance notification"
        );

        Ok(())
    }

    /// Encrypt message content using AES-256-GCM
    fn encrypt_message(&self, content: &str, key: &[u8; 32]) -> Result<String> {
        let cipher = Aes256Gcm::new(key.into());
        
        // Generate random nonce
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt
        let ciphertext = cipher
            .encrypt(nonce, content.as_bytes())
            .map_err(|e| Error::Internal(format!("Encryption failed: {}", e)))?;

        // Combine nonce + ciphertext and encode as hex
        let mut combined = nonce_bytes.to_vec();
        combined.extend_from_slice(&ciphertext);
        
        Ok(hex::encode(combined))
    }

    /// Decrypt message content using AES-256-GCM
    fn decrypt_message(&self, encrypted_hex: &str, key: &[u8; 32]) -> Result<String> {
        let combined = hex::decode(encrypted_hex)
            .map_err(|e| Error::Internal(format!("Invalid hex encoding: {}", e)))?;

        if combined.len() < 12 {
            return Err(Error::Internal("Invalid encrypted message format".to_string()));
        }

        // Split nonce and ciphertext
        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let cipher = Aes256Gcm::new(key.into());
        
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| Error::Internal(format!("Decryption failed: {}", e)))?;

        String::from_utf8(plaintext)
            .map_err(|e| Error::Internal(format!("Invalid UTF-8 in decrypted message: {}", e)))
    }

    /// Submit message hash to blockchain for verification
    async fn submit_message_hash(&self, content: &str, blockchain: Blockchain) -> Result<String> {
        use sha2::{Digest, Sha256};

        // Generate SHA-256 hash of message
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let hash_bytes = hasher.finalize();
        let message_hash = format!("0x{}", hex::encode(hash_bytes));

        debug!("Generated message hash: {}", message_hash);

        // Create a receipt for the message hash
        // This will submit the hash to the blockchain
        let receipt_data = ReceiptData {
            payment_id: None,
            trade_id: None,
            conversion_id: None,
            proximity_transfer_id: None,
            amount: rust_decimal::Decimal::ZERO,
            currency: "MESSAGE_HASH".to_string(),
            sender: "CHAT_SERVICE".to_string(),
            recipient: "BLOCKCHAIN".to_string(),
            blockchain,
        };

        let receipt = self.receipt_service.create_receipt(receipt_data).await?;
        
        Ok(receipt.transaction_hash)
    }

    /// Store encrypted message in database
    async fn store_message(
        &self,
        from_user_id: Uuid,
        to_user_id: Uuid,
        encrypted_content: String,
        blockchain_hash: Option<String>,
    ) -> Result<ChatMessage> {
        let id = Uuid::new_v4();
        
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let verification_status = if blockchain_hash.is_some() {
            Some("PENDING")
        } else {
            None
        };

        let row = client
            .query_one(
                r#"
                INSERT INTO chat_messages (
                    id, from_user_id, to_user_id, content, encrypted,
                    blockchain_hash, verification_status, read, created_at
                )
                VALUES ($1, $2, $3, $4, TRUE, $5, $6, FALSE, NOW())
                RETURNING id, from_user_id, to_user_id, content, encrypted,
                          blockchain_hash, verification_status, read, created_at
                "#,
                &[
                    &id,
                    &from_user_id,
                    &to_user_id,
                    &encrypted_content,
                    &blockchain_hash,
                    &verification_status,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to store message: {}", e)))?;

        self.row_to_message(&row)
    }

    /// Convert database row to ChatMessage
    fn row_to_message(&self, row: &tokio_postgres::Row) -> Result<ChatMessage> {
        let created_at_systime: std::time::SystemTime = row
            .try_get("created_at")
            .map_err(|e| Error::Database(format!("Failed to get created_at: {}", e)))?;
        
        Ok(ChatMessage {
            id: row.try_get("id").map_err(|e| Error::Database(format!("Failed to get id: {}", e)))?,
            from_user_id: row.try_get("from_user_id").map_err(|e| Error::Database(format!("Failed to get from_user_id: {}", e)))?,
            to_user_id: row.try_get("to_user_id").map_err(|e| Error::Database(format!("Failed to get to_user_id: {}", e)))?,
            content: row.try_get("content").map_err(|e| Error::Database(format!("Failed to get content: {}", e)))?,
            encrypted: row.try_get("encrypted").map_err(|e| Error::Database(format!("Failed to get encrypted: {}", e)))?,
            blockchain_hash: row.try_get("blockchain_hash").ok(),
            verification_status: row.try_get("verification_status").ok(),
            read: row.try_get("read").map_err(|e| Error::Database(format!("Failed to get read: {}", e)))?,
            created_at: DateTime::<Utc>::from(created_at_systime),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        // Create a mock service for testing encryption/decryption
        let key = [42u8; 32]; // Test key
        let message = "Hello, World! This is a secret message.";

        // Test encryption
        let cipher = Aes256Gcm::new(&key.into());
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher.encrypt(nonce, message.as_bytes()).unwrap();
        
        // Test decryption
        let plaintext = cipher.decrypt(nonce, ciphertext.as_ref()).unwrap();
        let decrypted = String::from_utf8(plaintext).unwrap();

        assert_eq!(message, decrypted);
    }
}
