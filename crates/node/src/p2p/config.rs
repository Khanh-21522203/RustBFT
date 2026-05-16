#[derive(Clone, Debug)]
pub struct P2pConfig {
    pub listen_addr: String,
    pub seeds: Vec<String>, // "node@ip:port"
    pub max_peers: usize,

    pub handshake_timeout_ms: u64,
    pub ping_interval_ms: u64,
    pub pong_timeout_ms: u64,

    pub max_message_size_bytes: usize,

    pub inbound_buffer_size: usize,
    pub outbound_buffer_size: usize,

    pub reconnect_backoff_max_ms: u64,
    pub peer_ban_duration_ms: u64,
}

impl Default for P2pConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:26656".to_string(),
            seeds: vec![],
            max_peers: 50,
            handshake_timeout_ms: 5000,
            ping_interval_ms: 10_000,
            pong_timeout_ms: 5000,
            max_message_size_bytes: 4 * 1024 * 1024,
            inbound_buffer_size: 64,
            outbound_buffer_size: 64,
            reconnect_backoff_max_ms: 60_000,
            peer_ban_duration_ms: 300_000,
        }
    }
}
