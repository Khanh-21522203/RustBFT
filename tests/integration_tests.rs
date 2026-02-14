//! Integration tests for RustBFT.
//!
//! These tests exercise multiple modules working together end-to-end:
//!   1. Storage roundtrip (BlockStore + StateStore)
//!   2. State executor + storage pipeline
//!   3. RPC handler dispatch with real storage backend
//!   4. Consensus → Router → Storage pipeline (single-validator commit)
//!   5. Serialization roundtrip for all message types
//!   6. Crypto: sign + verify + hash consistency
//!   7. AppState snapshot/revert across contract storage deletes (M20 regression)
//!   8. P2P message encode/decode roundtrip
//!   9. WAL crash recovery simulation

use std::sync::Arc;

use crossbeam_channel::bounded;
use serde_json::json;
use tokio::sync::{mpsc, RwLock};

use RustBFT::consensus::events::{ConsensusCommand, ConsensusEvent};
use RustBFT::contracts::ContractRuntime;
use RustBFT::crypto::ed25519;
use RustBFT::crypto::hash::sha256;
use RustBFT::metrics::registry::Metrics;
use RustBFT::p2p::msg::{decode_message, encode_message, NetworkMessage};
use RustBFT::router::CommandRouter;
use RustBFT::rpc::handlers::{dispatch, RpcState};
use RustBFT::rpc::types::{JsonRpcRequest, ERR_BLOCK_NOT_FOUND, ERR_METHOD_NOT_FOUND};
use RustBFT::state::accounts::{AppState, ChainParams};
use RustBFT::state::executor::StateExecutor;
use RustBFT::storage::block_store::BlockStore;
use RustBFT::storage::state_store::StateStore;
use RustBFT::storage::wal::WAL;
use RustBFT::types::*;
use RustBFT::types::serialization::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir()
        .join("rustbft_integration_tests")
        .join(name)
        .join(format!("{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_id(seed: u8) -> ValidatorId {
    let mut id = [0u8; 32];
    id[0] = seed;
    ValidatorId(id)
}

fn make_vset(n: usize) -> (ValidatorSet, Vec<ValidatorId>) {
    let mut validators = Vec::new();
    let mut ids = Vec::new();
    for i in 0..n {
        let id = make_id(i as u8 + 1);
        ids.push(id);
        validators.push(Validator { id, voting_power: 1 });
    }
    (ValidatorSet::new(validators, sha256(&[n as u8])), ids)
}

fn make_block(height: u64, proposer: ValidatorId) -> Block {
    Block {
        header: BlockHeader {
            height,
            timestamp_ms: 1000 * height,
            prev_block_hash: Hash::ZERO,
            proposer,
            validator_set_hash: Hash::ZERO,
            state_root: Hash::ZERO,
            tx_merkle_root: Hash::ZERO,
        },
        txs: vec![],
        last_commit: None,
    }
}

fn make_signed_vote(
    vote_type: VoteType,
    height: u64,
    round: u32,
    block_hash: Option<Hash>,
    validator: ValidatorId,
) -> SignedVote {
    SignedVote {
        vote: Vote { vote_type, height, round, block_hash, validator },
        signature: [0u8; 64],
    }
}



// ===========================================================================
// 1. Storage roundtrip: BlockStore + StateStore
// ===========================================================================

#[test]
fn test_block_store_save_and_load() {
    let dir = temp_dir("block_store_roundtrip");
    let store = BlockStore::open(&dir).unwrap();
    let (vset, ids) = make_vset(3);

    let block = make_block(1, ids[0]);
    let state_root = sha256(b"state1");
    store.save_block(&block, state_root, &vset).unwrap();

    // Load block
    let loaded = store.load_block(1).unwrap().expect("block should exist");
    assert_eq!(loaded.header.height, 1);
    assert_eq!(loaded.header.proposer, ids[0]);

    // Load block hash
    let hash = store.load_block_hash(1).unwrap().expect("hash should exist");
    assert_ne!(hash, Hash::ZERO);

    // Load state root
    let sr = store.load_state_root(1).unwrap().expect("state root should exist");
    assert_eq!(sr, state_root);

    // Load validator set
    let loaded_vs = store.load_validator_set(1).unwrap().expect("vset should exist");
    assert_eq!(loaded_vs.len(), 3);
    assert_eq!(loaded_vs.total_power(), vset.total_power());

    // Last height
    assert_eq!(store.last_height().unwrap(), 1);

    // Non-existent height
    assert!(store.load_block(999).unwrap().is_none());
}

#[test]
fn test_block_store_multiple_heights() {
    let dir = temp_dir("block_store_multi");
    let store = BlockStore::open(&dir).unwrap();
    let (vset, ids) = make_vset(1);

    for h in 1..=5 {
        let block = make_block(h, ids[0]);
        store.save_block(&block, sha256(&h.to_be_bytes()), &vset).unwrap();
    }

    assert_eq!(store.last_height().unwrap(), 5);
    for h in 1..=5 {
        let b = store.load_block(h).unwrap().expect("block should exist");
        assert_eq!(b.header.height, h);
    }
}

#[test]
fn test_block_store_prune() {
    let dir = temp_dir("block_store_prune");
    let store = BlockStore::open(&dir).unwrap();
    let (vset, ids) = make_vset(1);

    for h in 1..=10 {
        store.save_block(&make_block(h, ids[0]), Hash::ZERO, &vset).unwrap();
    }

    let pruned = store.prune_below(6).unwrap();
    assert_eq!(pruned, 5);

    // Heights 1-5 should be gone
    for h in 1..=5 {
        assert!(store.load_block(h).unwrap().is_none(), "height {} should be pruned", h);
    }
    // Heights 6-10 should still exist
    for h in 6..=10 {
        assert!(store.load_block(h).unwrap().is_some(), "height {} should exist", h);
    }
}

#[test]
fn test_state_store_save_and_load() {
    let dir = temp_dir("state_store_roundtrip");
    let store = StateStore::open(&dir).unwrap();

    let mut state = AppState::new(ChainParams::default());
    let addr = Address([1u8; 20]);
    let mut acc = state.get_account(addr);
    acc.balance = 1_000_000;
    acc.nonce = 5;
    state.set_account(acc);

    store.save_state(1, &state).unwrap();

    let (h, loaded) = store.load_latest_state().unwrap().expect("state should exist");
    assert_eq!(h, 1);
    let loaded_acc = loaded.get_account(addr);
    assert_eq!(loaded_acc.balance, 1_000_000);
    assert_eq!(loaded_acc.nonce, 5);
}

// ===========================================================================
// 2. State executor + storage pipeline
// ===========================================================================

#[test]
fn test_executor_empty_block() {
    let executor = StateExecutor::new();
    let mut state = AppState::new(ChainParams::default());
    let mut contracts = ContractRuntime::new().unwrap();

    let block = make_block(1, make_id(1));
    let (state_root, val_updates) = executor.execute_block(&mut state, &mut contracts, &block).unwrap();

    assert_ne!(state_root, Hash::ZERO, "state root should be computed");
    assert!(val_updates.is_empty(), "no validator updates for empty block");
}

#[test]
fn test_executor_transfer_tx() {
    let executor = StateExecutor::new();
    let mut state = AppState::new(ChainParams::default());
    let mut contracts = ContractRuntime::new().unwrap();

    // Fund sender
    let sender = Address([1u8; 20]);
    let receiver = Address([2u8; 20]);
    let mut sender_acc = state.get_account(sender);
    sender_acc.balance = 1_000_000;
    state.set_account(sender_acc);

    // Build a transfer tx: type 0x01
    let tx_bytes = build_transfer_tx(sender, receiver, 500, 0, 100_000);

    let block = Block {
        header: BlockHeader {
            height: 1,
            timestamp_ms: 0,
            prev_block_hash: Hash::ZERO,
            proposer: make_id(1),
            validator_set_hash: Hash::ZERO,
            state_root: Hash::ZERO,
            tx_merkle_root: Hash::ZERO,
        },
        txs: vec![tx_bytes],
        last_commit: None,
    };

    let (state_root, _) = executor.execute_block(&mut state, &mut contracts, &block).unwrap();
    assert_ne!(state_root, Hash::ZERO);

    let sender_after = state.get_account(sender);
    let receiver_after = state.get_account(receiver);

    // Sender should have less balance (transfer + gas)
    assert!(sender_after.balance < 1_000_000);
    // Receiver should have 500
    assert_eq!(receiver_after.balance, 500);
    // Sender nonce should increment
    assert_eq!(sender_after.nonce, 1);
}

/// Build a raw transfer tx in canonical wire format.
fn build_transfer_tx(from: Address, to: Address, amount: u128, nonce: u64, gas_limit: u64) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.push(0x01); // tx type
    buf.extend_from_slice(&from.0);
    buf.extend_from_slice(&to.0);
    buf.extend_from_slice(&amount.to_be_bytes());
    buf.extend_from_slice(&nonce.to_be_bytes());
    buf.extend_from_slice(&gas_limit.to_be_bytes());
    buf.extend_from_slice(&[0u8; 64]); // signature placeholder
    buf
}

