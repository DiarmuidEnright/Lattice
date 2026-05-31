//! Secure storage abstraction for stealth keys

// Platform-specific implementations
pub mod platform;

use crate::error::{StealthError, StealthResult};
use crate::keypair::StealthKeyPair;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Trait for platform-agnostic secure storage
/// 
/// This trait provides a secure storage interface for stealth key pairs and arbitrary data.
/// All implementations MUST:
/// - Encrypt private keys at rest using AES-256-GCM (Requirement 9.2)
/// - Store spending and viewing keys separately (Requirement 9.3)
/// - Never log or transmit private keys in plaintext (Requirement 9.6)
/// 
/// # Requirements
/// Validates: Requirements 9.1, 9.2, 9.3, 9.6
#[async_trait]
pub trait SecureStorage: Send + Sync {
    /// Store a stealth key pair securely
    /// 
    /// The implementation MUST encrypt the spending and viewing keys separately
    /// using AES-256-GCM before persisting to storage.
    async fn store_keypair(&self, id: &str, keypair: &StealthKeyPair) -> StealthResult<()>;

    /// Load a stealth key pair
    /// 
    /// The implementation MUST decrypt the stored keys and reconstruct the keypair.
    async fn load_keypair(&self, id: &str) -> StealthResult<StealthKeyPair>;

    /// Delete a stealth key pair
    async fn delete_keypair(&self, id: &str) -> StealthResult<()>;

    /// List all stored key pair IDs
    async fn list_keypairs(&self) -> StealthResult<Vec<String>>;

    /// Store arbitrary encrypted data
    async fn store_data(&self, key: &str, data: &[u8]) -> StealthResult<()>;

    /// Load arbitrary encrypted data
    async fn load_data(&self, key: &str) -> StealthResult<Vec<u8>>;
}

/// Encrypted key pair storage format
/// 
/// This structure represents how key pairs are stored with encryption at rest.
/// The spending and viewing keys are stored separately to enable view-only wallet modes.
#[derive(Serialize, Deserialize, Clone)]
struct EncryptedKeyPair {
    /// Encrypted spending secret key (32 bytes + auth tag)
    spending_secret_encrypted: Vec<u8>,
    /// Nonce used for spending key encryption
    spending_nonce: [u8; 12],
    /// Encrypted viewing secret key (32 bytes + auth tag)
    viewing_secret_encrypted: Vec<u8>,
    /// Nonce used for viewing key encryption
    viewing_nonce: [u8; 12],
    /// Spending public key (not encrypted, needed for reconstruction)
    spending_public: [u8; 32],
    /// Viewing public key (not encrypted, needed for reconstruction)
    viewing_public: [u8; 32],
    /// Version of the key pair (1 = standard, 2 = hybrid)
    version: u8,
}

/// In-memory storage implementation with AES-256-GCM encryption
/// 
/// This implementation stores encrypted key pairs in memory and is suitable for:
/// - Testing and development
/// - Temporary storage during application runtime
/// - As a base for platform-specific implementations
/// 
/// # Security
/// - Uses AES-256-GCM for encryption at rest
/// - Derives device-specific encryption key from provided master key
/// - Stores spending and viewing keys separately
/// - Never logs private key material
/// 
/// # Requirements
/// Validates: Requirements 9.1, 9.2, 9.3, 9.6
pub struct InMemoryStorage {
    /// Encrypted key pairs indexed by ID
    keypairs: Arc<RwLock<HashMap<String, EncryptedKeyPair>>>,
    /// Encrypted arbitrary data indexed by key
    data: Arc<RwLock<HashMap<String, Vec<u8>>>>,
    /// Master encryption key (32 bytes for AES-256)
    encryption_key: [u8; 32],
}

impl InMemoryStorage {
    /// Create a new in-memory storage with a device-specific encryption key
    /// 
    /// # Arguments
    /// * `device_key` - A device-specific key used to derive the encryption key.
    ///                  In production, this should come from platform-specific secure storage
    ///                  (iOS Keychain, Android Keystore).
    /// 
    /// # Security
    /// The device_key is hashed with SHA-256 to derive a 32-byte AES-256 key.
    pub fn new(device_key: &[u8]) -> Self {
        // Derive encryption key from device key using SHA-256
        let mut hasher = Sha256::new();
        hasher.update(device_key);
        let hash = hasher.finalize();
        
        let mut encryption_key = [0u8; 32];
        encryption_key.copy_from_slice(&hash);
        
        Self {
            keypairs: Arc::new(RwLock::new(HashMap::new())),
            data: Arc::new(RwLock::new(HashMap::new())),
            encryption_key,
        }
    }
    
