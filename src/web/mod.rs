//! Local web dashboard (SPEC-web-dashboard, slice A: FR-520..FR-530).
//!
//! API contract shared by `src/main.rs` (CLI/startup) and `src/app.rs`
//! (in-process publishing). Implementation lives in the submodules.

pub mod assets;
mod auth;
pub mod commands;
pub mod discovery;
pub mod docs_api;
mod file_source;
mod hub;
pub mod layout;
pub mod parity;
mod server;
mod snapshot;

pub use auth::{BindCheck, check_bind, dashboard_url, generate_token};
pub use docs_api::{DocsService, OpenTickets};
pub use file_source::FileGraphSource;
pub use hub::DashboardHub;
pub use layout::{NodeLayout, layout_graph};
pub use server::{
    DashboardServer, DashboardServices, ServerConfig, build_router, build_router_with, start_server,
};
pub use snapshot::{
    AgentView, DashboardSnapshot, GraphCounts, GraphView, NodeDetail, NodeView,
    SESSION_MESSAGE_MAX_BYTES, SESSION_TRANSCRIPT_MAX_BYTES, SESSION_TRANSCRIPT_MAX_MESSAGES,
    SESSION_TRUNCATED_MARKER, SNAPSHOT_SCHEMA, SessionAgent, SessionCommandInfo,
    SessionConversation, SessionMessage, SessionModel, SessionView, SnapshotSource, UiTheme,
    cap_session_message, cap_session_transcript, snapshot_from_run, truncate_with_ellipsis,
};

/// Default dashboard port (OQ-520). If it is taken, an ephemeral port is used.
pub const DEFAULT_PORT: u16 = 7420;
