use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::rpc::types::*;
use crate::state::accounts::AppState;
use crate::storage::block_store::BlockStore;
use crate::types::{Address, ValidatorSet};

/// Shared state accessible by all RPC handlers.
pub struct RpcState {
    pub block_store: Arc<BlockStore>,
    pub app_state: Arc<RwLock<AppState>>,
    pub validator_set: Arc<RwLock<ValidatorSet>>,
    pub node_id: String,
    pub chain_id: String,
    /// Channel to submit transactions to the mempool.
    pub tx_submit: tokio::sync::mpsc::Sender<Vec<u8>>,
}

/// Route a JSON-RPC request to the appropriate handler.
pub async fn dispatch(state: &RpcState, req: JsonRpcRequest) -> JsonRpcResponse {
    match req.method.as_str() {
        "broadcast_tx" => handle_broadcast_tx(state, req.params, req.id).await,
        "get_block" => handle_get_block(state, req.params, req.id).await,
        "get_block_hash" => handle_get_block_hash(state, req.params, req.id).await,
        "get_account" => handle_get_account(state, req.params, req.id).await,
        "get_validators" => handle_get_validators(state, req.params, req.id).await,
        "status" => handle_status(state, req.id).await,
        "get_latest_height" => handle_get_latest_height(state, req.id).await,
        _ => JsonRpcResponse::error(req.id, ERR_METHOD_NOT_FOUND, "method not found".into()),
    }
}

/// broadcast_tx: submit a raw transaction (hex-encoded bytes).
async fn handle_broadcast_tx(state: &RpcState, params: Value, id: Value) -> JsonRpcResponse {
    let tx_hex = match params.get("tx").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return JsonRpcResponse::error(id, ERR_INVALID_PARAMS, "missing 'tx' param".into()),
    };

    let tx_bytes = match hex_decode(tx_hex) {
        Some(b) => b,
        None => return JsonRpcResponse::error(id, ERR_INVALID_PARAMS, "invalid hex".into()),
    };

    match state.tx_submit.try_send(tx_bytes) {
        Ok(_) => JsonRpcResponse::success(id, json!({"status": "accepted"})),
        Err(_) => JsonRpcResponse::error(id, ERR_INTERNAL, "mempool full".into()),
    }
}

/// get_block: retrieve a block by height.
async fn handle_get_block(state: &RpcState, params: Value, id: Value) -> JsonRpcResponse {
    let height = match params.get("height").and_then(|v| v.as_u64()) {
        Some(h) => h,
        None => return JsonRpcResponse::error(id, ERR_INVALID_PARAMS, "missing 'height'".into()),
    };

    match state.block_store.load_block(height) {
        Ok(Some(block)) => {
            let block_json = json!({
                "height": block.header.height,
                "timestamp_ms": block.header.timestamp_ms,
                "proposer": hex_encode(&block.header.proposer.0),
                "prev_block_hash": hex_encode(&block.header.prev_block_hash.0),
                "state_root": hex_encode(&block.header.state_root.0),
                "tx_merkle_root": hex_encode(&block.header.tx_merkle_root.0),
                "validator_set_hash": hex_encode(&block.header.validator_set_hash.0),
                "num_txs": block.txs.len(),
            });
            JsonRpcResponse::success(id, block_json)
        }
        Ok(None) => JsonRpcResponse::error(id, ERR_BLOCK_NOT_FOUND, "block not found".into()),
        Err(e) => JsonRpcResponse::error(id, ERR_INTERNAL, format!("storage error: {}", e)),
    }
}

/// get_block_hash: retrieve block hash by height.
async fn handle_get_block_hash(state: &RpcState, params: Value, id: Value) -> JsonRpcResponse {
    let height = match params.get("height").and_then(|v| v.as_u64()) {
        Some(h) => h,
        None => return JsonRpcResponse::error(id, ERR_INVALID_PARAMS, "missing 'height'".into()),
    };

    match state.block_store.load_block_hash(height) {
        Ok(Some(hash)) => JsonRpcResponse::success(id, json!({"hash": hex_encode(&hash.0)})),
        Ok(None) => JsonRpcResponse::error(id, ERR_BLOCK_NOT_FOUND, "block not found".into()),
        Err(e) => JsonRpcResponse::error(id, ERR_INTERNAL, format!("storage error: {}", e)),
    }
}

/// get_account: query account state by address (hex).
async fn handle_get_account(state: &RpcState, params: Value, id: Value) -> JsonRpcResponse {
    let addr_hex = match params.get("address").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => return JsonRpcResponse::error(id, ERR_INVALID_PARAMS, "missing 'address'".into()),
    };

    let addr_bytes = match hex_decode(addr_hex) {
        Some(b) if b.len() == 20 => {
            let mut a = [0u8; 20];
            a.copy_from_slice(&b);
            Address(a)
        }
        _ => return JsonRpcResponse::error(id, ERR_INVALID_PARAMS, "invalid address hex (20 bytes)".into()),
    };

    let app = state.app_state.read().await;
    let acc = app.get_account(addr_bytes);
    let acc_json = json!({
        "address": hex_encode(&acc.address.0),
        "balance": acc.balance.to_string(),
        "nonce": acc.nonce,
        "code_hash": acc.code_hash.map(|h| hex_encode(&h.0)),
        "storage_root": hex_encode(&acc.storage_root.0),
    });
    JsonRpcResponse::success(id, acc_json)
}

/// get_validators: return current validator set.
async fn handle_get_validators(state: &RpcState, _params: Value, id: Value) -> JsonRpcResponse {
    let vs = state.validator_set.read().await;
    let validators: Vec<Value> = vs
        .validators_in_order()
        .map(|v| {
            json!({
                "id": hex_encode(&v.id.0),
                "voting_power": v.voting_power,
            })
        })
        .collect();

    JsonRpcResponse::success(
        id,
        json!({
            "validators": validators,
            "total_power": vs.total_power(),
            "set_hash": hex_encode(&vs.set_hash.0),
        }),
    )
}

/// status: node info.
async fn handle_status(state: &RpcState, id: Value) -> JsonRpcResponse {
    let last_height = state.block_store.last_height().unwrap_or(0);
    JsonRpcResponse::success(
        id,
        json!({
            "node_id": state.node_id,
            "chain_id": state.chain_id,
            "latest_block_height": last_height,
        }),
    )
}

/// get_latest_height: return the last committed block height.
async fn handle_get_latest_height(state: &RpcState, id: Value) -> JsonRpcResponse {
    let last_height = state.block_store.last_height().unwrap_or(0);
    JsonRpcResponse::success(id, json!({"height": last_height}))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        let byte = u8::from_str_radix(&s[i..i + 2], 16).ok()?;
        out.push(byte);
    }
    Some(out)
}
