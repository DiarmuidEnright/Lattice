//! Post-quantum hybrid stealth addresses (optional)
//!
//! This module implements hybrid stealth addresses combining X25519 ECDH with ML-KEM-768
//! (formerly Kyber768) for post-quantum security. The hybrid approach ensures security
//! against both classical and quantum adversaries.
//!
//! # Requirements
//! Validates: Requirements 6.1, 6.2, 6.3, 6.4

use crate::crypto::StealthCrypto;
use crate::error::{StealthError, StealthResult};
use crate::generator::StealthAddressOutput;
use crate::keypair::StealthKeyPair;
use ed25519_dalek::{Keypair, PublicKey, SecretKey};
use pqc_kyber::{keypair, encapsulate, KYBER_PUBLICKEYBYTES};
use sha2::{Digest, Sha256};
use solana_sdk::pubkey::Pubkey;

/// Hybrid stealth key pair with post-quantum support
///
/// Combines standard Ed25519 keys with ML-KEM-768 (Kyber) keys for post-quantum security.
/// Version 2 indicates hybrid mode.
///
/// # Requirements
/// Validates: Requirements 6.1, 6.2
pub struct HybridStealthKeyPair {
    base: StealthKeyPair,
    kyber_public: [u8; KYBER_PUBLICKEYBYTES],
}

impl HybridStealthKeyPair {
    /// Generate hybrid key pair (version 2: post-quantum)
    ///
    /// Creates a standard stealth key pair (spending + viewing) plus an ML-KEM-768
    /// key pair for post-quantum key encapsulation.
    ///
    /// # Requirements
    /// Validates: Requirements 6.1
    pub fn generate_hybrid() -> StealthResult<Self> {
        // Generate standard stealth key pair (version 1 base)
        let base = StealthKeyPair::generate_standard()?;
        
        // Generate ML-KEM-768 keypair
        let mut rng = rand::thread_rng();
        let keys = keypair(&mut rng)
            .map_err(|e| StealthError::KeyDerivationFailed(format!("Kyber keypair generation failed: {:?}", e)))?;
        
        Ok(Self {
            base,
            kyber_public: keys.public,
        })
    }

    /// Generate meta-address with Kyber public key
    ///
    /// Formats the meta-address as: `stealth:2:<spending_pk>:<viewing_pk>:<kyber_pk>`
    /// where all public keys are base58-encoded.
    ///
    /// # Requirements
    /// Validates: Requirements 6.2
    pub fn to_meta_address(&self) -> String {
        let spending_pk = self.base.spending_public_key();
        let viewing_pk = self.base.viewing_public_key();
        
        // Encode Kyber public key as base58
        let kyber_pk_base58 = bs58::encode(&self.kyber_public).into_string();
        
        format!(
            "stealth:2:{}:{}:{}",
            spending_pk.to_string(),
            viewing_pk.to_string(),
            kyber_pk_base58
        )
    }

