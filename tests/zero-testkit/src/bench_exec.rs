//! V6-R12 benchmark execution program (ZS-BENCH-001 / ZS-METRIC-002 substrate).
//!
//! The program that runs a benchmark catalog entry:
//!
//! - **Catalog entry (ZS-BENCH-001 task manifest schema).** [`BenchCatalogEntryV1`]
//!   is the machine-readable workload spec loaded from `benchmarks/workloads/`.
//!   It carries the protected metric set, the Q99 target (exact rationals), the
//!   declared trial count, canonical run parameters, and a content-derived
//!   **root seal** (`workload_digest`) over the task-definition fields:
//!   every released entry passes schema validation and root-seal
//!   verification, and tampering with the task is loud. `benchmark_id` and
//!   `parameters` are run identity outside the seal, so a paired
//!   baseline/treatment run of one task shares the workload seal and differs
//!   only in the declared treatment dimension.
//! - **Engine seam.** [`BenchAdapterV1`] is the trait engine repositories
//!   implement. The program runs trials through the adapter and never touches
//!   process transport; fixture adapters exercise the full path in tests, and
//!   real engines plug in by implementing the trait.
//! - **Honest labels.** [`MeasurementKindV1::Exact`] aggregates only
//!   receipt-bearing counters (an exact metric without a receipt is refused);
//!   wall-clock and other timing are [`MeasurementKindV1::Estimate`] and never
//!   feed Q99. The Q99 claim is produced only through the zero-gauge bound
//!   (`zero_gauge::bounds::zero_failure_bound_certifies`, Proposition 11.1),
//!   which enforces the exact sample-size precondition (299 zero-failure
//!   independent trials for q = 99/100, alpha = 1/20) before any claim; refusal
//!   is recorded in the run result, never approximated.
//! - **Sealed manifests (R14 trace export).** Every run emits a
//!   [`SealedBenchmarkManifestV1`] (workload/engine/worker digests, canonical
//!   parameters, result and receipt digests). A run that cannot execute at all
//!   emits a `NonReproducible` manifest carrying the refusal reason. A sealed
//!   manifest is never overwritten ([`emit_manifest_v1`] refuses).
//! - **Paired baseline manifest (ZS-METRIC-002).** [`BenchPairingV1`] +
//!   [`paired_diff_over_treatment_only_v1`] produce a machine-readable diff over
//!   measured metrics and refuse any diff that touches more than the declared
//!   treatment dimension (same workload/engine/worker digests, parameters
//!   differing only in the variable dimension).

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use zero_abi::{DigestV1, canonical_json, sha256};
use zero_cert::{
    BenchmarkReproducibilityV1, SealedBenchmarkManifestV1, TRACE_EXPORT_MANIFEST_FILE_V1,
    export_benchmark_manifest_v1, read_exported_benchmark_manifest_v1,
};
use zero_gauge::bounds::{
    ZeroFailureBoundInput, min_zero_failure_trials, zero_failure_bound_certifies,
};
use zero_gauge::solver::Rational;

/// Schema version of the execution program artifacts.
pub const BENCH_EXEC_SCHEMA_VERSION_V1: u16 = 1;
/// ABI tag carried by execution program artifacts.
pub const BENCH_EXEC_ABI_VERSION_V1: &str = "v6-r12";
/// Run-result schema tag.
pub const BENCH_EXEC_RUN_SCHEMA_VERSION_V1: &str = "bench-exec-run/v1";
/// Paired-diff schema tag.
pub const BENCH_EXEC_PAIRED_DIFF_SCHEMA_VERSION_V1: &str = "bench-exec-paired-diff/v1";
/// Upper bound on declared trials in one run.
pub const BENCH_EXEC_MAX_TRIALS_V1: u32 = 1_000_000;
/// Upper bound on declared metrics in one catalog entry.
pub const BENCH_EXEC_MAX_METRICS_V1: usize = 64;

const CATALOG_DOMAIN_V1: &[u8] = b"zerostack.bench-exec.catalog.v1\0";
const RESULT_DOMAIN_V1: &[u8] = b"zerostack.bench-exec.result.v1\0";
const REFUSAL_DOMAIN_V1: &[u8] = b"zerostack.bench-exec.refusal.v1\0";
const PAIRED_DIFF_DOMAIN_V1: &[u8] = b"zerostack.bench-exec.paired-diff.v1\0";

/// Honest measurement label.
///
/// `Exact` metrics aggregate receipt-bearing counters and are the only kind
/// that may feed a Q99 claim; `Estimate` metrics (wall-clock and other
/// timing) are reported as values and never feed Q99.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementKindV1 {
    Exact,
    Estimate,
}

/// The Q99 target of a catalog entry: exact rationals, the independence
/// premise, and an optional cluster design effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchQ99SpecV1 {
    /// Certified success rate `q` numerator.
    pub success_target_num: i128,
    /// Certified success rate `q` denominator.
    pub success_target_den: u128,
    /// One-sided error `alpha` numerator (`1/20` is 95% one-sided).
    pub alpha_num: i128,
    /// One-sided error `alpha` denominator.
    pub alpha_den: u128,
    /// Premise: trials are independent cold traces (no warm reuse, no
    /// temporal dependence, no project clustering). `false` means Q99 can
    /// never be certified from this workload.
    pub independent: bool,
    /// Cluster design effect numerator (`>= 1`) when trials are clustered.
    pub design_effect_num: Option<i128>,
    /// Cluster design effect denominator.
    pub design_effect_den: Option<u128>,
}

impl BenchQ99SpecV1 {
    pub fn success_target(&self) -> Result<Rational, BenchExecErrorV1> {
        rational(self.success_target_num, self.success_target_den, "success_target")
    }

    pub fn alpha(&self) -> Result<Rational, BenchExecErrorV1> {
        rational(self.alpha_num, self.alpha_den, "alpha")
    }

    pub fn design_effect(&self) -> Result<Option<Rational>, BenchExecErrorV1> {
        match (self.design_effect_num, self.design_effect_den) {
            (None, None) => Ok(None),
            (Some(num), Some(den)) => Ok(Some(rational(num, den, "design_effect")?)),
            _ => Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidParameters,
                "design effect must be declared as both numerator and denominator or neither",
            )),
        }
    }
}

/// One declared measured metric of a catalog entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchMetricSpecV1 {
    pub name: String,
    pub kind: MeasurementKindV1,
}

/// The workload side of a catalog entry: what the adapter executes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchWorkloadSpecV1 {
    /// Adapter protocol name; an engine must declare support for it.
    pub fixture_kind: String,
    /// Declared trial count. Must already meet the exact Proposition 11.1
    /// sample-size precondition (299 for q = 99/100, alpha = 1/20); the
    /// runtime gate re-checks the observed trials before any Q99 claim.
    pub trials_required: u32,
    /// Randomization metadata (covered by the root seal).
    pub seed: Option<u64>,
    /// Adapter-specific trial parameters.
    pub adapter_parameters: Value,
}

/// Benchmark catalog entry (ZS-BENCH-001 task manifest schema).
///
/// `workload_digest` is the root seal over the **task-definition fields**
/// (schema version, requirement id, title, workload, Q99 target, metric
/// set): a released task cannot be edited without breaking the seal.
/// `benchmark_id` and `parameters` are run identity and sit deliberately
/// outside the seal, so a paired baseline/treatment run of the same task
/// shares one workload seal and differs only in the declared treatment
/// dimension (ZS-METRIC-002).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchCatalogEntryV1 {
    pub schema_version: u16,
    pub benchmark_id: String,
    /// Requirement row this entry serves (e.g. `ZS-BENCH-001`).
    pub requirement_id: String,
    pub title: String,
    pub workload: BenchWorkloadSpecV1,
    pub q99: BenchQ99SpecV1,
    /// Protected metrics the run must report, exactly, every trial.
    pub metrics: Vec<BenchMetricSpecV1>,
    /// Canonical run parameters; paired runs must differ only in the
    /// declared treatment dimension.
    pub parameters: Value,
    /// Root seal over all other fields.
    pub workload_digest: DigestV1,
}

