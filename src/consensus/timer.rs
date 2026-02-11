use crate::consensus::events::{ConsensusEvent, Timeout, TimeoutKind};
use crossbeam_channel::Sender;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

/// Timer service: runs on the Tokio runtime.
/// Receives ScheduleTimeout requests and fires ConsensusEvent timeout events
/// back to the consensus core after the specified duration.
pub struct TimerService {
    rx: mpsc::Receiver<TimerCommand>,
    to_consensus: Sender<ConsensusEvent>,
}

#[derive(Clone, Debug)]
pub enum TimerCommand {
    Schedule(Timeout),
    Cancel(Timeout),
}

pub struct TimerHandle {
    pub tx: mpsc::Sender<TimerCommand>,
}

impl TimerService {
    pub fn new(to_consensus: Sender<ConsensusEvent>) -> (Self, TimerHandle) {
        let (tx, rx) = mpsc::channel(256);
        (
            Self { rx, to_consensus },
            TimerHandle { tx },
        )
    }

    /// Run the timer service. Each scheduled timeout spawns a Tokio sleep task.
    /// Cancel is best-effort (we track active timeouts by (height, round, kind)).
    pub async fn run(mut self) {
        // Track active timeouts via a shared cancellation map
        let active = Arc::new(tokio::sync::Mutex::new(
            std::collections::BTreeMap::<(u64, u32, u8), tokio::sync::watch::Sender<bool>>::new(),
        ));

        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                TimerCommand::Schedule(t) => {
                    let key = (t.height, t.round, t.kind as u8);
                    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);

                    // Store cancel handle
                    {
                        let mut map = active.lock().await;
                        map.insert(key, cancel_tx);
                    }

                    let to_consensus = self.to_consensus.clone();
                    let active_clone = active.clone();

                    tokio::spawn(async move {
                        let dur = Duration::from_millis(t.duration_ms);

                        tokio::select! {
                            _ = sleep(dur) => {
                                // Timer fired: send timeout event
                                let ev = match t.kind {
                                    TimeoutKind::Propose => ConsensusEvent::TimeoutPropose {
                                        height: t.height,
                                        round: t.round,
                                    },
                                    TimeoutKind::Prevote => ConsensusEvent::TimeoutPrevote {
                                        height: t.height,
                                        round: t.round,
                                    },
                                    TimeoutKind::Precommit => ConsensusEvent::TimeoutPrecommit {
                                        height: t.height,
                                        round: t.round,
                                    },
                                };
                                let _ = to_consensus.send(ev);
                            }
                            _ = wait_cancel(cancel_rx) => {
                                // Cancelled
                            }
                        }

                        // Clean up
                        let mut map = active_clone.lock().await;
                        map.remove(&(t.height, t.round, t.kind as u8));
                    });
                }
                TimerCommand::Cancel(t) => {
                    let key = (t.height, t.round, t.kind as u8);
                    let mut map = active.lock().await;
                    if let Some(tx) = map.remove(&key) {
                        let _ = tx.send(true);
                    }
                }
            }
        }
    }
}

async fn wait_cancel(mut rx: tokio::sync::watch::Receiver<bool>) {
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            // Sender dropped, treat as cancel
            return;
        }
    }
}
