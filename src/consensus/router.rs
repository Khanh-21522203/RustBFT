use std::sync::Arc;

use crossbeam_channel::Receiver;
use tokio::sync::{mpsc, RwLock};

use crate::consensus::events::{ConsensusCommand, ConsensusEvent};
use crate::consensus::timer::TimerCommand;
use crate::contracts::ContractRuntime;
use crate::state::accounts::AppState;
use crate::state::executor::StateExecutor;
use crate::storage::block_store::BlockStore;
use crate::storage::state_store::StateStore;
use crate::types::ValidatorSet;

/// The command router runs on the Tokio runtime and bridges the synchronous
/// consensus core (which emits ConsensusCommand via crossbeam) to the async
/// subsystems (P2P, timers, state machine, storage).
///
/// Doc 11 section 3: channel topology.
pub struct CommandRouter {
    /// Receive commands from consensus core (crossbeam, blocking on sender side).
    rx_cmd: Receiver<ConsensusCommand>,

    /// Send events back to consensus core.
    to_consensus: crossbeam_channel::Sender<ConsensusEvent>,

    /// Forward broadcast commands to P2P manager.
    to_p2p: mpsc::Sender<ConsensusCommand>,

    /// Forward timer commands to timer service.
    to_timer: mpsc::Sender<TimerCommand>,

    /// State executor (synchronous, deterministic).
    executor: StateExecutor,

    /// Mutable app state (shared with RPC for reads).
    app_state: Arc<RwLock<AppState>>,

    /// Contract runtime.
    contracts: Arc<tokio::sync::Mutex<ContractRuntime>>,

    /// Block store for persistence.
    block_store: Arc<BlockStore>,

    /// State store for persistence.
    state_store: Arc<StateStore>,

    /// Current validator set (shared with RPC).
    validator_set: Arc<RwLock<ValidatorSet>>,
}

impl CommandRouter {
    pub fn new(
        rx_cmd: Receiver<ConsensusCommand>,
        to_consensus: crossbeam_channel::Sender<ConsensusEvent>,
        to_p2p: mpsc::Sender<ConsensusCommand>,
        to_timer: mpsc::Sender<TimerCommand>,
        executor: StateExecutor,
        app_state: Arc<RwLock<AppState>>,
        contracts: Arc<tokio::sync::Mutex<ContractRuntime>>,
        block_store: Arc<BlockStore>,
        state_store: Arc<StateStore>,
        validator_set: Arc<RwLock<ValidatorSet>>,
    ) -> Self {
        Self {
            rx_cmd,
            to_consensus,
            to_p2p,
            to_timer,
            executor,
            app_state,
            contracts,
            block_store,
            state_store,
            validator_set,
        }
    }

    /// Run the router. This should be spawned on a dedicated thread
    /// (or tokio::spawn_blocking) since it blocks on crossbeam recv.
    pub async fn run(self) {
        loop {
            // Block on crossbeam channel (consensus -> router)
            let cmd = match self.rx_cmd.recv() {
                Ok(cmd) => cmd,
                Err(_) => break, // consensus channel closed
            };

            match cmd {
                // P2P commands: forward to P2P manager
                ConsensusCommand::BroadcastProposal { .. }
                | ConsensusCommand::BroadcastVote { .. } => {
                    let _ = self.to_p2p.send(cmd).await;
                }

                // Timer commands
                ConsensusCommand::ScheduleTimeout { timeout } => {
                    let _ = self.to_timer.send(TimerCommand::Schedule(timeout)).await;
                }
                ConsensusCommand::CancelTimeout { timeout } => {
                    let _ = self.to_timer.send(TimerCommand::Cancel(timeout)).await;
                }

                // State machine: execute block synchronously
                ConsensusCommand::ExecuteBlock { block } => {
                    let height = block.header.height;
                    let mut state = self.app_state.write().await;
                    let mut contracts = self.contracts.lock().await;

                    match self.executor.execute_block(&mut state, &mut contracts, &block) {
                        Ok((state_root, validator_updates)) => {
                            // Update shared validator set
                            // (consensus will also update its own copy via BlockExecuted)
                            if !validator_updates.is_empty() {
                                let mut vs = self.validator_set.write().await;
                                if let Ok(new_vs) = vs.apply_updates(
                                    &validator_updates,
                                    &crate::types::ValidatorSetPolicy::default(),
                                ) {
                                    *vs = new_vs;
                                }
                            }

                            let _ = self.to_consensus.send(ConsensusEvent::BlockExecuted {
                                height,
                                state_root,
                                validator_updates,
                            });
                        }
                        Err(e) => {
                            eprintln!("block execution failed at height {}: {}", height, e);
                        }
                    }
                }

                // Storage: persist block
                ConsensusCommand::PersistBlock { block, state_root } => {
                    let vs = self.validator_set.read().await;
                    if let Err(e) = self.block_store.save_block(&block, state_root, &vs) {
                        eprintln!("block persist failed: {}", e);
                    }
                    // Also persist state snapshot
                    let state = self.app_state.read().await;
                    if let Err(e) = self.state_store.save_state(block.header.height, &state) {
                        eprintln!("state persist failed: {}", e);
                    }
                }

                // WAL write (placeholder)
                ConsensusCommand::WriteWAL { entry: _ } => {
                    // WAL writes would go here
                }

                // Mempool commands (placeholder for now)
                ConsensusCommand::ReapTxs { max_bytes: _ } => {
                    // In a full implementation, this would query the mempool.
                    // For MVP, respond with empty txs immediately.
                    let _ = self.to_consensus.send(ConsensusEvent::TxsAvailable {
                        txs: vec![],
                    });
                }
                ConsensusCommand::EvictTxs { tx_hashes: _ } => {
                    // Mempool eviction placeholder
                }
            }
        }
    }
}