impl BenchCatalogEntryV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        benchmark_id: impl Into<String>,
        requirement_id: impl Into<String>,
        title: impl Into<String>,
        workload: BenchWorkloadSpecV1,
        q99: BenchQ99SpecV1,
        metrics: Vec<BenchMetricSpecV1>,
        parameters: Value,
    ) -> Result<Self, BenchExecErrorV1> {
        let mut entry = Self {
            schema_version: BENCH_EXEC_SCHEMA_VERSION_V1,
            benchmark_id: benchmark_id.into(),
            requirement_id: requirement_id.into(),
            title: title.into(),
            workload,
            q99,
            metrics,
            parameters,
            workload_digest: DigestV1::ZERO,
        };
        entry.workload_digest = entry.expected_digest()?;
        entry.validate()?;
        Ok(entry)
    }

    pub fn validate(&self) -> Result<(), BenchExecErrorV1> {
        if self.schema_version != BENCH_EXEC_SCHEMA_VERSION_V1 {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::SchemaVersionMismatch,
                format!("catalog schema version {} is not supported", self.schema_version),
            ));
        }
        if self.benchmark_id.is_empty()
            || self.requirement_id.is_empty()
            || self.title.is_empty()
            || self.workload.fixture_kind.is_empty()
        {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidParameters,
                "benchmark id, requirement id, title, and fixture kind must be nonempty",
            ));
        }
        if self.workload.trials_required == 0
            || self.workload.trials_required > BENCH_EXEC_MAX_TRIALS_V1
        {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidParameters,
                "declared trial count is outside the supported range",
            ));
        }
        if self.metrics.is_empty() || self.metrics.len() > BENCH_EXEC_MAX_METRICS_V1 {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidParameters,
                "a catalog entry must declare between 1 and 64 measured metrics",
            ));
        }
        let mut names = BTreeSet::new();
        for metric in &self.metrics {
            if metric.name.is_empty() || !names.insert(metric.name.clone()) {
                return Err(bench_exec_error(
                    BenchExecFailureCodeV1::InvalidParameters,
                    format!("metric names must be nonempty and unique: '{}'", metric.name),
                ));
            }
        }
        if !self.parameters.is_object() {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidParameters,
                "canonical run parameters must be a JSON object (paired-diff needs named dimensions)",
            ));
        }
        let q = self.q99.success_target()?;
        let alpha = self.q99.alpha()?;
        let minimum = min_zero_failure_trials(q, alpha).map_err(|error| {
            bench_exec_error(
                BenchExecFailureCodeV1::InvalidRational,
                format!("cannot derive the sample-size precondition: {error}"),
            )
        })?;
        if u64::from(self.workload.trials_required) < minimum {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::SampleSizePrecondition,
                format!(
                    "{} declared trials do not meet the exact Proposition 11.1 sample-size \
                     precondition (minimum {minimum} zero-failure independent trials for \
                     q = {}/{} at alpha = {}/{})",
                    self.workload.trials_required,
                    q.num(),
                    q.den(),
                    alpha.num(),
                    alpha.den(),
                ),
            ));
        }
        if self.workload_digest != self.expected_digest()? {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::CatalogRootSealMismatch,
                "catalog entry root seal does not match its content",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, BenchExecErrorV1> {
        self.validate()?;
        Ok(canonical_json(&self.expected_value()).into_bytes())
    }

    pub fn digest(&self) -> Result<DigestV1, BenchExecErrorV1> {
        self.validate()?;
        Ok(digest_value(CATALOG_DOMAIN_V1, &self.expected_value()))
    }

    fn expected_digest(&self) -> Result<DigestV1, BenchExecErrorV1> {
        Ok(digest_value(CATALOG_DOMAIN_V1, &self.expected_value()))
    }

    /// The canonical digest input: the task-definition fields. Run identity
    /// (`benchmark_id`, `parameters`) is deliberately outside the seal so
    /// paired runs of the same task share one workload digest.
    fn expected_value(&self) -> Value {
        json!({
            "metrics": self.metrics,
            "q99": self.q99,
            "requirement_id": self.requirement_id,
            "schema_version": self.schema_version,
            "title": self.title,
            "workload": self.workload,
        })
    }
}

/// One receipt-bearing exact metric reading from a trial.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchExactMetricV1 {
    pub name: String,
    pub value: u64,
    /// The receipt the value was aggregated from; an exact metric without a
    /// receipt is refused, never relabeled.
    pub receipt_digest: DigestV1,
}

/// One estimated metric reading from a trial (never feeds Q99).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchEstimateMetricV1 {
    pub name: String,
    pub value: u64,
    /// Where the estimate came from (e.g. `wall-clock`).
    pub source: String,
}

/// One trial outcome returned by an adapter.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchTrialOutcomeV1 {
    pub trial_index: u32,
    pub success: bool,
    /// Must cover every declared `Exact` metric (value `0` when nothing was
    /// observed); each carries its receipt digest.
    pub exact_metrics: Vec<BenchExactMetricV1>,
    /// Must cover every declared `Estimate` metric.
    pub estimated_metrics: Vec<BenchEstimateMetricV1>,
    /// Optional trial-level receipt digest.
    pub trial_receipt_digest: Option<DigestV1>,
}

/// The engine seam. Engine repositories implement this trait; the execution
/// program runs trials through it and never constructs a shell command or
/// touches process transport. Fixture adapters exercise the full path in
/// tests; real engines plug in here.
pub trait BenchAdapterV1 {
    fn engine_id(&self) -> &str;
    fn engine_digest(&self) -> DigestV1;
    fn worker_digests(&self) -> Vec<String>;
    fn run_trial(
        &mut self,
        trial_index: u32,
        parameters: &Value,
    ) -> Result<BenchTrialOutcomeV1, BenchExecErrorV1>;
}

/// A Q99 claim: present only when the zero-gauge bound certified it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchQ99ClaimV1 {
    pub effective_trials: u64,
    pub success_target: String,
    pub alpha: String,
}

/// Certified, or refused with the exact reason. Never approximated.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum BenchQ99OutcomeV1 {
    Certified { claim: BenchQ99ClaimV1 },
    Refused { reason: String },
}

/// One full run result. `result_digest` is content-derived, so the result
/// is reproducible from the sealed manifest.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchRunResultV1 {
    pub schema_version: String,
    pub benchmark_id: String,
    pub trials_attempted: u32,
    pub successes: u64,
    pub failures: u64,
    pub exact_metric_totals: Vec<BenchExactMetricV1>,
    pub estimates: Vec<BenchEstimateMetricV1>,
    pub q99: BenchQ99OutcomeV1,
    pub receipt_digests: Vec<String>,
    pub result_digest: DigestV1,
}

impl BenchRunResultV1 {
    fn new(
        benchmark_id: String,
        trials_attempted: u32,
        successes: u64,
        failures: u64,
        exact_metric_totals: Vec<BenchExactMetricV1>,
        estimates: Vec<BenchEstimateMetricV1>,
        q99: BenchQ99OutcomeV1,
        receipt_digests: Vec<String>,
    ) -> Result<Self, BenchExecErrorV1> {
        let mut result = Self {
            schema_version: BENCH_EXEC_RUN_SCHEMA_VERSION_V1.into(),
            benchmark_id,
            trials_attempted,
            successes,
            failures,
            exact_metric_totals,
            estimates,
            q99,
            receipt_digests,
            result_digest: DigestV1::ZERO,
        };
        result.result_digest = result.expected_digest()?;
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), BenchExecErrorV1> {
        if self.schema_version != BENCH_EXEC_RUN_SCHEMA_VERSION_V1 {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::SchemaVersionMismatch,
                "run result schema version is not supported",
            ));
        }
        if self.benchmark_id.is_empty() {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidManifest,
                "run result benchmark id must be nonempty",
            ));
        }
        if self.successes + self.failures != u64::from(self.trials_attempted) {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidManifest,
                "run result successes plus failures must equal the trials attempted",
            ));
        }
        if self.exact_metric_totals.is_empty() {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidManifest,
                "run result carries no exact metric totals",
            ));
        }
        let mut names = BTreeSet::new();
        for metric in &self.exact_metric_totals {
            if metric.receipt_digest == DigestV1::ZERO || !names.insert(metric.name.clone()) {
                return Err(bench_exec_error(
                    BenchExecFailureCodeV1::InvalidManifest,
                    "exact metric totals need nonzero receipts and unique names",
                ));
            }
        }
        if self.receipt_digests.is_empty()
            || !is_sorted(&self.receipt_digests)
            || self.receipt_digests.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidManifest,
                "receipt digests must be nonempty, sorted, and unique",
            ));
        }
        if self.result_digest != self.expected_digest()? {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidManifest,
                "run result digest mismatch",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<DigestV1, BenchExecErrorV1> {
        Ok(digest_value(
            RESULT_DOMAIN_V1,
            &json!({
                "benchmark_id": self.benchmark_id,
                "estimates": self.estimates,
                "exact_metric_totals": self.exact_metric_totals,
                "failures": self.failures,
                "q99": self.q99,
                "receipt_digests": self.receipt_digests,
                "schema_version": self.schema_version,
                "successes": self.successes,
                "trials_attempted": self.trials_attempted,
            }),
        ))
    }
}

/// Outcome of an executed benchmark.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "detail", rename_all = "snake_case")]
pub enum BenchExecOutcomeV1 {
    /// The run executed and its sealed manifest was emitted and read back.
    Ran { run: Box<BenchRunResultV1> },
    /// The benchmark could not run; a NonReproducible manifest carrying the
    /// reason was emitted instead.
    Refused { reason: String },
}

/// Receipt of [`execute_and_emit_v1`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchExecReceiptV1 {
    pub schema_version: u16,
    pub benchmark_id: String,
    pub outcome: BenchExecOutcomeV1,
    pub manifest_seal: DigestV1,
}

/// Paired baseline manifest metadata (ZS-METRIC-002).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchPairingV1 {
    pub baseline_benchmark_id: String,
    pub treatment_benchmark_id: String,
    /// The single canonical parameter dimension the treatment may change.
    pub variable_dimension: String,
}

impl BenchPairingV1 {
    pub fn validate(&self) -> Result<(), BenchExecErrorV1> {
        if self.baseline_benchmark_id.is_empty()
            || self.treatment_benchmark_id.is_empty()
            || self.variable_dimension.is_empty()
        {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidParameters,
                "pairing ids and the variable dimension must be nonempty",
            ));
        }
        if self.baseline_benchmark_id == self.treatment_benchmark_id {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidParameters,
                "baseline and treatment must be distinct benchmarks",
            ));
        }
        Ok(())
    }
}

/// One measured-metric delta of a paired diff. `Exact` deltas are computed
/// over receipt-bearing totals; `Estimate` deltas are values only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchMetricDeltaV1 {
    pub name: String,
    pub kind: MeasurementKindV1,
    pub baseline_value: u64,
    pub treatment_value: u64,
    /// `treatment - baseline` in i128 (checked).
    pub delta: i128,
}

/// Machine-readable diff over treatment only (ZS-METRIC-002).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchPairedDiffV1 {
    pub schema_version: String,
    pub baseline_benchmark_id: String,
    pub treatment_benchmark_id: String,
    pub variable_dimension: String,
    pub metric_deltas: Vec<BenchMetricDeltaV1>,
    pub diff_digest: DigestV1,
}

impl BenchPairedDiffV1 {
    fn new(
        baseline_benchmark_id: String,
        treatment_benchmark_id: String,
        variable_dimension: String,
        metric_deltas: Vec<BenchMetricDeltaV1>,
    ) -> Result<Self, BenchExecErrorV1> {
        let mut diff = Self {
            schema_version: BENCH_EXEC_PAIRED_DIFF_SCHEMA_VERSION_V1.into(),
            baseline_benchmark_id,
            treatment_benchmark_id,
            variable_dimension,
            metric_deltas,
            diff_digest: DigestV1::ZERO,
        };
        diff.diff_digest = diff.expected_digest()?;
        Ok(diff)
    }

    pub fn validate(&self) -> Result<(), BenchExecErrorV1> {
        if self.schema_version != BENCH_EXEC_PAIRED_DIFF_SCHEMA_VERSION_V1 {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::SchemaVersionMismatch,
                "paired diff schema version is not supported",
            ));
        }
        if self.metric_deltas.is_empty() || self.diff_digest != self.expected_digest()? {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::InvalidManifest,
                "paired diff is empty or its digest does not match",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> Result<DigestV1, BenchExecErrorV1> {
        Ok(digest_value(
            PAIRED_DIFF_DOMAIN_V1,
            &json!({
                "baseline_benchmark_id": self.baseline_benchmark_id,
                "metric_deltas": self.metric_deltas,
                "schema_version": self.schema_version,
                "treatment_benchmark_id": self.treatment_benchmark_id,
                "variable_dimension": self.variable_dimension,
            }),
        ))
    }
}

// ---------------------------------------------------------------------------
// Failures.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BenchExecFailureCodeV1 {
    SchemaVersionMismatch,
    CatalogRootSealMismatch,
    InvalidParameters,
    InvalidRational,
    /// The declared trial count cannot certify the Q99 target.
    SampleSizePrecondition,
    /// A trial reported an exact metric the catalog does not declare.
    UndeclaredMetric,
    /// A trial did not report every metric the catalog declares.
    MissingDeclaredMetric,
    /// An exact metric without a receipt digest: never relabeled.
    ExactMetricWithoutReceipt,
    MetricTotalOverflow,
    AdapterFailed,
    /// The zero-gauge bound refused a Q99 claim.
    Q99Refused,
    /// A sealed manifest already exists at the export path.
    ManifestExists,
    ExportFailed,
    ReadBackFailed,
    InvalidManifest,
    NotPaired,
    ParametersDifferBeyondVariable,
    Io,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BenchExecErrorV1 {
    pub code: BenchExecFailureCodeV1,
    pub detail: String,
}

impl fmt::Display for BenchExecErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for BenchExecErrorV1 {}

fn bench_exec_error(code: BenchExecFailureCodeV1, detail: impl Into<String>) -> BenchExecErrorV1 {
    BenchExecErrorV1 {
        code,
        detail: detail.into(),
    }
}

// ---------------------------------------------------------------------------
// The execution program.
// ---------------------------------------------------------------------------

/// Load and root-verify a catalog entry from `benchmarks/workloads/`.
pub fn load_catalog_entry_v1(path: impl AsRef<Path>) -> Result<BenchCatalogEntryV1, BenchExecErrorV1> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path).map_err(|error| {
        bench_exec_error(
            BenchExecFailureCodeV1::Io,
            format!("read {}: {error}", path.display()),
        )
    })?;
    let entry: BenchCatalogEntryV1 = serde_json::from_str(&content).map_err(|error| {
        bench_exec_error(
            BenchExecFailureCodeV1::InvalidManifest,
            format!("cannot parse catalog entry {}: {error}", path.display()),
        )
    })?;
    entry.validate()?;
    Ok(entry)
}

/// The zero-gauge Q99 gate: certifies the claim only when the exact
/// sample-size precondition holds; otherwise refuses loudly.
///
/// `trials` are the observed (attempted) trials and `failures` the observed
/// failures; the zero-failure bound applies only to independent cold traces
/// and refuses dependent traces, nonzero failures, and insufficient
/// effective samples (Proposition 11.1, `q^effective_n <= alpha`).
pub fn bench_q99_gate_v1(
    trials: u64,
    failures: u64,
    spec: &BenchQ99SpecV1,
) -> Result<BenchQ99ClaimV1, BenchExecErrorV1> {
    let success_target = spec.success_target()?;
    let alpha = spec.alpha()?;
    let input = ZeroFailureBoundInput {
        trials,
        failures,
        success_target,
        alpha,
        independent: spec.independent,
        design_effect: spec.design_effect()?,
    };
    let certification = zero_failure_bound_certifies(&input).map_err(|error| {
        bench_exec_error(BenchExecFailureCodeV1::Q99Refused, error.to_string())
    })?;
    Ok(BenchQ99ClaimV1 {
        effective_trials: certification.effective_trials,
        success_target: format!("{}/{}", success_target.num(), success_target.den()),
        alpha: format!("{}/{}", alpha.num(), alpha.den()),
    })
}

