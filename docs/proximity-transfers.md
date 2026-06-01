# Proximity Transfers - How It Works

This document explains the proximity-transfer feature end to end, backed by the
actual code paths. It maps to three diagrams in this folder:

- `proximity-discovery.excalidraw` - how two devices find each other on a LAN
- `proximity-transfer-flow.excalidraw` - create → accept → settle (online & offline)
- `architecture.excalidraw` - the whole platform

The feature lets two people who are physically near each other (same Wi-Fi /
LAN) discover one another with **zero configuration**, then send a Solana
transfer that settles on-chain. If the network is down at accept time, the
transfer is durably queued and settles automatically once connectivity returns.

Code lives in two places:

- `crates/proximity/` - the reusable engine (discovery, mDNS, transfer state machine, settlement)
- `crates/api/src/proximity/` - the HTTP/WebSocket layer that exposes it

---

## 1. Discovery (mDNS over Wi-Fi)

Each running node both **announces itself** and **listens** for others using
mDNS (multicast DNS, the same tech behind AirPlay/Bonjour). No server, no
pairing codes.

### Announce

Every node registers a service of type `_crypto-p2p._tcp.local.` and embeds its
identity in the TXT records.

```rust
// crates/proximity/src/mdns.rs - MdnsAnnouncer::start()
let mut properties = HashMap::new();
properties.insert("user_id".to_string(),   self.user_id.clone());
properties.insert("user_tag".to_string(),  self.user_tag.clone());
properties.insert("wallet".to_string(),    self.wallet_address.clone());
properties.insert("device_id".to_string(), self.device_id.clone());
properties.insert("version".to_string(),   PROTOCOL_VERSION.to_string());

let hostname = format!("{}.local.", instance_name);
let service_info = ServiceInfo::new(
    SERVICE_TYPE,        // "_crypto-p2p._tcp.local."
    &instance_name,      // "crypto-p2p-<device_id>"
    &hostname,
    "",
    SERVICE_PORT,        // 3000 - must be non-zero to be browsable
    Some(properties),
)?
.enable_addr_auto();     // auto-advertise this host's LAN IPs

self.daemon.register(service_info.clone())?;
```

Two details that matter (both were real bugs we fixed): the SRV record **must
have a non-zero port** and **`enable_addr_auto()`** so the daemon publishes a
resolvable address - otherwise the service registers locally but is invisible
on the wire.

### Listen + parse

A listener browses the same service type and turns each resolved service into a
`DiscoveredPeer`.

```rust
// crates/proximity/src/mdns.rs - MdnsListener::parse_service_info()
// Use val_str() (NOT to_string(), which returns "key=value").
let user_tag       = properties.get("user_tag")?.val_str().to_string();
let wallet_address = properties.get("wallet")?.val_str().to_string();
let device_id      = properties.get("device_id")?.val_str().to_string();
let version        = properties.get("version").map(|v| v.val_str().to_string());

// Reject incompatible protocol versions.
if let Some(v) = version {
    if v != PROTOCOL_VERSION { return None; }
}
```

The discovery service self-filters (a node ignores its own announcement) and
ages peers out after `PEER_TIMEOUT_SECS = 30`. The result: with one node
running the nearby list is empty; start a second node and each genuinely
discovers the other. Nothing is hardcoded.

> Diagram: `proximity-discovery.excalidraw`

---

## 2. Create a transfer request

The browser posts to the API. The handler is tolerant of a missing wallet (the
dashboard may not have one connected) and resolves a sender.

```rust
// crates/api/src/proximity/proximity_handlers.rs - create_transfer()
let sender_wallet = req
    .sender_wallet
    .filter(|w| !w.trim().is_empty())
    .or_else(|| std::env::var("PROXIMITY_WALLET").ok())
    .ok_or_else(|| ApiError::ValidationError("sender_wallet is required".to_string()))?;

let transfer = state.proximity_transfer_service
    .create_transfer_request(/* sender, recipient, asset, amount */)
    .await
    .map_err(map_transfer_error)?; // validation errors -> 400 with a real message
```

`create_transfer_request` validates the amount, checks the balance, builds the
request, enforces a per-user concurrency limit (queueing if needed), and - key
for the two-user case - **persists the pending request to PostgreSQL** so the
*recipient's* node (a different process) can load and accept it.

```rust
// crates/proximity/src/transfer.rs - create_transfer_request()
if amount <= Decimal::ZERO { /* reject */ }

self.validate_sender_balance(&sender_wallet, &asset, amount).await?;

let request = TransferRequest { id: Uuid::new_v4(), status: TransferStatus::Pending, /* ... */ };

// Persist so a DIFFERENT instance (the recipient's node) can accept it.
self.persist_transfer_to_db(&request, "", TransferStatus::Pending).await?;
```

### Honest balance check

The platform never holds the user's private key, so the on-chain source of
funds in this demo is the backend's **custodial devnet signer**. The balance
check validates against the wallet that *actually pays*, not an empty connected
wallet:

```rust
// crates/proximity/src/transfer.rs - validate_sender_balance()
if is_sol {
    if let Some(signer) = &self.custodial_signer {
        let lamports = self.solana_client.get_sol_balance(&signer.pubkey().to_string()).await?;
        let balance  = Decimal::from(lamports) / Decimal::from(LAMPORTS_PER_SOL);
        let required = amount + amount * Decimal::new(1, 2); // amount + ~1% fee headroom
        if balance < required {
            return Err(ProximityError::InsufficientBalance { /* required, available */ });
        }
    }
}
```