#[test]
fn test_executor_and_storage_pipeline() {
    let dir = temp_dir("executor_storage_pipeline");
    let block_store = BlockStore::open(&dir.join("blocks")).unwrap();
    let state_store = StateStore::open(&dir.join("state")).unwrap();

    let executor = StateExecutor::new();
    let mut state = AppState::new(ChainParams::default());
    let mut contracts = ContractRuntime::new().unwrap();
    let (vset, ids) = make_vset(1);

    // Execute block
    let block = make_block(1, ids[0]);
    let (state_root, _) = executor.execute_block(&mut state, &mut contracts, &block).unwrap();

    // Persist
    block_store.save_block(&block, state_root, &vset).unwrap();
    state_store.save_state(1, &state).unwrap();

    // Verify persistence
    assert_eq!(block_store.last_height().unwrap(), 1);
    let loaded_block = block_store.load_block(1).unwrap().unwrap();
    assert_eq!(loaded_block.header.height, 1);

    let (h, loaded_state) = state_store.load_latest_state().unwrap().unwrap();
    assert_eq!(h, 1);
    // State should be consistent
    let _ = loaded_state;
}

// ===========================================================================
// 3. RPC handler dispatch with real storage backend
// ===========================================================================

#[tokio::test]
async fn test_rpc_status() {
    let dir = temp_dir("rpc_status");
    let block_store = Arc::new(BlockStore::open(&dir).unwrap());
    let (vset, _ids) = make_vset(1);
    let (tx_submit, _rx) = mpsc::channel(1);

    let rpc_state = RpcState {
        block_store,
        app_state: Arc::new(RwLock::new(AppState::new(ChainParams::default()))),
        validator_set: Arc::new(RwLock::new(vset)),
        node_id: "test-node".to_string(),
        chain_id: "test-chain".to_string(),
        tx_submit,
    };

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "status".to_string(),
        params: json!({}),
        id: json!(1),
    };

    let resp = dispatch(&rpc_state, req).await;
    assert!(resp.error.is_none(), "status should succeed");
    let result = resp.result.unwrap();
    assert_eq!(result["node_id"], "test-node");
    assert_eq!(result["chain_id"], "test-chain");
    assert_eq!(result["latest_block_height"], 0);
}

