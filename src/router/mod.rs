use std::sync::Arc;

use crossbeam_channel::Receiver;
use tokio::sync::{mpsc, RwLock};
use tracing::{info, warn, error};

use crate::consensus::events::{ConsensusCommand, ConsensusEvent};
use crate::consensus::timer::TimerCommand;
use crate::contracts::ContractRuntime;
use crate::metrics::Metrics;
use crate::state::accounts::AppState;
use crate::state::executor::StateExecutor;
use crate::storage::block_store::BlockStore;
use crate::storage::state_store::StateStore;
use crate::storage::wal::WAL;
use crate::types::ValidatorSet;

/// The command router runs on the Tokio runtime and bridges the synchronous
/// consensus core (which emits ConsensusCommand via crossbeam) to the async
/// subsystems (P2P, timers, state machine, storage).
///
/// Doc 11 section 3: channel topology.
///
/// NOTE: This module lives outside `consensus/` to respect the layer boundary:
/// consensus MUST NOT import state, storage, or contracts (doc 11 section 3).
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

    /// Write-ahead log for crash recovery.
    wal: Arc<tokio::sync::Mutex<WAL>>,

    /// Current validator set (shared with RPC).
    validator_set: Arc<RwLock<ValidatorSet>>,

    /// Prometheus metrics.
    metrics: Arc<Metrics>,
}

impl CommandRouter {
    #[allow(clippy::too_many_arguments)]
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
        wal: Arc<tokio::sync::Mutex<WAL>>,
        validator_set: Arc<RwLock<ValidatorSet>>,
        metrics: Arc<Metrics>,
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
            wal,
            validator_set,
            metrics,
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

                    let start = std::time::Instant::now();
                    match self.executor.execute_block(&mut state, &mut contracts, &block) {
                        Ok((state_root, validator_updates)) => {
                            let elapsed = start.elapsed().as_secs_f64();
                            self.metrics.state_block_execution_duration.observe(elapsed);

                            // Update shared validator set
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
                            error!(height = height, error = %e, "Block execution failed");
                        }
                    }
                }

                // Storage: persist block
                ConsensusCommand::PersistBlock { block, state_root } => {
                    let height = block.header.height;
                    let vs = self.validator_set.read().await;

                    let start = std::time::Instant::now();
                    if let Err(e) = self.block_store.save_block(&block, state_root, &vs) {
                        error!(height = height, error = %e, "Block persist failed");
                    }
                    let elapsed = start.elapsed().as_secs_f64();
                    self.metrics.storage_block_persist_duration.observe(elapsed);

                    // Also persist state snapshot
                    let state = self.app_state.read().await;
                    if let Err(e) = self.state_store.save_state(height, &state) {
                        error!(height = height, error = %e, "State persist failed");
                    }

                    // Truncate WAL after successful persist
                    let mut wal = self.wal.lock().await;
                    if let Err(e) = wal.truncate() {
                        warn!(error = %e, "WAL truncate failed");
                    }

                    info!(height = height, "Block committed and persisted");
                    self.metrics.consensus_height.set(height as i64);
                }

                // WAL write
                ConsensusCommand::WriteWAL { entry } => {
                    let start = std::time::Instant::now();
                    let mut wal = self.wal.lock().await;
                    if let Err(e) = wal.write_entry(&entry) {
                        error!(error = %e, "WAL write failed");
                    }
                    let elapsed = start.elapsed().as_secs_f64();
                    self.metrics.storage_wal_write_duration.observe(elapsed);
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
