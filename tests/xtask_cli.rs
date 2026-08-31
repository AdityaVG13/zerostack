use std::process::Command;

#[test]
fn doctor_json_reports_the_zerostack_repository() {
    let output = Command::new(env!("CARGO_BIN_EXE_zerostack-xtask"))
        .args(["doctor", "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["repository"], "zerostack");
    assert_eq!(report["healthy"], true);
    assert_eq!(report["missing"], serde_json::json!([]));
}