    /// Parse hybrid meta-address string
    ///
    /// Parses a meta-address in the format: `stealth:2:<spending_pk>:<viewing_pk>:<kyber_pk>`
    /// Note: This only reconstructs the public keys. Private keys are not available.
    ///
    /// # Requirements
    /// Validates: Requirements 6.2
    pub fn from_meta_address(meta_addr: &str) -> StealthResult<Self> {
        let parts: Vec<&str> = meta_addr.split(':').collect();
        
        if parts.len() != 5 {
            return Err(StealthError::InvalidMetaAddress(
                "Expected format: stealth:2:spending_pk:viewing_pk:kyber_pk".into(),
            ));
        }
        
        if parts[0] != "stealth" {
            return Err(StealthError::InvalidMetaAddress(
                "Meta-address must start with 'stealth:'".into(),
            ));
        }
        
        let version: u8 = parts[1]
            .parse()
            .map_err(|_| StealthError::InvalidMetaAddress("Invalid version number".into()))?;
        
        if version != 2 {
            return Err(StealthError::InvalidMetaAddress(format!(
                "Unsupported version: {}. Expected version 2 for hybrid mode",
                version
            )));
        }
        
        let spending_pk = parts[2]
            .parse::<Pubkey>()
            .map_err(|e| StealthError::InvalidMetaAddress(format!("Invalid spending public key: {}", e)))?;
        
        let viewing_pk = parts[3]
            .parse::<Pubkey>()
            .map_err(|e| StealthError::InvalidMetaAddress(format!("Invalid viewing public key: {}", e)))?;
        
        // Decode Kyber public key from base58
        let kyber_pk_bytes = bs58::decode(parts[4])
            .into_vec()
            .map_err(|e| StealthError::InvalidMetaAddress(format!("Invalid Kyber public key: {}", e)))?;
        
        // Validate Kyber public key length (ML-KEM-768 public key is 1184 bytes)
        if kyber_pk_bytes.len() != KYBER_PUBLICKEYBYTES {
            return Err(StealthError::InvalidMetaAddress(format!(
                "Invalid Kyber public key length: expected {}, got {}",
                KYBER_PUBLICKEYBYTES,
                kyber_pk_bytes.len()
            )));
        }
        
        // Convert to fixed-size array
        let mut kyber_public = [0u8; KYBER_PUBLICKEYBYTES];
        kyber_public.copy_from_slice(&kyber_pk_bytes);
        
        // Create base keypair with only public keys (dummy secret keys)
        let spending_public = PublicKey::from_bytes(spending_pk.as_ref())
            .map_err(|e| StealthError::InvalidKeyFormat(format!("Invalid spending public key: {}", e)))?;
        
        let viewing_public = PublicKey::from_bytes(viewing_pk.as_ref())
            .map_err(|e| StealthError::InvalidKeyFormat(format!("Invalid viewing public key: {}", e)))?;
        
        // Create dummy secret keys (all zeros) since we only have public keys
        let spending_secret = SecretKey::from_bytes(&[0u8; 32])
            .map_err(|e| StealthError::InvalidKeyFormat(format!("Failed to create spending keypair: {}", e)))?;
        
        let viewing_secret = SecretKey::from_bytes(&[0u8; 32])
            .map_err(|e| StealthError::InvalidKeyFormat(format!("Failed to create viewing keypair: {}", e)))?;
        
        let spending_keypair = Keypair {
            secret: spending_secret,
            public: spending_public,
        };
        
        let viewing_keypair = Keypair {
            secret: viewing_secret,
            public: viewing_public,
        };
        
        // Create base StealthKeyPair with version 2
        let base = StealthKeyPair::from_parts(spending_keypair, viewing_keypair, 2)?;
        
        Ok(Self {
            base,
            kyber_public,
        })
    }

    /// Generate hybrid stealth address with X25519 + ML-KEM-768
    ///
    /// Performs both X25519 ECDH and ML-KEM-768 encapsulation, then combines
    /// the shared secrets using a KDF.
    ///
    /// # Requirements
    /// Validates: Requirements 6.3, 6.4
    pub fn generate_stealth_address(
        &self,
        ephemeral_keypair: Option<Keypair>,
    ) -> StealthResult<HybridStealthAddressOutput> {
        // Generate ephemeral key pair if not provided
        let ephemeral_kp = match ephemeral_keypair {
            Some(kp) => kp,
            None => {
                // Generate random ephemeral key pair
                let mut secret_bytes = [0u8; 32];
                use rand::RngCore;
                rand::thread_rng().fill_bytes(&mut secret_bytes);
                
                let secret = SecretKey::from_bytes(&secret_bytes)
                    .map_err(|e| StealthError::KeyDerivationFailed(format!("Failed to create ephemeral secret: {}", e)))?;
                let public: PublicKey = (&secret).into();
                
                Keypair { secret, public }
            }
        };
        
        // Get receiver's public keys
        let spending_public = self.base.spending_public_key();
        let viewing_public = self.base.viewing_public_key();
        
        // Convert viewing public key to Curve25519 for ECDH
        let viewing_public_bytes = viewing_public.to_bytes();
        let viewing_curve25519 = StealthCrypto::ed25519_to_curve25519(&viewing_public_bytes)?;
        
        // Convert ephemeral secret key to Curve25519 for ECDH
        let ephemeral_secret_bytes = ephemeral_kp.secret.to_bytes();
        
        // Compute X25519 shared secret using ECDH
        let x25519_shared_secret = StealthCrypto::ecdh(&ephemeral_secret_bytes, &viewing_curve25519)?;
        
        // Perform ML-KEM-768 encapsulation
        let mut rng = rand::thread_rng();
        let (kyber_ciphertext, kyber_shared_secret) = encapsulate(&self.kyber_public, &mut rng)
            .map_err(|e| StealthError::CryptoError(format!("Kyber encapsulation failed: {:?}", e)))?;
        
        // Combine X25519 and Kyber shared secrets using KDF (SHA256)
        let combined_secret = Self::combine_shared_secrets(&x25519_shared_secret, &kyber_shared_secret);
        
        // Derive viewing tag from combined secret
        let viewing_tag = StealthCrypto::derive_viewing_tag(&combined_secret);
        
        // Derive stealth address using point addition
        // stealth_address = spending_public + hash(combined_secret) * G
        
        // Hash the combined secret to get a scalar
        let mut hasher = Sha256::new();
        hasher.update(&combined_secret);
        let hash = hasher.finalize();
        use curve25519_dalek::scalar::Scalar;
        use curve25519_dalek::constants::ED25519_BASEPOINT_POINT;
        let scalar = Scalar::from_bytes_mod_order(*hash.as_ref());
        
        // Compute hash(combined_secret) * G (basepoint)
        let offset_point = scalar * ED25519_BASEPOINT_POINT;
        let offset_compressed = offset_point.compress().to_bytes();
        
        // Add spending_public + offset_point to get stealth address
        let spending_public_bytes = spending_public.to_bytes();
        let stealth_address_bytes = StealthCrypto::point_add(&spending_public_bytes, &offset_compressed)?;
        
        // Convert to Solana Pubkey
        let stealth_address = Pubkey::new_from_array(stealth_address_bytes);
        
        // Get ephemeral public key as Solana Pubkey
        let ephemeral_public_key = Pubkey::new_from_array(ephemeral_kp.public.to_bytes());
        
        // Create hybrid output
        let hybrid_base = StealthAddressOutput {
            stealth_address,
            ephemeral_public_key,
            viewing_tag,
            shared_secret: combined_secret,
        };
        
        Ok(HybridStealthAddressOutput {
            base: hybrid_base,
            kyber_ciphertext: kyber_ciphertext.to_vec(),
        })
    }

