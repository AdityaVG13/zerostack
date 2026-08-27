//! `gz://codemode/execution/<id>[/part]` ref parse/round-trip contract (FR-017).

use graphzero_store::store::refs::{CodeModeExecutionPart, GzRef};

#[test]
fn codemode_execution_ref_parses_default_and_named_parts() {
    let id = "cm_exec_01";
    let cases = [
        (
            "gz://codemode/execution/cm_exec_01",
            CodeModeExecutionPart::Execution,
        ),
        (
            "gz://codemode/execution/cm_exec_01/code",
            CodeModeExecutionPart::Code,
        ),
        (
            "gz://codemode/execution/cm_exec_01/steps",
            CodeModeExecutionPart::Steps,
        ),
        (
            "gz://codemode/execution/cm_exec_01/telemetry",
            CodeModeExecutionPart::Telemetry,
        ),
        (
            "gz://codemode/execution/cm_exec_01/result",
            CodeModeExecutionPart::Result,
        ),
        (
            "gz://codemode/execution/cm_exec_01/error",
            CodeModeExecutionPart::Error,
        ),
    ];
    for (input, expected_part) in cases {
        let gz = GzRef::parse(input).unwrap();
        match gz {
            GzRef::CodeModeExecution {
                id: parsed_id,
                ref part,
            } => {
                assert_eq!(parsed_id, id);
                assert_eq!(*part, expected_part);
                assert_eq!(GzRef::parse(input).unwrap().to_string(), input);
            }
            other => panic!("expected CodeModeExecution, got {other:?}"),
        }
    }
}

#[test]
fn codemode_execution_ref_rejects_malformed_paths() {
    for input in [
        "gz://codemode/not_execution/x",
        "gz://codemode/execution/",
        "gz://codemode/execution/../evil/code",
        "gz://codemode/execution/id/extra/segment",
        "gz://codemode/execution/id/not_a_part",
    ] {
        assert!(GzRef::parse(input).is_err(), "should reject: {input}");
    }
}

#[test]
fn codemode_execution_part_parse_defaults_to_execution() {
    let part = CodeModeExecutionPart::parse(None, "gz://codemode/execution/x").unwrap();
    assert_eq!(part, CodeModeExecutionPart::Execution);
}