#[tokio::test]
async fn test_rpc_get_block_not_found() {
    let dir = temp_dir("rpc_get_block_404");
    let block_store = Arc::new(BlockStore::open(&dir).unwrap());
    let (vset, _) = make_vset(1);
    let (tx_submit, _rx) = mpsc::channel(1);

    let rpc_state = RpcState {
        block_store,
        app_state: Arc::new(RwLock::new(AppState::new(ChainParams::default()))),
        validator_set: Arc::new(RwLock::new(vset)),
        node_id: "n".to_string(),
        chain_id: "c".to_string(),
        tx_submit,
    };

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "get_block".to_string(),
        params: json!({"height": 1}),
        id: json!(1),
    };

    let resp = dispatch(&rpc_state, req).await;
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, ERR_BLOCK_NOT_FOUND);
}

#[tokio::test]
async fn test_rpc_get_block_found() {
    let dir = temp_dir("rpc_get_block_found");
    let block_store = Arc::new(BlockStore::open(&dir).unwrap());
    let (vset, ids) = make_vset(1);

    let block = make_block(1, ids[0]);
    block_store.save_block(&block, Hash::ZERO, &vset).unwrap();

    let (tx_submit, _rx) = mpsc::channel(1);
    let rpc_state = RpcState {
        block_store,
        app_state: Arc::new(RwLock::new(AppState::new(ChainParams::default()))),
        validator_set: Arc::new(RwLock::new(vset)),
        node_id: "n".to_string(),
        chain_id: "c".to_string(),
        tx_submit,
    };

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "get_block".to_string(),
        params: json!({"height": 1}),
        id: json!(1),
    };

    let resp = dispatch(&rpc_state, req).await;
    assert!(resp.error.is_none(), "get_block should succeed");
    let result = resp.result.unwrap();
    assert_eq!(result["height"], 1);
    assert_eq!(result["num_txs"], 0);
}

