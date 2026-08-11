//! Opt-in aggregate scale and runtime-overhead evidence.
//!
//! Run with a provenance binding, for example:
//! `ZEROSTACK_SOURCE_SHA=$(git rev-parse HEAD) cargo test -p zero-codemode
//!  --release --test aggregate_scale -- --ignored --nocapture --test-threads=1`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use serde_json::{Value, json};
use zero_codemode::{
    CapabilityDescriptor, Connector, ConnectorCompletion, ConnectorError, DispatchContext,
    GlobalRegistration, Host, HostLimits,
};

const DEFAULT_POINTS: &[usize] = &[1, 10, 100, 1_000, 10_000, 100_000];
const INDEPENDENT_MAX: usize = 100_000;

#[derive(Default)]
struct ScaleConnector {
    calls: Cell<u64>,
    surfaces: RefCell<Vec<String>>,
}

impl Connector for ScaleConnector {
    fn dispatch(
        &self,
        capability: &CapabilityDescriptor,
        args_json: &str,
        _: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        let args: Value = serde_json::from_str(args_json)
            .map_err(|error| ConnectorError::new(error.to_string()))?;
        self.calls.set(self.calls.get().saturating_add(1));
        self.surfaces.borrow_mut().push(capability.surface.clone());
        let sequence = args
            .get("sequence")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        completion.complete(Ok(json!({"sequence":sequence}).to_string()))
    }
}

fn host() -> Host {
    let limits = HostLimits::new(
        512 * 1024 * 1024,
        1024 * 1024,
        Duration::from_secs(60),
        100_000_000,
        1_000_000,
        64,
        1024 * 1024,
        128 * 1024 * 1024,
    )
    .expect("scale limits");
    Host::new(
        limits,
        GlobalRegistration::zero(vec![
            CapabilityDescriptor::new("fs", "read"),
            CapabilityDescriptor::new("graph", "query"),
            CapabilityDescriptor::new("token", "find"),
        ]),
    )
    .expect("scale host")
}

