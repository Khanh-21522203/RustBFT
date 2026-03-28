# RPC Server

## Purpose

Exposes a JSON-RPC 2.0 HTTP API on port 26657 for clients to query node state and submit transactions. Also provides liveness and readiness probes.

## Scope

**In scope:**
- JSON-RPC 2.0 method dispatch over `POST /`
- `broadcast_tx` — submit raw hex-encoded transactions
- `get_block`, `get_block_hash` — block queries by height
- `get_account` — account state by 20-byte hex address
- `get_validators` — current validator set
- `status` — node info (node_id, chain_id, latest height)
- `get_latest_height` — convenience height query
- `GET /health` — liveness probe
- `GET /ready` — readiness probe (ready once height > 0)

**Out of scope:**
- Transaction receipts / event logs
- WebSocket subscriptions
- Authentication / TLS
- Rate limiting (config field exists, not implemented)
- `broadcast_tx_async` / `broadcast_tx_sync` variants

## Primary User Flow

1. Client sends `POST /` with JSON: `{"jsonrpc":"2.0","method":"get_block","params":{"height":42},"id":1}`.
2. `handle_jsonrpc` deserializes into `JsonRpcRequest`, checks `jsonrpc == "2.0"`.
3. `dispatch` routes by `method` string to a handler function.
4. Handler reads `Arc<RpcState>` (block_store, app_state, validator_set, node_id, chain_id, tx_submit channel).
5. Returns `JsonRpcResponse { jsonrpc: "2.0", result: Some(json), id }` or `{ error: { code, message } }`.

## System Flow

```
HTTP POST /
    │
    ▼
handle_jsonrpc (src/rpc/server.rs)
    │  checks jsonrpc == "2.0"
    ▼
dispatch (src/rpc/handlers.rs)
    ├── "broadcast_tx"       → try_send to tx_submit mpsc channel
    ├── "get_block"          → block_store.load_block(height)
    ├── "get_block_hash"     → block_store.load_block_hash(height)
    ├── "get_account"        → app_state.read().get_account(addr)
    ├── "get_validators"     → validator_set.read()
    ├── "status"             → block_store.last_height() + node_id + chain_id
    ├── "get_latest_height"  → block_store.last_height()
    └── unknown method       → ERR_METHOD_NOT_FOUND (-32601)

GET /health → always 200 + { healthy: true, latest_height }
GET /ready  → 200 if height > 0, else 503 + { ready: false }
```

## Data Model

`RpcState` (`src/rpc/handlers.rs`):
- `block_store: Arc<BlockStore>` — for block queries
- `app_state: Arc<RwLock<AppState>>` — for account queries (read lock)
- `validator_set: Arc<RwLock<ValidatorSet>>` — for validator queries (read lock)
- `node_id: String` — first 8 bytes of verify key, hex-encoded
- `chain_id: String` — from `NodeConfig.node.chain_id`
- `tx_submit: mpsc::Sender<Vec<u8>>` — capacity 1024; receives raw tx bytes from `broadcast_tx`

`JsonRpcRequest` (`src/rpc/types.rs`): `{ jsonrpc, method, params: Value, id: Value }`
`JsonRpcResponse` (`src/rpc/types.rs`): `{ jsonrpc, result?: Value, error?: { code, message }, id }`

Error codes:
- `-32700` parse error, `-32600` invalid request, `-32601` method not found, `-32602` invalid params, `-32603` internal
- `-32000` tx not found, `-32001` block not found, `-32002` account not found

## Interfaces and Contracts

`POST /` — JSON-RPC 2.0 endpoint. Request body max 1MB (`RpcConfig.max_request_size`).

**`broadcast_tx` params:** `{"tx": "<hex-encoded bytes>"}`
- Response: `{"status": "accepted"}` or error `-32603` ("mempool full") if channel full.
- Note: receiver `_rx_submit` is dropped in `main.rs` — all sends succeed until Tokio detects the closed receiver; currently sends never fail because the receiver is leaked.

**`get_block` params:** `{"height": <u64>}`
- Response: `{height, timestamp_ms, proposer (hex), prev_block_hash (hex), state_root (hex), tx_merkle_root (hex), validator_set_hash (hex), num_txs}`

**`get_block_hash` params:** `{"height": <u64>}`
- Response: `{"hash": "<hex>"}`

**`get_account` params:** `{"address": "<40-char hex>"}`
- Response: `{address, balance (decimal string), nonce, code_hash (hex or null), storage_root (hex)}`

**`get_validators` params:** *(any)*
- Response: `{validators: [{id (hex), voting_power}], total_power, set_hash (hex)}`

**`status` params:** *(any)*
- Response: `{node_id, chain_id, latest_block_height}`

**`GET /health`** → always `200 OK` with `{"healthy": true, "checks": {"storage": {"status": "ok", "latest_height": N}}}`

**`GET /ready`** → `200` if `last_height > 0`, else `503`

## Dependencies

**Internal modules:**
- `src/storage/block_store.rs` — block queries
- `src/state/accounts.rs` — `AppState` and `Account`
- `src/types/validator.rs` — `ValidatorSet`

**External crates:**
- `axum v0.7` — HTTP router, `State` extractor, `Json` extractor/responder
- `tokio` — async TCP listener, `mpsc` channel for tx submission
- `serde_json` — JSON serialization

## Failure Modes and Edge Cases

- **`broadcast_tx` receiver dropped:** `tx_submit` receiver `_rx_submit` is dropped in `main.rs` — the channel is disconnected. `try_send` returns `Err(Disconnected)` which maps to `-32603` "mempool full" for all sends.
- **Invalid hex address in `get_account`:** Returns `-32602` if not 40 hex chars or not valid hex.
- **Block not found:** Returns `-32001` error, not a null result.
- **`app_state` read lock held by block execution:** RwLock will block RPC account queries during block execution (which holds write lock via `CommandRouter`).
- **`get_block` missing `last_commit`:** `decode_block` always sets `last_commit: None` — commit signatures are not returned.
- **No input validation on `get_block` height 0:** Will call `load_block(0)` which returns `Ok(None)` → block not found error.

## Observability and Debugging

- `Metrics.rpc_requests` counter and `rpc_request_duration` histogram registered but never updated.
- No request logging.
- Debugging: add `tracing::info!` at `dispatch` entry in `src/rpc/handlers.rs`.

## Risks and Notes

- `broadcast_tx` channel receiver is dropped (`_rx_submit`) — transactions submitted via RPC are silently discarded. This is a known MVP placeholder.
- No request size enforcement beyond axum defaults — `RpcConfig.max_request_size = 1MB` is configured but not applied to the axum router.
- No CORS configuration in MVP (tower-http CORS imported but not used in `RpcServer::run`).
- No rate limiting despite `RpcConfig.rate_limit_per_second` field.

Changes:

