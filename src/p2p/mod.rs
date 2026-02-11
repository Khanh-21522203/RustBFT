pub mod config;
pub mod msg;
pub mod codec;
pub mod peer;
pub mod manager;
pub mod gossip;

pub use config::P2pConfig;
pub use manager::{P2pHandle, P2pManager};
