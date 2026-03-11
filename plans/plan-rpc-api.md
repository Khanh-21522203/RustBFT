# Feature: RPC / API Layer

## 1. Purpose

The RPC layer is the external interface between the blockchain node and client software. It accepts HTTP JSON-RPC 2.0 requests, routes them to the appropriate internal component (mempool, storage, contract engine), enforces rate limits, and returns structured JSON responses. It is the only module that exposes node state to the outside world and therefore applies the strictest input validation.

## 2. Responsibilities

- Serve HTTP JSON-RPC 2.0 on a configurable address (default `127.0.0.1:26657`)
- Accept and forward `broadcast_tx` and `broadcast_tx_commit` transactions to the mempool via channel
- Serve read-only queries (`get_block`, `get_block_by_hash`, `get_tx`, `get_account`, `query_contract`, `get_validators`) against committed state via `StorageReader`
- Serve node metadata endpoints: `status`, `health`, `net_info`
- Enforce per-IP token-bucket rate limiting (separate stricter limits for transaction submission)
- Validate JSON-RPC request format, return standard error codes on malformed input
- Return JSON-RPC error objects with descriptive messages on all failure paths
- For `broadcast_tx_commit`: subscribe to block commit notifications, poll until the transaction's hash appears in a committed receipt, or return timeout error

## 3. Non-Responsibilities

- Does not mutate consensus state or write to storage directly
- Does not execute contracts with side effects — `query_contract` is read-only on committed state
- Does not bypass the mempool for transaction submission
- Does not expose consensus internals (locked block, in-progress votes)
- Does not serve WebSocket subscriptions (future feature)
- Does not serve Prometheus metrics — those are on port 26660 via a separate handler

## 4. Architecture Design

```
Client (curl / SDK)
        |  HTTP POST application/json
        v
+---------------------------------------+
|   axum HTTP Server (Tokio runtime)    |
|   listen: 127.0.0.1:26657            |
|                                       |
|   Middleware stack:                   |
|     - Request size limit (1 MB)       |
|     - Per-IP rate limiter             |
|     - Request timeout (30s)           |
|     - JSON-RPC router                 |
|                                       |
|   broadcast_tx ────► Mempool channel  |
|   get_block ────────► StorageReader   |
|   get_account ──────► StorageReader   |
|   query_contract ───► ContractEngine (read-only clone)
|   status ───────────► NodeStatus (Arc<RwLock<...>>)
|   health ───────────► HealthChecker   |
|   get_validators ───► StorageReader   |
+---------------------------------------+
```

## 5. Core Data Structures (Rust)

```rust
// src/rpc/mod.rs

pub struct RpcServer {
    config: RpcConfig,
    mempool_tx: crossbeam::channel::Sender<MempoolCommand>,
    storage: StorageReader,
    contract_engine: Arc<dyn ContractEngine + Send + Sync>,
    node_status: Arc<RwLock<NodeStatus>>,
    commit_notifier: broadcast::Sender<CommittedBlock>,  // for broadcast_tx_commit
}

pub struct NodeStatus {
    pub node_id: ValidatorId,
    pub chain_id: String,
    pub latest_height: u64,
    pub latest_block_hash: Hash,
    pub latest_state_root: Hash,
    pub catching_up: bool,
    pub peer_count: usize,
    pub mempool_size: usize,
    pub consensus_height: u64,
    pub consensus_round: u32,
    pub consensus_step: String,
    pub version: String,
}

// JSON-RPC 2.0 envelope types
pub struct JsonRpcRequest {
    pub jsonrpc: String,      // must equal "2.0"
    pub method: String,
    pub params: serde_json::Value,
    pub id: serde_json::Value,
}

pub struct JsonRpcResponse<T: Serialize> {
    pub jsonrpc: String,
    pub result: Option<T>,
    pub error: Option<JsonRpcError>,
    pub id: serde_json::Value,
}

pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

// Error code constants
pub const ERR_PARSE:       i64 = -32700;
pub const ERR_INVALID_REQ: i64 = -32600;
pub const ERR_METHOD:      i64 = -32601;
pub const ERR_PARAMS:      i64 = -32602;
pub const ERR_TX_REJECTED: i64 = -32001;
pub const ERR_TIMEOUT:     i64 = -32002;
pub const ERR_NOT_FOUND:   i64 = -32003;
pub const ERR_PRUNED:      i64 = -32004;
pub const ERR_NOT_READY:   i64 = -32005;
pub const ERR_RATE_LIMIT:  i64 = -32006;
```

## 6. Public Interfaces

