# RustBFT — RPC / API Layer

**Purpose:** Define the external API surface: endpoints, request/response formats, authentication, and rate limiting.
**Audience:** Engineers implementing the RPC server and client-facing interfaces.

---

## 1. Responsibilities

The RPC layer is responsible for:

1. **Transaction submission** — accept signed transactions from clients.
2. **State queries** — serve read-only queries about blocks, transactions, accounts, and validator sets.
3. **Node status** — expose health checks and node metadata.
4. **Metrics endpoint** — serve Prometheus metrics (delegated to observability layer).

The RPC layer MUST NOT:

- Mutate consensus state.
- Execute contracts (except read-only queries against committed state).
- Bypass the mempool for transaction submission.
- Access consensus internal state directly.

---

## 2. Transport

### 2.1 HTTP JSON-RPC

- **Protocol:** HTTP/1.1
- **Format:** JSON-RPC 2.0
- **Default port:** 26657
- **Content-Type:** `application/json`

### 2.2 WebSocket (Future)

- For event subscriptions (new blocks, transaction confirmations).
- Out of scope for MVP but the architecture accommodates it.

---

## 3. API Endpoints

### 3.1 Transaction Submission

#### `broadcast_tx`

Submit a signed transaction to the mempool.

```
Request:
{
    "jsonrpc": "2.0",
    "method": "broadcast_tx",
    "params": {
        "tx": "<hex-encoded signed transaction>"
    },
    "id": 1
}

Response (accepted):
{
    "jsonrpc": "2.0",
    "result": {
        "tx_hash": "abcdef1234...",
        "accepted": true
    },
    "id": 1
}

Response (rejected):
{
    "jsonrpc": "2.0",
    "error": {
        "code": -32001,
        "message": "invalid nonce: expected 5, got 3"
    },
    "id": 1
}
```

**Semantics:** The transaction is added to the local mempool and gossiped to peers. Acceptance does NOT guarantee inclusion in a block. The client must poll for confirmation.

#### `broadcast_tx_commit`

Submit a transaction and wait for it to be committed (with timeout).

```
Request:
{
    "jsonrpc": "2.0",
    "method": "broadcast_tx_commit",
    "params": {
        "tx": "<hex-encoded signed transaction>",
        "timeout_ms": 10000
    },
    "id": 1
}

Response (committed):
{
    "jsonrpc": "2.0",
    "result": {
        "tx_hash": "abcdef1234...",
        "height": 42,
        "tx_index": 3,
        "receipt": {
            "success": true,
            "gas_used": 21000,
            "logs": []
        }
    },
    "id": 1
}

Response (timeout):
{
    "jsonrpc": "2.0",
    "error": {
        "code": -32002,
        "message": "transaction not committed within timeout"
    },
    "id": 1
}
```

### 3.2 Block Queries

#### `get_block`

```
Request:
{
    "jsonrpc": "2.0",
    "method": "get_block",
    "params": {
        "height": 42          // optional; omit for latest
    },
    "id": 1
}

Response:
{
    "jsonrpc": "2.0",
    "result": {
        "block": {
            "header": {
                "height": 42,
                "timestamp": 1700000042000,
                "prev_block_hash": "...",
                "proposer": "validator_id_hex",
                "validator_set_hash": "...",
                "state_root": "...",
                "tx_merkle_root": "..."
            },
            "txs": ["<hex-tx-1>", "<hex-tx-2>"],
            "last_commit": {
                "height": 41,
                "signatures": [...]
            }
        }
    },
    "id": 1
}
```

#### `get_block_by_hash`

```
Request:
{
    "jsonrpc": "2.0",
    "method": "get_block_by_hash",
    "params": {
        "hash": "abcdef1234..."
    },
    "id": 1
}
```

Response format identical to `get_block`.

### 3.3 Transaction Queries

#### `get_tx`

