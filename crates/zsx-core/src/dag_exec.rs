//! Execution DAG runtime (V6-R15, ZS-EXEC-001/002/005).
//!
//! The runtime half of the exec surface: a bounded streaming sink
//! ([`StreamSink`]) that delivers typed [`ExecStreamEvent`] events with
//! exactly one terminal event, and a deterministic DAG executor
//! ([`DagExecutor`]) that schedules a validated [`ExecDag`] in either
//! [`ScheduleMode::Sequential`] or [`ScheduleMode::ParallelLayers`]
//! (independent layers run concurrently; dependent ops are never reordered)
//! and settles into an [`ExecTrace`] whose equivalence is
//! mode-independent (deterministic result merge in topological order).
//!
//! The hub-side contingent-policy crossing rule is enforced before any node
//! runs: a plan with decision-boundary nodes and no attached policy fails
//! closed without executing, emitting a single `Failed` terminal event.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use zero_abi::exec_dag::{ExecDagError, ExecDag, ExecNode};
use zero_abi::exec_stream::{ExecStreamEvent, StepReceipt};
use zero_abi::exec_trace::{
    ExecTraceRecord, ExecTrace, ProtectedDecisionView, TraceOutcome,
};
use zero_abi::schema::canonical_json;
use zero_abi::{ContingentPolicy, sha256_hex};

/// Stream errors: bounded buffering and terminal-event guarantees, never
/// silent drops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    /// The bounded buffer is full; the producer must not grow it silently.
    Full,
    /// A terminal event was already delivered; no further events are
    /// accepted.
    TerminalAlreadySent,
    /// `finish` requires a terminal event (Completed or Failed).
    InvalidTerminal,
    /// Capacity must be at least one.
    ZeroCapacity,
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StreamError::Full => write!(f, "stream buffer full (bounded buffering)"),
            StreamError::TerminalAlreadySent => {
                write!(f, "terminal event already sent; stream is closed")
            }
            StreamError::InvalidTerminal => {
                write!(f, "finish requires a terminal event (Completed or Failed)")
            }
            StreamError::ZeroCapacity => write!(f, "stream capacity must be at least one"),
        }
    }
}

impl std::error::Error for StreamError {}

/// Bounded, ordered, thread-safe stream sink delivering typed execution
/// events (ZS-ADAPTER-007 streaming channel). Guarantees:
/// - bounded buffering: `try_push` fails with [`StreamError::Full`] when
///   the capacity is reached -- non-terminal growth is never unbounded and
///   events are never silently dropped;
/// - exactly one terminal event: `finish` accepts one `Completed`/`Failed`
///   (the terminal is always deliverable so the stream can always complete
///   honestly); afterwards the sink rejects everything
///   ([`StreamError::TerminalAlreadySent`]);
/// - ordered delivery: `drain`/`try_recv` preserve push order.
#[derive(Debug)]
pub struct StreamSink {
    capacity: usize,
    events: Mutex<VecDeque<ExecStreamEvent>>,
    terminal: AtomicBool,
}

impl StreamSink {
    /// New sink with the given capacity (>= 1).
    pub fn new(capacity: usize) -> Result<Self, StreamError> {
        if capacity == 0 {
            return Err(StreamError::ZeroCapacity);
        }
        Ok(StreamSink {
            capacity,
            events: Mutex::new(VecDeque::with_capacity(capacity)),
            terminal: AtomicBool::new(false),
        })
    }

    /// Bounded push of a non-terminal event. Terminal events are routed to
    /// `finish`.
    pub fn try_push(&self, event: ExecStreamEvent) -> Result<(), StreamError> {
        if self.terminal.load(Ordering::Acquire) {
            return Err(StreamError::TerminalAlreadySent);
        }
        if event.is_terminal() {
            return self.finish(event);
        }
        let mut events = self.events.lock().expect("stream events lock");
        if events.len() >= self.capacity {
            return Err(StreamError::Full);
        }
        events.push_back(event);
        Ok(())
    }

    /// Deliver the single terminal event. Accepted exactly once and always
    /// deliverable (the completion event must never be dropped); rejected
    /// only after a terminal was already sent or for non-terminal events.
    pub fn finish(&self, terminal: ExecStreamEvent) -> Result<(), StreamError> {
        if !terminal.is_terminal() {
            return Err(StreamError::InvalidTerminal);
        }
        if self.terminal.load(Ordering::Acquire) {
            return Err(StreamError::TerminalAlreadySent);
        }
        self.events.lock().expect("stream events lock").push_back(terminal);
        self.terminal.store(true, Ordering::Release);
        Ok(())
    }

    /// Pop one event from the front, preserving order.
    pub fn try_recv(&self) -> Option<ExecStreamEvent> {
        self.events.lock().expect("stream events lock").pop_front()
    }

    /// Snapshot all buffered events in order.
    pub fn drain(&self) -> Vec<ExecStreamEvent> {
        self.events.lock().expect("stream events lock").iter().cloned().collect()
    }

    /// Whether the terminal event was delivered.
    pub fn is_terminal(&self) -> bool {
        self.terminal.load(Ordering::Acquire)
    }

