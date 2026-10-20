//! Agent Client Protocol (ACP) JSON-RPC client over stdio (SPEC-acp-runners FR-1300..FR-1304).

mod client;
mod permissions;
mod updates;

pub use client::run_acp_turn;
pub use permissions::PermissionContext;

pub use dmuxai::runner_protocol::RunnerProtocol;
