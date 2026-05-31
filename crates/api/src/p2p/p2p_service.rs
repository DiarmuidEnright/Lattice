use chrono::{DateTime, Utc};
use database::DbPool;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use shared::{Error, Result};
use tracing::info;
use uuid::Uuid;

/// P2P offer type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfferType {
    Buy,
    Sell,
}

impl OfferType {
    pub fn as_str(&self) -> &'static str {
        match self {
            OfferType::Buy => "BUY",
            OfferType::Sell => "SELL",
        }
    }
}

/// P2P offer status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OfferStatus {
    Active,
    Matched,
    Executed,
    Cancelled,
    Expired,
}

impl OfferStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OfferStatus::Active => "ACTIVE",
            OfferStatus::Matched => "MATCHED",
            OfferStatus::Executed => "EXECUTED",
            OfferStatus::Cancelled => "CANCELLED",
            OfferStatus::Expired => "EXPIRED",
        }
    }
}

/// P2P offer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2POffer {
    pub id: Uuid,
    pub user_id: Uuid,
    pub offer_type: OfferType,
    pub from_asset: String,
    pub to_asset: String,
    pub from_amount: Decimal,
    pub to_amount: Decimal,
    pub price: Decimal,
    pub status: OfferStatus,
    pub escrow_tx_hash: Option<String>,
    pub matched_with_offer_id: Option<Uuid>,
    pub is_proximity_offer: bool,
    pub acceptor_id: Option<Uuid>,
    pub accepted_at: Option<DateTime<Utc>>,
    pub conversation_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// P2P exchange record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct P2PExchange {
    pub id: Uuid,
    pub buyer_offer_id: Uuid,
    pub seller_offer_id: Uuid,
    pub buyer_user_id: Uuid,
    pub seller_user_id: Uuid,
    pub asset: String,
    pub amount: Decimal,
    pub price: Decimal,
    pub platform_fee: Decimal,
    pub transaction_hash: Option<String>,
    pub status: String,
    pub executed_at: DateTime<Utc>,
}

/// P2P exchange service
/// 
/// NOTE: This is a minimal stub implementation. Full implementation would include:
/// - Escrow wallet management
/// - Atomic swap logic for supported chains
/// - Background worker for offer matching
/// - Price-time priority matching algorithm
/// - Automatic escrow release on failures
pub struct P2PService {
    db: DbPool,
}

impl P2PService {
    pub fn new(db: DbPool) -> Self {
        info!("Initializing P2P exchange service (stub implementation)");
        Self { db }
    }

    /// Create a new P2P offer
    /// 
    /// NOTE: Full implementation would lock assets in escrow
    pub async fn create_offer(
        &self,
        user_id: Uuid,
        offer_type: OfferType,
        from_asset: String,
        to_asset: String,
        from_amount: Decimal,
        to_amount: Decimal,
        price: Decimal,
        is_proximity_offer: bool,
    ) -> Result<P2POffer> {
        info!(
            "Creating P2P {} offer for user {}: {} {} -> {} {} (proximity: {})",
            offer_type.as_str(),
            user_id,
            from_amount,
            from_asset,
            to_amount,
            to_asset,
            is_proximity_offer
        );

        let id = Uuid::new_v4();

        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let row = client
            .query_one(
                r#"
                INSERT INTO p2p_offers (
                    id, user_id, offer_type, from_asset, to_asset,
                    from_amount, to_amount, price, status, is_proximity_offer, created_at, expires_at
                )
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'ACTIVE', $9, NOW(), NOW() + INTERVAL '24 hours')
                RETURNING id, user_id, offer_type, from_asset, to_asset,
                          from_amount, to_amount, price, status, escrow_tx_hash,
                          matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                          conversation_id, created_at, expires_at
                "#,
                &[
                    &id,
                    &user_id,
                    &offer_type.as_str(),
                    &from_asset,
                    &to_asset,
                    &from_amount,
                    &to_amount,
                    &price,
                    &is_proximity_offer,
                ],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to create offer: {}", e)))?;

        self.row_to_offer(&row)
    }