> Diagram: `proximity-transfer-flow.excalidraw` (top half)

---

## 3. Accept and settle

The recipient accepts. The handler does two things: mark the request accepted,
then settle it with offline tolerance.

```rust
// crates/api/src/proximity/proximity_handlers.rs - accept_transfer()
state.proximity_transfer_service.accept_transfer(transfer_id).await?;

let tx_hash = state.proximity_transfer_service
    .execute_or_queue_transfer(transfer_id)
    .await?;

let message = if tx_hash.is_some() {
    "Transfer accepted and settled on-chain"
} else {
    "Transfer accepted and queued for settlement (offline); it will sync to the blockchain when connectivity returns"
};
```

`accept_transfer` first loads the request from the DB if it was created on
another node:

```rust
// crates/proximity/src/transfer.rs - accept_transfer()
self.ensure_request_loaded(request_id).await?; // load from DB if created elsewhere
// status must be Pending and not expired …
request.status = TransferStatus::Accepted;
```

### Online vs offline branch

```rust
// crates/proximity/src/transfer.rs - execute_or_queue_transfer()
if self.is_blockchain_online().await {
    // submit on-chain now -> Completed + real tx hash
    return Ok(Some(self.execute_transfer(request_id).await?));
}
// offline: durably record as PendingSettlement (no tx hash yet)
self.persist_transfer_to_db(&request_clone, "", TransferStatus::PendingSettlement).await?;
Ok(None)
```

The actual on-chain submit uses the custodial signer on a blocking thread (the
Solana RPC client is synchronous):

```rust
// crates/proximity/src/transfer.rs - execute_sol_transfer()
if let Some(signer) = &self.custodial_signer {
    let signature = tokio::task::spawn_blocking(move || {
        let rpc = client.primary_client();
        let recent_blockhash = rpc.get_latest_blockhash()?;
        let instruction = system_instruction::transfer(&from, &recipient, lamports);
        let tx = Transaction::new_signed_with_payer(&[instruction], Some(&from), &[signer.as_ref()], recent_blockhash);
        rpc.send_and_confirm_transaction_with_spinner_and_config(&tx, CommitmentConfig::confirmed(), /* ... */)
    }).await??;
    return Ok(signature.to_string());
}
```

### Offline → online catch-up

A background task periodically settles anything left in `PendingSettlement`
once the chain is reachable again. It's idempotent - it only touches rows still
pending.

```rust
// crates/proximity/src/transfer.rs - sync_pending_settlements()
if !self.is_blockchain_online().await { return Ok(0); }

// SELECT … FROM proximity_transfers WHERE status = 'PendingSettlement' ORDER BY created_at ASC
for request in pending {
    match self.execute_blockchain_transaction(&request).await {
        Ok(tx_hash) => { /* UPDATE … SET status='Completed', transaction_hash=$2 WHERE id=$1 AND status='PendingSettlement' */ }
        Err(e)      => { /* leave PendingSettlement; a future sync retries */ }
    }
}
```

`start_settlement_sync_task` spawns this on a fixed interval at startup, and
`POST /api/proximity/transfers/sync` can trigger it on demand.

> Diagram: `proximity-transfer-flow.excalidraw` (bottom half)

---

## 4. Transfer state machine

```
Pending ──accept──▶ Accepted ──online──▶ Executing ──▶ Completed (tx hash)
   │                    │
   │                    └──offline──▶ PendingSettlement ──sync(online)──▶ Completed
   ├──reject──▶ Rejected
   └──expire──▶ Expired
```

`TransferStatus` is defined in `crates/proximity/src/types.rs` and persisted as
text in the `proximity_transfers` table.

---

## 5. HTTP surface (from `crates/api/src/routes.rs`)

| Method | Path | Purpose |
|--------|------|---------|
| POST | `/api/proximity/discovery/start` | Start announcing + listening |
| POST | `/api/proximity/discovery/stop` | Stop discovery |
| GET  | `/api/proximity/peers` | List discovered nearby peers |
| POST | `/api/proximity/peers/:peer_id/block` | Block a peer |
| POST | `/api/proximity/transfers` | Create a transfer request |
| POST | `/api/proximity/transfers/:id/accept` | Accept + settle (or queue) |
| POST | `/api/proximity/transfers/:id/reject` | Reject |
| GET  | `/api/proximity/transfers/:id` | Transfer status |
| GET  | `/api/proximity/transfers/history` | History |
| POST | `/api/proximity/transfers/sync` | Force settle pending (offline catch-up) |
| GET  | `/api/proximity/events` | WebSocket: live transfer/peer events |

---

## 6. Talk track for the demo

1. **Zero-config discovery.** "Both laptops are on the same Wi-Fi. They announce
   over mDNS and find each other - no pairing, no server. One laptop alone shows
   nobody; the second appears the moment it starts."
2. **Create the transfer.** "Alice picks Bob and sends 0.01 SOL. We validate
   against the wallet that actually settles, and we persist the request so Bob's
   independent node can see it."
3. **Accept + on-chain settle.** "Bob accepts on his device. We submit a real
   devnet transaction and return the signature - verifiable on a Solana
   explorer."
4. **Offline resilience.** "If the chain is unreachable at accept time, we don't
   fail - we mark it PendingSettlement and a background task settles it the
   moment connectivity returns."
5. **Honest scope.** "The platform never stores user private keys; the demo uses
   a custodial devnet signer as the on-chain source. In production the signing
   would happen client-side."