/// Run one catalog entry through an adapter. The catalog is validated first
/// (schema, root seal, and the Proposition 11.1 sample-size precondition);
/// every trial outcome must report exactly the declared metrics, exact
/// metrics must carry receipts, and the Q99 claim goes through
/// [`bench_q99_gate_v1`] -- refusal is recorded in the result, never
/// approximated. Adapter failures refuse the whole run loudly.
pub fn run_benchmark_v1(
    entry: &BenchCatalogEntryV1,
    adapter: &mut dyn BenchAdapterV1,
) -> Result<BenchRunResultV1, BenchExecErrorV1> {
    entry.validate()?;
    let declared_exact: Vec<&BenchMetricSpecV1> = entry
        .metrics
        .iter()
        .filter(|metric| metric.kind == MeasurementKindV1::Exact)
        .collect();
    let declared_estimates: Vec<&BenchMetricSpecV1> = entry
        .metrics
        .iter()
        .filter(|metric| metric.kind == MeasurementKindV1::Estimate)
        .collect();
    let mut exact_totals: BTreeMap<String, (u64, Vec<DigestV1>)> = declared_exact
        .iter()
        .map(|metric| (metric.name.clone(), (0, Vec::new())))
        .collect();
    let mut estimate_totals: BTreeMap<String, (u64, String)> = declared_estimates
        .iter()
        .map(|metric| (metric.name.clone(), (0, String::new())))
        .collect();
    let mut trial_receipts: Vec<DigestV1> = Vec::new();
    let mut successes = 0_u64;
    let mut failures = 0_u64;

    for trial_index in 0..entry.workload.trials_required {
        let outcome = adapter
            .run_trial(trial_index, &entry.workload.adapter_parameters)
            .map_err(|error| {
                bench_exec_error(
                    BenchExecFailureCodeV1::AdapterFailed,
                    format!("trial {trial_index}: {error}"),
                )
            })?;
        if outcome.trial_index != trial_index {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::AdapterFailed,
                format!("trial {trial_index}: adapter returned a different trial index"),
            ));
        }
        if outcome.success {
            successes = successes.checked_add(1).ok_or_else(|| {
                bench_exec_error(
                    BenchExecFailureCodeV1::MetricTotalOverflow,
                    "success count overflow",
                )
            })?;
        } else {
            failures = failures.checked_add(1).ok_or_else(|| {
                bench_exec_error(
                    BenchExecFailureCodeV1::MetricTotalOverflow,
                    "failure count overflow",
                )
            })?;
        }
        let mut reported_exact: BTreeSet<&str> = BTreeSet::new();
        for metric in &outcome.exact_metrics {
            let declared = declared_exact
                .iter()
                .find(|declared| declared.name == metric.name)
                .ok_or_else(|| {
                    bench_exec_error(
                        BenchExecFailureCodeV1::UndeclaredMetric,
                        format!(
                            "trial {trial_index}: exact metric '{}' is not declared in the catalog",
                            metric.name
                        ),
                    )
                })?;
            if metric.receipt_digest == DigestV1::ZERO {
                return Err(bench_exec_error(
                    BenchExecFailureCodeV1::ExactMetricWithoutReceipt,
                    format!(
                        "trial {trial_index}: exact metric '{}' has no receipt digest; it cannot \
                         be labeled Exact",
                        metric.name
                    ),
                ));
            }
            let (total, receipts) = exact_totals.get_mut(&declared.name).expect("declared key");
            *total = total.checked_add(metric.value).ok_or_else(|| {
                bench_exec_error(
                    BenchExecFailureCodeV1::MetricTotalOverflow,
                    format!("exact metric '{}' total overflow", metric.name),
                )
            })?;
            receipts.push(metric.receipt_digest);
            trial_receipts.push(metric.receipt_digest);
            reported_exact.insert(&metric.name);
        }
        if reported_exact.len() != declared_exact.len() {
            let missing = declared_exact
                .iter()
                .map(|metric| metric.name.as_str())
                .find(|name| !reported_exact.contains(name))
                .expect("length mismatch implies a missing name");
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::MissingDeclaredMetric,
                format!("trial {trial_index}: declared exact metric '{missing}' was not reported"),
            ));
        }
        let mut reported_estimates: BTreeSet<&str> = BTreeSet::new();
        for metric in &outcome.estimated_metrics {
            let declared = declared_estimates
                .iter()
                .find(|declared| declared.name == metric.name)
                .ok_or_else(|| {
                    bench_exec_error(
                        BenchExecFailureCodeV1::UndeclaredMetric,
                        format!(
                            "trial {trial_index}: estimate metric '{}' is not declared in the catalog",
                            metric.name
                        ),
                    )
                })?;
            let (total, source) = estimate_totals.get_mut(&declared.name).expect("declared key");
            *total = total.checked_add(metric.value).ok_or_else(|| {
                bench_exec_error(
                    BenchExecFailureCodeV1::MetricTotalOverflow,
                    format!("estimate metric '{}' total overflow", metric.name),
                )
            })?;
            if source.is_empty() {
                *source = metric.source.clone();
            }
            reported_estimates.insert(&metric.name);
        }
        if reported_estimates.len() != declared_estimates.len() {
            let missing = declared_estimates
                .iter()
                .map(|metric| metric.name.as_str())
                .find(|name| !reported_estimates.contains(name))
                .expect("length mismatch implies a missing name");
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::MissingDeclaredMetric,
                format!("trial {trial_index}: declared estimate metric '{missing}' was not reported"),
            ));
        }
        if let Some(receipt) = outcome.trial_receipt_digest {
            if receipt != DigestV1::ZERO {
                trial_receipts.push(receipt);
            }
        }
    }

    let exact_metric_totals: Vec<BenchExactMetricV1> = exact_totals
        .into_iter()
        .map(|(name, (value, mut receipts))| {
            receipts.sort_unstable();
            receipts.dedup();
            BenchExactMetricV1 {
                name,
                value,
                receipt_digest: digest_value(RESULT_DOMAIN_V1, &json!({ "receipts": receipts })),
            }
        })
        .collect();
    let estimates: Vec<BenchEstimateMetricV1> = estimate_totals
        .into_iter()
        .map(|(name, (value, source))| BenchEstimateMetricV1 {
            name,
            value,
            source,
        })
        .collect();
    let mut receipt_digests: Vec<String> = trial_receipts
        .into_iter()
        .map(|digest| digest.to_hex())
        .collect();
    receipt_digests.sort_unstable();
    receipt_digests.dedup();

    let q99 = match bench_q99_gate_v1(
        u64::from(entry.workload.trials_required),
        failures,
        &entry.q99,
    ) {
        Ok(claim) => BenchQ99OutcomeV1::Certified { claim },
        Err(error) => BenchQ99OutcomeV1::Refused {
            reason: error.detail,
        },
    };

    BenchRunResultV1::new(
        entry.benchmark_id.clone(),
        entry.workload.trials_required,
        successes,
        failures,
        exact_metric_totals,
        estimates,
        q99,
        receipt_digests,
    )
}

