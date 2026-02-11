use crate::state::accounts::{AppState};
use crate::state::merkle::compute_state_root;
use crate::state::tx::{decode_tx, DecodedTx, TxDecodeError};
use crate::types::{Address, Block, Hash};

#[derive(Clone, Debug)]
pub struct GasSchedule {
    pub base_tx: u64,      // 21_000
    pub sig_verify: u64,   // 3_000
}

impl Default for GasSchedule {
    fn default() -> Self {
        Self { base_tx: 21_000, sig_verify: 3_000 }
    }
}

pub struct StateExecutor {
    pub gas: GasSchedule,
}

impl StateExecutor {
    pub fn new() -> Self {
        Self { gas: GasSchedule::default() }
    }

    /// Execute committed block synchronously (no async, no IO). :contentReference[oaicite:6]{index=6}
    pub fn execute_block(&self, state: &mut AppState, block: &Block) -> Result<Hash, anyhow::Error> {
        self.begin_block(state, block)?;

        // Enforce block size limit deterministically (MVP: sum of tx bytes)
        let mut total_bytes: u64 = 0;
        for txb in block.txs.iter() {
            total_bytes += txb.len() as u64;
        }
        if total_bytes > state.params.max_block_bytes as u64 {
            return Err(anyhow::anyhow!("block exceeds max_block_bytes"));
        }

        let mut gas_used_total: u64 = 0;

        for tx_bytes in block.txs.iter() {
            // Per-tx max size
            if (tx_bytes.len() as u32) > state.params.max_tx_bytes {
                // Skip tx deterministically: still charge base gas to a "null sender"? MVP: no sender => no charge.
                continue;
            }

            let gas_used = match self.deliver_tx(state, tx_bytes) {
                Ok(g) => g,
                Err(_) => {
                    // Decode error: treat as failed tx with zero gas charge (no sender known)
                    0
                }
            };

            gas_used_total = gas_used_total.saturating_add(gas_used);
            if gas_used_total > state.params.max_block_gas {
                return Err(anyhow::anyhow!("block gas limit exceeded"));
            }
        }

        self.end_block(state, block)?;

        // Commit: compute state_root after executing all txs. :contentReference[oaicite:7]{index=7}
        let root = compute_state_root(state);
        Ok(root)
    }

    fn begin_block(&self, _state: &mut AppState, _block: &Block) -> Result<(), anyhow::Error> {
        Ok(())
    }

    fn end_block(&self, _state: &mut AppState, _block: &Block) -> Result<(), anyhow::Error> {
        Ok(())
    }

    fn deliver_tx(&self, state: &mut AppState, tx_bytes: &[u8]) -> Result<u64, TxDecodeError> {
        let decoded = decode_tx(tx_bytes)?;
        match decoded {
            DecodedTx::Transfer(t) => Ok(self.exec_transfer(state, t)),
        }
    }

    fn exec_transfer(&self, state: &mut AppState, tx: crate::state::tx::TransferTx) -> u64 {
        // Gas charged before execution; if fail, state must revert but gas still deducted. :contentReference[oaicite:8]{index=8}
        let mut gas_used = 0u64;
        gas_used = gas_used.saturating_add(self.gas.base_tx);
        gas_used = gas_used.saturating_add(self.gas.sig_verify);

        // Enforce tx gas_limit deterministically
        if gas_used > tx.gas_limit {
            self.charge_gas(state, tx.from, tx.gas_limit);
            return tx.gas_limit;
        }

        let snap = state.snapshot();

        // NOTE: signature verification will be added once you define how address relates to pubkey.
        // For now, we only enforce nonce/balance deterministically.

        let mut from = state.get_account(tx.from);
        let mut to = state.get_account(tx.to);

        let ok = if from.nonce != tx.nonce {
            false
        } else if from.balance < tx.amount.saturating_add(gas_used as u128) {
            false
        } else {
            // Apply transfer
            from.balance = from.balance.saturating_sub(tx.amount);
            to.balance = to.balance.saturating_add(tx.amount);
            from.nonce = from.nonce.saturating_add(1);
            state.set_account(from);
            state.set_account(to);
            true
        };

        if ok {
            state.commit_snapshot(snap);
            self.charge_gas(state, tx.from, gas_used);
            gas_used
        } else {
            state.revert_to(snap);
            self.charge_gas(state, tx.from, gas_used);
            gas_used
        }
    }

    fn charge_gas(&self, state: &mut AppState, from: Address, gas_used: u64) {
        let mut acc = state.get_account(from);
        acc.balance = acc.balance.saturating_sub(gas_used as u128);
        state.set_account(acc);
    }
}