#[tokio::test]
async fn test_rpc_get_account() {
    let dir = temp_dir("rpc_get_account");
    let block_store = Arc::new(BlockStore::open(&dir).unwrap());
    let (vset, _) = make_vset(1);

    let mut state = AppState::new(ChainParams::default());
    let addr = Address([0xABu8; 20]);
    let mut acc = state.get_account(addr);
    acc.balance = 42_000;
    state.set_account(acc);

    let (tx_submit, _rx) = mpsc::channel(1);
    let rpc_state = RpcState {
        block_store,
        app_state: Arc::new(RwLock::new(state)),
        validator_set: Arc::new(RwLock::new(vset)),
        node_id: "n".to_string(),
        chain_id: "c".to_string(),
        tx_submit,
    };

    let addr_hex: String = [0xABu8; 20].iter().map(|b| format!("{:02x}", b)).collect();
    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "get_account".to_string(),
        params: json!({"address": addr_hex}),
        id: json!(1),
    };

    let resp = dispatch(&rpc_state, req).await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["balance"], "42000");
}

#[tokio::test]
async fn test_rpc_get_validators() {
    let dir = temp_dir("rpc_get_validators");
    let block_store = Arc::new(BlockStore::open(&dir).unwrap());
    let (vset, _) = make_vset(3);
    let (tx_submit, _rx) = mpsc::channel(1);

    let rpc_state = RpcState {
        block_store,
        app_state: Arc::new(RwLock::new(AppState::new(ChainParams::default()))),
        validator_set: Arc::new(RwLock::new(vset)),
        node_id: "n".to_string(),
        chain_id: "c".to_string(),
        tx_submit,
    };

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "get_validators".to_string(),
        params: json!({}),
        id: json!(1),
    };

    let resp = dispatch(&rpc_state, req).await;
    assert!(resp.error.is_none());
    let result = resp.result.unwrap();
    assert_eq!(result["validators"].as_array().unwrap().len(), 3);
    assert_eq!(result["total_power"], 3);
}

#[tokio::test]
async fn test_rpc_unknown_method() {
    let dir = temp_dir("rpc_unknown_method");
    let block_store = Arc::new(BlockStore::open(&dir).unwrap());
    let (vset, _) = make_vset(1);
    let (tx_submit, _rx) = mpsc::channel(1);

    let rpc_state = RpcState {
        block_store,
        app_state: Arc::new(RwLock::new(AppState::new(ChainParams::default()))),
        validator_set: Arc::new(RwLock::new(vset)),
        node_id: "n".to_string(),
        chain_id: "c".to_string(),
        tx_submit,
    };

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "nonexistent_method".to_string(),
        params: json!({}),
        id: json!(1),
    };

    let resp = dispatch(&rpc_state, req).await;
    assert!(resp.error.is_some());
    assert_eq!(resp.error.unwrap().code, ERR_METHOD_NOT_FOUND);
}

#[tokio::test]
async fn test_rpc_get_latest_height() {
    let dir = temp_dir("rpc_latest_height");
    let block_store = Arc::new(BlockStore::open(&dir).unwrap());
    let (vset, ids) = make_vset(1);

    // Save 3 blocks
    for h in 1..=3 {
        block_store.save_block(&make_block(h, ids[0]), Hash::ZERO, &vset).unwrap();
    }

    let (tx_submit, _rx) = mpsc::channel(1);
    let rpc_state = RpcState {
        block_store,
        app_state: Arc::new(RwLock::new(AppState::new(ChainParams::default()))),
        validator_set: Arc::new(RwLock::new(vset)),
        node_id: "n".to_string(),
        chain_id: "c".to_string(),
        tx_submit,
    };

    let req = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "get_latest_height".to_string(),
        params: json!({}),
        id: json!(1),
    };

    let resp = dispatch(&rpc_state, req).await;
    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["height"], 3);
}

