# RustBFT JSON-RPC API Reference

RustBFT exposes a **JSON-RPC 2.0** API over HTTP. All requests are `POST` to the RPC listen address (default `http://localhost:26657/`).

---

## Request Format

```json
{
  "jsonrpc": "2.0",
  "method": "<method-name>",
  "params": { ... },
  "id": 1
}
```

## Response Format

**Success:**
```json
{
  "jsonrpc": "2.0",
  "result": { ... },
  "id": 1
}
```

**Error:**
```json
{
  "jsonrpc": "2.0",
  "error": {
    "code": -32601,
    "message": "method not found"
  },
  "id": 1
}
```

---

## Methods

### `status`

Return node identity and latest committed height.

**Params:** none

**Response:**
```json
{
  "node_id": "a1b2c3d4e5f6a7b8",
  "chain_id": "rustbft-local-1",
  "latest_block_height": 42
}
```

**Example:**
```bash
curl -s -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"status","params":{},"id":1}'
```

---

### `get_latest_height`

Return only the latest committed block height.

**Params:** none

**Response:**
```json
{ "height": 42 }
```

**Example:**
```bash
curl -s -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_latest_height","params":{},"id":1}'
```

---

### `get_block`

Retrieve a block by height.

**Params:**
| Field | Type | Description |
|-------|------|-------------|
| `height` | integer | Block height (1-indexed) |

**Response:**
```json
{
  "height": 5,
  "timestamp_ms": 1712345678000,
  "proposer": "a1b2c3d4...",
  "prev_block_hash": "0000000000000000...",
  "state_root": "f1e2d3c4...",
  "tx_merkle_root": "abcdef01...",
  "validator_set_hash": "11223344...",
  "num_txs": 3
}
```

All hashes are 64-character lowercase hex strings (32 bytes).

**Example:**
```bash
curl -s -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_block","params":{"height":1},"id":1}'
```

**Errors:**
- `-32001` `block not found` — no block exists at the given height.

---

### `get_block_hash`

Retrieve only the block hash at a given height.

**Params:**
| Field | Type | Description |
|-------|------|-------------|
| `height` | integer | Block height |

**Response:**
```json
{ "hash": "a1b2c3d4..." }
```

**Example:**
```bash
curl -s -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_block_hash","params":{"height":1},"id":1}'
```

---

### `get_account`

Query account state by hex-encoded 20-byte address.

**Params:**
| Field | Type | Description |
|-------|------|-------------|
| `address` | string | Hex-encoded 20-byte address (40 hex chars) |

**Response:**
```json
{
  "address": "aabbccdd...",
  "balance": "1000000000000000000",
  "nonce": 7,
  "code_hash": null,
  "storage_root": "0000000000000000..."
}
```

- `balance` is returned as a decimal string (u128).
- `code_hash` is `null` for EOA accounts, hex string for contract accounts.

**Example:**
```bash
curl -s -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_account","params":{"address":"000000000000000000000000000000000000dead"},"id":1}'
```

**Errors:**
- `-32602` `invalid address hex (20 bytes)` — address is not 40 hex characters.

---

### `get_validators`

Return the current active validator set.

**Params:** none

**Response:**
```json
{
  "validators": [
    { "id": "a1b2c3d4...", "voting_power": 100 },
    { "id": "e5f6a7b8...", "voting_power": 50 }
  ],
  "total_power": 150,
  "set_hash": "deadbeef..."
}
```

`id` is a 64-character hex string (Ed25519 verify key, 32 bytes).

**Example:**
```bash
curl -s -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_validators","params":{},"id":1}'
```

---

### `broadcast_tx`

Submit a raw transaction. The transaction is forwarded to the mempool and included in a future block.

**Params:**
| Field | Type | Description |
|-------|------|-------------|
| `tx` | string | Hex-encoded raw transaction bytes |

**Response (accepted):**
```json
{ "status": "accepted" }
```

**Example:**
```bash
curl -s -X POST http://localhost:26657/ \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"broadcast_tx","params":{"tx":"0102030405..."},"id":1}'
```

**Errors:**
- `-32602` `missing 'tx' param` — `tx` field not provided.
- `-32602` `invalid hex` — `tx` is not valid hex.
- `-32603` `mempool full` — the submission channel is at capacity.

> **Note:** In the current MVP, the mempool module is a stub. Submitted transactions are accepted by the channel but not yet persisted or included in blocks. Full mempool routing is a planned feature.

---

## Error Codes

| Code | Name | Meaning |
|------|------|---------|
| `-32700` | Parse error | Invalid JSON |
| `-32600` | Invalid request | JSON-RPC structure invalid |
| `-32601` | Method not found | Unknown method name |
| `-32602` | Invalid params | Missing or malformed parameters |
| `-32603` | Internal error | Storage or internal failure |
| `-32001` | Block not found | No block at the requested height |
| `-32002` | Account not found | (reserved) |

---

## Transaction Wire Format

All transactions are binary-encoded (big-endian) with a 1-byte type prefix.

### Type `0x01` — Transfer

Transfer native tokens between accounts.

```
[0x01][from: 20 bytes][to: 20 bytes][amount: 16 bytes u128 BE]
[nonce: 8 bytes u64 BE][gas_limit: 8 bytes u64 BE]
[signature: 64 bytes Ed25519]
```

Gas cost: `base_tx(21000) + sig_verify(3000) = 24000`

### Type `0x02` — ContractDeploy

Deploy a WASM smart contract.

```
[0x02][from: 20 bytes][nonce: 8 bytes][gas_limit: 8 bytes]
[wasm_len: 4 bytes u32 BE][wasm: N bytes]
[init_input_len: 4 bytes u32 BE][init_input: M bytes]
[signature: 64 bytes]
```

Gas cost: `base_tx(21000) + sig_verify(3000) + deploy_per_byte(200) * wasm_len + vm_gas`

Contract address is deterministic: `sha256(from || nonce)[0..20]`

### Type `0x03` — ContractCall

Call a deployed WASM contract.

```
[0x03][from: 20 bytes][to: 20 bytes (contract)][nonce: 8 bytes]
[value: 16 bytes u128 BE][gas_limit: 8 bytes]
[input_len: 4 bytes u32 BE][input: N bytes]
[signature: 64 bytes]
```

Gas cost: `base_tx(21000) + sig_verify(3000) + call_base(10000) + vm_gas`

### Type `0x04` — ValidatorUpdate

Add, remove, or modify a validator's voting power.

```
[0x04][from: 20 bytes][nonce: 8 bytes][gas_limit: 8 bytes]
[validator_id: 32 bytes Ed25519 pubkey][new_power: 8 bytes u64 BE]
[action: 1 byte][signature: 64 bytes]
```

Action codes: `0x01` = Add, `0x02` = Remove, `0x03` = UpdatePower

Safety constraints enforced at EndBlock:
- At least 1 validator must remain.
- At most 150 validators.
- Total voting power change per block ≤ 1/3 of current total power.

---

## Gas Schedule

| Operation | Gas |
|-----------|-----|
| Base transaction | 21,000 |
| Signature verification | 3,000 |
| Contract deploy (per WASM byte) | 200 |
| Contract call base | 10,000 |

Chain limits (default):
- `max_block_gas`: 10,000,000
- `max_tx_bytes`: 65,536 (64 KiB)
- `max_block_bytes`: 1,048,576 (1 MiB)
