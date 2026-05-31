use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use deadpool_postgres::Pool;
use rand::Rng;
use totp_lite::{totp_custom, Sha1};
use base32::{Alphabet, encode as base32_encode, decode as base32_decode};
use qrcode::QrCode;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String, // User ID
    pub exp: usize,  // Expiration time
}

pub struct JwtConfig {
    pub secret: String,
    pub expiration_hours: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub user_tag: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Invalid credentials")]
    InvalidCredentials,
    #[error("User already exists")]
    UserAlreadyExists,
    #[error("Invalid email format")]
    InvalidEmail,
    #[error("Password too weak: {0}")]
    WeakPassword(String),
    #[error("Database error: {0}")]
    DatabaseError(String),
    #[error("Password hashing error: {0}")]
    HashingError(String),
    #[error("User tag generation failed")]
    UserTagGenerationFailed,
}

impl JwtConfig {
    pub fn new(secret: String) -> Self {
        Self {
            secret,
            expiration_hours: 24,
        }
    }

    /// Generate JWT token with 24-hour expiration
    /// Requirements: 17.6
    pub fn generate_token(&self, user_id: Uuid) -> Result<String, jsonwebtoken::errors::Error> {
        let expiration = chrono::Utc::now()
            .checked_add_signed(chrono::Duration::hours(self.expiration_hours))
            .expect("valid timestamp")
            .timestamp() as usize;

        let claims = Claims {
            sub: user_id.to_string(),
            exp: expiration,
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(self.secret.as_bytes()),
        )
    }

    pub fn verify_token(&self, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
        decode::<Claims>(
            token,
            &DecodingKey::from_secret(self.secret.as_bytes()),
            &Validation::default(),
        )
        .map(|data| data.claims)
    }