    /// Number of buffered events.
    pub fn len(&self) -> usize {
        self.events.lock().expect("stream events lock").len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Configured capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Schedule mode for DAG execution (ZS-EXEC-001/005).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleMode {
    /// Run nodes one at a time, in deterministic topological order.
    Sequential,
    /// Run each layer's mutually independent nodes concurrently on scoped
    /// threads; results merge deterministically in topological order and
    /// dependent ops are never reordered.
    ParallelLayers,
}

/// Outcome of one executed node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagNodeOutcome {
    /// Deterministic result digest of the node's output.
    pub result_digest: String,
    /// Protected decision info the model saw at this node, when applicable.
    pub protected_decision: Option<ProtectedDecisionView>,
}

/// Outcome of one settled DAG execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagExecutionOutcome {
    /// Deterministic, mode-independent execution trace.
    pub trace: ExecTrace,
    /// Critical path of the plan (dependency-aware scheduling report).
    pub critical_path: Vec<String>,
    /// Merged result digest over (node_id, result_digest) in topo order.
    pub merged_result_digest: String,
}

/// Fail-closed DAG runtime errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagExecError {
    /// The plan DAG failed structural validation.
    DagInvalid(String),
    /// A decision-boundary node is uncovered by any contingent policy:
    /// crossing refused before any node runs.
    CrossingRuleViolation { node_id: String },
    /// A node op failed.
    OpFailed { node_id: String, detail: String },
    /// The stream sink is full; the execution aborts loudly rather than
    /// silently dropping events.
    StreamBackpressure,
    /// The stream sink rejected the terminal event.
    StreamClosed,
}

impl std::fmt::Display for DagExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DagExecError::DagInvalid(detail) => write!(f, "invalid plan DAG: {detail}"),
            DagExecError::CrossingRuleViolation { node_id } => {
                write!(f, "decision boundary {node_id} uncovered: contingent policy required")
            }
            DagExecError::OpFailed { node_id, detail } => {
                write!(f, "node {node_id} failed: {detail}")
            }
            DagExecError::StreamBackpressure => {
                write!(f, "stream buffer full: execution aborted rather than dropping events")
            }
            DagExecError::StreamClosed => write!(f, "stream already delivered a terminal event"),
        }
    }
}

impl std::error::Error for DagExecError {}

/// Deterministic DAG executor (ZS-EXEC-001/002/005). Stateless: the plan,
/// input digest, contingent policy, op, sink, and schedule mode are passed
/// per execution.
#[derive(Debug, Default)]
pub struct DagExecutor {
    _private: (),
}

impl DagExecutor {
    /// New executor.
    pub fn new() -> Self {
        DagExecutor { _private: () }
    }