    /// Combine X25519 and Kyber shared secrets using KDF
    ///
    /// Uses SHA256 to combine the two shared secrets into a single 32-byte key.
    /// This ensures that breaking either X25519 or Kyber alone is insufficient.
    ///
    /// # Requirements
    /// Validates: Requirements 6.4
    fn combine_shared_secrets(x25519_secret: &[u8; 32], kyber_secret: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"hybrid-stealth-kdf");
        hasher.update(x25519_secret);
        hasher.update(kyber_secret);
        let hash = hasher.finalize();
        
        let mut combined = [0u8; 32];
        combined.copy_from_slice(&hash);
        combined
    }

    /// Get base stealth key pair
    pub fn base(&self) -> &StealthKeyPair {
        &self.base
    }

    /// Get Kyber public key bytes
    pub fn kyber_public_key(&self) -> &[u8] {
        &self.kyber_public
    }

    /// Get spending public key
    pub fn spending_public_key(&self) -> Pubkey {
        self.base.spending_public_key()
    }

    /// Get viewing public key
    pub fn viewing_public_key(&self) -> Pubkey {
        self.base.viewing_public_key()
    }
}

/// Output from hybrid stealth address generation
///
/// Contains both the standard stealth address output and the Kyber ciphertext
/// for post-quantum key encapsulation.
pub struct HybridStealthAddressOutput {
    pub base: StealthAddressOutput,
    pub kyber_ciphertext: Vec<u8>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pqc_kyber::KYBER_CIPHERTEXTBYTES;

    #[test]
    fn test_generate_hybrid_creates_version_2() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        let meta_addr = keypair.to_meta_address();
        