    /// Get active offers
    pub async fn get_active_offers(
        &self,
        from_asset: Option<String>,
        to_asset: Option<String>,
        limit: i64,
    ) -> Result<Vec<P2POffer>> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                r#"
                SELECT id, user_id, offer_type, from_asset, to_asset,
                       from_amount, to_amount, price, status, escrow_tx_hash,
                       matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                       conversation_id, created_at, expires_at
                FROM p2p_offers
                WHERE status = 'ACTIVE'
                  AND expires_at > NOW()
                  AND ($1::TEXT IS NULL OR from_asset = $1)
                  AND ($2::TEXT IS NULL OR to_asset = $2)
                ORDER BY created_at DESC
                LIMIT $3
                "#,
                &[&from_asset, &to_asset, &limit],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch offers: {}", e)))?;

        rows.iter().map(|row| self.row_to_offer(row)).collect()
    }
    /// Get all active offers excluding the specified user's offers
    /// Used for marketplace view
    pub async fn get_marketplace_offers(
        &self,
        exclude_user_id: Uuid,
        from_asset: Option<String>,
        to_asset: Option<String>,
        limit: i64,
    ) -> Result<Vec<P2POffer>> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                r#"
                SELECT id, user_id, offer_type, from_asset, to_asset,
                       from_amount, to_amount, price, status, escrow_tx_hash,
                       matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                       conversation_id, created_at, expires_at
                FROM p2p_offers
                WHERE status = 'ACTIVE'
                  AND expires_at > NOW()
                  AND user_id != $1
                  AND ($2::TEXT IS NULL OR from_asset = $2)
                  AND ($3::TEXT IS NULL OR to_asset = $3)
                ORDER BY created_at DESC
                LIMIT $4
                "#,
                &[&exclude_user_id, &from_asset, &to_asset, &limit],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch marketplace offers: {}", e)))?;

        rows.iter().map(|row| self.row_to_offer(row)).collect()
    }



    /// Get active offers for discovered proximity peers
    pub async fn get_proximity_offers(
        &self,
        discovered_peer_ids: Vec<Uuid>,
        from_asset: Option<String>,
        to_asset: Option<String>,
        limit: i64,
    ) -> Result<Vec<P2POffer>> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                r#"
                SELECT id, user_id, offer_type, from_asset, to_asset,
                       from_amount, to_amount, price, status, escrow_tx_hash,
                       matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                       conversation_id, created_at, expires_at
                FROM p2p_offers
                WHERE status = 'ACTIVE'
                  AND expires_at > NOW()
                  AND is_proximity_offer = TRUE
                  AND user_id = ANY($1)
                  AND ($2::TEXT IS NULL OR from_asset = $2)
                  AND ($3::TEXT IS NULL OR to_asset = $3)
                ORDER BY created_at DESC
                LIMIT $4
                "#,
                &[&discovered_peer_ids, &from_asset, &to_asset, &limit],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch proximity offers: {}", e)))?;

        rows.iter().map(|row| self.row_to_offer(row)).collect()
    }

    /// Get active offers with proximity priority
    /// Proximity offers from discovered peers are shown first
    pub async fn get_offers_with_proximity_priority(
        &self,
        discovered_peer_ids: Vec<Uuid>,
        from_asset: Option<String>,
        to_asset: Option<String>,
        limit: i64,
    ) -> Result<Vec<P2POffer>> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                r#"
                SELECT id, user_id, offer_type, from_asset, to_asset,
                       from_amount, to_amount, price, status, escrow_tx_hash,
                       matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                       conversation_id, created_at, expires_at
                FROM p2p_offers
                WHERE status = 'ACTIVE'
                  AND expires_at > NOW()
                  AND ($1::TEXT IS NULL OR from_asset = $1)
                  AND ($2::TEXT IS NULL OR to_asset = $2)
                ORDER BY 
                  CASE 
                    WHEN is_proximity_offer = TRUE AND user_id = ANY($3) THEN 0
                    ELSE 1
                  END,
                  created_at DESC
                LIMIT $4
                "#,
                &[&from_asset, &to_asset, &discovered_peer_ids, &limit],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch offers with proximity priority: {}", e)))?;

        rows.iter().map(|row| self.row_to_offer(row)).collect()
    }

    /// Cancel an offer
    pub async fn cancel_offer(&self, offer_id: Uuid, user_id: Uuid) -> Result<()> {
        info!("Cancelling offer {} for user {}", offer_id, user_id);

        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows_updated = client
            .execute(
                r#"
                UPDATE p2p_offers
                SET status = 'CANCELLED'
                WHERE id = $1 AND user_id = $2 AND status = 'ACTIVE'
                "#,
                &[&offer_id, &user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to cancel offer: {}", e)))?;

        if rows_updated == 0 {
            return Err(Error::Internal(
                "Offer not found or already processed".to_string(),
            ));
        }

        // NOTE: Full implementation would release escrow here

        Ok(())
    }

    /// Accept an offer and create an exchange
    ///
    /// This method:
    /// 1. Validates the offer is in ACTIVE status
    /// 2. Validates the acceptor is not the creator
    /// 3. Updates offer status to MATCHED
    /// 4. Records acceptor_id and accepted_at
    /// 5. Creates an exchange record
    ///
    /// Returns the updated offer with acceptor information
    /// Accept an offer and create an exchange
    /// 
    /// This method:
    /// 1. Validates the offer is in ACTIVE status
    /// 2. Validates the acceptor is not the creator
    /// 3. Updates offer status to MATCHED
    /// 4. Records acceptor_id and accepted_at
    /// 5. Creates an exchange record
    /// 6. Initiates chat conversation
    /// 
    /// Returns the updated offer with acceptor information
    pub async fn accept_offer(
        &self,
        offer_id: Uuid,
        acceptor_id: Uuid,
        chat_service: &crate::chat_service::ChatService,
    ) -> Result<P2POffer> {
        tracing::info!(
            offer_id = %offer_id,
            acceptor_id = %acceptor_id,
            "Attempting to accept offer"
        );

        let mut client = self.db.get().await.map_err(|e| {
            tracing::error!(
                offer_id = %offer_id,
                acceptor_id = %acceptor_id,
                error = %e,
                "Failed to get database connection for offer acceptance"
            );
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        // Start transaction for atomicity
        let transaction = client.transaction().await.map_err(|e| {
            tracing::error!(
                offer_id = %offer_id,
                acceptor_id = %acceptor_id,
                error = %e,
                "Failed to start database transaction for offer acceptance"
            );
            Error::Database(format!("Failed to start transaction: {}", e))
        })?;

        // Lock the offer row and fetch it
        let offer_row = transaction
            .query_opt(
                r#"
                SELECT id, user_id, offer_type, from_asset, to_asset,
                       from_amount, to_amount, price, status, escrow_tx_hash,
                       matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                       conversation_id, created_at, expires_at
                FROM p2p_offers
                WHERE id = $1
                FOR UPDATE
                "#,
                &[&offer_id],
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    offer_id = %offer_id,
                    acceptor_id = %acceptor_id,
                    error = %e,
                    "Failed to fetch and lock offer for acceptance"
                );
                Error::Database(format!("Failed to fetch offer: {}", e))
            })?;

        let offer_row = offer_row.ok_or_else(|| {
            tracing::warn!(
                offer_id = %offer_id,
                acceptor_id = %acceptor_id,
                "Offer not found for acceptance"
            );
            Error::Internal(format!("Offer {} not found", offer_id))
        })?;

        // Parse the offer to validate
        let offer = self.row_to_offer(&offer_row)?;

        // Validate acceptor is not the creator
        if offer.user_id == acceptor_id {
            tracing::warn!(
                offer_id = %offer_id,
                user_id = %offer.user_id,
                acceptor_id = %acceptor_id,
                "Validation failed: User attempted to accept their own offer"
            );
            return Err(Error::Validation(
                "Cannot accept your own offer".to_string(),
            ));
        }

        // Validate offer is in ACTIVE status
        if offer.status != OfferStatus::Active {
            tracing::warn!(
                offer_id = %offer_id,
                acceptor_id = %acceptor_id,
                current_status = %offer.status.as_str(),
                "Validation failed: Offer is not in ACTIVE status"
            );
            return Err(Error::Validation(format!(
                "Offer is not available for acceptance (status: {})",
                offer.status.as_str()
            )));
        }

        // Validate offer has not expired
        if offer.expires_at < Utc::now() {
            tracing::warn!(
                offer_id = %offer_id,
                acceptor_id = %acceptor_id,
                expires_at = %offer.expires_at,
                "Validation failed: Offer has expired"
            );
            return Err(Error::Validation("Offer has expired".to_string()));
        }

        // Update offer status to MATCHED and record acceptor
        let updated_row = transaction
            .query_one(
                r#"
                UPDATE p2p_offers
                SET status = 'MATCHED',
                    acceptor_id = $2,
                    accepted_at = NOW()
                WHERE id = $1
                RETURNING id, user_id, offer_type, from_asset, to_asset,
                          from_amount, to_amount, price, status, escrow_tx_hash,
                          matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                          conversation_id, created_at, expires_at
                "#,
                &[&offer_id, &acceptor_id],
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    offer_id = %offer_id,
                    acceptor_id = %acceptor_id,
                    error = %e,
                    "Failed to update offer status to MATCHED"
                );
                Error::Database(format!("Failed to update offer: {}", e))
            })?;

        // Determine buyer and seller based on offer type
        let (buyer_user_id, seller_user_id) = match offer.offer_type {
            OfferType::Buy => (offer.user_id, acceptor_id),
            OfferType::Sell => (acceptor_id, offer.user_id),
        };

        // Create exchange record
        let exchange_id = Uuid::new_v4();
        transaction
            .execute(
                r#"
                INSERT INTO p2p_exchanges (
                    id, buyer_offer_id, seller_offer_id, buyer_user_id, seller_user_id,
                    asset, amount, price, platform_fee, status, executed_at
                )
                VALUES ($1, $2, $2, $3, $4, $5, $6, $7, 0, 'PENDING', NOW())
                "#,
                &[
                    &exchange_id,
                    &offer_id,
                    &buyer_user_id,
                    &seller_user_id,
                    &offer.to_asset,
                    &offer.to_amount,
                    &offer.price,
                ],
            )
            .await
            .map_err(|e| {
                tracing::error!(
                    offer_id = %offer_id,
                    acceptor_id = %acceptor_id,
                    exchange_id = %exchange_id,
                    buyer_user_id = %buyer_user_id,
                    seller_user_id = %seller_user_id,
                    error = %e,
                    "Failed to create exchange record"
                );
                Error::Database(format!("Failed to create exchange record: {}", e))
            })?;

        // Commit transaction
        transaction.commit().await.map_err(|e| {
            tracing::error!(
                offer_id = %offer_id,
                acceptor_id = %acceptor_id,
                exchange_id = %exchange_id,
                error = %e,
                "Failed to commit transaction for offer acceptance"
            );
            Error::Database(format!("Failed to commit transaction: {}", e))
        })?;

        tracing::info!(
            offer_id = %offer_id,
            acceptor_id = %acceptor_id,
            creator_id = %offer.user_id,
            exchange_id = %exchange_id,
            "Successfully accepted offer and created exchange"
        );

        // Create chat conversation (graceful failure handling)
        match chat_service
            .create_conversation_from_offer(offer_id, offer.user_id, acceptor_id)
            .await
        {
            Ok(conversation_id) => {
                tracing::info!(
                    offer_id = %offer_id,
                    conversation_id = %conversation_id,
                    creator_id = %offer.user_id,
                    acceptor_id = %acceptor_id,
                    "Successfully created conversation for accepted offer"
                );

                // Send notification to offer creator
                if let Err(e) = chat_service
                    .send_offer_notification(
                        offer.user_id,
                        acceptor_id,
                        offer_id,
                        format!("Your offer has been accepted!"),
                    )
                    .await
                {
                    tracing::error!(
                        offer_id = %offer_id,
                        creator_id = %offer.user_id,
                        acceptor_id = %acceptor_id,
                        conversation_id = %conversation_id,
                        error = %e,
                        "Failed to send offer acceptance notification"
                    );
                }
            }
            Err(e) => {
                tracing::error!(
                    offer_id = %offer_id,
                    creator_id = %offer.user_id,
                    acceptor_id = %acceptor_id,
                    error = %e,
                    "Failed to create chat conversation for offer acceptance, continuing with acceptance"
                );
                // Continue with offer acceptance even if chat fails
            }
        }

        // Return updated offer
        self.row_to_offer(&updated_row)
    }


    /// Get user's offers
    pub async fn get_user_offers(&self, user_id: Uuid, limit: i64) -> Result<Vec<P2POffer>> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                r#"
                SELECT id, user_id, offer_type, from_asset, to_asset,
                       from_amount, to_amount, price, status, escrow_tx_hash,
                       matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                       conversation_id, created_at, expires_at
                FROM p2p_offers
                WHERE user_id = $1
                ORDER BY created_at DESC
                LIMIT $2
                "#,
                &[&user_id, &limit],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch user offers: {}", e)))?;

        rows.iter().map(|row| self.row_to_offer(row)).collect()
    }
    /// Get a specific offer by ID
    pub async fn get_offer(&self, offer_id: Uuid) -> Result<P2POffer> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let row = client
            .query_opt(
                r#"
                SELECT id, user_id, offer_type, from_asset, to_asset,
                       from_amount, to_amount, price, status, escrow_tx_hash,
                       matched_with_offer_id, is_proximity_offer, acceptor_id, accepted_at,
                       conversation_id, created_at, expires_at
                FROM p2p_offers
                WHERE id = $1
                "#,
                &[&offer_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to fetch offer: {}", e)))?;

        match row {
            Some(r) => self.row_to_offer(&r),
            None => Err(Error::Internal(format!("Offer {} not found", offer_id))),
        }
    }



    /// Expire old offers (background job)
    pub async fn expire_old_offers(&self) -> Result<u64> {
        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows_updated = client
            .execute(
                r#"
                UPDATE p2p_offers
                SET status = 'EXPIRED'
                WHERE status = 'ACTIVE' AND expires_at < NOW()
                "#,
                &[],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to expire offers: {}", e)))?;

        if rows_updated > 0 {
            info!("Expired {} old P2P offers", rows_updated);
        }

        Ok(rows_updated)
    }

    /// Convert database row to P2POffer
    fn row_to_offer(&self, row: &tokio_postgres::Row) -> Result<P2POffer> {
        let offer_type_str: String = row
            .try_get("offer_type")
            .map_err(|e| Error::Database(format!("Failed to get offer_type: {}", e)))?;
        let offer_type = match offer_type_str.as_str() {
            "BUY" => OfferType::Buy,
            "SELL" => OfferType::Sell,
            _ => return Err(Error::Internal(format!("Invalid offer type: {}", offer_type_str))),
        };

        let status_str: String = row
            .try_get("status")
            .map_err(|e| Error::Database(format!("Failed to get status: {}", e)))?;
        let status = match status_str.as_str() {
            "ACTIVE" => OfferStatus::Active,
            "MATCHED" => OfferStatus::Matched,
            "EXECUTED" => OfferStatus::Executed,
            "CANCELLED" => OfferStatus::Cancelled,
            "EXPIRED" => OfferStatus::Expired,
            _ => return Err(Error::Internal(format!("Invalid status: {}", status_str))),
        };

        let created_at_systime: std::time::SystemTime = row
            .try_get("created_at")
            .map_err(|e| Error::Database(format!("Failed to get created_at: {}", e)))?;
        let expires_at_systime: std::time::SystemTime = row
            .try_get("expires_at")
            .map_err(|e| Error::Database(format!("Failed to get expires_at: {}", e)))?;

        // Handle new optional fields
        let acceptor_id: Option<Uuid> = row.try_get("acceptor_id").ok();
        let accepted_at: Option<DateTime<Utc>> = row
            .try_get::<_, Option<std::time::SystemTime>>("accepted_at")
            .ok()
            .flatten()
            .map(DateTime::<Utc>::from);
        let conversation_id: Option<Uuid> = row.try_get("conversation_id").ok();

        Ok(P2POffer {
            id: row.try_get("id").map_err(|e| Error::Database(format!("Failed to get id: {}", e)))?,
            user_id: row.try_get("user_id").map_err(|e| Error::Database(format!("Failed to get user_id: {}", e)))?,
            offer_type,
            from_asset: row.try_get("from_asset").map_err(|e| Error::Database(format!("Failed to get from_asset: {}", e)))?,
            to_asset: row.try_get("to_asset").map_err(|e| Error::Database(format!("Failed to get to_asset: {}", e)))?,
            from_amount: row.try_get("from_amount").map_err(|e| Error::Database(format!("Failed to get from_amount: {}", e)))?,
            to_amount: row.try_get("to_amount").map_err(|e| Error::Database(format!("Failed to get to_amount: {}", e)))?,
            price: row.try_get("price").map_err(|e| Error::Database(format!("Failed to get price: {}", e)))?,
            status,
            escrow_tx_hash: row.try_get("escrow_tx_hash").ok(),
            matched_with_offer_id: row.try_get("matched_with_offer_id").ok(),
            is_proximity_offer: row.try_get("is_proximity_offer").unwrap_or(false),
            acceptor_id,
            accepted_at,
            conversation_id,
            created_at: DateTime::<Utc>::from(created_at_systime),
            expires_at: DateTime::<Utc>::from(expires_at_systime),
        })
    }

    /// Get user's exchange history
    /// 
    /// NOTE: Stub implementation
    pub async fn get_user_exchanges(&self, user_id: Uuid) -> Result<Vec<P2PExchange>> {
        info!("Getting exchange history for user {}", user_id);

        let client = self.db.get().await.map_err(|e| {
            Error::Database(format!("Failed to get database connection: {}", e))
        })?;

        let rows = client
            .query(
                "SELECT id, buyer_offer_id, seller_offer_id, buyer_user_id, seller_user_id,
                        asset, amount, price, platform_fee, transaction_hash, status, executed_at
                 FROM p2p_exchanges
                 WHERE buyer_user_id = $1 OR seller_user_id = $1
                 ORDER BY executed_at DESC
                 LIMIT 50",
                &[&user_id],
            )
            .await
            .map_err(|e| Error::Database(format!("Failed to query exchanges: {}", e)))?;

        let exchanges = rows
            .iter()
            .map(|row: &tokio_postgres::Row| P2PExchange {
                id: row.get(0),
                buyer_offer_id: row.get(1),
                seller_offer_id: row.get(2),
                buyer_user_id: row.get(3),
                seller_user_id: row.get(4),
                asset: row.get(5),
                amount: row.get(6),
                price: row.get(7),
                platform_fee: row.get(8),
                transaction_hash: row.get(9),
                status: row.get(10),
                executed_at: row.get(11),
            })
            .collect();

        Ok(exchanges)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_offer_type_as_str() {
        assert_eq!(OfferType::Buy.as_str(), "BUY");
        assert_eq!(OfferType::Sell.as_str(), "SELL");
    }

    #[test]
    fn test_offer_status_as_str() {
        assert_eq!(OfferStatus::Active.as_str(), "ACTIVE");
        assert_eq!(OfferStatus::Matched.as_str(), "MATCHED");
        assert_eq!(OfferStatus::Executed.as_str(), "EXECUTED");
        assert_eq!(OfferStatus::Cancelled.as_str(), "CANCELLED");
        assert_eq!(OfferStatus::Expired.as_str(), "EXPIRED");
    }
}
