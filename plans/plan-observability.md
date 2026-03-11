# Feature: Observability

## 1. Purpose

The Observability module provides operators with complete visibility into a running RustBFT node: Prometheus metrics for dashboards and alerting, structured JSON logs for debugging, and HTTP health/readiness endpoints for container orchestration. It is a shared infrastructure layer used by every other module — each module calls into the observability registry to record counters, histograms, and gauge updates without owning any I/O code.

## 2. Responsibilities

- Register and expose all Prometheus metrics (counters, gauges, histograms) for every module
- Serve metrics on `GET /metrics` (port 26660, separate from RPC port 26657)
- Initialize the `tracing` subscriber with structured JSON output and configurable per-module log levels
- Provide a span-based context that automatically attaches `height`, `round`, and `node_id` to all log entries within a consensus event
- Serve `GET /health` and `GET /ready` endpoints with per-subsystem checks
- Support runtime log-level changes via an admin endpoint
- Support graceful shutdown: flush final metrics before process exit

## 3. Non-Responsibilities

- Does not interpret metric values or make operational decisions
- Does not run Prometheus or Grafana — only exposes the scrape endpoint
- Does not store log history — logs go to stdout or a rolling file; log aggregation is external
- Does not implement distributed tracing (OpenTelemetry spans) — out of scope for MVP
- Does not alert — alerting rules live in Prometheus/Alertmanager configuration

## 4. Architecture Design

```
Every module
    |
    | counters.increment() / histograms.observe() / gauges.set()
    v
+------------------------------------------+
|   MetricsRegistry (src/metrics/mod.rs)   |
|   prometheus::Registry (shared Arc)       |
|   All metric handles pre-registered      |
+------------------------------------------+
    |
    | GET /metrics (Tokio HTTP handler)
    v
Prometheus Scraper (port 26660)

Every module (log events)
    |
    | tracing::info! / debug! / warn! / error!
    v
+------------------------------------------+
|   tracing-subscriber (JSON formatter)    |
|   EnvFilter: RUST_LOG or config file     |
|   Output: stdout or rolling file         |
+------------------------------------------+

Health checks
    |
    v
+------------------------------------------+
|   HealthChecker (src/health/mod.rs)      |
|   GET /health  → liveness  (port 26660)  |
|   GET /ready   → readiness (port 26660)  |
+------------------------------------------+
```

## 5. Core Data Structures (Rust)

```rust
// src/metrics/mod.rs

pub struct Metrics {
    pub consensus: ConsensusMetrics,
    pub p2p: P2PMetrics,
    pub mempool: MempoolMetrics,
    pub state: StateMetrics,
    pub contracts: ContractMetrics,
    pub storage: StorageMetrics,
    pub channels: ChannelMetrics,
    pub rpc: RpcMetrics,
}

pub struct ConsensusMetrics {
    pub height: IntGauge,
    pub round: IntGauge,
    pub step: IntGaugeVec,          // label: step
    pub block_commit_duration: Histogram,
    pub round_duration: HistogramVec,
    pub rounds_per_height: Histogram,
    pub proposals_received: CounterVec,    // label: valid (true/false)
    pub votes_received: CounterVec,        // labels: type, valid
    pub timeouts_total: CounterVec,        // label: type
    pub equivocations_total: IntCounter,
}

pub struct HealthStatus {
    pub healthy: bool,
    pub checks: BTreeMap<String, SubsystemHealth>,
}

pub struct SubsystemHealth {
    pub status: HealthStatusKind,
    pub detail: Option<String>,
}

pub enum HealthStatusKind { Ok, Degraded, Down }

// Shared state read by health checks
pub struct HealthProbes {
    pub last_committed_at: Arc<AtomicU64>,   // unix timestamp of last commit
    pub peer_count: Arc<AtomicUsize>,
    pub storage_latest_height: Arc<AtomicU64>,
    pub consensus_height: Arc<AtomicU64>,
    pub mempool_size: Arc<AtomicUsize>,
    pub mempool_max_size: usize,
}
```

