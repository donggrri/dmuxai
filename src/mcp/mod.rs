//! Shared MCP server configuration, per-runner propagation, the dashboard's
//! MCP client and a deterministic mock server used by tests.
//! See docs/specs/SPEC-web-dashboard.md slice C and the PLAN "Slice C contract".

pub mod client;
pub mod config;
pub mod inject;
#[doc(hidden)]
pub mod mock;
pub mod propagate;
pub mod registry;

pub use config::{McpServerSpec, ResolvedMcpServer, ServerKind, Transport, resolve_servers};
pub use inject::RunnerMcp;
