use chrono::Utc;
use uuid::Uuid;

use crate::error::ApiError;
use crate::mesh_types::{PriceData, PriceUpdate};

/// Validator for price update messages
/// 
/// Validates that price updates meet all requirements before processing:
/// - All required fields are present
/// - Price values are valid (positive, non-zero)
/// - Timestamps are not in the future
/// - Source node IDs are valid UUIDs
/// 
/// Requirements: 14.1, 14.2, 14.3, 14.4, 14.5
pub struct PriceUpdateValidator;

impl PriceUpdateValidator {
    /// Validate a price update message
    /// 
    /// Returns Ok(()) if the update is valid, or an error describing the validation failure.
    /// All validation failures are logged with the source node ID for security monitoring.
    /// 
    /// Requirements: 14.1, 14.2, 14.3, 14.4, 14.5
    pub fn validate(update: &PriceUpdate) -> Result<(), ApiError> {
        // Validate required fields are present (message_id, source_node_id, timestamp, prices, ttl)
        // Note: These are enforced by the type system, but we validate their values
        
        // Validate source node ID is not nil UUID
        if update.source_node_id == Uuid::nil() {
            let error_msg = "Invalid source node ID: nil UUID";
            tracing::error!(
                source_node_id = %update.source_node_id,
                message_id = %update.message_id,
                "{}",
                error_msg
            );
            return Err(ApiError::InvalidPriceUpdate(error_msg.to_string()));
        }
        
        // Validate timestamp is not in the future
        let now = Utc::now();
        if update.timestamp > now {
            let error_msg = format!(
                "Invalid timestamp: future timestamp (update: {}, now: {})",
                update.timestamp, now
            );
            tracing::error!(
                source_node_id = %update.source_node_id,
                message_id = %update.message_id,
                timestamp = %update.timestamp,
                "{}",
                error_msg
            );
            return Err(ApiError::InvalidPriceUpdate(error_msg));
        }
        
        // Validate prices map is not empty
        if update.prices.is_empty() {
            let error_msg = "Invalid price update: prices map is empty";
            tracing::error!(
                source_node_id = %update.source_node_id,
                message_id = %update.message_id,
                "{}",
                error_msg
            );
            return Err(ApiError::InvalidPriceUpdate(error_msg.to_string()));
        }
        
        // Validate each price data entry
        for (asset, price_data) in &update.prices {
            Self::validate_price_data(asset, price_data, &update.source_node_id, &update.message_id)?;
        }
        
        Ok(())
    }
    
