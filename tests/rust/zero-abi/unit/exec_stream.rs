//! Unit tests for typed stream events (V6-R15, ZS-ADAPTER-007 streaming
//! channel): event vocabulary, terminal-event contract, sequence numbers.

use super::*;

#[test]
fn terminal_events_are_completed_and_failed_only() {
    let started = ExecStreamEventV1::StepStarted { seq: 1, node_id: "a".into() };
    assert!(!started.is_terminal());
    let completed = ExecStreamEventV1::Completed {
        seq: 3,
        trace_root: "root".into(),
        result_digest: "d".into(),
    };
    assert!(completed.is_terminal());
    let failed = ExecStreamEventV1::Failed {
        seq: 3,
        failure_code: "OpFailed".into(),
        detail: "boom".into(),
    };
    assert!(failed.is_terminal());
}

#[test]
fn sequences_are_monotonic_across_variants() {
    let events = vec![
        ExecStreamEventV1::PlanStarted { seq: 0, plan_digest: "p".into(), total_nodes: 2 },
        ExecStreamEventV1::StepStarted { seq: 1, node_id: "a".into() },
        ExecStreamEventV1::Progress { seq: 2, node_id: "a".into(), detail: "working".into() },
        ExecStreamEventV1::StepCompleted {
            seq: 3,
            node_id: "a".into(),
            step_receipt: StepReceiptV1 {
                node_id: "a".into(),
                result_digest: "d".into(),
                output_bytes: 12,
            },
        },
        ExecStreamEventV1::Completed { seq: 4, trace_root: "r".into(), result_digest: "m".into() },
    ];
    let seqs: Vec<u64> = events.iter().map(ExecStreamEventV1::seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3, 4]);
    assert_eq!(events[1].node_id(), Some("a"));
    assert_eq!(events[4].node_id(), None);
}

#[test]
fn events_round_trip_through_json_with_stable_tags() {
    let event = ExecStreamEventV1::StepCompleted {
        seq: 3,
        node_id: "a".into(),
        step_receipt: StepReceiptV1 {
            node_id: "a".into(),
            result_digest: "d".into(),
            output_bytes: 12,
        },
    };
    let json = serde_json::to_value(&event).expect("serializes");
    assert_eq!(json["event"], "StepCompleted");
    let decoded: ExecStreamEventV1 = serde_json::from_value(json).expect("deserializes");
    assert_eq!(decoded, event);
}