/// Build the sealed manifest of a completed run.
pub fn sealed_manifest_v1(
    entry: &BenchCatalogEntryV1,
    adapter: &dyn BenchAdapterV1,
    run: &BenchRunResultV1,
) -> Result<SealedBenchmarkManifestV1, BenchExecErrorV1> {
    entry.validate()?;
    run.validate()?;
    SealedBenchmarkManifestV1::new(
        run.benchmark_id.clone(),
        entry.workload_digest,
        adapter.engine_digest(),
        adapter.worker_digests(),
        entry.parameters.clone(),
        run.result_digest,
        run.receipt_digests.clone(),
        BenchmarkReproducibilityV1::Sealed,
    )
    .map_err(|error| {
        bench_exec_error(
            BenchExecFailureCodeV1::InvalidManifest,
            format!("cannot seal manifest: {error}"),
        )
    })
}

/// Build a `NonReproducible` manifest carrying the refusal reason: a
/// benchmark that cannot run refuses loudly in the manifest itself.
pub fn refusal_manifest_v1(
    entry: &BenchCatalogEntryV1,
    adapter: &dyn BenchAdapterV1,
    reason: impl Into<String>,
) -> Result<SealedBenchmarkManifestV1, BenchExecErrorV1> {
    let reason = reason.into();
    let result_digest = digest_value(REFUSAL_DOMAIN_V1, &json!({ "reason": reason }));
    SealedBenchmarkManifestV1::new(
        entry.benchmark_id.clone(),
        entry.workload_digest,
        adapter.engine_digest(),
        adapter.worker_digests(),
        entry.parameters.clone(),
        result_digest,
        Vec::new(),
        BenchmarkReproducibilityV1::NonReproducible { reason },
    )
    .map_err(|error| {
        bench_exec_error(
            BenchExecFailureCodeV1::InvalidManifest,
            format!("cannot seal refusal manifest: {error}"),
        )
    })
}

/// Persist a sealed manifest. Refuses to overwrite an existing sealed
/// manifest at the export path.
pub fn emit_manifest_v1(
    dir: impl AsRef<Path>,
    manifest: SealedBenchmarkManifestV1,
) -> Result<DigestV1, BenchExecErrorV1> {
    let dir = dir.as_ref();
    if dir.join(TRACE_EXPORT_MANIFEST_FILE_V1).exists() {
        return Err(bench_exec_error(
            BenchExecFailureCodeV1::ManifestExists,
            format!(
                "refusing to overwrite the sealed manifest at {}",
                dir.join(TRACE_EXPORT_MANIFEST_FILE_V1).display()
            ),
        ));
    }
    export_benchmark_manifest_v1(dir, manifest).map_err(|error| {
        bench_exec_error(
            BenchExecFailureCodeV1::ExportFailed,
            format!("export failed: {error}"),
        )
    })
}

/// Execute and emit: run the catalog entry, seal the manifest, persist it
/// (never overwriting), and verify it on read-back. A run that cannot
/// execute at all emits a `NonReproducible` manifest with the reason.
pub fn execute_and_emit_v1(
    dir: impl AsRef<Path>,
    entry: &BenchCatalogEntryV1,
    adapter: &mut dyn BenchAdapterV1,
) -> Result<BenchExecReceiptV1, BenchExecErrorV1> {
    let dir = dir.as_ref();
    match run_benchmark_v1(entry, adapter) {
        Ok(run) => {
            let manifest = sealed_manifest_v1(entry, adapter, &run)?;
            let seal = emit_manifest_v1(dir, manifest)?;
            read_exported_benchmark_manifest_v1(dir).map_err(|error| {
                bench_exec_error(
                    BenchExecFailureCodeV1::ReadBackFailed,
                    format!("read-back verification failed: {error}"),
                )
            })?;
            Ok(BenchExecReceiptV1 {
                schema_version: BENCH_EXEC_SCHEMA_VERSION_V1,
                benchmark_id: run.benchmark_id.clone(),
                outcome: BenchExecOutcomeV1::Ran {
                    run: Box::new(run),
                },
                manifest_seal: seal,
            })
        }
        Err(run_error) => {
            let manifest = refusal_manifest_v1(entry, adapter, run_error.to_string())?;
            let seal = emit_manifest_v1(dir, manifest)?;
            read_exported_benchmark_manifest_v1(dir).map_err(|error| {
                bench_exec_error(
                    BenchExecFailureCodeV1::ReadBackFailed,
                    format!("read-back verification failed: {error}"),
                )
            })?;
            Ok(BenchExecReceiptV1 {
                schema_version: BENCH_EXEC_SCHEMA_VERSION_V1,
                benchmark_id: entry.benchmark_id.clone(),
                outcome: BenchExecOutcomeV1::Refused {
                    reason: run_error.to_string(),
                },
                manifest_seal: seal,
            })
        }
    }
}

/// ZS-METRIC-002 paired baseline diff: machine-readable metric deltas over
/// treatment only. Refuses when the manifests differ in workload/engine/
/// worker identity, when the canonical parameters differ beyond the single
/// declared variable dimension, or when the variable does not actually
/// change. `Exact` deltas come from receipt-bearing totals; `Estimate`
/// deltas are values only.
#[allow(clippy::too_many_arguments)]
pub fn paired_diff_over_treatment_only_v1(
    baseline_manifest: &SealedBenchmarkManifestV1,
    baseline_run: &BenchRunResultV1,
    treatment_manifest: &SealedBenchmarkManifestV1,
    treatment_run: &BenchRunResultV1,
    pairing: &BenchPairingV1,
) -> Result<BenchPairedDiffV1, BenchExecErrorV1> {
    pairing.validate()?;
    baseline_manifest.validate().map_err(|error| {
        bench_exec_error(
            BenchExecFailureCodeV1::InvalidManifest,
            format!("baseline manifest invalid: {error}"),
        )
    })?;
    treatment_manifest.validate().map_err(|error| {
        bench_exec_error(
            BenchExecFailureCodeV1::InvalidManifest,
            format!("treatment manifest invalid: {error}"),
        )
    })?;
    baseline_run.validate()?;
    treatment_run.validate()?;
    if baseline_manifest.benchmark_id != pairing.baseline_benchmark_id
        || treatment_manifest.benchmark_id != pairing.treatment_benchmark_id
        || baseline_run.benchmark_id != baseline_manifest.benchmark_id
        || treatment_run.benchmark_id != treatment_manifest.benchmark_id
    {
        return Err(bench_exec_error(
            BenchExecFailureCodeV1::NotPaired,
            "baseline/treatment manifests and runs do not match the pairing",
        ));
    }
    if baseline_manifest.workload_digest != treatment_manifest.workload_digest
        || baseline_manifest.engine_digest != treatment_manifest.engine_digest
        || baseline_manifest.worker_digests != treatment_manifest.worker_digests
    {
        return Err(bench_exec_error(
            BenchExecFailureCodeV1::NotPaired,
            "paired runs must share workload, engine, and worker digests",
        ));
    }
    parameters_differ_only_in(
        &baseline_manifest.parameters,
        &treatment_manifest.parameters,
        &pairing.variable_dimension,
    )?;

    let mut metric_deltas = Vec::new();
    for baseline in &baseline_run.exact_metric_totals {
        if let Some(treatment) = treatment_run
            .exact_metric_totals
            .iter()
            .find(|metric| metric.name == baseline.name)
        {
            metric_deltas.push(BenchMetricDeltaV1 {
                name: baseline.name.clone(),
                kind: MeasurementKindV1::Exact,
                baseline_value: baseline.value,
                treatment_value: treatment.value,
                delta: i128::from(treatment.value) - i128::from(baseline.value),
            });
        } else {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::NotPaired,
                format!(
                    "treatment run lacks exact metric '{}' reported by the baseline",
                    baseline.name
                ),
            ));
        }
    }
    for baseline in &baseline_run.estimates {
        if let Some(treatment) = treatment_run
            .estimates
            .iter()
            .find(|metric| metric.name == baseline.name)
        {
            metric_deltas.push(BenchMetricDeltaV1 {
                name: baseline.name.clone(),
                kind: MeasurementKindV1::Estimate,
                baseline_value: baseline.value,
                treatment_value: treatment.value,
                delta: i128::from(treatment.value) - i128::from(baseline.value),
            });
        } else {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::NotPaired,
                format!(
                    "treatment run lacks estimate metric '{}' reported by the baseline",
                    baseline.name
                ),
            ));
        }
    }
    if metric_deltas.is_empty() {
        return Err(bench_exec_error(
            BenchExecFailureCodeV1::NotPaired,
            "paired runs share no measured metrics",
        ));
    }
    BenchPairedDiffV1::new(
        pairing.baseline_benchmark_id.clone(),
        pairing.treatment_benchmark_id.clone(),
        pairing.variable_dimension.clone(),
        metric_deltas,
    )
}

