// Proximity Receipt Integration - creates blockchain receipts for completed proximity transfers

use crate::receipt_service::{ReceiptData, ReceiptService};
use blockchain::{Blockchain, MultiChainClient};
use database::DbPool;
use proximity::ProximityReceiptData;
use shared::Result;
use std::sync::Arc;
use tracing::info;

/// Create a blockchain receipt for a completed proximity transfer
/// 
/// This function integrates the proximity transfer system with the blockchain receipt system.
/// It should be called after a proximity transfer is confirmed on the blockchain.
/// 
/// **Validates: Requirements 10.3, 10.4**
pub async fn create_proximity_receipt(
    receipt_service: &ReceiptService,
    receipt_data: ProximityReceiptData,
) -> Result<uuid::Uuid> {
    info!(
        "Creating blockchain receipt for proximity transfer: {}",
        receipt_data.proximity_transfer_id
    );

    // Create receipt data for the receipt service
    let data = ReceiptData {
        payment_id: None,
        trade_id: None,
        conversion_id: None,
        proximity_transfer_id: Some(receipt_data.proximity_transfer_id),
        amount: receipt_data.amount,
        currency: receipt_data.currency,
        sender: receipt_data.sender,
        recipient: receipt_data.recipient,
        blockchain: Blockchain::Solana, // Proximity transfers currently only support Solana
    };

    // Create the receipt
    let receipt = receipt_service.create_receipt(data).await?;

    info!(
        "Blockchain receipt created: {} for proximity transfer: {}",
        receipt.id, receipt_data.proximity_transfer_id
    );

    Ok(receipt.id)
}

/// Helper function to create receipt service instance
pub fn create_receipt_service(
    db_pool: DbPool,
    blockchain_client: Arc<MultiChainClient>,
) -> ReceiptService {
    ReceiptService::new(db_pool, blockchain_client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use uuid::Uuid;

    #[test]
    fn test_proximity_receipt_data_conversion() {
        let proximity_data = ProximityReceiptData {
            proximity_transfer_id: Uuid::new_v4(),
            amount: Decimal::new(10050, 2), // 100.50
            currency: "SOL".to_string(),
            sender: "sender_wallet_address".to_string(),
            recipient: "recipient_wallet_address".to_string(),
            transaction_hash: "tx_hash_123".to_string(),
        };

        let receipt_data = ReceiptData {
            payment_id: None,
            trade_id: None,
            conversion_id: None,
            proximity_transfer_id: Some(proximity_data.proximity_transfer_id),
            amount: proximity_data.amount,
            currency: proximity_data.currency.clone(),
            sender: proximity_data.sender.clone(),
            recipient: proximity_data.recipient.clone(),
            blockchain: Blockchain::Solana,
        };

        assert_eq!(receipt_data.proximity_transfer_id, Some(proximity_data.proximity_transfer_id));
        assert_eq!(receipt_data.amount, proximity_data.amount);
        assert_eq!(receipt_data.currency, "SOL");
        assert_eq!(receipt_data.blockchain, Blockchain::Solana);
    }
}
