use super::*;

#[test]
fn managed_block_upsert_is_idempotent_and_preserves_surrounding_bytes() {
    let previous = "# Law\r\nbefore\n<!-- tokenzero:rust-core:start -->\nold\n<!-- tokenzero:rust-core:end -->\r\nafter\r\n";
    let merged = merge_instructions(previous, McpToolSurface::Classic).expect("merge");
    assert!(merged.starts_with("# Law\r\nbefore\n"));
    assert!(merged.ends_with("after\r\n"));
    assert!(merged.contains("<!-- tokenzero:rust-core:end -->\r\nafter\r\n"));
    assert!(!merged.contains("\nold\n"));
    assert_eq!(merged.matches(INSTRUCTIONS_START).count(), 1);
    assert_eq!(merged.matches(INSTRUCTIONS_END).count(), 1);
    assert_eq!(
        merge_instructions(&merged, McpToolSurface::Classic).expect("repeat merge"),
        merged
    );
}

#[test]
fn managed_block_at_eof_preserves_the_missing_final_newline() {
    let previous = "law\n<!-- tokenzero:rust-core:start -->\nold\n<!-- tokenzero:rust-core:end -->";
    let merged = merge_instructions(previous, McpToolSurface::Classic).expect("merge");
    assert!(merged.ends_with(INSTRUCTIONS_END));
    assert!(!merged.ends_with('\n'));
}

#[test]
fn malformed_managed_markers_fail_without_a_replacement() {
    let previous = "project law\n<!-- tokenzero:rust-core:start -->\nunterminated\n";
    let err = merge_instructions(previous, McpToolSurface::Classic)
        .expect_err("malformed markers must fail closed");
    assert_eq!(err.kind(), ErrorKind::InvalidData);
    assert!(err.to_string().contains("malformed or duplicate"));
}

#[test]
fn generated_mcp_args_use_direct_hub_transport() {
    let root = Path::new("/tmp/tokenzero-project");
    let args = mcp_args(root);
    assert_eq!(args.first().map(String::as_str), Some("mcp-server"));
    assert!(!args.iter().any(|arg| arg == "--supervise"));
}