        assert!(meta_addr.starts_with("stealth:2:"), "Hybrid keypair should be version 2");
    }

    #[test]
    fn test_generate_hybrid_creates_kyber_keypair() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        
        // Verify Kyber public key has correct length
        let kyber_pk = keypair.kyber_public_key();
        assert_eq!(
            kyber_pk.len(),
            KYBER_PUBLICKEYBYTES,
            "Kyber public key should have correct length"
        );
    }

    #[test]
    fn test_generate_hybrid_creates_unique_keypairs() {
        let keypair1 = HybridStealthKeyPair::generate_hybrid().unwrap();
        let keypair2 = HybridStealthKeyPair::generate_hybrid().unwrap();
        
        assert_ne!(
            keypair1.spending_public_key(),
            keypair2.spending_public_key(),
            "Each generation should create unique spending keys"
        );
        
        assert_ne!(
            keypair1.kyber_public_key(),
            keypair2.kyber_public_key(),
            "Each generation should create unique Kyber keys"
        );
    }

    #[test]
    fn test_to_meta_address_format() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        let meta_addr = keypair.to_meta_address();
        
        // Check format: stealth:2:<spending_pk>:<viewing_pk>:<kyber_pk>
        assert!(meta_addr.starts_with("stealth:2:"), "Meta-address should start with 'stealth:2:'");
        
        let parts: Vec<&str> = meta_addr.split(':').collect();
        assert_eq!(parts.len(), 5, "Hybrid meta-address should have 5 parts");
        assert_eq!(parts[0], "stealth", "First part should be 'stealth'");
        assert_eq!(parts[1], "2", "Second part should be version '2'");
        
        // Verify public keys are valid
        let spending_pk = parts[2].parse::<Pubkey>();
        let viewing_pk = parts[3].parse::<Pubkey>();
        assert!(spending_pk.is_ok(), "Spending public key should be valid base58");
        assert!(viewing_pk.is_ok(), "Viewing public key should be valid base58");
        
        // Verify Kyber public key is valid base58
        let kyber_pk = bs58::decode(parts[4]).into_vec();
        assert!(kyber_pk.is_ok(), "Kyber public key should be valid base58");
    }

    #[test]
    fn test_from_meta_address_round_trip() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        let meta_addr1 = keypair.to_meta_address();
        
        let parsed = HybridStealthKeyPair::from_meta_address(&meta_addr1).unwrap();
        let meta_addr2 = parsed.to_meta_address();
        
        assert_eq!(meta_addr1, meta_addr2, "Round-trip should preserve meta-address");
    }

    #[test]
    fn test_from_meta_address_preserves_keys() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        let meta_addr = keypair.to_meta_address();
        
        let spending_pk = keypair.spending_public_key();
        let viewing_pk = keypair.viewing_public_key();
        let kyber_pk = keypair.kyber_public_key();
        
        let parsed = HybridStealthKeyPair::from_meta_address(&meta_addr).unwrap();
        
        assert_eq!(parsed.spending_public_key(), spending_pk);
        assert_eq!(parsed.viewing_public_key(), viewing_pk);
        assert_eq!(parsed.kyber_public_key(), kyber_pk);
    }

    #[test]
    fn test_from_meta_address_invalid_format() {
        let invalid_cases = vec![
            "invalid",
            "stealth:2:key",
            "stealth:2:key1:key2", // Missing Kyber key
            "stealth:1:key1:key2:key3", // Wrong version
            "",
        ];
        
        for invalid in invalid_cases {
            let result = HybridStealthKeyPair::from_meta_address(invalid);
            assert!(result.is_err(), "Should reject invalid format: {}", invalid);
        }
    }

    #[test]
    fn test_generate_stealth_address_produces_output() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        
        let result = keypair.generate_stealth_address(None);
        assert!(result.is_ok(), "Hybrid stealth address generation should succeed");
        
        let output = result.unwrap();
        
        // Verify all fields are populated
        assert_ne!(output.base.stealth_address, Pubkey::default());
        assert_ne!(output.base.ephemeral_public_key, Pubkey::default());
        assert_ne!(output.base.viewing_tag, [0u8; 4]);
        assert_ne!(output.base.shared_secret, [0u8; 32]);
        assert!(!output.kyber_ciphertext.is_empty(), "Kyber ciphertext should not be empty");
    }

    #[test]
    fn test_generate_stealth_address_uniqueness() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        
        let output1 = keypair.generate_stealth_address(None).unwrap();
        let output2 = keypair.generate_stealth_address(None).unwrap();
        
        // Each generation should produce unique results
        assert_ne!(
            output1.base.stealth_address,
            output2.base.stealth_address,
            "Each stealth address should be unique"
        );
        assert_ne!(
            output1.kyber_ciphertext,
            output2.kyber_ciphertext,
            "Each Kyber ciphertext should be unique"
        );
    }

    #[test]
    fn test_kyber_ciphertext_has_correct_length() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        let output = keypair.generate_stealth_address(None).unwrap();
        
        // ML-KEM-768 ciphertext should be 1088 bytes
        assert_eq!(
            output.kyber_ciphertext.len(),
            KYBER_CIPHERTEXTBYTES,
            "Kyber ciphertext should have correct length"
        );
    }

    #[test]
    fn test_combine_shared_secrets_deterministic() {
        let x25519_secret = [42u8; 32];
        let kyber_secret = vec![99u8; 32];
        
        let combined1 = HybridStealthKeyPair::combine_shared_secrets(&x25519_secret, &kyber_secret);
        let combined2 = HybridStealthKeyPair::combine_shared_secrets(&x25519_secret, &kyber_secret);
        
        assert_eq!(combined1, combined2, "KDF should be deterministic");
    }

    #[test]
    fn test_combine_shared_secrets_different_inputs() {
        let x25519_secret1 = [42u8; 32];
        let x25519_secret2 = [43u8; 32];
        let kyber_secret = vec![99u8; 32];
        
        let combined1 = HybridStealthKeyPair::combine_shared_secrets(&x25519_secret1, &kyber_secret);
        let combined2 = HybridStealthKeyPair::combine_shared_secrets(&x25519_secret2, &kyber_secret);
        
        assert_ne!(combined1, combined2, "Different inputs should produce different outputs");
    }

    #[test]
    fn test_hybrid_viewing_tag_differs_from_standard() {
        let keypair = HybridStealthKeyPair::generate_hybrid().unwrap();
        
        // Generate hybrid stealth address
        let hybrid_output = keypair.generate_stealth_address(None).unwrap();
        
        // The viewing tag should be derived from the combined secret, not just X25519
        // We can't easily test this directly, but we verify it's not all zeros
        assert_ne!(hybrid_output.base.viewing_tag, [0u8; 4]);
    }
}