    /// Hash password using Argon2
    /// Requirements: 17.4
    pub fn hash_password(&self, password: &str) -> Result<String, AuthError> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        
        argon2
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|e| AuthError::HashingError(e.to_string()))
    }

    /// Verify password against hash
    pub fn verify_password(&self, password: &str, hash: &str) -> Result<bool, AuthError> {
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|e| AuthError::HashingError(e.to_string()))?;
        
        let argon2 = Argon2::default();
        Ok(argon2.verify_password(password.as_bytes(), &parsed_hash).is_ok())
    }

    /// Validate email format
    /// Requirements: 17.3
    pub fn validate_email(&self, email: &str) -> Result<(), AuthError> {
        // Basic email validation
        if email.len() < 5 {
            return Err(AuthError::InvalidEmail);
        }
        
        let at_pos = email.find('@').ok_or(AuthError::InvalidEmail)?;
        
        // Must have at least one character before @
        if at_pos == 0 {
            return Err(AuthError::InvalidEmail);
        }
        
        // Must have at least one character after @
        if at_pos >= email.len() - 1 {
            return Err(AuthError::InvalidEmail);
        }
        
        // Must have a dot after @
        let domain = &email[at_pos + 1..];
        if !domain.contains('.') {
            return Err(AuthError::InvalidEmail);
        }
        
        Ok(())
    }

    /// Validate password strength
    /// Requirements: 17.3, 17.4
    pub fn validate_password_strength(&self, password: &str) -> Result<(), AuthError> {
        if password.len() < 8 {
            return Err(AuthError::WeakPassword("Password must be at least 8 characters".to_string()));
        }
        
        let has_uppercase = password.chars().any(|c| c.is_uppercase());
        let has_lowercase = password.chars().any(|c| c.is_lowercase());
        let has_digit = password.chars().any(|c| c.is_numeric());
        
        if !has_uppercase || !has_lowercase || !has_digit {
            return Err(AuthError::WeakPassword(
                "Password must contain uppercase, lowercase, and digit".to_string()
            ));
        }
        
        Ok(())
    }

    /// Generate unique user tag
    /// Requirements: 20.1, 20.2
    pub fn generate_user_tag(&self) -> String {
        let chars: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789".chars().collect();
        let mut rng = rand::thread_rng();
        
        let random_part: String = (0..6)
            .map(|_| chars[rng.gen_range(0..chars.len())])
            .collect();
        
        format!("Trader_{}", random_part)
    }

    /// Register a new user with minimal fields
    /// Requirements: 17.1, 17.2, 17.3
    pub async fn register_user(
        &self,
        db_pool: &Pool,
        email: &str,
        password: &str,
        wallet_address: &str,
    ) -> Result<User, AuthError> {
        // Validate email format
        self.validate_email(email)?;
        
        // Validate password strength
        self.validate_password_strength(password)?;
        
        // Hash password
        let password_hash = self.hash_password(password)?;
        
        // Generate unique user tag
        let mut user_tag = self.generate_user_tag();
        let mut attempts = 0;
        const MAX_ATTEMPTS: u32 = 10;
        
        let client = db_pool.get().await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        // Try to generate a unique user tag
        loop {
            if attempts >= MAX_ATTEMPTS {
                return Err(AuthError::UserTagGenerationFailed);
            }
            
            // Check if user tag is unique
            let tag_check = client
                .query_opt("SELECT id FROM users WHERE user_tag = $1", &[&user_tag])
                .await
                .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
            
            if tag_check.is_none() {
                break;
            }
            
            user_tag = self.generate_user_tag();
            attempts += 1;
        }
        
        // Insert user
        let row = client
            .query_one(
                "INSERT INTO users (email, password_hash, user_tag, created_at, updated_at)
                 VALUES ($1, $2, $3, NOW(), NOW())
                 RETURNING id, email, user_tag, created_at",
                &[&email, &password_hash, &user_tag],
            )
            .await
            .map_err(|e| {
                if e.to_string().contains("duplicate key") {
                    AuthError::UserAlreadyExists
                } else {
                    AuthError::DatabaseError(e.to_string())
                }
            })?;
        
        // Store wallet address
        client
            .execute(
                "INSERT INTO wallets (user_id, address, blockchain, is_primary, created_at)
                 VALUES ($1, $2, 'solana', true, NOW())",
                &[&row.get::<_, Uuid>(0), &wallet_address],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        Ok(User {
            id: row.get(0),
            email: row.get(1),
            user_tag: row.get(2),
            created_at: row.get(3),
        })
    }

    /// Login user and return JWT token
    /// Requirements: 17.6
    pub async fn login_user(
        &self,
        db_pool: &Pool,
        email: &str,
        password: &str,
    ) -> Result<(User, String), AuthError> {
        let client = db_pool.get().await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        // Get user by email
        let row = client
            .query_opt(
                "SELECT id, email, password_hash, user_tag, created_at FROM users WHERE email = $1",
                &[&email],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;
        
        let user_id: Uuid = row.get(0);
        let stored_hash: String = row.get(2);
        
        // Verify password
        if !self.verify_password(password, &stored_hash)? {
            return Err(AuthError::InvalidCredentials);
        }
        
        // Generate JWT token
        let token = self.generate_token(user_id)
            .map_err(|_| AuthError::DatabaseError("Failed to generate token".to_string()))?;
        
        let user = User {
            id: row.get(0),
            email: row.get(1),
            user_tag: row.get(3),
            created_at: row.get(4),
        };
        
        Ok((user, token))
    }

    /// Generate TOTP secret for 2FA setup
    /// Requirements: 17.5
    pub fn generate_totp_secret(&self) -> String {
        let mut secret = [0u8; 20];
        rand::thread_rng().fill(&mut secret);
        base32_encode(Alphabet::RFC4648 { padding: false }, &secret)
    }

    /// Generate QR code for TOTP setup
    /// Requirements: 17.5
    pub fn generate_totp_qr_code(
        &self,
        email: &str,
        secret: &str,
    ) -> Result<String, AuthError> {
        let issuer = "CryptoTradingPlatform";
        let otpauth_url = format!(
            "otpauth://totp/{}:{}?secret={}&issuer={}",
            issuer, email, secret, issuer
        );
        
        let code = QrCode::new(otpauth_url.as_bytes())
            .map_err(|e| AuthError::DatabaseError(format!("QR code generation failed: {}", e)))?;
        
        // Convert to SVG string
        let svg = code.render::<qrcode::render::svg::Color>()
            .min_dimensions(200, 200)
            .build();
        
        Ok(svg)
    }

    /// Enable 2FA for a user
    /// Requirements: 17.5
    pub async fn enable_2fa(
        &self,
        db_pool: &Pool,
        user_id: Uuid,
    ) -> Result<(String, String), AuthError> {
        let client = db_pool.get().await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        // Get user email
        let row = client
            .query_opt("SELECT email FROM users WHERE id = $1", &[&user_id])
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;
        
        let email: String = row.get(0);
        
        // Generate TOTP secret
        let secret = self.generate_totp_secret();
        
        // Store secret in database (not yet enabled)
        client
            .execute(
                "UPDATE users SET totp_secret = $1, totp_enabled = false WHERE id = $2",
                &[&secret, &user_id],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        // Generate QR code
        let qr_code = self.generate_totp_qr_code(&email, &secret)?;
        
        Ok((secret, qr_code))
    }

    /// Verify TOTP code and enable 2FA
    /// Requirements: 17.5
    pub async fn verify_and_enable_2fa(
        &self,
        db_pool: &Pool,
        user_id: Uuid,
        code: &str,
    ) -> Result<(), AuthError> {
        let client = db_pool.get().await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        // Get user's TOTP secret
        let row = client
            .query_opt(
                "SELECT totp_secret FROM users WHERE id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;
        
        let secret: Option<String> = row.get(0);
        let secret = secret.ok_or(AuthError::InvalidCredentials)?;
        
        // Verify TOTP code
        if !self.verify_totp_code(&secret, code)? {
            return Err(AuthError::InvalidCredentials);
        }
        
        // Enable 2FA
        client
            .execute(
                "UPDATE users SET totp_enabled = true, totp_verified_at = NOW() WHERE id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        Ok(())
    }

    /// Verify TOTP code
    /// Requirements: 17.5
    pub fn verify_totp_code(&self, secret: &str, code: &str) -> Result<bool, AuthError> {
        let decoded_secret = base32_decode(Alphabet::RFC4648 { padding: false }, secret)
            .ok_or(AuthError::InvalidCredentials)?;
        
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?
            .as_secs();
        
        // Check current time step and one step before/after for clock skew
        for time_offset in [-1, 0, 1] {
            let time_step = (current_time as i64 + (time_offset * 30)) as u64;
            let expected_code = totp_custom::<Sha1>(30, 6, &decoded_secret, time_step);
            
            if code == expected_code {
                return Ok(true);
            }
        }
        
        Ok(false)
    }

    /// Disable 2FA for a user
    /// Requirements: 17.5
    pub async fn disable_2fa(
        &self,
        db_pool: &Pool,
        user_id: Uuid,
        password: &str,
    ) -> Result<(), AuthError> {
        let client = db_pool.get().await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        // Get user's password hash to verify
        let row = client
            .query_opt(
                "SELECT password_hash FROM users WHERE id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;
        
        let stored_hash: String = row.get(0);
        
        // Verify password
        if !self.verify_password(password, &stored_hash)? {
            return Err(AuthError::InvalidCredentials);
        }
        
        // Disable 2FA
        client
            .execute(
                "UPDATE users SET totp_enabled = false, totp_secret = NULL, totp_verified_at = NULL WHERE id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        Ok(())
    }

    /// Check if user has 2FA enabled
    pub async fn is_2fa_enabled(
        &self,
        db_pool: &Pool,
        user_id: Uuid,
    ) -> Result<bool, AuthError> {
        let client = db_pool.get().await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        let row = client
            .query_opt(
                "SELECT totp_enabled FROM users WHERE id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;
        
        Ok(row.get(0))
    }

    /// Verify password for a user by user_id
    pub async fn verify_user_password(
        &self,
        db_pool: &Pool,
        user_id: Uuid,
        password: &str,
    ) -> Result<bool, AuthError> {
        let client = db_pool.get().await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?;
        
        // Get user's password hash
        let row = client
            .query_opt(
                "SELECT password_hash FROM users WHERE id = $1",
                &[&user_id],
            )
            .await
            .map_err(|e| AuthError::DatabaseError(e.to_string()))?
            .ok_or(AuthError::InvalidCredentials)?;
        
        let stored_hash: String = row.get(0);
        
        // Verify password against hash. A stored hash that isn't a valid
        // Argon2 PHC string (e.g. legacy/seed/demo data) must NOT crash auth —
        // treat it as a failed verification so the caller returns "invalid
        // password" (401) rather than a 500 server error.
        let parsed_hash = match PasswordHash::new(&stored_hash) {
            Ok(h) => h,
            Err(e) => {
                tracing::warn!(
                    "Stored password hash for user {} is not a valid Argon2 hash ({}); treating as failed verification",
                    user_id, e
                );
                return Ok(false);
            }
        };
        
        let argon2 = Argon2::default();
        Ok(argon2.verify_password(password.as_bytes(), &parsed_hash).is_ok())
    }
}

/// Middleware to verify JWT tokens
pub async fn auth_middleware(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract token from Authorization header
    let auth_header = req
        .headers()
        .get("Authorization")
        .and_then(|h| h.to_str().ok());

    if let Some(auth_header) = auth_header {
        if let Some(token) = auth_header.strip_prefix("Bearer ") {
            // In production, verify token here
            // For MVP, we'll just check if it exists
            if !token.is_empty() {
                return Ok(next.run(req).await);
            }
        }
    }

    Err(StatusCode::UNAUTHORIZED)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_verify_token() {
        let config = JwtConfig::new("test_secret".to_string());
        let user_id = Uuid::new_v4();

        let token = config.generate_token(user_id).unwrap();
        let claims = config.verify_token(&token).unwrap();

        assert_eq!(claims.sub, user_id.to_string());
    }

    #[test]
    fn test_verify_invalid_token() {
        let config = JwtConfig::new("test_secret".to_string());
        let result = config.verify_token("invalid_token");

        assert!(result.is_err());
    }

    #[test]
    fn test_password_hashing() {
        let config = JwtConfig::new("test_secret".to_string());
        let password = "TestPassword123";
        
        let hash = config.hash_password(password).unwrap();
        assert!(config.verify_password(password, &hash).unwrap());
        assert!(!config.verify_password("WrongPassword", &hash).unwrap());
    }

    #[test]
    fn test_email_validation() {
        let config = JwtConfig::new("test_secret".to_string());
        
        assert!(config.validate_email("user@example.com").is_ok());
        assert!(config.validate_email("invalid").is_err());
        assert!(config.validate_email("@example.com").is_err());
        assert!(config.validate_email("user@").is_err());
    }

    #[test]
    fn test_password_strength_validation() {
        let config = JwtConfig::new("test_secret".to_string());
        
        // Valid password
        assert!(config.validate_password_strength("Password123").is_ok());
        
        // Too short
        assert!(config.validate_password_strength("Pass1").is_err());
        
        // No uppercase
        assert!(config.validate_password_strength("password123").is_err());
        
        // No lowercase
        assert!(config.validate_password_strength("PASSWORD123").is_err());
        
        // No digit
        assert!(config.validate_password_strength("PasswordABC").is_err());
    }

    #[test]
    fn test_user_tag_generation() {
        let config = JwtConfig::new("test_secret".to_string());
        
        let tag = config.generate_user_tag();
        assert!(tag.starts_with("Trader_"));
        assert_eq!(tag.len(), 13); // "Trader_" + 6 chars
        
        // Generate multiple tags to check uniqueness
        let tag1 = config.generate_user_tag();
        let tag2 = config.generate_user_tag();
        // They might be the same by chance, but format should be correct
        assert!(tag1.starts_with("Trader_"));
        assert!(tag2.starts_with("Trader_"));
    }
}