// ===========================================================================
// 4. Router → Storage pipeline (drive router directly with commands)
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_router_storage_pipeline() {
    let dir = temp_dir("router_storage_pipeline");
    let block_store = Arc::new(BlockStore::open(&dir.join("blocks")).unwrap());
    let state_store = Arc::new(StateStore::open(&dir.join("state")).unwrap());
    let wal = Arc::new(tokio::sync::Mutex::new(
        WAL::open(&dir.join("consensus.wal")).unwrap(),
    ));
    let metrics = Arc::new(Metrics::new());

    let (vset, ids) = make_vset(1);

    let app_state = Arc::new(RwLock::new(AppState::new(ChainParams::default())));
    let contracts = Arc::new(tokio::sync::Mutex::new(ContractRuntime::new().unwrap()));
    let validator_set_shared = Arc::new(RwLock::new(vset.clone()));

    // Channels: we send commands, router processes them
    let (tx_ev, _rx_ev) = bounded::<ConsensusEvent>(256);
    let (tx_cmd, rx_cmd) = bounded::<ConsensusCommand>(1024);
    let (p2p_tx, mut _p2p_rx) = mpsc::channel::<ConsensusCommand>(256);
    let (timer_tx, mut _timer_rx) = mpsc::channel(256);

    // Router
    let router = CommandRouter::new(
        rx_cmd,
        tx_ev.clone(),
        p2p_tx,
        timer_tx,
        StateExecutor::new(),
        app_state.clone(),
        contracts.clone(),
        block_store.clone(),
        state_store.clone(),
        wal.clone(),
        validator_set_shared.clone(),
        metrics.clone(),
    );
    tokio::spawn(async move { router.run().await });

    let block = make_block(1, ids[0]);

    // Step 1: Send WAL write command
    tx_cmd.send(ConsensusCommand::WriteWAL {
        entry: RustBFT::storage::wal::WalEntry {
            height: 1,
            round: 0,
            kind: RustBFT::storage::wal::WalEntryKind::Proposal,
            data: vec![1, 2, 3],
        },
    }).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Step 2: Send ExecuteBlock — router will execute and send BlockExecuted event back
    tx_cmd.send(ConsensusCommand::ExecuteBlock { block: block.clone() }).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // The router should have sent a BlockExecuted event back via tx_ev
    // We can check by reading from _rx_ev
    let ev = _rx_ev.try_recv();
    assert!(ev.is_ok(), "router should send BlockExecuted event");
    match ev.unwrap() {
        ConsensusEvent::BlockExecuted { height, state_root, .. } => {
            assert_eq!(height, 1);

            // Step 3: Send PersistBlock with the state_root from execution
            tx_cmd.send(ConsensusCommand::PersistBlock {
                block: block.clone(),
                state_root,
            }).unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // Verify block was persisted
            let last_h = block_store.last_height().unwrap();
            assert_eq!(last_h, 1, "block should be persisted at height 1");

            let loaded = block_store.load_block(1).unwrap();
            assert!(loaded.is_some(), "block 1 should be loadable");

            // Verify state was persisted
            let state_snap = state_store.load_latest_state().unwrap();
            assert!(state_snap.is_some(), "state snapshot should be persisted");

            // Verify WAL was truncated after persist
            let wal_path = dir.join("consensus.wal");
            let wal_entries = WAL::read_all(&wal_path).unwrap_or_default();
            assert!(wal_entries.is_empty(), "WAL should be truncated after persist");
        }
        other => panic!("expected BlockExecuted, got {:?}", other),
    }

    // Step 4: Send ReapTxs — router should respond with TxsAvailable
    tx_cmd.send(ConsensusCommand::ReapTxs { max_bytes: 1024 }).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let ev = _rx_ev.try_recv();
    assert!(ev.is_ok(), "router should send TxsAvailable for ReapTxs");
    match ev.unwrap() {
        ConsensusEvent::TxsAvailable { txs } => {
            assert!(txs.is_empty(), "MVP mempool returns empty txs");
        }
        other => panic!("expected TxsAvailable, got {:?}", other),
    }

    drop(tx_cmd); // shutdown router
}

// ===========================================================================
// 5. Serialization roundtrip for all message types
// ===========================================================================

#[test]
fn test_block_serialization_roundtrip() {
    let block = Block {
        header: BlockHeader {
            height: 42,
            timestamp_ms: 1234567890,
            prev_block_hash: sha256(b"prev"),
            proposer: make_id(1),
            validator_set_hash: sha256(b"vset"),
            state_root: sha256(b"state"),
            tx_merkle_root: sha256(b"txs"),
        },
        txs: vec![vec![1, 2, 3], vec![4, 5, 6, 7]],
        last_commit: None,
    };

    let encoded = encode_block(&block);
    let decoded = decode_block(&encoded).unwrap();

    assert_eq!(decoded.header.height, 42);
    assert_eq!(decoded.header.timestamp_ms, 1234567890);
    assert_eq!(decoded.header.prev_block_hash, block.header.prev_block_hash);
    assert_eq!(decoded.header.proposer, block.header.proposer);
    assert_eq!(decoded.txs.len(), 2);
    assert_eq!(decoded.txs[0], vec![1, 2, 3]);
    assert_eq!(decoded.txs[1], vec![4, 5, 6, 7]);
}