```
Request:
{
    "jsonrpc": "2.0",
    "method": "get_tx",
    "params": {
        "hash": "tx_hash_hex"
    },
    "id": 1
}

Response:
{
    "jsonrpc": "2.0",
    "result": {
        "tx": "<hex-encoded transaction>",
        "height": 42,
        "tx_index": 3,
        "receipt": {
            "success": true,
            "gas_used": 21000,
            "logs": [
                {
                    "topic": "transfer",
                    "data": "..."
                }
            ]
        }
    },
    "id": 1
}
```

### 3.4 State Queries

#### `get_account`

```
Request:
{
    "jsonrpc": "2.0",
    "method": "get_account",
    "params": {
        "address": "0xabc...",
        "height": 42           // optional; omit for latest
    },
    "id": 1
}

Response:
{
    "jsonrpc": "2.0",
    "result": {
        "address": "0xabc...",
        "balance": "1000000",
        "nonce": 5,
        "code_hash": null,
        "height": 42
    },
    "id": 1
}
```

#### `query_contract`

Execute a read-only contract call (no state mutation, no gas charge).

```
Request:
{
    "jsonrpc": "2.0",
    "method": "query_contract",
    "params": {
        "contract": "0xdef...",
        "input": "<hex-encoded ABI call>",
        "height": 42           // optional
    },
    "id": 1
}

Response:
{
    "jsonrpc": "2.0",
    "result": {
        "output": "<hex-encoded return data>",
        "gas_used": 15000
    },
    "id": 1
}
```

### 3.5 Validator Set

#### `get_validators`

```
Request:
{
    "jsonrpc": "2.0",
    "method": "get_validators",
    "params": {
        "height": 42           // optional
    },
    "id": 1
}

Response:
{
    "jsonrpc": "2.0",
    "result": {
        "height": 42,
        "validators": [
            {
                "id": "validator_id_hex",
                "public_key": "ed25519:...",
                "voting_power": 100,
                "address": "0x..."
            }
        ],
        "total_power": 400
    },
    "id": 1
}
```

### 3.6 Node Status

#### `status`

```
Request:
{
    "jsonrpc": "2.0",
    "method": "status",
    "params": {},
    "id": 1
}

Response:
{
    "jsonrpc": "2.0",
    "result": {
        "node_id": "node_id_hex",
        "chain_id": "rustbft-testnet-1",
        "latest_height": 42,
        "latest_block_hash": "...",
        "latest_state_root": "...",
        "catching_up": false,
        "peer_count": 3,
        "mempool_size": 12,
        "consensus": {
            "height": 43,
            "round": 0,
            "step": "Prevote"
        },
        "version": "0.1.0"
    },
    "id": 1
}
```

#### `health`

```
Request:
{
    "jsonrpc": "2.0",
    "method": "health",
    "params": {},
    "id": 1
}

Response:
{
    "jsonrpc": "2.0",
    "result": {
        "healthy": true
    },
    "id": 1
}
```

#### `net_info`

```
Request:
{
    "jsonrpc": "2.0",
    "method": "net_info",
    "params": {},
    "id": 1
}

Response:
{
    "jsonrpc": "2.0",
    "result": {
        "listening": true,
        "listen_addr": "0.0.0.0:26656",
        "peers": [
            {
                "node_id": "...",
                "remote_addr": "192.168.1.11:26656",
                "is_outbound": true,
                "connected_since": 1700000000
            }
        ]
    },
    "id": 1
}
```

---

## 4. Error Codes

| Code | Message | Description |
|------|---------|-------------|
| -32700 | Parse error | Invalid JSON |
| -32600 | Invalid request | Missing required fields |
| -32601 | Method not found | Unknown RPC method |
| -32602 | Invalid params | Parameter validation failed |
| -32001 | Tx rejected | Transaction failed mempool validation |
| -32002 | Timeout | broadcast_tx_commit timed out |
| -32003 | Not found | Block/tx/account not found |
| -32004 | Height pruned | Requested height has been pruned |
| -32005 | Node not ready | Node is syncing or not yet initialized |
| -32006 | Rate limited | Too many requests |

---

