# C4 Level 1: System Context

Shows external actors and systems that interact with a RustBFT node.

```mermaid
flowchart TB
    subgraph boundary["RustBFT Node (System Boundary)"]
        N[("⚙️ RustBFT Node\nBFT consensus engine\nRust / Tokio")]
    end

    Client[("👤 Client\nDApp / CLI / SDK")]
    Peer1[("🌐 Peer Node A\nRustBFT validator")]
    Peer2[("🌐 Peer Node B\nRustBFT validator")]
    Peer3[("🌐 Peer Node C\nRustBFT validator")]
    Prom[("📊 Prometheus\nMonitoring system")]
    Ops[("🔧 Operator\nDevOps / admin")]

    Client -->|"JSON-RPC 2.0\nHTTP POST /\nbroadcast_tx, get_block"| N
    Peer1 <-->|"TCP encrypted\nChaCha20-Poly1305\nProposals, Votes, Txs"| N
    Peer2 <-->|"TCP encrypted"| N
    Peer3 <-->|"TCP encrypted"| N
    Prom -->|"HTTP GET /metrics\nPrometheus scrape"| N
    Ops -->|"TOML config\ndata_dir / CLI flags"| N
```

## External Actors

| Actor | Role | Protocol |
|-------|------|----------|
| **Client** | Submits transactions, queries state | JSON-RPC 2.0 over HTTP |
| **Peer Node** | Fellow BFT validator, gossips proposals/votes | TCP with ChaCha20-Poly1305 encryption |
| **Prometheus** | Scrapes metrics for observability | HTTP `/metrics` endpoint |
| **Operator** | Configures and deploys node | TOML config file, CLI |