#[test]
fn test_proposal_serialization_roundtrip() {
    let block = make_block(10, make_id(1));
    let sp = SignedProposal {
        proposal: Proposal {
            height: 10,
            round: 2,
            block,
            valid_round: -1,
            proposer: make_id(1),
        },
        signature: [0xAB; 64],
    };

    let encoded = encode_signed_proposal(&sp);
    let decoded = decode_signed_proposal(&encoded).unwrap();

    assert_eq!(decoded.proposal.height, 10);
    assert_eq!(decoded.proposal.round, 2);
    assert_eq!(decoded.proposal.valid_round, -1);
    assert_eq!(decoded.proposal.block.header.height, 10);
    assert_eq!(decoded.signature, [0xAB; 64]);
}

#[test]
fn test_vote_serialization_roundtrip() {
    let sv = SignedVote {
        vote: Vote {
            vote_type: VoteType::Precommit,
            height: 99,
            round: 3,
            block_hash: Some(sha256(b"block")),
            validator: make_id(5),
        },
        signature: [0xCD; 64],
    };

    let encoded = encode_signed_vote(&sv);
    let decoded = decode_signed_vote(&encoded).unwrap();

    assert_eq!(decoded.vote.vote_type, VoteType::Precommit);
    assert_eq!(decoded.vote.height, 99);
    assert_eq!(decoded.vote.round, 3);
    assert_eq!(decoded.vote.block_hash, sv.vote.block_hash);
    assert_eq!(decoded.vote.validator, make_id(5));
    assert_eq!(decoded.signature, [0xCD; 64]);
}

#[test]
fn test_vote_nil_serialization_roundtrip() {
    let sv = SignedVote {
        vote: Vote {
            vote_type: VoteType::Prevote,
            height: 1,
            round: 0,
            block_hash: None,
            validator: make_id(1),
        },
        signature: [0u8; 64],
    };

    let encoded = encode_signed_vote(&sv);
    let decoded = decode_signed_vote(&encoded).unwrap();
    assert_eq!(decoded.vote.block_hash, None);
}

// ===========================================================================
// 6. Crypto: sign + verify + hash consistency
// ===========================================================================

#[test]
fn test_ed25519_sign_verify() {
    let (signing_key, verify_key) = ed25519::generate_keypair();
    let message = b"hello rustbft";

    let signature = ed25519::sign(&signing_key, message);
    assert!(ed25519::verify(&verify_key, message, &signature), "valid signature should verify");

    // Tampered message should fail
    assert!(!ed25519::verify(&verify_key, b"tampered", &signature), "tampered message should fail");
}

#[test]
fn test_sha256_deterministic() {
    let a = sha256(b"test data");
    let b = sha256(b"test data");
    assert_eq!(a, b, "sha256 must be deterministic");

    let c = sha256(b"different data");
    assert_ne!(a, c, "different inputs should produce different hashes");
}

#[test]
fn test_sha256_known_vector() {
    // SHA256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    let empty_hash = sha256(b"");
    assert_eq!(empty_hash.0[0], 0xe3);
    assert_eq!(empty_hash.0[1], 0xb0);
    assert_eq!(empty_hash.0[31], 0x55);
}

#[test]
fn test_keypair_load_or_generate() {
    let dir = temp_dir("keypair_test");
    let path = dir.join("test_key.json");
    let path_str = path.to_str().unwrap();

    // First call generates
    let (sk1, vk1) = ed25519::load_or_generate_keypair(path_str).unwrap();

    // Second call loads same key
    let (sk2, vk2) = ed25519::load_or_generate_keypair(path_str).unwrap();

    assert_eq!(vk1.to_bytes(), vk2.to_bytes(), "loaded key should match generated key");
    assert_eq!(sk1.to_bytes(), sk2.to_bytes());
}

// ===========================================================================
// 7. AppState snapshot/revert — M20 regression test
// ===========================================================================

#[test]
fn test_snapshot_revert_contract_storage_delete() {
    let mut state = AppState::new(ChainParams::default());
    let addr = Address([1u8; 20]);
    let key = vec![0x01, 0x02];
    let value = vec![0xAA, 0xBB];

    // Set storage
    state.set_storage(addr, key.clone(), value.clone());
    assert_eq!(
        state.contract_storage.get(&(addr, key.clone())),
        Some(&value),
    );

    // Snapshot, then delete
    let snap = state.snapshot();
    state.delete_storage(addr, key.clone());
    assert!(state.contract_storage.get(&(addr, key.clone())).is_none(), "should be deleted");

    // Revert — the delete should be undone
    state.revert_to(snap);
    assert_eq!(
        state.contract_storage.get(&(addr, key.clone())),
        Some(&value),
        "revert should restore deleted storage (M20 regression)"
    );
}