    /// Encrypt data using AES-256-GCM
    /// 
    /// Returns (ciphertext, nonce) tuple
    fn encrypt(&self, plaintext: &[u8]) -> StealthResult<(Vec<u8>, [u8; 12])> {
        let cipher = Aes256Gcm::new_from_slice(&self.encryption_key)
            .map_err(|e| StealthError::EncryptionFailed(format!("Failed to create cipher: {}", e)))?;
        
        // Generate random nonce
        let mut nonce_bytes = [0u8; 12];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        
        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| StealthError::EncryptionFailed(format!("Encryption failed: {}", e)))?;
        
        Ok((ciphertext, nonce_bytes))
    }
    
    /// Decrypt data using AES-256-GCM
    fn decrypt(&self, ciphertext: &[u8], nonce_bytes: &[u8; 12]) -> StealthResult<Vec<u8>> {
        let cipher = Aes256Gcm::new_from_slice(&self.encryption_key)
            .map_err(|e| StealthError::DecryptionFailed(format!("Failed to create cipher: {}", e)))?;
        
        let nonce = Nonce::from_slice(nonce_bytes);
        
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| StealthError::DecryptionFailed(format!("Decryption failed: {}", e)))?;
        
        Ok(plaintext)
    }
}

#[async_trait]
impl SecureStorage for InMemoryStorage {
    /// Store a stealth key pair securely with AES-256-GCM encryption
    /// 
    /// This implementation:
    /// 1. Extracts spending and viewing secret keys
    /// 2. Encrypts each secret key separately using AES-256-GCM
    /// 3. Stores public keys unencrypted (they're public anyway)
    /// 4. Persists the encrypted structure
    /// 
    /// # Requirements
    /// Validates: Requirements 9.2 (encryption at rest), 9.3 (key separation)
    async fn store_keypair(&self, id: &str, keypair: &StealthKeyPair) -> StealthResult<()> {
        // Extract secret keys (using internal accessor)
        let spending_secret = keypair.spending_keypair().secret.to_bytes();
        let viewing_secret = keypair.viewing_keypair().secret.to_bytes();
        
        // Encrypt spending secret key
        let (spending_encrypted, spending_nonce) = self.encrypt(&spending_secret)?;
        
        // Encrypt viewing secret key separately (Requirement 9.3)
        let (viewing_encrypted, viewing_nonce) = self.encrypt(&viewing_secret)?;
        
        // Create encrypted keypair structure
        let encrypted = EncryptedKeyPair {
            spending_secret_encrypted: spending_encrypted,
            spending_nonce,
            viewing_secret_encrypted: viewing_encrypted,
            viewing_nonce,
            spending_public: keypair.spending_keypair().public.to_bytes(),
            viewing_public: keypair.viewing_keypair().public.to_bytes(),
            version: 1, // Standard version
        };
        
        // Store encrypted keypair
        let mut keypairs = self.keypairs.write().await;
        keypairs.insert(id.to_string(), encrypted);
        
        Ok(())
    }

