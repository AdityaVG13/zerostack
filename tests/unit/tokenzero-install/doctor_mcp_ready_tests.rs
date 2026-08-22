    use super::*;

    #[test]
    fn blocked_workspace_is_never_mcp_ready() {
        assert!(!mcp_surface_ready(false));
    }

    #[test]
    fn mcp_surface_value_rejects_unknown_launch_mode() {
        assert!(mcp_tool_surface_value_ok("mcp"));
        assert!(mcp_tool_surface_value_ok(""));
        assert!(!mcp_tool_surface_value_ok("not-a-surface"));
    }

    #[test]
    fn mcp_idle_timeout_value_rejects_non_integers() {
        assert!(mcp_idle_timeout_value_ok("30"));
        assert!(!mcp_idle_timeout_value_ok("nope"));
    }

