//! Per-TCP-connection peer metadata for loopback UID checks (FR-2955).

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::connect_info::Connected;
use axum::serve::IncomingStream;
use tokio::net::TcpListener;

/// Connection-scoped state: one instance is created per accepted TCP connection
/// and reused for every HTTP request on that connection (keep-alive).
#[derive(Clone)]
pub struct PeerConnectInfo {
    pub addr: SocketAddr,
    pub loopback_uid: Arc<Mutex<Option<Result<(), ()>>>>,
}

impl PeerConnectInfo {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            loopback_uid: Arc::new(Mutex::new(None)),
        }
    }
}

impl Connected<IncomingStream<'_, TcpListener>> for PeerConnectInfo {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        PeerConnectInfo::new(*stream.remote_addr())
    }
}