    /// Load a stealth key pair and decrypt it
    /// 
    /// This implementation:
    /// 1. Retrieves the encrypted keypair structure
    /// 2. Decrypts spending and viewing secret keys separately
    /// 3. Reconstructs the StealthKeyPair from decrypted keys
    /// 
    /// # Requirements
    /// Validates: Requirements 9.2 (decryption), 9.3 (key separation)
    async fn load_keypair(&self, id: &str) -> StealthResult<StealthKeyPair> {
        // Retrieve encrypted keypair
        let keypairs = self.keypairs.read().await;
        let encrypted = keypairs
            .get(id)
            .ok_or_else(|| StealthError::StorageFailed(format!("Keypair not found: {}", id)))?;
        
        // Decrypt spending secret key
        let spending_secret_bytes = self.decrypt(
            &encrypted.spending_secret_encrypted,
            &encrypted.spending_nonce,
        )?;
        
        // Decrypt viewing secret key
        let viewing_secret_bytes = self.decrypt(
            &encrypted.viewing_secret_encrypted,
            &encrypted.viewing_nonce,
        )?;
        
        // Reconstruct keypairs from decrypted secrets
        use ed25519_dalek::{Keypair, PublicKey, SecretKey};
        
        let spending_secret = SecretKey::from_bytes(&spending_secret_bytes)
            .map_err(|e| StealthError::KeyDerivationFailed(format!("Invalid spending secret: {}", e)))?;
        let spending_public = PublicKey::from_bytes(&encrypted.spending_public)
            .map_err(|e| StealthError::InvalidKeyFormat(format!("Invalid spending public key: {}", e)))?;
        
        let viewing_secret = SecretKey::from_bytes(&viewing_secret_bytes)
            .map_err(|e| StealthError::KeyDerivationFailed(format!("Invalid viewing secret: {}", e)))?;
        let viewing_public = PublicKey::from_bytes(&encrypted.viewing_public)
            .map_err(|e| StealthError::InvalidKeyFormat(format!("Invalid viewing public key: {}", e)))?;
        
        let spending_keypair = Keypair {
            secret: spending_secret,
            public: spending_public,
        };
        
        let viewing_keypair = Keypair {
            secret: viewing_secret,
            public: viewing_public,
        };
        
        // Reconstruct StealthKeyPair (we need to use from_parts or similar)
        // Since StealthKeyPair doesn't expose a constructor, we'll need to add one
        // For now, let's use the internal structure
        Ok(StealthKeyPair::from_parts(
            spending_keypair,
            viewing_keypair,
            encrypted.version,
        )?)
    }

    /// Delete a stealth key pair from storage
    async fn delete_keypair(&self, id: &str) -> StealthResult<()> {
        let mut keypairs = self.keypairs.write().await;
        keypairs
            .remove(id)
            .ok_or_else(|| StealthError::StorageFailed(format!("Keypair not found: {}", id)))?;
        Ok(())
    }

    /// List all stored key pair IDs
    async fn list_keypairs(&self) -> StealthResult<Vec<String>> {
        let keypairs = self.keypairs.read().await;
        Ok(keypairs.keys().cloned().collect())
    }

    /// Store arbitrary encrypted data
    /// 
    /// The data is encrypted using AES-256-GCM before storage.
    async fn store_data(&self, key: &str, data: &[u8]) -> StealthResult<()> {
        let (encrypted, nonce) = self.encrypt(data)?;
        
        // Prepend nonce to encrypted data for storage
        let mut stored = Vec::with_capacity(12 + encrypted.len());
        stored.extend_from_slice(&nonce);
        stored.extend_from_slice(&encrypted);
        
        let mut data_map = self.data.write().await;
        data_map.insert(key.to_string(), stored);
        
        Ok(())
    }

    /// Load arbitrary encrypted data
    /// 
    /// The data is decrypted using AES-256-GCM after retrieval.
    async fn load_data(&self, key: &str) -> StealthResult<Vec<u8>> {
        let data_map = self.data.read().await;
        let stored = data_map
            .get(key)
            .ok_or_else(|| StealthError::StorageFailed(format!("Data not found: {}", key)))?;
        
        // Extract nonce and ciphertext
        if stored.len() < 12 {
            return Err(StealthError::StorageFailed(
                "Invalid stored data: too short".to_string(),
            ));
        }
        
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&stored[..12]);
        let ciphertext = &stored[12..];
        