## 6. Public Interfaces

```rust
// src/metrics/mod.rs

/// Initialize metrics registry. Called once at startup.
pub fn init(node_id: &str) -> Arc<Metrics>;

/// Start the metrics + health HTTP server (separate port from RPC).
pub async fn start_server(
    listen_addr: SocketAddr,
    metrics: Arc<Metrics>,
    health: Arc<HealthProbes>,
);

// src/logging/mod.rs

pub struct LoggingConfig {
    pub format: LogFormat,         // Json | Text
    pub level: tracing::Level,
    pub module_levels: String,     // "rustbft_consensus=debug,rustbft_p2p=info"
    pub output: LogOutput,         // Stdout | File(path)
    pub file_rotation: Option<FileRotation>,
}

pub enum LogFormat { Json, Text }
pub enum LogOutput { Stdout, File(PathBuf) }
pub enum FileRotation { Daily, SizeMb(u64) }

/// Initialize the tracing subscriber. Called once before spawning any threads.
pub fn init_logging(config: &LoggingConfig);

// Consensus span macro — attaches height/round to all logs within a scope
macro_rules! consensus_span {
    ($height:expr, $round:expr) => {
        tracing::info_span!("consensus", height = $height, round = $round)
    }
}
```

## 7. Internal Algorithms

### Metrics Server
```
async fn serve_metrics(registry, listen_addr):
    listener = TcpListener::bind(listen_addr).await?
    loop:
        (conn, _) = listener.accept().await?
        tokio::spawn(handle_metrics_conn(conn, registry.clone()))

async fn handle_metrics_conn(conn, registry):
    request = read_http_request(conn)
    match request.path:
        "/metrics" =>
            body = prometheus::encode_text_format(registry)
            respond 200, "text/plain; version=0.0.4", body
        "/health" =>
            status = compute_health(health_probes)
            code = if status.healthy: 200 else: 503
            respond code, "application/json", serde_json::to_string(status)
        "/ready" =>
            status = compute_readiness(health_probes)
            code = if status.ready: 200 else: 503
            respond code, "application/json", body
        _ =>
            respond 404, "text/plain", "not found"
```

### Health Check Computation
```
fn compute_health(probes) -> HealthStatus:
    now = unix_timestamp()
    checks = {}

    // Consensus: height must advance in last 30s
    last_commit = probes.last_committed_at.load()
    consensus_ok = (now - last_commit) < 30
    checks["consensus"] = if consensus_ok: Ok else: Down("stalled")

    // Peers: at least 1 connected
    peer_ok = probes.peer_count.load() >= 1
    checks["peers"] = if peer_ok: Ok else: Down("no peers")

    // Storage: within 1 height of consensus
    storage_h = probes.storage_latest_height.load()
    consensus_h = probes.consensus_height.load()
    storage_ok = consensus_h <= storage_h + 1
    checks["storage"] = if storage_ok: Ok else: Degraded("lagging")

    // Mempool: not completely full
    mempool_ok = probes.mempool_size.load() < probes.mempool_max_size
    checks["mempool"] = if mempool_ok: Ok else: Degraded("full")

    healthy = checks.values().all(|c| c.status != Down)
    HealthStatus { healthy, checks }
```

### Structured Log Format
```
Every log call with tracing::info!/debug!/warn!/error! produces:
{
    "timestamp": "2024-01-15T10:30:45.123456Z",
    "level": "INFO",
    "target": "rustbft_consensus",   // module path
    "node_id": "node0",
    "height": 42,                    // from span
    "round": 0,                      // from span
    "message": "Committed block",
    "block_hash": "abcdef12...",
    "state_root": "...",
    "tx_count": 5,
    "duration_ms": 234
}
```

