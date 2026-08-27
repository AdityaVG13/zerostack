//! MCP protocol layer — dual support for legacy stdio and 2026-07-28 stateless HTTP.
//!
//! Stateless HTTP transport is feature-gated (`mcp-http`, default-on for lib
//! API / examples). Product `fszero-mcp` / `fszero-codemode` installs use
//! `--no-default-features` and do not enable `mcp-http` (stdio / FastMCP only).

mod handler;
#[cfg(feature = "mcp-http")]
mod http;
mod meta;
pub mod raw_worker;
mod request_guard;
mod stdio;
pub mod surface;
mod version;

pub use crate::mcp_rpc::TOOLS_LIST_TTL_MS;
pub use handler::{McpHandler, TransportProfile, tool_name_from_params};
#[cfg(feature = "mcp-http")]
pub use http::HttpMcpServer;
pub use meta::{RequestMeta, effective_protocol_version, extract_request_meta};
pub use raw_worker::{
    raw_worker_call_once, raw_worker_requested, run_raw_worker_stdio,
    supports_handshake_and_call_frames,
};
pub use request_guard::{
    DEFAULT_REQUEST_TIMEOUT_MS, REQUEST_CLEANUP_BOUND_MS, RPC_REQUEST_CANCELLED,
    RPC_REQUEST_DEADLINE, RequestGuard, deadline_error_data, resolve_request_timeout_ms,
};
pub use stdio::run_stdio_server;
pub use surface::{
    SurfaceKind, assert_server_surface_boundary, resolve_codemode_response, tools_list_for_surface,
};
pub use version::{
    PROTOCOL_2025, PROTOCOL_LEGACY, PROTOCOL_RC, SUPPORTED_VERSIONS, is_stateless_version,
    negotiate_version,
};

/// Legacy FastMCP stdio server entry.
///
/// Transport ownership moved to the `fszero-mcp` package, which runs the hub
/// `zero-codemode/fastmcp` adapter (fszero-xg53). The retired root transport
/// (`src/mcp_protocol/fastmcp_mode.rs`) was deleted under the hub cutover;
/// this stub keeps API compatibility for any remaining caller and fails
/// closed with a precise diagnostic.
pub fn run_fastmcp_server() -> Result<(), String> {
    Err( "fszero: the FastMCP transport now lives in the fszero-mcp package (hub zero-codemode/fastmcp adapter). Install fszero-mcp instead of calling the root library server.".into(),)
}