```rust
// src/rpc/mod.rs

pub struct RpcConfig {
    pub enabled: bool,
    pub listen_addr: SocketAddr,
    pub max_concurrent_requests: usize,   // default: 1000
    pub max_request_body_bytes: usize,    // default: 1 MB
    pub request_timeout_ms: u64,         // default: 30000
    pub rate_limit_per_second: u32,      // default: 100 (per IP)
    pub rate_limit_burst: u32,           // default: 200
    pub broadcast_rate_limit_per_second: u32,  // default: 10
    pub cors_allowed_origins: Vec<String>,
}

/// Start the RPC server in the Tokio runtime.
/// Returns a handle to update node status and send commit notifications.
pub async fn start(
    config: RpcConfig,
    mempool_tx: crossbeam::channel::Sender<MempoolCommand>,
    storage: StorageReader,
    contract_engine: Arc<dyn ContractEngine + Send + Sync>,
) -> RpcHandle;

pub struct RpcHandle {
    pub status_tx: Arc<RwLock<NodeStatus>>,
    pub commit_tx: broadcast::Sender<CommittedBlock>,
}

// Endpoint handler signatures (called by axum router):
async fn handle_broadcast_tx(req: BroadcastTxParams, ...) -> JsonRpcResponse<BroadcastTxResult>;
async fn handle_broadcast_tx_commit(req: BroadcastTxCommitParams, ...) -> JsonRpcResponse<BroadcastTxCommitResult>;
async fn handle_get_block(req: GetBlockParams, ...) -> JsonRpcResponse<GetBlockResult>;
async fn handle_get_tx(req: GetTxParams, ...) -> JsonRpcResponse<GetTxResult>;
async fn handle_get_account(req: GetAccountParams, ...) -> JsonRpcResponse<GetAccountResult>;
async fn handle_query_contract(req: QueryContractParams, ...) -> JsonRpcResponse<QueryContractResult>;
async fn handle_get_validators(req: GetValidatorsParams, ...) -> JsonRpcResponse<GetValidatorsResult>;
async fn handle_status(...) -> JsonRpcResponse<NodeStatus>;
async fn handle_health(...) -> JsonRpcResponse<HealthResult>;
async fn handle_net_info(...) -> JsonRpcResponse<NetInfoResult>;
```

## 7. Internal Algorithms

### JSON-RPC Dispatch
```
async fn dispatch(body: Bytes, server_state):
    req = serde_json::from_slice(body)?  else return parse_error()
    validate jsonrpc == "2.0"            else return invalid_request()
    match req.method:
        "broadcast_tx"        => handle_broadcast_tx(req.params)
        "broadcast_tx_commit" => handle_broadcast_tx_commit(req.params)
        "get_block"           => handle_get_block(req.params)
        "get_block_by_hash"   => handle_get_block_by_hash(req.params)
        "get_tx"              => handle_get_tx(req.params)
        "get_account"         => handle_get_account(req.params)
        "query_contract"      => handle_query_contract(req.params)
        "get_validators"      => handle_get_validators(req.params)
        "status"              => handle_status()
        "health"              => handle_health()
        "net_info"            => handle_net_info()
        _                     => method_not_found(req.method)
```

### broadcast_tx_commit
```
async fn handle_broadcast_tx_commit(params, timeout_ms):
    tx_bytes = hex_decode(params.tx)?
    tx = canonical_decode(tx_bytes)?

    // 1. Submit to mempool
    let (resp_tx, resp_rx) = oneshot::channel()
    mempool_tx.send(AddTx { tx, source: Rpc, responder: resp_tx })?
    tx_hash = resp_rx.await??   // propagate mempool rejection

    // 2. Subscribe to commit notifications
    let mut sub = commit_notifier.subscribe()

    // 3. Poll until tx is included or timeout
    let deadline = Instant::now() + Duration::from_millis(timeout_ms)
    loop:
        select!:
            _ = tokio::time::sleep_until(deadline) =>
                return Err(ERR_TIMEOUT)
            Ok(committed_block) = sub.recv() =>
                if let Some(receipt) = committed_block.receipts.get(&tx_hash):
                    return Ok(BroadcastTxCommitResult { ... })
```

### Token Bucket Rate Limiter
```
struct RateLimiter:
    buckets: DashMap<IpAddr, TokenBucket>

struct TokenBucket:
    tokens: f64
    last_refill: Instant

fn check_rate(ip: IpAddr, is_broadcast: bool) -> Result<(), ()>:
    bucket = buckets.entry(ip).or_insert(full_bucket())
    refill(bucket)
    limit = if is_broadcast: broadcast_limit else standard_limit
    if bucket.tokens >= 1.0:
        bucket.tokens -= 1.0
        Ok(())
    else:
        Err(())  // 429
```

