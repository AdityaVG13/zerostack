//! GraphZero operator capabilities must not advertise a live V6 model catalog.

#[test]
fn capabilities_json_does_not_advertise_v6_or_codemode_plan() {
    let hay = graphzero::agent_cli::capabilities_json();
    for token in [
        "zero.graph.",
        "zero.fs.",
        "zero.token.",
        "graphzero code-mode",
        "install graphzero-mcp",
        "zerostack-codemode-host",
        "zsx",
    ] {
        assert!(
            !hay.contains(token),
            "capabilities advertises {token}:\n{hay}"
        );
    }
}
