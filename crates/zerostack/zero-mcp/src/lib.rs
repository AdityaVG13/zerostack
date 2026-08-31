#![forbid(unsafe_code)]

//! MCP carrier for ZeroKernel. This crate maps the single `zero` tool to the
//! direct ZeroKernel executor. Domain engines never register tools or catalogs here.

mod mcp_transport;

#[cfg(feature = "fastmcp")]
pub use mcp_transport::FastMcpZeroCarrier;
pub use mcp_transport::{
    DEFAULT_MCP_MAX_INFLIGHT, DEFAULT_MCP_TOOL_TIMEOUT, MAX_MCP_MAX_INFLIGHT, MAX_MCP_TOOL_TIMEOUT,
    McpCallContext, McpDispatchError, McpTransportConfig, McpTransportError,
    ZERO_CARRIER_MESSAGE_BYTE_LIMIT, ZERO_CARRIER_PLAN_BYTE_LIMIT, ZERO_CARRIER_TOOL_NAME,
    ZeroCarrierCapabilities, ZeroCarrierDispatcher, ZeroCarrierExecutor, ZeroCarrierRequest,
    ZeroCarrierSampling, decode_zero_carrier_request, execute_call, execute_call_with_cancel,
    render_zero_carrier_response, zero_carrier_catalog,
};