        self.decrypt(ciphertext, &nonce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_store_and_load_keypair() {
        let storage = InMemoryStorage::new(b"test-device-key");
        let keypair = StealthKeyPair::generate_standard().unwrap();
        let original_meta = keypair.to_meta_address();
        
        // Store keypair
        storage.store_keypair("test-id", &keypair).await.unwrap();
        
        // Load keypair
        let loaded = storage.load_keypair("test-id").await.unwrap();
        let loaded_meta = loaded.to_meta_address();
        
        // Verify meta-addresses match
        assert_eq!(original_meta, loaded_meta);
    }
    
    #[tokio::test]
    async fn test_store_keypair_encrypts_data() {
        let storage = InMemoryStorage::new(b"test-device-key");
        let keypair = StealthKeyPair::generate_standard().unwrap();
        
        // Store keypair
        storage.store_keypair("test-id", &keypair).await.unwrap();
        
        // Verify encrypted data exists and is not plaintext
        let keypairs = storage.keypairs.read().await;
        let encrypted = keypairs.get("test-id").unwrap();
        
        // Encrypted data should be longer than plaintext due to auth tag
        assert!(encrypted.spending_secret_encrypted.len() > 32);
        assert!(encrypted.viewing_secret_encrypted.len() > 32);
        
        // Verify spending and viewing keys are stored separately
        assert_ne!(
            encrypted.spending_secret_encrypted,
            encrypted.viewing_secret_encrypted
        );
    }
    
    #[tokio::test]
    async fn test_load_nonexistent_keypair_fails() {
        let storage = InMemoryStorage::new(b"test-device-key");
        
        let result = storage.load_keypair("nonexistent").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StealthError::StorageFailed(_)));
    }
    
    #[tokio::test]
    async fn test_delete_keypair() {
        let storage = InMemoryStorage::new(b"test-device-key");
        let keypair = StealthKeyPair::generate_standard().unwrap();
        
        // Store and verify
        storage.store_keypair("test-id", &keypair).await.unwrap();
        assert!(storage.load_keypair("test-id").await.is_ok());
        
        // Delete
        storage.delete_keypair("test-id").await.unwrap();
        
        // Verify deleted
        assert!(storage.load_keypair("test-id").await.is_err());
    }
    
    #[tokio::test]
    async fn test_list_keypairs() {
        let storage = InMemoryStorage::new(b"test-device-key");
        let keypair1 = StealthKeyPair::generate_standard().unwrap();
        let keypair2 = StealthKeyPair::generate_standard().unwrap();
        
        // Store multiple keypairs
        storage.store_keypair("id1", &keypair1).await.unwrap();
        storage.store_keypair("id2", &keypair2).await.unwrap();
        
        // List keypairs
        let mut ids = storage.list_keypairs().await.unwrap();
        ids.sort();
        
        assert_eq!(ids, vec!["id1", "id2"]);
    }
    
    #[tokio::test]
    async fn test_store_and_load_data() {
        let storage = InMemoryStorage::new(b"test-device-key");
        let data = b"sensitive data that should be encrypted";
        
        // Store data
        storage.store_data("test-key", data).await.unwrap();
        
        // Load data
        let loaded = storage.load_data("test-key").await.unwrap();
        
        assert_eq!(data.as_slice(), loaded.as_slice());
    }
    
    #[tokio::test]
    async fn test_store_data_encrypts() {
        let storage = InMemoryStorage::new(b"test-device-key");
        let data = b"sensitive data";
        
        // Store data
        storage.store_data("test-key", data).await.unwrap();
        
        // Verify stored data is encrypted (longer due to nonce + auth tag)
        let data_map = storage.data.read().await;
        let stored = data_map.get("test-key").unwrap();
        
        // Should be: 12 bytes (nonce) + data length + 16 bytes (auth tag)
        assert!(stored.len() > data.len());
        
        // Verify it's not plaintext
        assert!(!stored.windows(data.len()).any(|w| w == data));
    }
    
    #[tokio::test]
    async fn test_different_device_keys_produce_different_encryption() {
        let storage1 = InMemoryStorage::new(b"device-key-1");
        let storage2 = InMemoryStorage::new(b"device-key-2");
        let keypair = StealthKeyPair::generate_standard().unwrap();
        
        // Store with different device keys
        storage1.store_keypair("test-id", &keypair).await.unwrap();
        storage2.store_keypair("test-id", &keypair).await.unwrap();
        
        // Get encrypted data
        let keypairs1 = storage1.keypairs.read().await;
        let keypairs2 = storage2.keypairs.read().await;
        let encrypted1 = keypairs1.get("test-id").unwrap();
        let encrypted2 = keypairs2.get("test-id").unwrap();
        
        // Encrypted data should be different (different keys)
        assert_ne!(
            encrypted1.spending_secret_encrypted,
            encrypted2.spending_secret_encrypted
        );
    }
    
    #[tokio::test]
    async fn test_key_separation_spending_and_viewing() {
        let storage = InMemoryStorage::new(b"test-device-key");
        let keypair = StealthKeyPair::generate_standard().unwrap();
        
        // Store keypair
        storage.store_keypair("test-id", &keypair).await.unwrap();
        
        // Verify spending and viewing keys are encrypted separately
        let keypairs = storage.keypairs.read().await;
        let encrypted = keypairs.get("test-id").unwrap();
        
        // Different nonces for spending and viewing
        assert_ne!(encrypted.spending_nonce, encrypted.viewing_nonce);
        
        // Different ciphertexts for spending and viewing
        assert_ne!(
            encrypted.spending_secret_encrypted,
            encrypted.viewing_secret_encrypted
        );
    }
}
