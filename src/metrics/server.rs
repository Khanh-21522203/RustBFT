use std::net::SocketAddr;
use std::sync::Arc;

use axum::{extract::State, routing::get, Router};

use crate::metrics::registry::Metrics;

/// Metrics server configuration (doc 14 section 2.1).
#[derive(Clone, Debug)]
pub struct MetricsConfig {
    pub listen_addr: String,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:26660".to_string(),
        }
    }
}

/// Standalone HTTP server that exposes `GET /metrics` in Prometheus text format.
/// Runs on a separate port (26660) from the RPC server (26657).
pub struct MetricsServer {
    pub config: MetricsConfig,
    pub metrics: Arc<Metrics>,
}

impl MetricsServer {
    pub fn new(config: MetricsConfig, metrics: Arc<Metrics>) -> Self {
        Self { config, metrics }
    }

    pub async fn run(self) -> Result<(), anyhow::Error> {
        let app = Router::new()
            .route("/metrics", get(handle_metrics))
            .with_state(self.metrics);

        let addr: SocketAddr = self.config.listen_addr.parse()?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn handle_metrics(State(metrics): State<Arc<Metrics>>) -> String {
    metrics.encode()
}
