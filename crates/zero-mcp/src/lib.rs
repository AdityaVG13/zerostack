#![forbid(unsafe_code)]

//! Domain-neutral MCP transport authority.
//!
//! `zero-mcp` owns the FastMCP compatibility carrier: tool registration,
//! bounded callback execution, cancellation hooks, and FastMCP stdio
//! lifecycle. Engine adapters provide a validated
//! [`zero_abi::SurfaceRegistration`] and callbacks that own operation
//! semantics; this crate does not import an engine crate or execute CodeMode
//! plans.
//!
//! The 2026-08-17 receive audit of
//! `GraphZero/crates/graphzero-mcp-compat` found the compatibility crate
//! already delegates registration, bounded dispatch, cancellation, and stdio
//! lifecycle here. Its GraphZero operation catalog and callbacks remain a
//! domain adapter; no second MCP transport is received.

pub mod mcp_transport;

#[cfg(feature = "fastmcp")]
pub use mcp_transport::FastMcpTransport;
pub use mcp_transport::{
    DEFAULT_MCP_MAX_INFLIGHT, DEFAULT_MCP_TOOL_TIMEOUT, MAX_MCP_MAX_INFLIGHT, MAX_MCP_TOOL_TIMEOUT,
    McpAliasMetadata, McpCallContext, McpDispatchError, McpDispatchOutput, McpDispatcher,
    McpErrorPresentation, McpResourceOutput, McpResourceReader, McpServerIdentity, McpTextContent,
    McpTransportConfig, McpTransportError, execute_call, execute_call_with_cancel,
    validate_mcp_registration,
};
// Surface-registration contract authority, re-exported from zero-abi so engine
// MCP adapters can consume registration and transport from one crate.
pub use zero_abi::{
    CapabilityDescriptor, DomainAdapterRegistration, GlobalRegistration, RegistrationError,
    SURFACE_CONTRACT_VERSION, SurfaceContractError, SurfaceKind, SurfaceRegistration,
};