#[test]
fn test_snapshot_revert_nested() {
    let mut state = AppState::new(ChainParams::default());
    let addr = Address([1u8; 20]);

    // Set initial balance
    let mut acc = state.get_account(addr);
    acc.balance = 1000;
    state.set_account(acc);

    // Snapshot 1
    let snap1 = state.snapshot();
    let mut acc = state.get_account(addr);
    acc.balance = 2000;
    state.set_account(acc);

    // Snapshot 2 (nested)
    let snap2 = state.snapshot();
    let mut acc = state.get_account(addr);
    acc.balance = 3000;
    state.set_account(acc);

    assert_eq!(state.get_account(addr).balance, 3000);

    // Revert snap2 only
    state.revert_to(snap2);
    assert_eq!(state.get_account(addr).balance, 2000);

    // Revert snap1
    state.revert_to(snap1);
    assert_eq!(state.get_account(addr).balance, 1000);
}

#[test]
fn test_snapshot_commit() {
    let mut state = AppState::new(ChainParams::default());
    let addr = Address([1u8; 20]);

    let mut acc = state.get_account(addr);
    acc.balance = 1000;
    state.set_account(acc);

    let snap = state.snapshot();
    let mut acc = state.get_account(addr);
    acc.balance = 2000;
    state.set_account(acc);

    // Commit snapshot — changes become permanent
    state.commit_snapshot(snap);
    assert_eq!(state.get_account(addr).balance, 2000);
}

// ===========================================================================
// 8. P2P message encode/decode roundtrip
// ===========================================================================

#[test]
fn test_p2p_proposal_message_roundtrip() {
    let block = make_block(5, make_id(1));
    let sp = SignedProposal {
        proposal: Proposal {
            height: 5,
            round: 0,
            block,
            valid_round: -1,
            proposer: make_id(1),
        },
        signature: [0u8; 64],
    };

    let msg = NetworkMessage::Proposal(sp);
    let (ty, payload) = encode_message(&msg);
    let decoded = decode_message(ty, &payload).unwrap();

    match decoded {
        NetworkMessage::Proposal(p) => {
            assert_eq!(p.proposal.height, 5);
            assert_eq!(p.proposal.round, 0);
        }
        _ => panic!("expected Proposal"),
    }
}

#[test]
fn test_p2p_vote_message_roundtrip() {
    let sv = make_signed_vote(VoteType::Prevote, 10, 2, Some(sha256(b"block")), make_id(3));

    let msg = NetworkMessage::Prevote(sv.clone());
    let (ty, payload) = encode_message(&msg);
    let decoded = decode_message(ty, &payload).unwrap();

    match decoded {
        NetworkMessage::Prevote(v) => {
            assert_eq!(v.vote.height, 10);
            assert_eq!(v.vote.round, 2);
            assert_eq!(v.vote.validator, make_id(3));
            assert_eq!(v.vote.block_hash, sv.vote.block_hash);
        }
        _ => panic!("expected Prevote"),
    }
}

#[test]
fn test_p2p_ping_pong_roundtrip() {
    let msg = NetworkMessage::Ping(12345);
    let (ty, payload) = encode_message(&msg);
    let decoded = decode_message(ty, &payload).unwrap();
    match decoded {
        NetworkMessage::Ping(n) => assert_eq!(n, 12345),
        _ => panic!("expected Ping"),
    }

    let msg = NetworkMessage::Pong(67890);
    let (ty, payload) = encode_message(&msg);
    let decoded = decode_message(ty, &payload).unwrap();
    match decoded {
        NetworkMessage::Pong(n) => assert_eq!(n, 67890),
        _ => panic!("expected Pong"),
    }
}

#[test]
fn test_p2p_transaction_message_roundtrip() {
    let tx_data = vec![0x01, 0x02, 0x03, 0x04];
    let msg = NetworkMessage::Transaction(tx_data.clone());
    let (ty, payload) = encode_message(&msg);
    let decoded = decode_message(ty, &payload).unwrap();
    match decoded {
        NetworkMessage::Transaction(d) => assert_eq!(d, tx_data),
        _ => panic!("expected Transaction"),
    }
}

// ===========================================================================
// 9. WAL crash recovery simulation
// ===========================================================================