### Runtime Log Level Change
```
POST /admin/log_level  (localhost only, no auth for MVP)
{
    "target": "rustbft_consensus",
    "level": "TRACE"
}

Implementation:
    reload_handle = tracing_subscriber::reload::Handle
    reload_handle.modify(|filter| filter.add_directive(new_directive))
```

## 8. Persistence Model

No persistent state. All metrics are in-memory counters/gauges reset on process restart. Logs are written to stdout or a rolling file managed by `tracing-appender`. Log files are not parsed by the node — they are consumed by external log aggregation tools (e.g., Loki, Elasticsearch).

## 9. Concurrency Model

The `Arc<Metrics>` handle is cloned into every module at startup. Prometheus's `IntCounter`, `Gauge`, and `Histogram` types are internally thread-safe (atomic operations). No `Mutex` is needed for metric updates.

The `tracing` subscriber is installed globally via `tracing::subscriber::set_global_default` and is inherently thread-safe. Log calls from any thread are safe.

The health probes use `Arc<AtomicU64>` / `Arc<AtomicUsize>` — updated from multiple threads with `Relaxed` ordering (they are advisory, not safety-critical). The metrics HTTP server runs as a single Tokio task.

## 10. Configuration

```toml
[observability]
metrics_enabled = true
metrics_listen_addr = "0.0.0.0:26660"

[logging]
format = "json"                    # "json" | "text"
level = "info"
module_levels = "rustbft_consensus=debug,rustbft_p2p=info"
output = "stdout"                  # "stdout" | "file"
file_path = "./logs/rustbft.log"   # Used if output = "file"
file_rotation = "daily"            # "daily" | "size:100MB"
```

## 11. Observability

The observability module observes itself minimally:

- `rustbft_metrics_scrape_duration_seconds` (Histogram) — time to render the `/metrics` response
- Log INFO `"Metrics server started"` with `{listen_addr}` on startup
- Log WARN if the metrics server fails to bind to the configured address
- Log INFO `"Log level changed"` with `{target, old_level, new_level}` on runtime changes

All other metrics are owned by their respective modules and merely registered with the shared `Arc<prometheus::Registry>`.

## 12. Testing Strategy

- **`test_metrics_registration`**: all metric handles are created without name conflicts → `registry.gather()` succeeds
- **`test_counter_increment`**: increment a counter, render metrics text → counter value is 1
- **`test_histogram_observe`**: observe a value, render metrics → bucket counts are correct
- **`test_gauge_set`**: set a gauge to 42, render metrics → value is "42"
- **`test_health_endpoint_healthy`**: all probes in healthy range → GET /health returns 200 with `{"healthy": true}`
- **`test_health_endpoint_no_peers`**: set peer_count=0 → GET /health returns 503
- **`test_health_consensus_stalled`**: set last_committed_at to 60s ago → consensus check is Down
- **`test_ready_endpoint_syncing`**: node catching_up=true → GET /ready returns 503
- **`test_json_log_format`**: emit a tracing info event, capture output → valid JSON with required fields
- **`test_log_level_filter`**: set level=INFO, emit a DEBUG event → DEBUG event not written to output
- **`test_runtime_log_level_change`**: change level to DEBUG via reload handle → subsequent DEBUG events appear
- **`test_metrics_server_responds`**: start server, send HTTP GET /metrics → 200 with prometheus text format
- **`test_consensus_span_attaches_height`**: enter a consensus span with height=5, emit log → log JSON contains `"height": 5`

## 13. Open Questions

- **OpenTelemetry traces**: Distributed traces would help diagnose latency across multiple nodes (e.g., time from proposal broadcast to prevote received). Deferred post-MVP; the `tracing` library already uses spans that could be connected to an OTLP exporter.
- **Alertmanager integration**: Alerting rules are defined in Prometheus config files outside the node binary. The node only needs to expose the correct metrics; alert thresholds are an operational concern.
