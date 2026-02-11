use std::sync::Arc;
use std::net::SocketAddr;

use axum::{
    extract::State,
    routing::post,
    Json, Router,
};

use crate::rpc::handlers::{dispatch, RpcState};
use crate::rpc::types::{JsonRpcRequest, JsonRpcResponse, ERR_INVALID_REQUEST};

/// RPC server configuration.
#[derive(Clone, Debug)]
pub struct RpcConfig {
    pub listen_addr: String,
    pub max_request_size: usize,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:26657".to_string(),
            max_request_size: 1024 * 1024, // 1MB
        }
    }
}

/// The RPC server wraps an axum HTTP server serving JSON-RPC 2.0 over POST.
pub struct RpcServer {
    pub config: RpcConfig,
    pub state: Arc<RpcState>,
}

impl RpcServer {
    pub fn new(config: RpcConfig, state: Arc<RpcState>) -> Self {
        Self { config, state }
    }

    /// Run the RPC server. This is async and should be spawned on the Tokio runtime.
    pub async fn run(self) -> Result<(), anyhow::Error> {
        let app = Router::new()
            .route("/", post(handle_jsonrpc))
            .with_state(self.state);

        let addr: SocketAddr = self.config.listen_addr.parse()?;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;
        Ok(())
    }
}

async fn handle_jsonrpc(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<JsonRpcRequest>,
) -> Json<JsonRpcResponse> {
    if req.jsonrpc != "2.0" {
        return Json(JsonRpcResponse::error(
            req.id,
            ERR_INVALID_REQUEST,
            "jsonrpc must be '2.0'".into(),
        ));
    }
    let resp = dispatch(&state, req).await;
    Json(resp)
}