#[test]
fn test_wal_crash_recovery_simulation() {
    let dir = temp_dir("wal_crash_recovery");
    let wal_path = dir.join("consensus.wal");

    // Simulate: write proposal + prevote + precommit, then "crash" (drop WAL)
    {
        let mut wal = WAL::open(&wal_path).unwrap();
        use RustBFT::storage::wal::{WalEntry, WalEntryKind};

        wal.write_entry(&WalEntry {
            height: 5, round: 0, kind: WalEntryKind::Proposal, data: vec![1, 2, 3],
        }).unwrap();
        wal.write_entry(&WalEntry {
            height: 5, round: 0, kind: WalEntryKind::Prevote, data: vec![4, 5],
        }).unwrap();
        wal.write_entry(&WalEntry {
            height: 5, round: 0, kind: WalEntryKind::Precommit, data: vec![6, 7, 8],
        }).unwrap();
        // Drop without truncate — simulates crash before commit
    }

    // "Recovery": read WAL entries
    let entries = WAL::read_all(&wal_path).unwrap();
    assert_eq!(entries.len(), 3, "should recover all 3 entries");
    assert_eq!(entries[0].height, 5);
    assert_eq!(entries[0].kind, RustBFT::storage::wal::WalEntryKind::Proposal);
    assert_eq!(entries[1].kind, RustBFT::storage::wal::WalEntryKind::Prevote);
    assert_eq!(entries[2].kind, RustBFT::storage::wal::WalEntryKind::Precommit);

    // After successful commit, truncate
    {
        let mut wal = WAL::open(&wal_path).unwrap();
        wal.truncate().unwrap();
    }

    let entries = WAL::read_all(&wal_path).unwrap();
    assert_eq!(entries.len(), 0, "WAL should be empty after truncate");
}

// ===========================================================================
// 10. End-to-end: transfer tx → execute → persist → query via RPC
// ===========================================================================

#[tokio::test]
async fn test_e2e_transfer_and_rpc_query() {
    let dir = temp_dir("e2e_transfer_rpc");
    let block_store = Arc::new(BlockStore::open(&dir.join("blocks")).unwrap());
    let state_store = Arc::new(StateStore::open(&dir.join("state")).unwrap());

    let executor = StateExecutor::new();
    let mut state = AppState::new(ChainParams::default());
    let mut contracts = ContractRuntime::new().unwrap();
    let (vset, ids) = make_vset(1);

    // Fund sender
    let sender = Address([0x01; 20]);
    let receiver = Address([0x02; 20]);
    let mut sender_acc = state.get_account(sender);
    sender_acc.balance = 10_000_000;
    state.set_account(sender_acc);

    // Build transfer tx
    let tx = build_transfer_tx(sender, receiver, 1_000, 0, 100_000);

    let block = Block {
        header: BlockHeader {
            height: 1,
            timestamp_ms: 1000,
            prev_block_hash: Hash::ZERO,
            proposer: ids[0],
            validator_set_hash: Hash::ZERO,
            state_root: Hash::ZERO,
            tx_merkle_root: Hash::ZERO,
        },
        txs: vec![tx],
        last_commit: None,
    };

    // Execute
    let (state_root, _) = executor.execute_block(&mut state, &mut contracts, &block).unwrap();

    // Persist
    block_store.save_block(&block, state_root, &vset).unwrap();
    state_store.save_state(1, &state).unwrap();

    // Query via RPC
    let (tx_submit, _rx) = mpsc::channel(1);
    let rpc_state = RpcState {
        block_store: block_store.clone(),
        app_state: Arc::new(RwLock::new(state)),
        validator_set: Arc::new(RwLock::new(vset)),
        node_id: "n".to_string(),
        chain_id: "c".to_string(),
        tx_submit,
    };

    // Check block
    let resp = dispatch(&rpc_state, JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "get_block".to_string(),
        params: json!({"height": 1}),
        id: json!(1),
    }).await;
    assert!(resp.error.is_none());
    assert_eq!(resp.result.as_ref().unwrap()["height"], 1);
    assert_eq!(resp.result.as_ref().unwrap()["num_txs"], 1);

    // Check receiver balance
    let receiver_hex: String = [0x02u8; 20].iter().map(|b| format!("{:02x}", b)).collect();
    let resp = dispatch(&rpc_state, JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "get_account".to_string(),
        params: json!({"address": receiver_hex}),
        id: json!(2),
    }).await;
    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["balance"], "1000");

    // Check latest height
    let resp = dispatch(&rpc_state, JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "get_latest_height".to_string(),
        params: json!({}),
        id: json!(3),
    }).await;
    assert_eq!(resp.result.unwrap()["height"], 1);
}