    /// Validate a single price data entry
    /// 
    /// Requirements: 14.2, 14.3, 14.5
    fn validate_price_data(
        asset: &str,
        price_data: &PriceData,
        source_node_id: &Uuid,
        message_id: &Uuid,
    ) -> Result<(), ApiError> {
        // Validate asset name is not empty
        if asset.is_empty() || price_data.asset.is_empty() {
            let error_msg = "Invalid price data: empty asset name";
            tracing::error!(
                source_node_id = %source_node_id,
                message_id = %message_id,
                asset = %asset,
                "{}",
                error_msg
            );
            return Err(ApiError::InvalidPriceUpdate(error_msg.to_string()));
        }
        
        // Validate price is a valid positive number
        match price_data.price.parse::<f64>() {
            Ok(price_value) => {
                if price_value <= 0.0 {
                    let error_msg = format!(
                        "Invalid price for asset {}: price must be positive (got: {})",
                        asset, price_value
                    );
                    tracing::error!(
                        source_node_id = %source_node_id,
                        message_id = %message_id,
                        asset = %asset,
                        price = %price_data.price,
                        "{}",
                        error_msg
                    );
                    return Err(ApiError::InvalidPriceUpdate(error_msg));
                }
                
                if !price_value.is_finite() {
                    let error_msg = format!(
                        "Invalid price for asset {}: price must be finite (got: {})",
                        asset, price_value
                    );
                    tracing::error!(
                        source_node_id = %source_node_id,
                        message_id = %message_id,
                        asset = %asset,
                        price = %price_data.price,
                        "{}",
                        error_msg
                    );
                    return Err(ApiError::InvalidPriceUpdate(error_msg));
                }
            }
            Err(_) => {
                let error_msg = format!(
                    "Invalid price for asset {}: not a valid number (got: {})",
                    asset, price_data.price
                );
                tracing::error!(
                    source_node_id = %source_node_id,
                    message_id = %message_id,
                    asset = %asset,
                    price = %price_data.price,
                    "{}",
                    error_msg
                );
                return Err(ApiError::InvalidPriceUpdate(error_msg));
            }
        }
        
        // Validate blockchain is not empty
        if price_data.blockchain.is_empty() {
            let error_msg = format!("Invalid price data for asset {}: empty blockchain", asset);
            tracing::error!(
                source_node_id = %source_node_id,
                message_id = %message_id,
                asset = %asset,
                "{}",
                error_msg
            );
            return Err(ApiError::InvalidPriceUpdate(error_msg));
        }
        
        // Validate change_24h if present
        if let Some(change) = &price_data.change_24h {
            if !change.is_empty() {
                if let Err(_) = change.parse::<f64>() {
                    let error_msg = format!(
                        "Invalid change_24h for asset {}: not a valid number (got: {})",
                        asset, change
                    );
                    tracing::error!(
                        source_node_id = %source_node_id,
                        message_id = %message_id,
                        asset = %asset,
                        change_24h = %change,
                        "{}",
                        error_msg
                    );
                    return Err(ApiError::InvalidPriceUpdate(error_msg));
                }
            }
        }
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use std::collections::HashMap;

    fn create_valid_price_update() -> PriceUpdate {
        let mut prices = HashMap::new();
        prices.insert(
            "SOL".to_string(),
            PriceData {
                asset: "SOL".to_string(),
                price: "100.50".to_string(),
                blockchain: "solana".to_string(),
                change_24h: Some("5.2".to_string()),
            },
        );

        PriceUpdate {
            message_id: Uuid::new_v4(),
            source_node_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            prices,
            ttl: 10,
        }
    }

    #[test]
    fn test_validate_valid_update() {
        let update = create_valid_price_update();
        assert!(PriceUpdateValidator::validate(&update).is_ok());
    }

    #[test]
    fn test_validate_nil_source_node_id() {
        let mut update = create_valid_price_update();
        update.source_node_id = Uuid::nil();
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_future_timestamp() {
        let mut update = create_valid_price_update();
        update.timestamp = Utc::now() + Duration::hours(1);
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_empty_prices() {
        let mut update = create_valid_price_update();
        update.prices.clear();
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_negative_price() {
        let mut update = create_valid_price_update();
        update.prices.insert(
            "BTC".to_string(),
            PriceData {
                asset: "BTC".to_string(),
                price: "-100.0".to_string(),
                blockchain: "bitcoin".to_string(),
                change_24h: None,
            },
        );
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_zero_price() {
        let mut update = create_valid_price_update();
        update.prices.insert(
            "ETH".to_string(),
            PriceData {
                asset: "ETH".to_string(),
                price: "0.0".to_string(),
                blockchain: "ethereum".to_string(),
                change_24h: None,
            },
        );
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_invalid_price_format() {
        let mut update = create_valid_price_update();
        update.prices.insert(
            "USDC".to_string(),
            PriceData {
                asset: "USDC".to_string(),
                price: "not_a_number".to_string(),
                blockchain: "solana".to_string(),
                change_24h: None,
            },
        );
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_empty_asset_name() {
        let mut update = create_valid_price_update();
        update.prices.insert(
            "".to_string(),
            PriceData {
                asset: "".to_string(),
                price: "100.0".to_string(),
                blockchain: "solana".to_string(),
                change_24h: None,
            },
        );
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_empty_blockchain() {
        let mut update = create_valid_price_update();
        update.prices.insert(
            "DOT".to_string(),
            PriceData {
                asset: "DOT".to_string(),
                price: "50.0".to_string(),
                blockchain: "".to_string(),
                change_24h: None,
            },
        );
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_invalid_change_24h() {
        let mut update = create_valid_price_update();
        update.prices.insert(
            "ADA".to_string(),
            PriceData {
                asset: "ADA".to_string(),
                price: "1.5".to_string(),
                blockchain: "cardano".to_string(),
                change_24h: Some("invalid".to_string()),
            },
        );
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_infinite_price() {
        let mut update = create_valid_price_update();
        update.prices.insert(
            "MATIC".to_string(),
            PriceData {
                asset: "MATIC".to_string(),
                price: "inf".to_string(),
                blockchain: "polygon".to_string(),
                change_24h: None,
            },
        );
        
        let result = PriceUpdateValidator::validate(&update);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::InvalidPriceUpdate(_)));
    }

    #[test]
    fn test_validate_multiple_valid_prices() {
        let mut update = create_valid_price_update();
        update.prices.insert(
            "BTC".to_string(),
            PriceData {
                asset: "BTC".to_string(),
                price: "45000.50".to_string(),
                blockchain: "bitcoin".to_string(),
                change_24h: Some("-2.5".to_string()),
            },
        );
        update.prices.insert(
            "ETH".to_string(),
            PriceData {
                asset: "ETH".to_string(),
                price: "3000.25".to_string(),
                blockchain: "ethereum".to_string(),
                change_24h: Some("1.8".to_string()),
            },
        );
        
        assert!(PriceUpdateValidator::validate(&update).is_ok());
    }
}
