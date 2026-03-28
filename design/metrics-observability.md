# Metrics and Observability

## Purpose

Exposes Prometheus metrics on port 26660 (`GET /metrics`) and structured logs via `tracing`. Together they provide runtime visibility into consensus progress, network activity, and storage performance.

## Scope

**In scope:**
- `Metrics` registry with counters, gauges, and histograms for consensus, P2P, state, storage, RPC, and channels
- `MetricsServer` — standalone axum HTTP server on `GET /metrics`
- Structured logging via `tracing` with JSON or text format, configurable log level and per-module filters

**Out of scope:**
- Metrics collection for P2P message counts/bytes (metrics registered but never incremented)
- RPC request duration tracking (registered but not wired)
- Alerting rules, dashboards, or Grafana configuration
- Distributed tracing (no OpenTelemetry spans)

## Primary User Flow

**Metrics scraping:**
1. Prometheus scrapes `GET http://<node>:26660/metrics`.
2. `handle_metrics` calls `Metrics::encode()` → `prometheus_client::encoding::text::encode`.
3. Returns Prometheus text exposition format.

**Logging:**
1. `main.rs:init_logging` called at startup with `LoggingSection` config.
2. Sets up `tracing_subscriber` with either JSON or text format.
3. Log level from `RUST_LOG` env var or `logging.level` config, with optional per-module overrides via `logging.module_levels`.

## System Flow

```
main.rs
    │
    ├── Metrics::new() (src/metrics/registry.rs)
    │       Registers all metrics with a prometheus_client Registry
    │       Arc<Metrics> shared with CommandRouter
    │
    ├── MetricsServer::new(MetricsConfig, Arc<Metrics>)
    │       tokio::spawn(metrics_server.run())
    │           axum Router: GET /metrics → handle_metrics
    │                        handle_metrics → metrics.encode() → text response
    │
    └── init_logging(cfg: &LoggingSection)
            EnvFilter from RUST_LOG or cfg.module_levels or cfg.level
            JSON format → tracing_subscriber::fmt().json()
            Text format → tracing_subscriber::fmt()
```

## Data Model

`Metrics` (`src/metrics/registry.rs`) — all fields are `Clone`, backed by atomics:

**Consensus** (updated by `CommandRouter`):
- `consensus_height: Gauge<i64>` — set to block height on each PersistBlock
- `consensus_round: Gauge<i64>` — never updated in MVP
- `consensus_proposals_received: Counter` — never updated in MVP
- `consensus_votes_received: Counter` — never updated in MVP
- `consensus_timeouts: Counter` — never updated in MVP
- `consensus_equivocations: Counter` — never updated in MVP
- `consensus_block_commit_duration: Histogram` — exponential buckets 0.01s to ~40s
- `consensus_rounds_per_height: Histogram` — exponential buckets 1..256

**Networking** (none updated in MVP):
- `p2p_peers_connected: Gauge<i64>`
- `p2p_messages_sent/received: Counter`
- `p2p_bytes_sent/received: Counter`

**State execution** (updated by `CommandRouter`):
- `state_block_execution_duration: Histogram` — exponential buckets 0.001s
- `state_tx_success/failure/gas_used: Counter` — never updated in MVP

**Storage** (updated by `CommandRouter`):
- `storage_block_persist_duration: Histogram` — exponential buckets 0.001s
- `storage_wal_write_duration: Histogram` — exponential buckets 0.0001s
- `storage_wal_entries: Gauge<i64>` — never updated

**Other**:
- `channel_drops: Counter` — never updated
- `rpc_requests: Counter`, `rpc_request_duration: Histogram` — never updated

`MetricsConfig` (`src/metrics/server.rs`):
- `listen_addr: String` — default `"0.0.0.0:26660"`

`LoggingSection` (`src/config/mod.rs`):
- `format: String` — `"json"` (default) or `"text"`
- `level: String` — default `"info"` (used as fallback if `RUST_LOG` unset)
- `module_levels: Option<String>` — e.g. `"rustbft::consensus=debug,rustbft::p2p=warn"`
- `output: String` — `"stdout"` (hardcoded to stdout; file output not implemented)
- `file_path: Option<String>` — defined but not used

## Interfaces and Contracts

**`GET /metrics`** — Prometheus text exposition format:
- All metric names prefixed with `rustbft_`
- Examples: `rustbft_consensus_height`, `rustbft_storage_block_persist_duration_seconds_bucket`

**`Metrics::new() -> Self`** — no arguments; initializes all metrics to zero.

**`Metrics::encode() -> String`** — takes `self.registry.lock()`, encodes to Prometheus text.

**Log fields** (structured key=value or JSON):
- `config_path`, `chain_id`, `node_id` — at startup
- `height`, `validators`, `total_power` — on startup after loading state
- `addr`, `seeds` — on P2P listener start
- `height` — on block commit (`"Block committed and persisted"`)
- `error` — on any subsystem failure

## Dependencies

**External crates:**
- `prometheus-client v0.22` — `Registry`, `Counter`, `Gauge`, `Histogram`, `exponential_buckets`, text encoder
- `tracing v0.1` — `info!`, `warn!`, `error!` macros used throughout
- `tracing-subscriber v0.3` — `fmt()` subscriber with JSON feature, `EnvFilter`
- `axum v0.7` — metrics HTTP server
- `tokio` — async runtime for metrics server

## Failure Modes and Edge Cases

- **Metrics server bind failure:** `MetricsServer::run()` returns `Err`; logged via `tracing::error!` but node continues.
- **`metrics.registry.lock()` contention:** All metrics instruments use atomics except the registry lock (only needed for encoding) — lock contention only during `/metrics` scrapes.
- **Log output to file:** `logging.output = "file"` and `logging.file_path` are parsed but `init_logging` always writes to stdout.
- **JSON format with large payloads:** Block commit events include height but not tx count or bytes — no cardinality explosion risk.

## Observability and Debugging

The most useful metrics actually updated in MVP:
- `rustbft_consensus_height` — progress indicator (updated each committed block)
- `rustbft_state_block_execution_duration_seconds` — execution performance
- `rustbft_storage_block_persist_duration_seconds` — storage performance
- `rustbft_storage_wal_write_duration_seconds` — WAL write latency

Log lines to watch:
- `"Block committed and persisted"` with `height=N` — normal progress
- `"Block execution failed"` — critical error
- `"WAL truncate failed"` — storage warning
- `"Shutdown initiated"` — graceful shutdown started

## Risks and Notes

- Most P2P, RPC, and consensus counters are registered but never incremented — scraped metrics will be misleadingly zero.
- `consensus_height` is the only reliable progress signal in MVP.
- `logging.output = "file"` silently falls back to stdout.
- `Metrics` is `Clone` (all fields are `Clone`) — cloning shares the same underlying atomics, not a copy. This is intentional for sharing across threads.

Changes:

