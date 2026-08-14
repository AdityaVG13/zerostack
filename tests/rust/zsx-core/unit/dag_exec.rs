//! Unit tests for the DAG runtime (V6-R15, ZS-EXEC-001/002/005): dependent
//! ops are serialized and never reordered, independent ops are batchable,
//! parallel-vs-sequential traces are equivalent, the stream is typed with
//! exactly one completion, and buffering is bounded.

use super::*;
use zero_abi::exec_dag::{ExecDagV1, ExecNodeKindV1, ExecNodeV1};
use zero_abi::exec_stream::ExecStreamEventV1;
use zero_abi::exec_trace::ProtectedDecisionViewV1;
use zero_abi::{ContingentPolicyRuleV1, ContingentPolicyV1};

fn node(id: &str, weight: u64, deps: &[&str]) -> ExecNodeV1 {
    ExecNodeV1::new(id, ExecNodeKindV1::Op, weight, deps.iter().copied()).expect("valid node")
}

fn boundary(id: &str, deps: &[&str]) -> ExecNodeV1 {
    ExecNodeV1::new(id, ExecNodeKindV1::DecisionBoundary, 0, deps.iter().copied())
        .expect("valid boundary node")
}

/// Diamond plan: a -> (b, c) -> d.
fn diamond() -> ExecDagV1 {
    ExecDagV1::new(vec![
        node("d", 1, &["b", "c"]),
        node("c", 1, &["a"]),
        node("b", 1, &["a"]),
        node("a", 1, &[]),
    ])
}

/// Op that records its call order and returns a digest derived from its id.
fn recording_op(calls: &std::sync::Mutex<Vec<String>>) -> impl Fn(&ExecNodeV1) -> Result<DagNodeOutcomeV1, String> + Sync {
    move |node: &ExecNodeV1| {
        calls.lock().expect("call log lock").push(node.id.clone());
        Ok(DagNodeOutcomeV1 {
            result_digest: format!("digest:{}", node.id),
            protected_decision: None,
        })
    }
}

#[test]
fn dependent_ops_are_serialized_and_never_reordered_sequential() {
    let dag = diamond();
    let calls = std::sync::Mutex::new(Vec::new());
    let sink = StreamSinkV1::new(64).expect("sink");
    let outcome = DagExecutorV1::new()
        .execute(&dag, "input:1", None, &recording_op(&calls), &sink, ScheduleModeV1::Sequential)
        .expect("settled");
    // Sequential: exact dependency-respecting order, id-sorted within ready.
    assert_eq!(*calls.lock().expect("lock"), vec!["a", "b", "c", "d"]);
    // Trace records in the same deterministic order.
    let node_ids: Vec<&str> = outcome.trace.records.iter().map(|r| r.node_id.as_str()).collect();
    assert_eq!(node_ids, vec!["a", "b", "c", "d"]);
}

#[test]
fn dependent_ops_never_run_before_their_deps_in_parallel_mode() {
    let dag = diamond();
    let calls = std::sync::Mutex::new(Vec::new());
    let sink = StreamSinkV1::new(64).expect("sink");
    DagExecutorV1::new()
        .execute(&dag, "input:1", None, &recording_op(&calls), &sink, ScheduleModeV1::ParallelLayers)
        .expect("settled");
    let log = calls.lock().expect("lock");
    let index = |id: &str| log.iter().position(|x| x == id).expect("node ran");
    // a before both b and c; b and c before d. b/c may interleave.
    assert!(index("a") < index("b") && index("a") < index("c"));
    assert!(index("b") < index("d") && index("c") < index("d"));
}

#[test]
fn independent_ops_are_observable_as_batchable() {
    let dag = diamond();
    let layers = dag.layers().expect("valid dag");
    assert_eq!(layers, vec![vec!["a"], vec!["b", "c"], vec!["d"]]);
    // b and c share no transitive dependency path: batchable together.
    assert_eq!(layers[1].len(), 2);
}

#[test]
fn parallel_and_sequential_traces_are_equivalent() {
    let dag = diamond();
    let seq_calls = std::sync::Mutex::new(Vec::new());
    let par_calls = std::sync::Mutex::new(Vec::new());
    let seq_sink = StreamSinkV1::new(64).expect("sink");
    let par_sink = StreamSinkV1::new(64).expect("sink");
    let executor = DagExecutorV1::new();
    let sequential = executor
        .execute(&dag, "input:1", None, &recording_op(&seq_calls), &seq_sink, ScheduleModeV1::Sequential)
        .expect("sequential settles");
    let parallel = executor
        .execute(&dag, "input:1", None, &recording_op(&par_calls), &par_sink, ScheduleModeV1::ParallelLayers)
        .expect("parallel settles");
    // Same plan + same inputs => equivalent trace, regardless of schedule.
    let equivalence = sequential.trace.equivalence(&parallel.trace);
    assert!(
        equivalence.equivalent,
        "parallel trace must equal sequential trace: {:?}",
        equivalence.first_divergence.map(|d| d.describe())
    );
    assert_eq!(sequential.merged_result_digest, parallel.merged_result_digest);
    // Deterministic merge: neither mode reorders the trace records.
    let ids: Vec<&str> = parallel.trace.records.iter().map(|r| r.node_id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b", "c", "d"]);
}

#[test]
fn stream_emits_typed_events_ending_in_exactly_one_completion() {
    let dag = diamond();
    let calls = std::sync::Mutex::new(Vec::new());
    let sink = StreamSinkV1::new(64).expect("sink");
    DagExecutorV1::new()
        .execute(&dag, "input:1", None, &recording_op(&calls), &sink, ScheduleModeV1::Sequential)
        .expect("settled");
    let events = sink.drain();
    let terminals: Vec<_> = events.iter().filter(|e| e.is_terminal()).collect();
    assert_eq!(terminals.len(), 1, "exactly one terminal event");
    assert!(matches!(terminals[0], ExecStreamEventV1::Completed { .. }));
    // Every started node completes with a step receipt carrying its digest.
    let started: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            ExecStreamEventV1::StepStarted { node_id, .. } => Some(node_id.as_str()),
            _ => None,
        })
        .collect();
    let completed: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            ExecStreamEventV1::StepCompleted { node_id, .. } => Some(node_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(started, vec!["a", "b", "c", "d"]);
    assert_eq!(completed, vec!["a", "b", "c", "d"]);
    // PlanStarted is first, terminal last, sequences monotonic.
    assert!(matches!(events[0], ExecStreamEventV1::PlanStarted { .. }));
    let seqs: Vec<u64> = events.iter().map(ExecStreamEventV1::seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "monotonic sequences");
}

#[test]
fn step_completed_carries_receipt_with_node_digest() {
    let dag = diamond();
    let calls = std::sync::Mutex::new(Vec::new());
    let sink = StreamSinkV1::new(64).expect("sink");
    DagExecutorV1::new()
        .execute(&dag, "input:1", None, &recording_op(&calls), &sink, ScheduleModeV1::Sequential)
        .expect("settled");
    let events = sink.drain();
    let step: &ExecStreamEventV1 = events
        .iter()
        .find(|e| matches!(e, ExecStreamEventV1::StepCompleted { node_id, .. } if node_id == "b"))
        .expect("step completed event for b");
    match step {
        ExecStreamEventV1::StepCompleted { step_receipt, .. } => {
            assert_eq!(step_receipt.node_id, "b");
            assert_eq!(step_receipt.result_digest, "digest:b");
        }
        other => panic!("unexpected event {other:?}"),
    }
}

#[test]
fn op_failure_emits_failed_terminal_and_no_completion() {
    let dag = diamond();
    let sink = StreamSinkV1::new(64).expect("sink");
    let failing_op = |node: &ExecNodeV1| -> Result<DagNodeOutcomeV1, String> {
        if node.id == "c" {
            Err("boom".into())
        } else {
            Ok(DagNodeOutcomeV1 { result_digest: format!("d:{}", node.id), protected_decision: None })
        }
    };
    let error = DagExecutorV1::new()
        .execute(&dag, "input:1", None, &failing_op, &sink, ScheduleModeV1::Sequential)
        .expect_err("must fail");
    assert_eq!(
        error,
        DagExecErrorV1::OpFailed { node_id: "c".into(), detail: "boom".into() }
    );
    let events = sink.drain();
    let terminals: Vec<_> = events.iter().filter(|e| e.is_terminal()).collect();
    assert_eq!(terminals.len(), 1);
    assert!(matches!(terminals[0], ExecStreamEventV1::Failed { failure_code, .. } if failure_code == "OpFailed"));
    // No node ran after the failure (d depends on c).
    assert!(!events.iter().any(|e| e.node_id() == Some("d")));
}

#[test]
fn uncovered_decision_boundary_fails_closed_before_any_node_runs() {
    let dag = ExecDagV1::new(vec![node("a", 1, &[]), boundary("dec:1", &["a"])]);
    let calls = std::sync::Mutex::new(Vec::new());
    let sink = StreamSinkV1::new(64).expect("sink");
    let error = DagExecutorV1::new()
        .execute(&dag, "input:1", None, &recording_op(&calls), &sink, ScheduleModeV1::Sequential)
        .expect_err("uncovered boundary must fail closed");
    assert_eq!(
        error,
        DagExecErrorV1::CrossingRuleViolation { node_id: "dec:1".into() }
    );
    assert!(calls.lock().expect("lock").is_empty(), "no node ran");
    let events = sink.drain();
    assert_eq!(events.len(), 1, "only the terminal Failed event");
    assert!(matches!(
        &events[0],
        ExecStreamEventV1::Failed { failure_code, .. } if failure_code == "DecisionBoundaryUncovered"
    ));
}