fn points() -> Vec<usize> {
    std::env::var("ZEROSTACK_SCALE_POINTS")
        .ok()
        .and_then(|raw| {
            raw.split(',')
                .map(|value| value.trim().parse::<usize>())
                .collect::<Result<Vec<_>, _>>()
                .ok()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| DEFAULT_POINTS.to_vec())
}

fn percentile(mut values: Vec<u64>, percentile: usize) -> u64 {
    values.sort_unstable();
    let index = values
        .len()
        .saturating_mul(percentile)
        .div_ceil(100)
        .saturating_sub(1)
        .min(values.len().saturating_sub(1));
    values[index]
}

fn receipt(mode: &str, operations: usize, outcome: &zero_codemode::ExecutionOutcome) -> Value {
    json!({
        "schema":"zerostack.aggregate_scale.v1",
        "source_sha":option_env!("ZEROSTACK_SOURCE_SHA").unwrap_or("unbound"),
        "worktree_diff_sha256":option_env!("ZEROSTACK_WORKTREE_DIFF_SHA256").unwrap_or("unbound"),
        "profile":if cfg!(debug_assertions) { "debug" } else { "release" },
        "target":format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "mode":mode,
        "operations":operations,
        "metrics":outcome.metrics,
    })
}

#[test]
#[ignore = "opt-in release scale curve through 100,000 operations"]
fn sequential_and_mixed_scale_curves() {
    let mut previous = std::collections::BTreeMap::new();
    for operations in points() {
        for (mode, plan) in [
            (
                "sequential_reduction",
                format!(
                    "let checksum=0;for(let sequence=0;sequence<{operations};sequence+=1){{const value=await zero.fs.read({{sequence}});checksum+=value.content.value.sequence;}}return {{count:{operations},checksum}};"
                ),
            ),
            (
                "mixed_sequential_dag",
                format!(
                    "let checksum=0;for(let sequence=0;sequence<{operations};sequence+=1){{let value;const lane=sequence%3;if(lane===0)value=await zero.fs.read({{sequence}});if(lane===1)value=await zero.graph.query({{sequence}});if(lane===2)value=await zero.token.find({{sequence}});checksum+=value.content.value.sequence;}}return {{count:{operations},checksum}};"
                ),
            ),
        ] {
            let connector = Rc::new(ScaleConnector::default());
            let outcome = host().execute_measured(&plan, connector.clone());
            let result = outcome
                .result
                .as_ref()
                .unwrap_or_else(|error| panic!("{mode}/{operations}: {error}"));
            assert_eq!(result["count"], operations);
            assert_eq!(outcome.metrics.logical_operations, operations as u64);
            assert_eq!(connector.calls.get(), operations as u64);
            assert_eq!(
                outcome.metrics.peak_retained_promises,
                usize::from(operations > 0)
            );
            if operations == 1_000 {
                assert!(
                    outcome.metrics.wall_time_ns < 500_000_000,
                    "{mode} 1,000-operation gate failed: {}ns",
                    outcome.metrics.wall_time_ns
                );
            }
            if operations == 100_000 {
                assert!(
                    outcome.metrics.wall_time_ns < 1_000_000_000,
                    "{mode} 100,000-operation gate failed: {}ns",
                    outcome.metrics.wall_time_ns
                );
            }
            if let Some((prior_operations, prior_ns)) =
                previous.insert(mode, (operations, outcome.metrics.wall_time_ns))
                && operations == prior_operations.saturating_mul(10)
                && prior_operations >= 1_000
            {
                assert!(
                    outcome.metrics.wall_time_ns
                        <= prior_ns.saturating_mul(20).saturating_add(2_000_000),
                    "{mode} scale curve is super-linear: {prior_operations}={prior_ns}ns, {operations}={}ns",
                    outcome.metrics.wall_time_ns
                );
            }
            println!("{}", receipt(mode, operations, &outcome));
        }
    }
}

#[test]
#[ignore = "opt-in bounded independent-call scale curve"]
fn independent_scale_curve_uses_backpressure() {
    for operations in points()
        .into_iter()
        .filter(|operations| *operations <= INDEPENDENT_MAX)
    {
        let connector = Rc::new(ScaleConnector::default());
        let plan = format!(
            "const values=await Promise.all(Array.from({{length:{operations}}},(_,sequence)=>zero.fs.read({{sequence}})));return {{count:values.length}};"
        );
        let outcome = host().execute_measured(&plan, connector.clone());
        let result = outcome
            .result
            .as_ref()
            .unwrap_or_else(|error| panic!("independent/{operations}: {error}"));
        assert_eq!(result["count"], operations);
        assert_eq!(connector.calls.get(), operations as u64);
        assert!(outcome.metrics.peak_inflight_connector_calls <= 64);
        if operations > 64 {
            assert!(outcome.metrics.backpressure_events > 0);
        }
        if operations == 1_000 {
            assert!(outcome.metrics.wall_time_ns < 250_000_000);
        }
        if operations == 100_000 {
            assert!(outcome.metrics.wall_time_ns < 1_000_000_000);
        }
        println!("{}", receipt("independent", operations, &outcome));
    }
}

#[test]
#[ignore = "opt-in 30-sample aggregate overhead gate"]
fn thousand_operation_runtime_overhead_gate() {
    const OPERATIONS: usize = 1_000;
    const RUNS: usize = 30;
    let plan = format!(
        "let checksum=0;for(let sequence=0;sequence<{OPERATIONS};sequence+=1){{const value=await zero.fs.read({{sequence}});checksum+=value.content.value.sequence;}}return {{count:{OPERATIONS},checksum}};"
    );
    let mut samples = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let outcome = host().execute_measured(&plan, Rc::new(ScaleConnector::default()));
        let result = outcome
            .result
            .as_ref()
            .unwrap_or_else(|error| panic!("1000-operation gate: {error}"));
        assert_eq!(result["count"], OPERATIONS);
        samples.push(outcome.metrics.wall_time_ns);
    }
    let p50_ns = percentile(samples.clone(), 50);
    let p95_ns = percentile(samples, 95);
    println!(
        "{}",
        json!({
            "schema":"zerostack.aggregate_overhead_gate.v1",
            "source_sha":option_env!("ZEROSTACK_SOURCE_SHA").unwrap_or("unbound"),
            "worktree_diff_sha256":option_env!("ZEROSTACK_WORKTREE_DIFF_SHA256").unwrap_or("unbound"),
            "profile":if cfg!(debug_assertions) { "debug" } else { "release" },
            "target":format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
            "operations":OPERATIONS,
            "runs":RUNS,
            "p50_ns":p50_ns,
            "p95_ns":p95_ns,
        })
    );
    assert!(p50_ns < 250_000_000, "p50 {p50_ns}ns exceeds 250ms");
    assert!(p95_ns < 500_000_000, "p95 {p95_ns}ns exceeds 500ms");
    assert!(p95_ns < 1_000_000_000, "hard 1,000ms gate failed");
}
