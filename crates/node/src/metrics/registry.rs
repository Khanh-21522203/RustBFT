use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::{exponential_buckets, Histogram};
use prometheus_client::registry::Registry;
use std::sync::atomic::AtomicI64;
use std::sync::Arc;

/// All Prometheus metrics for RustBFT (doc 14 section 2).
#[derive(Clone)]
pub struct Metrics {
    pub registry: Arc<std::sync::Mutex<Registry>>,

    // Consensus metrics (section 2.2)
    pub consensus_height: Gauge<i64, AtomicI64>,
    pub consensus_round: Gauge<i64, AtomicI64>,
    pub consensus_proposals_received: Counter,
    pub consensus_votes_received: Counter,
    pub consensus_timeouts: Counter,
    pub consensus_equivocations: Counter,
    pub consensus_block_commit_duration: Histogram,
    pub consensus_rounds_per_height: Histogram,

    // Networking metrics (section 2.3)
    pub p2p_peers_connected: Gauge<i64, AtomicI64>,
    pub p2p_messages_sent: Counter,
    pub p2p_messages_received: Counter,
    pub p2p_bytes_sent: Counter,
    pub p2p_bytes_received: Counter,

    // State machine metrics (section 2.5)
    pub state_block_execution_duration: Histogram,
    pub state_tx_success: Counter,
    pub state_tx_failure: Counter,
    pub state_gas_used: Counter,

    // Storage metrics (section 2.7)
    pub storage_block_persist_duration: Histogram,
    pub storage_wal_write_duration: Histogram,
    pub storage_wal_entries: Gauge<i64, AtomicI64>,

    // Channel metrics (section 2.8)
    pub channel_drops: Counter,

    // RPC metrics (section 2.9)
    pub rpc_requests: Counter,
    pub rpc_request_duration: Histogram,
}

impl Metrics {
    pub fn new() -> Self {
        let mut registry = Registry::default();

        let consensus_height = Gauge::<i64, AtomicI64>::default();
        registry.register("rustbft_consensus_height", "Current consensus height", consensus_height.clone());

        let consensus_round = Gauge::<i64, AtomicI64>::default();
        registry.register("rustbft_consensus_round", "Current consensus round", consensus_round.clone());

        let consensus_proposals_received = Counter::default();
        registry.register("rustbft_consensus_proposals_received_total", "Proposals received", consensus_proposals_received.clone());

        let consensus_votes_received = Counter::default();
        registry.register("rustbft_consensus_votes_received_total", "Votes received", consensus_votes_received.clone());

        let consensus_timeouts = Counter::default();
        registry.register("rustbft_consensus_timeouts_total", "Timeouts fired", consensus_timeouts.clone());

        let consensus_equivocations = Counter::default();
        registry.register("rustbft_consensus_equivocations_total", "Equivocations detected", consensus_equivocations.clone());

        let consensus_block_commit_duration = Histogram::new(exponential_buckets(0.01, 2.0, 12));
        registry.register("rustbft_consensus_block_commit_duration_seconds", "Time from entering height to commit", consensus_block_commit_duration.clone());

        let consensus_rounds_per_height = Histogram::new(exponential_buckets(1.0, 2.0, 8));
        registry.register("rustbft_consensus_rounds_per_height", "Rounds needed to commit", consensus_rounds_per_height.clone());

        let p2p_peers_connected = Gauge::<i64, AtomicI64>::default();
        registry.register("rustbft_p2p_peers_connected", "Connected peers", p2p_peers_connected.clone());

        let p2p_messages_sent = Counter::default();
        registry.register("rustbft_p2p_messages_sent_total", "Messages sent", p2p_messages_sent.clone());

        let p2p_messages_received = Counter::default();
        registry.register("rustbft_p2p_messages_received_total", "Messages received", p2p_messages_received.clone());

        let p2p_bytes_sent = Counter::default();
        registry.register("rustbft_p2p_bytes_sent_total", "Bytes sent", p2p_bytes_sent.clone());

        let p2p_bytes_received = Counter::default();
        registry.register("rustbft_p2p_bytes_received_total", "Bytes received", p2p_bytes_received.clone());

        let state_block_execution_duration = Histogram::new(exponential_buckets(0.001, 2.0, 14));
        registry.register("rustbft_state_block_execution_duration_seconds", "Block execution time", state_block_execution_duration.clone());

        let state_tx_success = Counter::default();
        registry.register("rustbft_state_tx_success_total", "Successful transactions", state_tx_success.clone());

        let state_tx_failure = Counter::default();
        registry.register("rustbft_state_tx_failure_total", "Failed transactions", state_tx_failure.clone());

        let state_gas_used = Counter::default();
        registry.register("rustbft_state_gas_used_total", "Total gas consumed", state_gas_used.clone());

        let storage_block_persist_duration = Histogram::new(exponential_buckets(0.001, 2.0, 12));
        registry.register("rustbft_storage_block_persist_duration_seconds", "Block persist time", storage_block_persist_duration.clone());

        let storage_wal_write_duration = Histogram::new(exponential_buckets(0.0001, 2.0, 12));
        registry.register("rustbft_storage_wal_write_duration_seconds", "WAL write time", storage_wal_write_duration.clone());

        let storage_wal_entries = Gauge::<i64, AtomicI64>::default();
        registry.register("rustbft_storage_wal_entries", "Current WAL entry count", storage_wal_entries.clone());

        let channel_drops = Counter::default();
        registry.register("rustbft_channel_drops_total", "Messages dropped due to full channel", channel_drops.clone());

        let rpc_requests = Counter::default();
        registry.register("rustbft_rpc_requests_total", "RPC requests", rpc_requests.clone());

        let rpc_request_duration = Histogram::new(exponential_buckets(0.0001, 2.0, 12));
        registry.register("rustbft_rpc_request_duration_seconds", "RPC request latency", rpc_request_duration.clone());

        Self {
            registry: Arc::new(std::sync::Mutex::new(registry)),
            consensus_height,
            consensus_round,
            consensus_proposals_received,
            consensus_votes_received,
            consensus_timeouts,
            consensus_equivocations,
            consensus_block_commit_duration,
            consensus_rounds_per_height,
            p2p_peers_connected,
            p2p_messages_sent,
            p2p_messages_received,
            p2p_bytes_sent,
            p2p_bytes_received,
            state_block_execution_duration,
            state_tx_success,
            state_tx_failure,
            state_gas_used,
            storage_block_persist_duration,
            storage_wal_write_duration,
            storage_wal_entries,
            channel_drops,
            rpc_requests,
            rpc_request_duration,
        }
    }

    /// Encode all metrics in Prometheus text exposition format.
    pub fn encode(&self) -> String {
        let mut buf = String::new();
        let registry = self.registry.lock().unwrap();
        prometheus_client::encoding::text::encode(&mut buf, &registry).unwrap();
        buf
    }
}