## 5. Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    RPC Server                            │
│                  (Tokio + hyper)                          │
│                                                          │
│  ┌──────────────┐                                       │
│  │ HTTP Listener │  ← Client requests                   │
│  │ (port 26657) │                                       │
│  └──────┬───────┘                                       │
│         │                                                │
│  ┌──────▼───────────────────────────────────────────┐   │
│  │              Request Router                       │   │
│  │                                                   │   │
│  │  broadcast_tx ──────▶ Mempool (via channel)       │   │
│  │  get_block ─────────▶ Storage (direct read)       │   │
│  │  get_account ───────▶ State Store (direct read)   │   │
│  │  query_contract ────▶ Contract Engine (read-only) │   │
│  │  status ────────────▶ Node Status (shared state)  │   │
│  │  health ────────────▶ Health Check                │   │
│  │  get_validators ────▶ State Store (direct read)   │   │
│  └──────────────────────────────────────────────────┘   │
│                                                          │
└─────────────────────────────────────────────────────────┘
```

### Data Access Patterns

| Endpoint | Data Source | Access Type |
|----------|------------|-------------|
| `broadcast_tx` | Mempool | Write (via channel) |
| `broadcast_tx_commit` | Mempool + subscription | Write + wait |
| `get_block` | Block Store | Read (RocksDB snapshot) |
| `get_tx` | Block Store | Read |
| `get_account` | State Store | Read (RocksDB snapshot) |
| `query_contract` | State Store + Contract Engine | Read (sandboxed execution) |
| `get_validators` | State Store | Read |
| `status` | Node shared status | Read (`Arc<AtomicU64>` etc.) |
| `health` | Multiple sources | Read |

**Key rule:** Read queries use RocksDB snapshots, which do not block writes. The RPC layer never holds locks that could affect the consensus or storage write path.

---

## 6. Rate Limiting

### Per-IP Rate Limits

```
[rpc]
rate_limit_per_second = 100        # Max requests per second per IP
rate_limit_burst = 200             # Burst allowance
broadcast_rate_limit_per_second = 10  # Stricter limit for tx submission
```

Rate limiting is implemented using a token bucket algorithm per source IP.

### Global Limits

```
max_concurrent_requests = 1000
max_request_body_bytes = 1048576   # 1 MB
request_timeout_ms = 30000         # 30 seconds
```

---

## 7. CORS and Security

```toml
[rpc]
listen_addr = "127.0.0.1:26657"   # Bind to localhost by default
cors_allowed_origins = ["*"]       # Configure for production
cors_allowed_methods = ["POST"]
cors_allowed_headers = ["Content-Type"]
```

### Security Considerations

| Concern | Mitigation |
|---------|------------|
| DoS via large requests | `max_request_body_bytes` limit |
| DoS via many requests | Rate limiting per IP |
| Unauthorized tx submission | Transactions are signed; mempool validates |
| State enumeration | No wildcard queries; specific keys only |
| Internal state exposure | RPC only exposes committed state, not consensus internals |

---

## 8. Implementation Notes

- **Framework:** `hyper` (low-level HTTP) or `axum` (ergonomic, built on hyper).
- **Serialization:** `serde_json` for JSON-RPC request/response.
- **Async:** The RPC server runs in the Tokio runtime (async outer shell).
- **No consensus mutation:** The RPC server communicates with the mempool via a bounded channel for tx submission. All other operations are read-only.

---

## 9. Configuration

```toml
[rpc]
enabled = true
listen_addr = "127.0.0.1:26657"
max_concurrent_requests = 1000
max_request_body_bytes = 1048576
request_timeout_ms = 30000
rate_limit_per_second = 100
rate_limit_burst = 200
broadcast_rate_limit_per_second = 10
cors_allowed_origins = ["*"]
```

---

## Definition of Done — RPC

- [x] All endpoints specified with request/response JSON examples
- [x] Error codes defined
- [x] Architecture diagram showing data flow
- [x] Data access patterns (read vs write) documented
- [x] Rate limiting strategy defined
- [x] Security considerations addressed
- [x] Configuration parameters listed
- [x] No Rust source code included
