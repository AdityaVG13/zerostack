//! CLI MCP entrypoint delegated to the hub-backed compatibility adapter.

/// Start GraphZero's thin ZeroStack FastMCP adapter.
pub fn run() -> ! {
    graphzero_mcp_compat::run()
}