#[test]
fn attached_policy_crosses_boundary() {
    let dag = ExecDagV1::new(vec![node("a", 1, &[]), boundary("dec:1", &["a"])]);
    let policy = ContingentPolicyV1::new(vec![
        ContingentPolicyRuleV1::new(
            zero_abi::ObservationClassV1::new("branch.test_suite").expect("class"),
            zero_abi::ObservedMatchV1::Exact { value: "fast".into() },
            "run_fast",
        )
        .expect("rule"),
    ])
    .expect("policy");
    let sink = StreamSinkV1::new(64).expect("sink");
    let op = |node: &ExecNodeV1| -> Result<DagNodeOutcomeV1, String> {
        let protected = (node.kind == ExecNodeKindV1::DecisionBoundary).then(|| ProtectedDecisionViewV1 {
            question: "which strategy?".into(),
            choices: vec!["run_fast".into()],
            observed_value: "fast".into(),
            resolved_alternative: "run_fast".into(),
            policy_rule_id: Some("rule:1".into()),
        });
        Ok(DagNodeOutcomeV1 { result_digest: format!("d:{}", node.id), protected_decision: protected })
    };
    let outcome = DagExecutorV1::new()
        .execute(&dag, "input:1", Some(&policy), &op, &sink, ScheduleModeV1::Sequential)
        .expect("policy-covered boundary settles");
    assert_eq!(outcome.trace.records.len(), 2);
    let boundary_record = &outcome.trace.records[1];
    assert_eq!(boundary_record.node_id, "dec:1");
    assert!(boundary_record.protected_decision.is_some());
}

#[test]
fn stream_buffering_is_bounded_and_ordered() {
    let sink = StreamSinkV1::new(2).expect("sink");
    assert_eq!(sink.capacity(), 2);
    sink.try_push(ExecStreamEventV1::StepStarted { seq: 1, node_id: "a".into() }).expect("fits");
    sink.try_push(ExecStreamEventV1::StepStarted { seq: 2, node_id: "b".into() }).expect("fits");
    assert_eq!(
        sink.try_push(ExecStreamEventV1::StepStarted { seq: 3, node_id: "c".into() }),
        Err(StreamErrorV1::Full)
    );
    assert_eq!(sink.len(), 2, "bounded: never grows past capacity");
    // Ordered delivery.
    assert_eq!(sink.drain().iter().map(ExecStreamEventV1::seq).collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn exactly_one_terminal_event_and_no_events_after() {
    let sink = StreamSinkV1::new(4).expect("sink");
    sink.finish(ExecStreamEventV1::Completed {
        seq: 1,
        trace_root: "root".into(),
        result_digest: "d".into(),
    })
    .expect("first finish accepted");
    assert!(sink.is_terminal());
    assert_eq!(
        sink.finish(ExecStreamEventV1::Completed {
            seq: 2,
            trace_root: "root".into(),
            result_digest: "d".into(),
        }),
        Err(StreamErrorV1::TerminalAlreadySent)
    );
    assert_eq!(
        sink.try_push(ExecStreamEventV1::StepStarted { seq: 3, node_id: "a".into() }),
        Err(StreamErrorV1::TerminalAlreadySent)
    );
    // Terminal is always deliverable even when the buffer is full.
    let full = StreamSinkV1::new(1).expect("sink");
    full.try_push(ExecStreamEventV1::StepStarted { seq: 1, node_id: "a".into() }).expect("fits");
    full.finish(ExecStreamEventV1::Failed {
        seq: 2,
        failure_code: "OpFailed".into(),
        detail: "boom".into(),
    })
    .expect("terminal deliverable at capacity");
    assert_eq!(full.drain().len(), 2);
}

#[test]
fn finish_rejects_non_terminal_events() {
    let sink = StreamSinkV1::new(4).expect("sink");
    assert_eq!(
        sink.finish(ExecStreamEventV1::StepStarted { seq: 1, node_id: "a".into() }),
        Err(StreamErrorV1::InvalidTerminal)
    );
    assert!(!sink.is_terminal());
}

#[test]
fn executor_aborts_loudly_on_stream_backpressure() {
    let dag = diamond();
    let calls = std::sync::Mutex::new(Vec::new());
    let sink = StreamSinkV1::new(2).expect("sink");
    let error = DagExecutorV1::new()
        .execute(&dag, "input:1", None, &recording_op(&calls), &sink, ScheduleModeV1::Sequential)
        .expect_err("must abort rather than drop events");
    assert_eq!(error, DagExecErrorV1::StreamBackpressure);
    assert_eq!(sink.len(), 2, "buffer never grew past capacity");
}

#[test]
fn critical_path_is_reported_and_deterministic() {
    let dag = diamond();
    let calls = std::sync::Mutex::new(Vec::new());
    let sink = StreamSinkV1::new(64).expect("sink");
    let outcome = DagExecutorV1::new()
        .execute(&dag, "input:1", None, &recording_op(&calls), &sink, ScheduleModeV1::Sequential)
        .expect("settled");
    // Diamond with uniform weights: two equal chains; smallest-id tie-break.
    assert_eq!(outcome.critical_path, vec!["a", "b", "d"]);
}