/// The canonical parameters must be JSON objects equal on every key except
/// the declared variable dimension, which must be present in both and must
/// actually differ.
fn parameters_differ_only_in(
    baseline: &Value,
    treatment: &Value,
    variable: &str,
) -> Result<(), BenchExecErrorV1> {
    let (baseline, treatment) = match (baseline.as_object(), treatment.as_object()) {
        (Some(baseline), Some(treatment)) => (baseline, treatment),
        _ => {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::ParametersDifferBeyondVariable,
                "canonical parameters must be JSON objects",
            ));
        }
    };
    let mut mismatched = Vec::new();
    for (key, value) in baseline {
        if key == variable {
            continue;
        }
        match treatment.get(key) {
            Some(treatment_value) if treatment_value == value => {}
            _ => mismatched.push(key.clone()),
        }
    }
    for key in treatment.keys() {
        if key != variable && !baseline.contains_key(key) {
            mismatched.push(key.clone());
        }
    }
    if !mismatched.is_empty() {
        return Err(bench_exec_error(
            BenchExecFailureCodeV1::ParametersDifferBeyondVariable,
            format!(
                "paired runs differ in parameters beyond the declared variable '{}': {}",
                variable,
                mismatched.join(", ")
            ),
        ));
    }
    match (baseline.get(variable), treatment.get(variable)) {
        (Some(baseline_value), Some(treatment_value)) if baseline_value != treatment_value => {}
        _ => {
            return Err(bench_exec_error(
                BenchExecFailureCodeV1::NotPaired,
                format!(
                    "the declared variable '{}' must be present in both runs and actually differ",
                    variable
                ),
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn rational(num: i128, den: u128, parameter: &'static str) -> Result<Rational, BenchExecErrorV1> {
    if den == 0 {
        return Err(bench_exec_error(
            BenchExecFailureCodeV1::InvalidRational,
            format!("{parameter} denominator cannot be zero"),
        ));
    }
    Rational::new(num, den).map_err(|error| {
        bench_exec_error(
            BenchExecFailureCodeV1::InvalidRational,
            format!("{parameter}: {error}"),
        )
    })
}

fn digest_value(domain: &[u8], value: &Value) -> DigestV1 {
    let mut tagged = Vec::with_capacity(domain.len() + 128);
    tagged.extend_from_slice(domain);
    tagged.extend_from_slice(canonical_json(value).as_bytes());
    DigestV1::from_bytes(sha256(&tagged))
}

fn is_sorted(values: &[String]) -> bool {
    values.windows(2).all(|pair| pair[0] <= pair[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn q99_spec() -> BenchQ99SpecV1 {
        BenchQ99SpecV1 {
            success_target_num: 99,
            success_target_den: 100,
            alpha_num: 1,
            alpha_den: 20,
            independent: true,
            design_effect_num: None,
            design_effect_den: None,
        }
    }

    fn metrics() -> Vec<BenchMetricSpecV1> {
        vec![
            BenchMetricSpecV1 {
                name: "exact_reads".into(),
                kind: MeasurementKindV1::Exact,
            },
            BenchMetricSpecV1 {
                name: "wall_clock_nanos".into(),
                kind: MeasurementKindV1::Estimate,
            },
        ]
    }

    fn smoke_entry(trials_required: u32, parameters: Value) -> Result<BenchCatalogEntryV1, BenchExecErrorV1> {
        BenchCatalogEntryV1::new(
            "r12-manifest-smoke",
            "ZS-BENCH-001",
            "Manifest smoke workload",
            BenchWorkloadSpecV1 {
                fixture_kind: "deterministic-fixture-v1".into(),
                trials_required,
                seed: Some(1),
                adapter_parameters: json!({}),
            },
            q99_spec(),
            metrics(),
            parameters,
        )
    }

    fn shipped_catalog_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../benchmarks/workloads/r12-manifest-smoke.json")
    }

    /// Deterministic in-process fixture adapter: one exact receipt-bearing
    /// counter per trial, an estimated wall-clock reading, optional failures.
    struct FixtureAdapter {
        failures_at: BTreeSet<u32>,
        no_receipts: bool,
        wall_clock_nanos: u64,
        engine: DigestV1,
    }

    impl FixtureAdapter {
        fn new(failures_at: impl IntoIterator<Item = u32>) -> Self {
            Self {
                failures_at: failures_at.into_iter().collect(),
                no_receipts: false,
                wall_clock_nanos: 1_000,
                engine: DigestV1::from_bytes(sha256(b"fixture-engine-v1")),
            }
        }

        fn with_engine(engine_tag: &[u8]) -> Self {
            Self {
                failures_at: BTreeSet::new(),
                no_receipts: false,
                wall_clock_nanos: 1_000,
                engine: DigestV1::from_bytes(sha256(engine_tag)),
            }
        }
    }

    impl BenchAdapterV1 for FixtureAdapter {
        fn engine_id(&self) -> &str {
            "fixture-deterministic-v1"
        }

        fn engine_digest(&self) -> DigestV1 {
            self.engine
        }

        fn worker_digests(&self) -> Vec<String> {
            vec!["fixture-worker-v1".into()]
        }

        fn run_trial(
            &mut self,
            trial_index: u32,
            _parameters: &Value,
        ) -> Result<BenchTrialOutcomeV1, BenchExecErrorV1> {
            let success = !self.failures_at.contains(&trial_index);
            let receipt = if self.no_receipts {
                DigestV1::ZERO
            } else {
                DigestV1::from_bytes(sha256(format!("fixture-trial-{trial_index}").as_bytes()))
            };
            Ok(BenchTrialOutcomeV1 {
                trial_index,
                success,
                exact_metrics: vec![BenchExactMetricV1 {
                    name: "exact_reads".into(),
                    value: 1,
                    receipt_digest: receipt,
                }],
                estimated_metrics: vec![BenchEstimateMetricV1 {
                    name: "wall_clock_nanos".into(),
                    value: self.wall_clock_nanos,
                    source: "wall-clock".into(),
                }],
                trial_receipt_digest: None,
            })
        }
    }

    /// An adapter whose engine cannot run at all: the "cannot run" path.
    struct CrashingAdapter;

    impl BenchAdapterV1 for CrashingAdapter {
        fn engine_id(&self) -> &str {
            "fixture-crashing-v1"
        }

        fn engine_digest(&self) -> DigestV1 {
            DigestV1::from_bytes(sha256(b"fixture-engine-crash"))
        }

        fn worker_digests(&self) -> Vec<String> {
            vec!["fixture-worker-crash".into()]
        }

        fn run_trial(
            &mut self,
            _trial_index: u32,
            _parameters: &Value,
        ) -> Result<BenchTrialOutcomeV1, BenchExecErrorV1> {
            Err(bench_exec_error(
                BenchExecFailureCodeV1::AdapterFailed,
                "fixture adapter cannot run: no engine wired",
            ))
        }
    }

    fn retained_parameters() -> Value {
        json!({"fixture_kind": "deterministic-fixture-v1", "prefix_mode": "retained"})
    }

    fn rewrite_parameters() -> Value {
        json!({"fixture_kind": "deterministic-fixture-v1", "prefix_mode": "rewrite"})
    }

    #[test]
    fn catalog_entry_round_trips_and_root_seal_holds() {
        let entry = smoke_entry(300, retained_parameters()).expect("valid entry");
        // Serialize -> parse -> identical.
        let value = serde_json::to_value(&entry).expect("serialize");
        let reparsed: BenchCatalogEntryV1 =
            serde_json::from_value(value).expect("parse round trip");
        assert_eq!(reparsed, entry);
        // Root seal is content-derived and self-consistent.
        assert_eq!(entry.digest().expect("digest"), entry.workload_digest);
        // The shipped catalog entry loads and root-verifies.
        let shipped = load_catalog_entry_v1(shipped_catalog_path()).expect("shipped entry loads");
        assert_eq!(shipped.workload_digest, shipped.digest().expect("digest"));
        assert_eq!(shipped.benchmark_id, "r12-manifest-smoke");
        assert_eq!(shipped.workload.trials_required, 300);
    }

    #[test]
    fn tampered_catalog_entry_fails_root_seal() {
        let entry = smoke_entry(300, retained_parameters()).expect("valid entry");
        let mut value = serde_json::to_value(&entry).expect("serialize");
        value["title"] = json!("tampered title");
        let tampered: BenchCatalogEntryV1 = serde_json::from_value(value).expect("parse");
        assert_eq!(
            tampered.validate(),
            Err(bench_exec_error(
                BenchExecFailureCodeV1::CatalogRootSealMismatch,
                "catalog entry root seal does not match its content"
            ))
        );
    }

    #[test]
    fn insufficient_declared_trials_refuse_the_catalog() {
        // 10 declared trials can never certify q = 99/100 at alpha = 1/20
        // (Proposition 11.1 needs exactly 299): the catalog itself is refused.
        let error = smoke_entry(10, retained_parameters()).expect_err("must refuse");
        assert_eq!(error.code, BenchExecFailureCodeV1::SampleSizePrecondition);
        assert!(error.detail.contains("299"));
    }

    #[test]
    fn q99_gate_refuses_insufficient_trials_failures_and_dependence() {
        let spec = q99_spec();
        // 298 zero-failure trials: one short of the exact precondition.
        let error = bench_q99_gate_v1(298, 0, &spec).expect_err("must refuse");
        assert_eq!(error.code, BenchExecFailureCodeV1::Q99Refused);
        assert!(error.detail.contains("sample-size precondition"));
        // Any observed failure refuses the zero-failure bound.
        let error = bench_q99_gate_v1(300, 1, &spec).expect_err("must refuse");
        assert_eq!(error.code, BenchExecFailureCodeV1::Q99Refused);
        assert!(error.detail.contains("failures"));
        // A dependent (warm) trace refuses universal Q99 entirely.
        let dependent = BenchQ99SpecV1 {
            independent: false,
            ..spec.clone()
        };
        let error = bench_q99_gate_v1(300, 0, &dependent).expect_err("must refuse");
        assert_eq!(error.code, BenchExecFailureCodeV1::Q99Refused);
        assert!(error.detail.contains("dependent"));
        // A cluster design effect of 2 halves the effective sample to 150.
        let clustered = BenchQ99SpecV1 {
            design_effect_num: Some(2),
            design_effect_den: Some(1),
            ..spec
        };
        let error = bench_q99_gate_v1(300, 0, &clustered).expect_err("must refuse");
        assert_eq!(error.code, BenchExecFailureCodeV1::Q99Refused);
        assert!(error.detail.contains("150"));
        // The exact precondition: 299 zero-failure independent trials certify.
        let claim = bench_q99_gate_v1(299, 0, &q99_spec()).expect("certifies at 299");
        assert_eq!(claim.effective_trials, 299);
        assert_eq!(claim.success_target, "99/100");
        assert_eq!(claim.alpha, "1/20");
    }

    #[test]
    fn fixture_run_emits_a_sealed_manifest_that_read_back_verifies() {
        let entry = smoke_entry(300, retained_parameters()).expect("valid entry");
        let mut adapter = FixtureAdapter::new([]);
        let dir = tempfile::tempdir().expect("tempdir");
        let receipt =
            execute_and_emit_v1(dir.path(), &entry, &mut adapter).expect("run and emit");
        assert_eq!(receipt.benchmark_id, "r12-manifest-smoke");
        let run = match receipt.outcome {
            BenchExecOutcomeV1::Ran { run } => run,
            BenchExecOutcomeV1::Refused { reason } => panic!("run refused: {reason}"),
        };
        assert_eq!(run.trials_attempted, 300);
        assert_eq!(run.successes, 300);
        assert_eq!(run.failures, 0);
        match &run.q99 {
            BenchQ99OutcomeV1::Certified { claim } => {
                assert_eq!(claim.effective_trials, 300);
            }
            BenchQ99OutcomeV1::Refused { reason } => panic!("q99 refused: {reason}"),
        }
        assert_eq!(
            run.exact_metric_totals
                .iter()
                .find(|metric| metric.name == "exact_reads")
                .expect("exact_reads total")
                .value,
            300
        );
        // The persisted manifest read-back verifies and binds every digest.
        let manifest = read_exported_benchmark_manifest_v1(dir.path()).expect("read-back");
        assert_eq!(manifest.benchmark_id, "r12-manifest-smoke");
        assert_eq!(manifest.workload_digest, entry.workload_digest);
        assert_eq!(manifest.engine_digest, adapter.engine_digest());
        assert_eq!(manifest.worker_digests, vec!["fixture-worker-v1"]);
        assert_eq!(manifest.parameters, retained_parameters());
        assert_eq!(manifest.result_digest, run.result_digest);
        assert_eq!(manifest.receipt_digests, run.receipt_digests);
        assert_eq!(manifest.reproducibility, BenchmarkReproducibilityV1::Sealed);
        assert_eq!(receipt.manifest_seal, manifest.digest().expect("seal"));
    }

    #[test]
    fn observed_failures_refuse_q99_but_the_run_stays_sealed() {
        let entry = smoke_entry(300, retained_parameters()).expect("valid entry");
        let mut adapter = FixtureAdapter::new([7, 42]);
        let dir = tempfile::tempdir().expect("tempdir");
        let receipt =
            execute_and_emit_v1(dir.path(), &entry, &mut adapter).expect("run and emit");
        let run = match receipt.outcome {
            BenchExecOutcomeV1::Ran { run } => run,
            BenchExecOutcomeV1::Refused { reason } => panic!("run refused: {reason}"),
        };
        assert_eq!((run.successes, run.failures), (298, 2));
        match &run.q99 {
            BenchQ99OutcomeV1::Refused { reason } => {
                assert!(reason.contains("failures"), "reason: {reason}");
            }
            BenchQ99OutcomeV1::Certified { claim } => {
                panic!("must not certify with observed failures: {claim:?}");
            }
        }
        // The observation itself is reproducible, so the manifest is Sealed.
        let manifest = read_exported_benchmark_manifest_v1(dir.path()).expect("read-back");
        assert_eq!(manifest.reproducibility, BenchmarkReproducibilityV1::Sealed);
    }

    #[test]
    fn rerun_refuses_to_overwrite_a_sealed_manifest() {
        let entry = smoke_entry(300, retained_parameters()).expect("valid entry");
        let mut adapter = FixtureAdapter::new([]);
        let dir = tempfile::tempdir().expect("tempdir");
        execute_and_emit_v1(dir.path(), &entry, &mut adapter).expect("first run");
        let error =
            execute_and_emit_v1(dir.path(), &entry, &mut adapter).expect_err("must refuse");
        assert_eq!(error.code, BenchExecFailureCodeV1::ManifestExists);
        assert!(error.detail.contains("refusing to overwrite"));
    }

    #[test]
    fn tampered_manifest_fails_read_back_verification() {
        let entry = smoke_entry(300, retained_parameters()).expect("valid entry");
        let mut adapter = FixtureAdapter::new([]);
        let dir = tempfile::tempdir().expect("tempdir");
        execute_and_emit_v1(dir.path(), &entry, &mut adapter).expect("run and emit");
        let path = dir.path().join(TRACE_EXPORT_MANIFEST_FILE_V1);
        let mut content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read manifest"))
                .expect("manifest is json");
        content["manifest"]["benchmark_id"] = json!("tampered-id");
        std::fs::write(&path, serde_json::to_vec(&content).expect("serialize"))
            .expect("write tampered manifest");
        let error =
            read_exported_benchmark_manifest_v1(dir.path()).expect_err("must refuse tampering");
        assert!(error.to_string().contains("seal mismatch"));
    }

    #[test]
    fn exact_metric_without_receipt_is_refused() {
        let entry = smoke_entry(300, retained_parameters()).expect("valid entry");
        let mut adapter = FixtureAdapter::new([]);
        adapter.no_receipts = true;
        let error = run_benchmark_v1(&entry, &mut adapter).expect_err("must refuse");
        assert_eq!(error.code, BenchExecFailureCodeV1::ExactMetricWithoutReceipt);
    }

    #[test]
    fn adapter_crash_emits_nonreproducible_manifest_with_reason() {
        let entry = smoke_entry(300, retained_parameters()).expect("valid entry");
        let mut adapter = CrashingAdapter;
        let dir = tempfile::tempdir().expect("tempdir");
        let receipt =
            execute_and_emit_v1(dir.path(), &entry, &mut adapter).expect("refusal receipt");
        let reason = match receipt.outcome {
            BenchExecOutcomeV1::Refused { reason } => reason,
            BenchExecOutcomeV1::Ran { .. } => panic!("crashing adapter must refuse"),
        };
        assert!(reason.contains("no engine wired"), "reason: {reason}");
        let manifest = read_exported_benchmark_manifest_v1(dir.path()).expect("read-back");
        match &manifest.reproducibility {
            BenchmarkReproducibilityV1::NonReproducible {
                reason: manifest_reason,
            } => {
                assert!(manifest_reason.contains("no engine wired"));
            }
            BenchmarkReproducibilityV1::Sealed => panic!("cannot-run benchmark must be nonreproducible"),
        }
        assert_ne!(manifest.result_digest, DigestV1::ZERO);
    }

    fn rename_entry(mut entry: BenchCatalogEntryV1, benchmark_id: &str) -> BenchCatalogEntryV1 {
        entry.benchmark_id = benchmark_id.into();
        entry.workload_digest = DigestV1::ZERO;
        entry.workload_digest = entry.expected_digest().expect("recompute root seal");
        entry.validate().expect("renamed entry validates");
        entry
    }

    #[test]
    fn paired_diff_covers_treatment_only_metric_deltas() {
        let baseline_entry = rename_entry(
            smoke_entry(300, retained_parameters()).expect("valid baseline entry"),
            "r12-paired-base",
        );
        let mut baseline_adapter = FixtureAdapter::new([]);
        baseline_adapter.wall_clock_nanos = 1_000;
        let baseline_dir = tempfile::tempdir().expect("tempdir");
        let baseline_receipt = execute_and_emit_v1(
            baseline_dir.path(),
            &baseline_entry,
            &mut baseline_adapter,
        )
        .expect("baseline run");
        let baseline_run = match baseline_receipt.outcome {
            BenchExecOutcomeV1::Ran { run } => run,
            BenchExecOutcomeV1::Refused { reason } => panic!("baseline refused: {reason}"),
        };
        let baseline_manifest =
            read_exported_benchmark_manifest_v1(baseline_dir.path()).expect("baseline manifest");

        let treatment_entry = rename_entry(
            smoke_entry(300, rewrite_parameters()).expect("valid treatment entry"),
            "r12-paired-tx",
        );
        // Same engine, faster treatment fixture.
        let mut treatment_adapter = FixtureAdapter::with_engine(b"fixture-engine-v1");
        treatment_adapter.wall_clock_nanos = 500;
        let treatment_dir = tempfile::tempdir().expect("tempdir");
        let treatment_receipt = execute_and_emit_v1(
            treatment_dir.path(),
            &treatment_entry,
            &mut treatment_adapter,
        )
        .expect("treatment run");
        let treatment_run = match treatment_receipt.outcome {
            BenchExecOutcomeV1::Ran { run } => run,
            BenchExecOutcomeV1::Refused { reason } => panic!("treatment refused: {reason}"),
        };
        let treatment_manifest =
            read_exported_benchmark_manifest_v1(treatment_dir.path()).expect("treatment manifest");

        let pairing = BenchPairingV1 {
            baseline_benchmark_id: "r12-paired-base".into(),
            treatment_benchmark_id: "r12-paired-tx".into(),
            variable_dimension: "prefix_mode".into(),
        };
        let diff = paired_diff_over_treatment_only_v1(
            &baseline_manifest,
            &baseline_run,
            &treatment_manifest,
            &treatment_run,
            &pairing,
        )
        .expect("paired diff");
        diff.validate().expect("diff validates");
        assert_eq!(
            diff.metric_deltas
                .iter()
                .find(|delta| delta.name == "exact_reads")
                .expect("exact delta"),
            &BenchMetricDeltaV1 {
                name: "exact_reads".into(),
                kind: MeasurementKindV1::Exact,
                baseline_value: 300,
                treatment_value: 300,
                delta: 0,
            }
        );
        assert_eq!(
            diff.metric_deltas
                .iter()
                .find(|delta| delta.name == "wall_clock_nanos")
                .expect("estimate delta"),
            &BenchMetricDeltaV1 {
                name: "wall_clock_nanos".into(),
                kind: MeasurementKindV1::Estimate,
                baseline_value: 1_000 * 300,
                treatment_value: 500 * 300,
                delta: -150_000,
            }
        );
        assert_ne!(diff.diff_digest, DigestV1::ZERO);

        // Different engine digest: not a pair.
        let other_entry = rename_entry(
            smoke_entry(300, retained_parameters()).expect("valid other entry"),
            "r12-other",
        );
        let mut other_adapter = FixtureAdapter::with_engine(b"fixture-engine-v2");
        let other_dir = tempfile::tempdir().expect("tempdir");
        let other_receipt =
            execute_and_emit_v1(other_dir.path(), &other_entry, &mut other_adapter)
                .expect("other run");
        let other_run = match other_receipt.outcome {
            BenchExecOutcomeV1::Ran { run } => run,
            BenchExecOutcomeV1::Refused { reason } => panic!("other refused: {reason}"),
        };
        let other_manifest =
            read_exported_benchmark_manifest_v1(other_dir.path()).expect("other manifest");
        let error = paired_diff_over_treatment_only_v1(
            &baseline_manifest,
            &baseline_run,
            &other_manifest,
            &other_run,
            &pairing,
        )
        .expect_err("must refuse");
        assert_eq!(error.code, BenchExecFailureCodeV1::NotPaired);

        // Parameters differing beyond the variable dimension: refused.
        let two_dims = json!({"fixture_kind": "deterministic-fixture-v1", "prefix_mode": "rewrite", "extra": 1});
        // Same treatment id as the pairing so the diff reaches the parameter check.
        let widened_entry = rename_entry(
            smoke_entry(300, two_dims).expect("valid widened entry"),
            "r12-paired-tx",
        );
        let mut widened_adapter = FixtureAdapter::new([]);
        let widened_dir = tempfile::tempdir().expect("tempdir");
        let widened_receipt =
            execute_and_emit_v1(widened_dir.path(), &widened_entry, &mut widened_adapter)
                .expect("widened run");
        let widened_run = match widened_receipt.outcome {
            BenchExecOutcomeV1::Ran { run } => run,
            BenchExecOutcomeV1::Refused { reason } => panic!("widened refused: {reason}"),
        };
        let widened_manifest =
            read_exported_benchmark_manifest_v1(widened_dir.path()).expect("widened manifest");
        let error = paired_diff_over_treatment_only_v1(
            &baseline_manifest,
            &baseline_run,
            &widened_manifest,
            &widened_run,
            &pairing,
        )
        .expect_err("must refuse");
        assert_eq!(
            error.code,
            BenchExecFailureCodeV1::ParametersDifferBeyondVariable
        );
    }
}
