use axum::{
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::Next,
    response::Response,
};
use tracing::warn;

/// Security headers middleware
pub async fn security_headers_middleware(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let mut response = next.run(req).await;

    let headers = response.headers_mut();

    // Add security headers
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::X_FRAME_OPTIONS,
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        "X-XSS-Protection",
        HeaderValue::from_static("1; mode=block"),
    );
    headers.insert(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static("max-age=31536000; includeSubDomains"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'self'"),
    );

    Ok(response)
}

/// Input validation for wallet addresses
pub fn validate_wallet_address(address: &str) -> Result<(), String> {
    // Solana addresses are base58 encoded and typically 32-44 characters
    if address.is_empty() {
        return Err("Wallet address cannot be empty".to_string());
    }

    if address.len() < 32 || address.len() > 44 {
        return Err("Invalid wallet address length".to_string());
    }

    // Check for valid base58 characters
    let valid_chars = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    if !address.chars().all(|c| valid_chars.contains(c)) {
        return Err("Invalid characters in wallet address".to_string());
    }

    Ok(())
}

/// Sanitize user input to prevent injection attacks
pub fn sanitize_input(input: &str) -> String {
    // Remove potentially dangerous characters
    input
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-' || *c == '_' || *c == '@' || *c == '.')
        .collect()
}

/// Check for suspicious activity patterns
pub struct SuspiciousActivityDetector {
    max_trades_per_hour: usize,
    max_position_change_percent: f64,
}

impl Default for SuspiciousActivityDetector {
    fn default() -> Self {
        Self {
            max_trades_per_hour: 20,
            max_position_change_percent: 50.0,
        }
    }
}

impl SuspiciousActivityDetector {
    pub fn new(max_trades_per_hour: usize, max_position_change_percent: f64) -> Self {
        Self {
            max_trades_per_hour,
            max_position_change_percent,
        }
    }

    /// Check if trading activity is suspicious
    pub fn is_suspicious_trading(&self, trades_in_last_hour: usize) -> bool {
        if trades_in_last_hour > self.max_trades_per_hour {
            warn!(
                "Suspicious trading activity detected: {} trades in last hour (max: {})",
                trades_in_last_hour, self.max_trades_per_hour
            );
            return true;
        }
        false
    }

    /// Check if position change is suspicious
    pub fn is_suspicious_position_change(&self, change_percent: f64) -> bool {
        if change_percent.abs() > self.max_position_change_percent {
            warn!(
                "Suspicious position change detected: {}% (max: {}%)",
                change_percent, self.max_position_change_percent
            );
            return true;
        }
        false
    }
}

/// Ensure no private keys are stored
pub fn contains_private_key_pattern(text: &str) -> bool {
    // Check for common private key patterns
    let patterns = [
        "private key",
        "privatekey",
        "secret key",
        "secretkey",
        "seed phrase",
        "mnemonic",
        "recovery phrase",
    ];

    let lower_text = text.to_lowercase();
    patterns.iter().any(|pattern| lower_text.contains(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_wallet_address_valid() {
        let valid_address = "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK";
        assert!(validate_wallet_address(valid_address).is_ok());
    }

    #[test]
    fn test_validate_wallet_address_empty() {
        assert!(validate_wallet_address("").is_err());
    }

    #[test]
    fn test_validate_wallet_address_too_short() {
        assert!(validate_wallet_address("short").is_err());
    }

    #[test]
    fn test_validate_wallet_address_invalid_chars() {
        let invalid = "DYw8jCTfwHNRJhhmFcbXvVDTqWMEVFBX6ZKUmG5CNSKK!@#";
        assert!(validate_wallet_address(invalid).is_err());
    }

    #[test]
    fn test_sanitize_input() {
        let input = "user@example.com<script>alert('xss')</script>";
        let sanitized = sanitize_input(input);
        assert!(!sanitized.contains("<"));
        assert!(!sanitized.contains(">"));
        assert!(sanitized.contains("@"));
    }

    #[test]
    fn test_suspicious_activity_detector() {
        let detector = SuspiciousActivityDetector::default();

        assert!(!detector.is_suspicious_trading(10));
        assert!(detector.is_suspicious_trading(25));

        assert!(!detector.is_suspicious_position_change(30.0));
        assert!(detector.is_suspicious_position_change(60.0));
    }

    #[test]
    fn test_contains_private_key_pattern() {
        assert!(contains_private_key_pattern("My private key is: abc123"));
        assert!(contains_private_key_pattern("Store this seed phrase"));
        assert!(!contains_private_key_pattern("Public address: DYw8..."));
    }
}