### query_contract (read-only)
```
async fn handle_query_contract(params):
    height = params.height.unwrap_or(storage.get_latest_height())
    state_snapshot = storage.state_snapshot_at(height)?
    // Execute with read-only flag: no state writes, no gas charge
    result = contract_engine.call_read_only(
        params.contract,
        params.input,
        gas_limit: READ_ONLY_GAS_LIMIT,
        state: &state_snapshot,
    )?
    return { output: hex(result.output), gas_used: result.gas_used }
```

## 8. Persistence Model

The RPC layer is entirely stateless and has no persistent storage of its own. All data it serves comes from `StorageReader` (committed blocks and state) or in-memory `NodeStatus` (updated by the consensus and P2P layers). Rate limiter state is in-memory and resets on restart.

## 9. Concurrency Model

The RPC server runs entirely inside the Tokio async runtime. Each HTTP request is handled by an independent async task (`axum` spawns one per connection). `StorageReader` is `Send + Sync` and uses RocksDB snapshots, so concurrent reads do not block each other or the write path. The `NodeStatus` is behind an `Arc<RwLock<_>>` — read-heavy, rarely written. The `commit_notifier` is a `tokio::sync::broadcast` channel; `broadcast_tx_commit` handlers subscribe and receive commit events without any mutex.

## 10. Configuration

```toml
[rpc]
enabled = true
listen_addr = "127.0.0.1:26657"
max_concurrent_requests = 1000
max_request_body_bytes = 1048576    # 1 MB
request_timeout_ms = 30000
rate_limit_per_second = 100
rate_limit_burst = 200
broadcast_rate_limit_per_second = 10
cors_allowed_origins = ["*"]
```

## 11. Observability

- `rustbft_rpc_requests_total{method, status}` (Counter) — requests by method and HTTP status code
- `rustbft_rpc_request_duration_seconds{method}` (Histogram) — latency per method
- `rustbft_rpc_active_connections` (Gauge) — concurrent HTTP connections
- `rustbft_rpc_rate_limit_rejections_total{ip}` (Counter) — requests rejected by rate limiter
- Log DEBUG each request with `{method, remote_addr, duration_ms, status}`
- Log WARN on rate-limit rejection with `{ip, method}`
- Log WARN on `broadcast_tx_commit` timeout with `{tx_hash, timeout_ms}`

## 12. Testing Strategy

- **`test_broadcast_tx_accepted`**: post a valid signed transaction → `{"tx_hash": "...", "accepted": true}`
- **`test_broadcast_tx_invalid_signature`**: post tx with wrong signature → `ERR_TX_REJECTED` with descriptive message
- **`test_broadcast_tx_duplicate`**: post same tx twice → second returns `ERR_TX_REJECTED("duplicate")`
- **`test_broadcast_tx_commit_success`**: submit tx, fire mock commit event containing tx → response with receipt
- **`test_broadcast_tx_commit_timeout`**: submit tx, never commit → `ERR_TIMEOUT` after timeout_ms
- **`test_get_block_by_height`**: store a block, query it → block fields match
- **`test_get_block_latest`**: omit height param → returns block at latest height
- **`test_get_block_not_found`**: query height not in store → `ERR_NOT_FOUND`
- **`test_get_account_found`**: account exists in state → balance and nonce match
- **`test_get_account_at_historical_height`**: account exists at H but pruned at H-10 → `ERR_PRUNED`
- **`test_query_contract_read_only`**: query contract that tries to write state → write is silently dropped, output returned
- **`test_status_returns_consensus_info`**: update NodeStatus, call status → fields match
- **`test_health_unhealthy_no_peers`**: set peer_count=0 → health returns `{"healthy": false}`
- **`test_rate_limit_enforced`**: send 201 requests from same IP with burst=200 → 201st returns 429
- **`test_request_too_large`**: post body > max_request_body_bytes → 413
- **`test_unknown_method`**: call `"foo"` → `ERR_METHOD` with method name in message
- **`test_malformed_json`**: send invalid JSON → `ERR_PARSE`
- **`test_missing_params`**: call get_block without required height → `ERR_PARAMS`

## 13. Open Questions

- **WebSocket subscriptions**: Clients need real-time commit notifications without polling `broadcast_tx_commit`. Deferred post-MVP; architecture accommodates it via `tokio::sync::broadcast`.
- **gRPC endpoint**: Some blockchain SDKs prefer gRPC. The JSON-RPC layer is sufficient for MVP; gRPC can be added as an additional transport binding the same handler logic.
