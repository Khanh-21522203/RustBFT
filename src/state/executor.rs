use crate::contracts::{CallContext, ContractError, ContractRuntime};
use crate::crypto::hash::sha256;
use crate::state::accounts::AppState;
use crate::state::merkle::compute_state_root;
use crate::state::tx::{decode_tx, DecodedTx, TxDecodeError};
use crate::types::{Address, Block, Hash, ValidatorId, ValidatorUpdate};

#[derive(Clone, Debug)]
pub struct GasSchedule {
    pub base_tx: u64,
    pub sig_verify: u64,
    pub deploy_per_byte: u64,
    pub call_base: u64,
}

impl Default for GasSchedule {
    fn default() -> Self {
        Self {
            base_tx: 21_000,
            sig_verify: 3_000,
            deploy_per_byte: 200,
            call_base: 10_000,
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ExecError {
    #[error("tx decode error: {0}")]
    Decode(#[from] TxDecodeError),
    #[error("bad nonce")]
    BadNonce,
    #[error("insufficient balance")]
    InsufficientBalance,
    #[error("contract error: {0}")]
    Contract(String),
}

pub struct StateExecutor {
    pub gas: GasSchedule,
}

impl StateExecutor {
    pub fn new() -> Self {
        Self {
            gas: GasSchedule::default(),
        }
    }

    /// Execute committed block synchronously (no async, no IO).
    /// Returns (state_root, validator_updates).
    pub fn execute_block(
        &self,
        state: &mut AppState,
        contracts: &mut ContractRuntime,
        block: &Block,
    ) -> Result<(Hash, Vec<ValidatorUpdate>), anyhow::Error> {
        // BeginBlock (MVP no-op)
        self.begin_block(state, block)?;

        // Basic deterministic block bytes limit
        let mut total_bytes: u64 = 0;
        for txb in block.txs.iter() {
            total_bytes = total_bytes.saturating_add(txb.len() as u64);
        }
        if total_bytes > state.params.max_block_bytes as u64 {
            return Err(anyhow::anyhow!("block exceeds max_block_bytes"));
        }

        let mut gas_used_total: u64 = 0;
        let mut pending_val_updates: Vec<crate::state::tx::ValidatorUpdateTx> = Vec::new();

        for tx_bytes in block.txs.iter() {
            if (tx_bytes.len() as u32) > state.params.max_tx_bytes {
                continue;
            }

            let gas_used = match self.execute_one_tx(state, contracts, block, tx_bytes, &mut pending_val_updates) {
                Ok(g) => g,
                Err(_) => 0,
            };

            gas_used_total = gas_used_total.saturating_add(gas_used);
            if gas_used_total > state.params.max_block_gas {
                return Err(anyhow::anyhow!("block gas limit exceeded"));
            }
        }

        // EndBlock: process validator updates collected during DeliverTx
        let validator_updates = self.end_block(state, &pending_val_updates)?;

        // Commit: compute root after execution
        let state_root = compute_state_root(state);
        Ok((state_root, validator_updates))
    }

    fn begin_block(&self, _state: &mut AppState, _block: &Block) -> Result<(), anyhow::Error> {
        Ok(())
    }

    /// EndBlock: convert pending validator update txs into ValidatorUpdate structs.
    /// These are returned to consensus which applies them at H+1.
    fn end_block(
        &self,
        _state: &mut AppState,
        pending: &[crate::state::tx::ValidatorUpdateTx],
    ) -> Result<Vec<ValidatorUpdate>, anyhow::Error> {
        let mut updates = Vec::new();
        for vtx in pending {
            let new_power = match vtx.action {
                0x01 => vtx.new_power, // Add
                0x02 => 0,             // Remove
                0x03 => vtx.new_power, // UpdatePower
                _ => continue,         // Unknown action, skip
            };
            updates.push(ValidatorUpdate {
                id: ValidatorId(vtx.validator_id),
                new_power,
            });
        }
        Ok(updates)
    }

    fn execute_one_tx(
        &self,
        state: &mut AppState,
        contracts: &mut ContractRuntime,
        block: &Block,
        tx_bytes: &[u8],
        pending_val_updates: &mut Vec<crate::state::tx::ValidatorUpdateTx>,
    ) -> Result<u64, ExecError> {
        let decoded = decode_tx(tx_bytes)?;

        match decoded {
            DecodedTx::Transfer(t) => Ok(self.exec_transfer(state, t)),
            DecodedTx::ContractDeploy(t) => Ok(self.exec_deploy(state, contracts, block, t)?),
            DecodedTx::ContractCall(t) => Ok(self.exec_call(state, contracts, block, t)?),
            DecodedTx::ValidatorUpdate(t) => {
                // Validator update txs are collected and processed in EndBlock.
                // Charge intrinsic gas now; actual update happens in end_block.
                let intrinsic = self.gas.base_tx.saturating_add(self.gas.sig_verify);
                let charge = intrinsic.min(t.gas_limit);
                self.charge_gas(state, t.from, charge);
                // Increment sender nonce
                let mut sender = state.get_account(t.from);
                if sender.nonce != t.nonce {
                    return Err(ExecError::BadNonce);
                }
                sender.nonce = sender.nonce.saturating_add(1);
                state.set_account(sender);
                pending_val_updates.push(t);
                Ok(charge)
            }
        }
    }

    fn exec_transfer(&self, state: &mut AppState, tx: crate::state::tx::TransferTx) -> u64 {
        // Intrinsic gas (always charged)
        let intrinsic = self.gas.base_tx.saturating_add(self.gas.sig_verify);

        // If intrinsic already exceeds tx gas limit, charge full gas_limit and fail
        if intrinsic > tx.gas_limit {
            self.charge_gas(state, tx.from, tx.gas_limit);
            return tx.gas_limit;
        }

        let snap = state.snapshot();

        let mut from = state.get_account(tx.from);
        let mut to = state.get_account(tx.to);

        let ok = if from.nonce != tx.nonce {
            false
        } else {
            // Sender must afford amount + gas (deterministic)
            let need = tx.amount.saturating_add(intrinsic as u128);
            if from.balance < need {
                false
            } else {
                from.balance = from.balance.saturating_sub(tx.amount);
                to.balance = to.balance.saturating_add(tx.amount);
                from.nonce = from.nonce.saturating_add(1);
                state.set_account(from);
                state.set_account(to);
                true
            }
        };

        if ok {
            state.commit_snapshot(snap);
        } else {
            state.revert_to(snap);
        }

        self.charge_gas(state, tx.from, intrinsic);
        intrinsic
    }

    fn exec_deploy(
        &self,
        state: &mut AppState,
        contracts: &mut ContractRuntime,
        block: &Block,
        tx: crate::state::tx::ContractDeployTx,
    ) -> Result<u64, ExecError> {
        // Intrinsic gas depends on wasm size
        let wasm_cost = (tx.wasm.len() as u64).saturating_mul(self.gas.deploy_per_byte);
        let intrinsic = self
            .gas
            .base_tx
            .saturating_add(self.gas.sig_verify)
            .saturating_add(wasm_cost);

        if intrinsic > tx.gas_limit {
            self.charge_gas(state, tx.from, tx.gas_limit);
            return Ok(tx.gas_limit);
        }

        let snap = state.snapshot();

        let mut sender = state.get_account(tx.from);
        if sender.nonce != tx.nonce {
            state.revert_to(snap);
            self.charge_gas(state, tx.from, intrinsic);
            return Err(ExecError::BadNonce);
        }

        // Deploy must be deterministic: code_hash = sha256(wasm)
        let deploy_res = contracts
            .deploy(&tx.wasm)
            .map_err(|e| ExecError::Contract(e.to_string()))?;
        let code_hash = deploy_res.code_hash;

        // Deterministic contract address from (sender, nonce)
        let contract_addr = address_from_sender_nonce(&tx.from, tx.nonce);

        // Create / update contract account
        let mut contract_acc = state.get_account(contract_addr);
        contract_acc.code_hash = Some(code_hash);
        state.set_account(contract_acc);

        // Increment sender nonce (stateful)
        sender.nonce = sender.nonce.saturating_add(1);
        state.set_account(sender);

        // Remaining gas goes to VM
        let vm_gas_limit = tx.gas_limit.saturating_sub(intrinsic);

        // Call constructor if init_input not empty
        if !tx.init_input.is_empty() {
            let ctx = CallContext {
                self_addr: contract_addr,
                caller: tx.from,
                origin: tx.from,
                block_height: block.header.height,
                block_timestamp_ms: block.header.timestamp_ms,
                value: 0,
                gas_limit: vm_gas_limit,
                call_depth: 0,
            };

            match contracts.call(state, code_hash, &tx.init_input, &ctx) {
                Ok(r) => {
                    if !r.success {
                        // Revert contract changes
                        state.revert_to(snap);
                        // Charge full gas_limit on contract failure (simple deterministic rule)
                        self.charge_gas(state, tx.from, tx.gas_limit);
                        return Ok(tx.gas_limit);
                    }
                    // Success: commit snapshot
                    state.commit_snapshot(snap);

                    // Total gas used = intrinsic + vm gas used
                    let total_used = intrinsic.saturating_add(r.gas_used);
                    let charge = total_used.min(tx.gas_limit);
                    self.charge_gas(state, tx.from, charge);
                    return Ok(charge);
                }
                Err(ContractError::OutOfGas) => {
                    state.revert_to(snap);
                    self.charge_gas(state, tx.from, tx.gas_limit);
                    return Ok(tx.gas_limit);
                }
                Err(e) => {
                    state.revert_to(snap);
                    self.charge_gas(state, tx.from, intrinsic);
                    return Err(ExecError::Contract(e.to_string()));
                }
            }
        }

        // No constructor call: commit deploy
        state.commit_snapshot(snap);
        self.charge_gas(state, tx.from, intrinsic);
        Ok(intrinsic)
    }

    fn exec_call(
        &self,
        state: &mut AppState,
        contracts: &mut ContractRuntime,
        block: &Block,
        tx: crate::state::tx::ContractCallTx,
    ) -> Result<u64, ExecError> {
        let intrinsic = self
            .gas
            .base_tx
            .saturating_add(self.gas.sig_verify)
            .saturating_add(self.gas.call_base);

        if intrinsic > tx.gas_limit {
            self.charge_gas(state, tx.from, tx.gas_limit);
            return Ok(tx.gas_limit);
        }

        let snap = state.snapshot();

        let mut sender = state.get_account(tx.from);
        if sender.nonce != tx.nonce {
            state.revert_to(snap);
            self.charge_gas(state, tx.from, intrinsic);
            return Err(ExecError::BadNonce);
        }

        // Transfer value to callee account before call (common pattern, deterministic)
        if sender.balance < tx.value.saturating_add(intrinsic as u128) {
            state.revert_to(snap);
            self.charge_gas(state, tx.from, intrinsic);
            return Err(ExecError::InsufficientBalance);
        }

        let mut callee = state.get_account(tx.to);
        sender.balance = sender.balance.saturating_sub(tx.value);
        callee.balance = callee.balance.saturating_add(tx.value);

        sender.nonce = sender.nonce.saturating_add(1);

        state.set_account(sender);
        state.set_account(callee);

        // Must exist code hash
        let callee_acc = state.get_account(tx.to);
        let code_hash = callee_acc
            .code_hash
            .ok_or_else(|| ExecError::Contract("callee has no code".into()))?;

        let vm_gas_limit = tx.gas_limit.saturating_sub(intrinsic);

        let ctx = CallContext {
            self_addr: tx.to,
            caller: tx.from,
            origin: tx.from,
            block_height: block.header.height,
            block_timestamp_ms: block.header.timestamp_ms,
            value: tx.value,
            gas_limit: vm_gas_limit,
            call_depth: 0,
        };

        match contracts.call(state, code_hash, &tx.input, &ctx) {
            Ok(r) => {
                if r.success {
                    state.commit_snapshot(snap);
                    let total_used = intrinsic.saturating_add(r.gas_used);
                    let charge = total_used.min(tx.gas_limit);
                    self.charge_gas(state, tx.from, charge);
                    Ok(charge)
                } else {
                    // Contract returned failure => revert everything including value transfer
                    state.revert_to(snap);
                    self.charge_gas(state, tx.from, tx.gas_limit);
                    Ok(tx.gas_limit)
                }
            }
            Err(ContractError::OutOfGas) => {
                state.revert_to(snap);
                self.charge_gas(state, tx.from, tx.gas_limit);
                Ok(tx.gas_limit)
            }
            Err(e) => {
                state.revert_to(snap);
                self.charge_gas(state, tx.from, intrinsic);
                Err(ExecError::Contract(e.to_string()))
            }
        }
    }

    fn charge_gas(&self, state: &mut AppState, from: Address, gas_used: u64) {
        let mut acc = state.get_account(from);
        acc.balance = acc.balance.saturating_sub(gas_used as u128);
        state.set_account(acc);
    }
}

/// Deterministic contract address = sha256(sender||nonce) first 20 bytes.
/// Keeps execution deterministic and platform-independent.
fn address_from_sender_nonce(sender: &Address, nonce: u64) -> Address {
    let mut b = Vec::with_capacity(20 + 8);
    b.extend_from_slice(&sender.0);
    b.extend_from_slice(&nonce.to_be_bytes());
    let h: Hash = sha256(&b);
    let mut out = [0u8; 20];
    out.copy_from_slice(&h.0[..20]);
    Address(out)
}
