//! Local web dashboard (SPEC-web-dashboard, slice A: FR-520..FR-530).
//!
//! API contract shared by `src/main.rs` (CLI/startup) and `src/app.rs`
//! (in-process publishing). Implementation lives in the submodules.

mod assets;
mod auth;
mod file_source;
mod hub;
mod server;
mod snapshot;

pub use auth::{BindCheck, check_bind, dashboard_url, generate_token};
pub use file_source::FileGraphSource;
pub use hub::DashboardHub;
pub use server::{DashboardServer, ServerConfig, build_router, start_server};
pub use snapshot::{
    AgentView, DashboardSnapshot, GraphCounts, GraphView, NodeView, SNAPSHOT_SCHEMA,
    SnapshotSource, snapshot_from_run,
};

/// Default dashboard port (OQ-520). If it is taken, an ephemeral port is used.
pub const DEFAULT_PORT: u16 = 7420;