    /// Execute a plan DAG, streaming typed events into `sink` and settling
    /// into a deterministic trace. The hub-side crossing rule is enforced
    /// before any node runs: an uncovered decision boundary emits a single
    /// `Failed` terminal event and returns
    /// [`DagExecError::CrossingRuleViolation`]. Node ops run in
    /// dependency order; independent layers may run concurrently in
    /// `ParallelLayers` mode, with results merged deterministically in
    /// topological order so dependent ops are never reordered.
    pub fn execute<F>(
        &self,
        dag: &ExecDag,
        input_digest: &str,
        policy: Option<&ContingentPolicy>,
        op: &F,
        sink: &StreamSink,
        mode: ScheduleMode,
    ) -> Result<DagExecutionOutcome, DagExecError>
    where
        F: Fn(&ExecNode) -> Result<DagNodeOutcome, String> + Sync,
    {
        dag.validate().map_err(|error| DagExecError::DagInvalid(error.to_string()))?;
        let plan_digest = dag
            .plan_digest()
            .map_err(|error| DagExecError::DagInvalid(error.to_string()))?;

        // Hub-side contingent-policy crossing rule, fail-closed, before any
        // node runs.
        if let Err(error) = dag.crossing_rule(policy) {
            let node_id = match error {
                ExecDagError::DecisionBoundaryUncovered { node_id } => node_id,
                other => return Err(DagExecError::DagInvalid(other.to_string())),
            };
            let _ = sink.finish(ExecStreamEvent::Failed {
                seq: 0,
                failure_code: "DecisionBoundaryUncovered".into(),
                detail: node_id.clone(),
            });
            return Err(DagExecError::CrossingRuleViolation { node_id });
        }

        let layers = dag
            .layers()
            .map_err(|error| DagExecError::DagInvalid(error.to_string()))?;
        let critical_path = dag
            .critical_path()
            .map_err(|error| DagExecError::DagInvalid(error.to_string()))?;
        let node_index: std::collections::HashMap<&str, &ExecNode> =
            dag.nodes.iter().map(|node| (node.id.as_str(), node)).collect();

        let push = |event: ExecStreamEvent| -> Result<(), DagExecError> {
            sink.try_push(event).map_err(|error| match error {
                StreamError::Full => DagExecError::StreamBackpressure,
                _ => DagExecError::StreamClosed,
            })
        };

        push(ExecStreamEvent::PlanStarted {
            seq: 0,
            plan_digest: plan_digest.clone(),
            total_nodes: dag.nodes.len(),
        })?;

        let mut seq: u64 = 1;
        let mut outcomes: Vec<(String, DagNodeOutcome)> = Vec::with_capacity(dag.nodes.len());

        for layer in &layers {
            match mode {
                ScheduleMode::Sequential => {
                    for node_id in layer {
                        let node = node_index[node_id.as_str()];
                        let started = seq;
                        seq += 1;
                        push(ExecStreamEvent::StepStarted {
                            seq: started,
                            node_id: node_id.clone(),
                        })?;
                        let outcome = op(node).map_err(|detail| {
                            let _ = sink.finish(ExecStreamEvent::Failed {
                                seq,
                                failure_code: "OpFailed".into(),
                                detail: format!("{node_id}: {detail}"),
                            });
                            DagExecError::OpFailed {
                                node_id: node_id.clone(),
                                detail,
                            }
                        })?;
                        let completed = seq;
                        seq += 1;
                        push(ExecStreamEvent::StepCompleted {
                            seq: completed,
                            node_id: node_id.clone(),
                            step_receipt: StepReceipt {
                                node_id: node_id.clone(),
                                result_digest: outcome.result_digest.clone(),
                                output_bytes: 0,
                            },
                        })?;
                        outcomes.push((node_id.clone(), outcome));
                    }
                }
                ScheduleMode::ParallelLayers => {
                    // Emit StepStarted for every layer node first (honest:
                    // "started" precedes the run), then run the layer
                    // concurrently, then emit StepCompleted in node-id order
                    // (deterministic merge).
                    for node_id in layer {
                        let started = seq;
                        seq += 1;
                        push(ExecStreamEvent::StepStarted {
                            seq: started,
                            node_id: node_id.clone(),
                        })?;
                    }
                    let results = std::thread::scope(|scope| {
                        let handles: Vec<_> = layer
                            .iter()
                            .map(|node_id| {
                                let node = node_index[node_id.as_str()];
                                scope.spawn(move || {
                                    op(node)
                                        .map(|outcome| (node.id.clone(), outcome))
                                        .map_err(|detail| (node.id.clone(), detail))
                                })
                            })
                            .collect();
                        handles
                            .into_iter()
                            .map(|handle| handle.join().expect("scoped op thread joined"))
                            .collect::<Vec<Result<(String, DagNodeOutcome), (String, String)>>>()
                    });
                    for (node_id, result) in layer.iter().zip(results) {
                        let (completed_id, outcome) = result.map_err(|(failed_id, detail)| {
                            let _ = sink.finish(ExecStreamEvent::Failed {
                                seq,
                                failure_code: "OpFailed".into(),
                                detail: format!("{failed_id}: {detail}"),
                            });
                            DagExecError::OpFailed {
                                node_id: failed_id,
                                detail,
                            }
                        })?;
                        debug_assert_eq!(&completed_id, node_id);
                        let completed = seq;
                        seq += 1;
                        push(ExecStreamEvent::StepCompleted {
                            seq: completed,
                            node_id: completed_id.clone(),
                            step_receipt: StepReceipt {
                                node_id: completed_id.clone(),
                                result_digest: outcome.result_digest.clone(),
                                output_bytes: 0,
                            },
                        })?;
                        outcomes.push((completed_id, outcome));
                    }
                }
            }
        }

        let records: Vec<ExecTraceRecord> = outcomes
            .iter()
            .map(|(node_id, outcome)| ExecTraceRecord {
                node_id: node_id.clone(),
                kind: node_index[node_id.as_str()].kind,
                outcome: TraceOutcome::Completed {
                    result_digest: outcome.result_digest.clone(),
                },
                protected_decision: outcome.protected_decision.clone(),
            })
            .collect();
        let trace = ExecTrace::new(plan_digest.clone(), input_digest.to_string(), records);
        let merged = merged_result_digest(&trace);
        let terminal_seq = seq;
        sink.finish(ExecStreamEvent::Completed {
            seq: terminal_seq,
            trace_root: trace.trace_root(),
            result_digest: merged.clone(),
        })
        .map_err(|_| DagExecError::StreamClosed)?;

        Ok(DagExecutionOutcome {
            trace,
            critical_path,
            merged_result_digest: merged,
        })
    }
}

/// Deterministic merged result digest over (node_id, result_digest) pairs in
/// topological order.
fn merged_result_digest(trace: &ExecTrace) -> String {
    let pairs: Vec<serde_json::Value> = trace
        .records
        .iter()
        .map(|record| {
            let digest = match &record.outcome {
                TraceOutcome::Completed { result_digest } => result_digest.clone(),
                TraceOutcome::Failed { failure_code } => failure_code.clone(),
            };
            serde_json::json!([record.node_id, digest])
        })
        .collect();
    let pairs = serde_json::Value::Array(pairs);
    sha256_hex(canonical_json(&pairs).as_bytes())
}

