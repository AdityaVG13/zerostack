//! Gauntlet Subject ≠ Oracle discriminator, greenfield `scenario()`,
//! content-addressed `ExecutionEnvelope`, and FailureBundle provenance.
//!
//! MCP `EngineIdentity::TokenZero` / `RegistryEngine::TokenZero` are registry
//! labels from `zero_abi`. They are forbidden as gauntlet identities (K-9).

use std::fmt;

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Pinned Phase 2 Subject string. Do not silently retarget.
pub const SUBJECT_IDENTITY: &str =
    "GauntletSubject::TokenZero@862e3e682cb8aee0e150c1cb0b116cb2e23a44e2";

pub const FORBIDDEN_MCP_ENGINE_IDENTITY: &str = "EngineIdentity::TokenZero";
pub const FORBIDDEN_MCP_REGISTRY_ENGINE: &str = "RegistryEngine::TokenZero";

/// Subject vs Oracle. Oracle modes match Phase 2 `[[mixed_oracles.modes]]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GauntletEngineIdentity {
    Subject,
    Oracle(GauntletOracle),
}

/// Greenfield mixed-oracle modes. `SelfOracle` is the Rust name for
/// `GauntletOracle::Self` (the keyword `Self` cannot be a variant).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GauntletOracle {
    Spec,
    Property,
    SelfOracle,
    RoundTrip,
    ExternalTool,
    ProviderTokenizer,
}

impl GauntletOracle {
    /// Every mixed-oracle mode. Subject is never in this list.
    pub const ALL: &[Self] = &[
        Self::Spec,
        Self::Property,
        Self::SelfOracle,
        Self::RoundTrip,
        Self::ExternalTool,
        Self::ProviderTokenizer,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Spec => "GauntletOracle::Spec::tokenzero-spec@HEAD-fb73416",
            Self::Property => "GauntletOracle::Property::proptest-1.11.0",
            Self::SelfOracle => "GauntletOracle::Self::prior-commit+cli-golden",
            Self::RoundTrip => "GauntletOracle::RoundTrip::pages-capsules-dual-store",
            Self::ExternalTool => "GauntletOracle::ExternalTool::nightly-2026-05-31",
            Self::ProviderTokenizer => "GauntletOracle::ProviderTokenizer::goldens",
        }
    }
}

impl GauntletEngineIdentity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Subject => SUBJECT_IDENTITY,
            Self::Oracle(mode) => mode.as_str(),
        }
    }
}

impl fmt::Display for GauntletEngineIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for GauntletOracle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Subject + one Oracle mode. Construction always pairs Subject vs Oracle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GauntletIdentityPair {
    pub subject: GauntletEngineIdentity,
    pub oracle: GauntletEngineIdentity,
}

impl GauntletIdentityPair {
    pub const fn new(oracle: GauntletOracle) -> Self {
        Self {
            subject: GauntletEngineIdentity::Subject,
            oracle: GauntletEngineIdentity::Oracle(oracle),
        }
    }

    pub fn assert_distinct(self) {
        assert_distinct(self.subject.as_str(), self.oracle.as_str());
        match (self.subject, self.oracle) {
            (GauntletEngineIdentity::Subject, GauntletEngineIdentity::Oracle(_)) => {}
            _ => panic!(
                "gauntlet pair must be Subject vs Oracle, got subject={} oracle={}",
                self.subject.as_str(),
                self.oracle.as_str()
            ),
        }
    }
}

/// Panics if `subject == oracle`, either side is empty, or either side is a
/// forbidden MCP registry identity.
pub fn assert_distinct(subject: &str, oracle: &str) {
    if subject.is_empty() || oracle.is_empty() {
        panic!("EngineIdentity unset: subject={subject:?} oracle={oracle:?}");
    }
    if is_forbidden_gauntlet_identity(subject) || is_forbidden_gauntlet_identity(oracle) {
        panic!(
            "forbidden MCP identity used as gauntlet Subject/Oracle: subject={subject:?} oracle={oracle:?}"
        );
    }
    if subject == oracle {
        panic!("EngineIdentity collision: oracle being compared against itself ({subject})");
    }
}

pub fn is_forbidden_gauntlet_identity(identity: &str) -> bool {
    identity.contains(FORBIDDEN_MCP_ENGINE_IDENTITY)
        || identity.contains(FORBIDDEN_MCP_REGISTRY_ENGINE)
}

/// Dual-store / fuzz fragment-error comparator.
///
/// Distinct taxonomy classes must not match. The only alias is RecoveryStore
/// `window-out-of-range` for embedded `fragment-out-of-range`. A Debug
/// `Fragment(...)` wrapper is not itself a class: `fragment-malformed` vs
/// `fragment-unknown-kind` is divergence.
pub fn fragment_reason_class_matches(embedded: &str, recovery: &str) -> bool {
    const CLASSES: &[&str] = &[
        "fragment-out-of-range",
        "fragment-not-utf8-boundary",
        "fragment-unknown-kind",
        "fragment-duplicate",
        "fragment-malformed",
        "fragment-reversed",
        "non_utf8_line_fragment",
        "non-utf8 line fragment",
        "NonUtf8Line",
    ];
    let Some(class) = CLASSES.iter().copied().find(|c| embedded.contains(c)) else {
        return false;
    };
    match class {
        "fragment-out-of-range" => {
            recovery.starts_with("fragment-out-of-range")
                || recovery.starts_with("window-out-of-range")
        }
        "non_utf8_line_fragment" | "non-utf8 line fragment" | "NonUtf8Line" => {
            recovery.starts_with("non_utf8_line_fragment")
                || recovery.contains("NonUtf8Line")
                || recovery.contains("non-utf8 line fragment")
        }
        class => recovery.starts_with(class),
    }
}

/// TokenZero persist / prune / WAL crash windows.
///
/// Names are protocol events, not SQL `BeforeWalHeaderWrite`. Live drivers
/// are `tests/unit/tokenzero-recovery/crash_windows.rs`. Protocol windows
/// arm Pattern 65 via `TOKENZERO_ARM_CRASH_BOUNDARY`. Concurrent lock and
/// torn-tail windows stay in-process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashBoundary {
    BeforePersistOnUnreadableSnapshot,
    BeforePruneOnUnreadableSnapshot,
    AfterJournalAppendBeforeSnapshotRewrite,
    AfterWalAppendSession,
    AfterWalTornTailKeepsComplete,
    AfterTmpWriteBeforeRename,
    PersistLockConcurrentWriters,
    PersistLockTmpSweep,
}

/// How the existing test exercises the named window. Not an arming claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashWindowKind {
    InProcessRefuse,
    InProcessRoundTrip,
    SimulatedKillBeforeRename,
    ConcurrentLockCoverage,
    SubprocessAbort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrashWindowDriver {
    pub path: &'static str,
    pub test_fn: &'static str,
    pub kind: CrashWindowKind,
}

impl CrashBoundary {
    pub const ALL: &[Self] = &[
        Self::BeforePersistOnUnreadableSnapshot,
        Self::BeforePruneOnUnreadableSnapshot,
        Self::AfterJournalAppendBeforeSnapshotRewrite,
        Self::AfterWalAppendSession,
        Self::AfterWalTornTailKeepsComplete,
        Self::AfterTmpWriteBeforeRename,
        Self::PersistLockConcurrentWriters,
        Self::PersistLockTmpSweep,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforePersistOnUnreadableSnapshot => "BeforePersistOnUnreadableSnapshot",
            Self::BeforePruneOnUnreadableSnapshot => "BeforePruneOnUnreadableSnapshot",
            Self::AfterJournalAppendBeforeSnapshotRewrite => {
                "AfterJournalAppendBeforeSnapshotRewrite"
            }
            Self::AfterWalAppendSession => "AfterWalAppendSession",
            Self::AfterWalTornTailKeepsComplete => "AfterWalTornTailKeepsComplete",
            Self::AfterTmpWriteBeforeRename => "AfterTmpWriteBeforeRename",
            Self::PersistLockConcurrentWriters => "PersistLockConcurrentWriters",
            Self::PersistLockTmpSweep => "PersistLockTmpSweep",
        }
    }

    /// Pattern 65: env-armed `std::process::abort` at this persist/WAL window.
    pub const fn is_subprocess_armed(self) -> bool {
        matches!(
            self,
            Self::BeforePersistOnUnreadableSnapshot
                | Self::BeforePruneOnUnreadableSnapshot
                | Self::AfterJournalAppendBeforeSnapshotRewrite
                | Self::AfterWalAppendSession
                | Self::AfterTmpWriteBeforeRename
        )
    }

    /// Live crash-window driver. Replaced the `d8c0844` deleted census.
    pub const fn existing_driver(self) -> Option<CrashWindowDriver> {
        Some(match self {
            Self::BeforePersistOnUnreadableSnapshot => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/crash_windows.rs",
                test_fn: "subprocess_abort_before_persist_on_unreadable",
                kind: CrashWindowKind::SubprocessAbort,
            },
            Self::BeforePruneOnUnreadableSnapshot => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/crash_windows.rs",
                test_fn: "subprocess_abort_before_prune_on_unreadable",
                kind: CrashWindowKind::SubprocessAbort,
            },
            Self::AfterJournalAppendBeforeSnapshotRewrite => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/crash_windows.rs",
                test_fn: "subprocess_abort_after_journal_append",
                kind: CrashWindowKind::SubprocessAbort,
            },
            Self::AfterWalAppendSession => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/crash_windows.rs",
                test_fn: "subprocess_abort_after_wal_append",
                kind: CrashWindowKind::SubprocessAbort,
            },
            Self::AfterWalTornTailKeepsComplete => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/crash_windows.rs",
                test_fn: "corrupt_journal_tail_keeps_complete_entries",
                kind: CrashWindowKind::InProcessRoundTrip,
            },
            Self::AfterTmpWriteBeforeRename => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/crash_windows.rs",
                test_fn: "subprocess_abort_after_tmp_before_rename",
                kind: CrashWindowKind::SubprocessAbort,
            },
            Self::PersistLockConcurrentWriters => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/crash_windows.rs",
                test_fn: "concurrent_persistence_preserves_all_thread_payloads",
                kind: CrashWindowKind::ConcurrentLockCoverage,
            },
            Self::PersistLockTmpSweep => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/crash_windows.rs",
                test_fn: "sweep_stale_tmp_reclaims_zero_store_leftovers",
                kind: CrashWindowKind::ConcurrentLockCoverage,
            },
        })
    }

    /// Historical paths only. Citing these as coverage is a lie.
    pub const fn deleted_driver_census(self) -> CrashWindowDriver {
        match self {
            Self::BeforePersistOnUnreadableSnapshot => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/store_hygiene_prune_option_tests.rs",
                test_fn: "persist_pending_refuses_unreadable_snapshot",
                kind: CrashWindowKind::InProcessRefuse,
            },
            Self::BeforePruneOnUnreadableSnapshot => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/store_hygiene_prune_option_tests.rs",
                test_fn: "prune_blob_sidecars_refuses_unreadable_snapshot",
                kind: CrashWindowKind::InProcessRefuse,
            },
            Self::AfterJournalAppendBeforeSnapshotRewrite => CrashWindowDriver {
                path: "tests/recovery/unit/store.rs",
                test_fn: "second_process_persist_appends_journal_without_snapshot_rewrite",
                kind: CrashWindowKind::InProcessRoundTrip,
            },
            Self::AfterWalAppendSession => CrashWindowDriver {
                path: "tests/recovery/inline/memory_verbs__tests.rs",
                test_fn: "apply_commit_session_persists_and_missing_path_fails_loud",
                kind: CrashWindowKind::InProcessRoundTrip,
            },
            Self::AfterWalTornTailKeepsComplete => CrashWindowDriver {
                path: "tests/recovery/unit/store.rs",
                test_fn: "corrupt_journal_tail_keeps_complete_entries",
                kind: CrashWindowKind::InProcessRoundTrip,
            },
            Self::AfterTmpWriteBeforeRename => CrashWindowDriver {
                path: "tests/engine/inline/ledger__ledger_tests.rs",
                test_fn: "task_cost_report_kill_before_rename_keeps_previous_complete_files",
                kind: CrashWindowKind::SimulatedKillBeforeRename,
            },
            Self::PersistLockConcurrentWriters => CrashWindowDriver {
                path: "tests/recovery/unit/store.rs",
                test_fn: "concurrent_persistence_preserves_all_thread_payloads",
                kind: CrashWindowKind::ConcurrentLockCoverage,
            },
            Self::PersistLockTmpSweep => CrashWindowDriver {
                path: "tests/unit/tokenzero-recovery/store_hygiene_prune_option_tests.rs",
                test_fn: "sweep_stale_tmp_removes_expired_under_lock",
                kind: CrashWindowKind::ConcurrentLockCoverage,
            },
        }
    }
}

/// Greenfield scenario outcome. Both-error is agreement; mixed Ok/Err panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScenarioAgreement<T, E> {
    BothOk(T),
    BothErr { subject: E, oracle: E },
}

/// Greenfield `scenario()`: action (Subject) + spec_check (Oracle).
///
/// - both Ok → `BothOk`
/// - both Err → `BothErr` (agreement)
/// - one Ok and one Err → hard fail (panic)
pub fn scenario<T, E: fmt::Debug>(
    name: &str,
    pair: GauntletIdentityPair,
    action: impl FnOnce() -> Result<T, E>,
    spec_check: impl FnOnce() -> Result<(), E>,
) -> ScenarioAgreement<T, E> {
    pair.assert_distinct();
    match (action(), spec_check()) {
        (Ok(output), Ok(())) => ScenarioAgreement::BothOk(output),
        (Err(subject), Err(oracle)) => ScenarioAgreement::BothErr { subject, oracle },
        (Ok(_), Err(oracle)) => {
            panic!("{name}: subject/spec divergence: subject Ok, oracle Err ({oracle:?})")
        }
        (Err(subject), Ok(())) => {
            panic!("{name}: subject/spec divergence: subject Err ({subject:?}), oracle Ok")
        }
    }
}

/// Differential V2 envelope, greenfield-adapted (no SQL PRAGMAs).
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionEnvelope {
    pub format_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub scenario_id: String,
    pub seed: u64,
    pub engines: EngineVersions,
    pub workload: Vec<String>,
    pub canonicalization: CanonicalizationRules,
}

#[derive(Debug, Clone, Serialize)]
pub struct EngineVersions {
    pub subject_identity: String,
    pub oracle_identity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CanonicalizationRules {
    pub float_tolerance: String,
    pub unordered_results_as_multiset: bool,
    pub error_match_by_category: bool,
    pub normalize_whitespace: bool,
}

#[derive(Serialize)]
struct CanonicalEnvelope<'a> {
    format_version: u32,
    scenario_id: &'a str,
    seed: u64,
    engines: &'a EngineVersions,
    workload: &'a [String],
    canonicalization: &'a CanonicalizationRules,
}

impl CanonicalizationRules {
    pub fn greenfield_bit_exact() -> Self {
        Self {
            float_tolerance: "0".to_string(),
            unordered_results_as_multiset: false,
            error_match_by_category: true,
            normalize_whitespace: true,
        }
    }
}

impl ExecutionEnvelope {
    pub const FORMAT_VERSION: u32 = 1;

    pub fn from_pair(
        scenario_id: impl Into<String>,
        seed: u64,
        pair: GauntletIdentityPair,
        workload: Vec<String>,
    ) -> Self {
        pair.assert_distinct();
        Self {
            format_version: Self::FORMAT_VERSION,
            run_id: None,
            scenario_id: scenario_id.into(),
            seed,
            engines: EngineVersions {
                subject_identity: pair.subject.as_str().to_string(),
                oracle_identity: pair.oracle.as_str().to_string(),
            },
            workload,
            canonicalization: CanonicalizationRules::greenfield_bit_exact(),
        }
    }

    /// SHA-256 hex of canonical JSON **excluding** `run_id`.
    pub fn artifact_id(&self) -> String {
        let canonical = CanonicalEnvelope {
            format_version: self.format_version,
            scenario_id: &self.scenario_id,
            seed: self.seed,
            engines: &self.engines,
            workload: &self.workload,
            canonicalization: &self.canonicalization,
        };
        let json = serde_json::to_string(&canonical).expect("envelope serialization must not fail");
        sha256_hex(json.as_bytes())
    }

    /// Pattern 15: envelope identities must match the pair and stay distinct.
    pub fn assert_engine_identities(&self, pair: GauntletIdentityPair) {
        pair.assert_distinct();
        assert_eq!(
            self.engines.subject_identity,
            pair.subject.as_str(),
            "envelope subject identity drifted from pair"
        );
        assert_eq!(
            self.engines.oracle_identity,
            pair.oracle.as_str(),
            "envelope oracle identity drifted from pair"
        );
        assert_distinct(
            &self.engines.subject_identity,
            &self.engines.oracle_identity,
        );
    }
}

/// Polish-bar FailureBundle schema. Partial provenance beats no bundle.
pub const FAILURE_BUNDLE_SCHEMA: &str = "failure_bundle.v1.0.0";

/// RFC 6901 pointer to the first byte/class disagreement, never "test failed".
pub const FAILURE_FIRST_DIVERGENCE_JSONPTR: &str = "/failure/first_divergence";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FailureType {
    Assertion,
    Panic,
    Divergence,
    Timeout,
    Other,
}

/// Exact disagreement the jsonptr `/failure/first_divergence` resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FirstDivergence {
    pub byte_offset: Option<u64>,
    pub subject_byte: Option<String>,
    pub oracle_byte: Option<String>,
    pub path: String,
    pub subject: String,
    pub oracle: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureBody {
    pub failure_type: FailureType,
    pub first_divergence: FirstDivergence,
    pub expected_vs_actual: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FailureProvenance {
    pub seed: u64,
    pub fixture_id: String,
    pub schedule_fingerprint: String,
    pub repro_command: String,
    pub git_sha: String,
    pub toolchain_version: String,
    pub platform: String,
    pub feature_flags: Vec<String>,
    pub artifact_sha256: Vec<String>,
}

/// Pattern 90 FailureBundle v1.0.0. Seed + fixture + repro command are required.
#[derive(Debug, Clone, Serialize)]
pub struct FailureBundle {
    pub schema: String,
    pub failure: FailureBody,
    pub provenance: FailureProvenance,
    pub engines: EngineVersions,
    pub envelope_artifact_id: String,
}

impl FirstDivergence {
    /// First index where `subject` and `oracle` differ, including EOF.
    pub fn of_bytes(subject: &[u8], oracle: &[u8]) -> Self {
        let mut i = 0usize;
        loop {
            match (subject.get(i), oracle.get(i)) {
                (Some(a), Some(b)) if a == b => {
                    i += 1;
                    continue;
                }
                (a, b) => {
                    return Self {
                        byte_offset: Some(i as u64),
                        subject_byte: a.map(|x| format!("0x{x:02x}")),
                        oracle_byte: b.map(|x| format!("0x{x:02x}")),
                        path: format!("/bytes/{i}"),
                        subject: format!("{subject:?}"),
                        oracle: format!("{oracle:?}"),
                    };
                }
            }
        }
    }
}

impl FailureBundle {
    pub fn first_divergence_jsonptr(&self) -> &'static str {
        FAILURE_FIRST_DIVERGENCE_JSONPTR
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("FailureBundle serialization must not fail")
    }

    /// RFC 6901 dereference. The polish-bar pointer must hit `first_divergence`.
    pub fn dereference(&self, jsonptr: &str) -> Option<serde_json::Value> {
        self.to_json().pointer(jsonptr).cloned()
    }

    /// `None` when bytes agree (no bundle on success).
    pub fn from_byte_divergence(
        envelope: &ExecutionEnvelope,
        fixture_id: impl Into<String>,
        repro_command: impl Into<String>,
        subject: &[u8],
        oracle: &[u8],
    ) -> Option<Self> {
        assert_distinct(
            &envelope.engines.subject_identity,
            &envelope.engines.oracle_identity,
        );
        if subject == oracle {
            return None;
        }
        let first = FirstDivergence::of_bytes(subject, oracle);
        let fixture_id = fixture_id.into();
        let repro_command = repro_command.into();
        let schedule = sha256_hex(
            format!(
                "{}:{}:{}",
                envelope.scenario_id,
                envelope.seed,
                envelope.workload.join("\n")
            )
            .as_bytes(),
        );
        Some(Self {
            schema: FAILURE_BUNDLE_SCHEMA.to_string(),
            failure: FailureBody {
                failure_type: FailureType::Divergence,
                expected_vs_actual: format!("subject={} oracle={}", first.subject, first.oracle),
                first_divergence: first,
            },
            provenance: FailureProvenance {
                seed: envelope.seed,
                fixture_id,
                schedule_fingerprint: schedule,
                repro_command,
                git_sha: SUBJECT_IDENTITY
                    .rsplit('@')
                    .next()
                    .unwrap_or(SUBJECT_IDENTITY)
                    .to_string(),
                toolchain_version: "nightly-2026-05-31".to_string(),
                platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                feature_flags: vec!["gauntlet-greenfield".into(), "spec-oracle".into()],
                artifact_sha256: vec![envelope.artifact_id()],
            },
            engines: envelope.engines.clone(),
            envelope_artifact_id: envelope.artifact_id(),
        })
    }
}

/// Byte comparator with EngineIdentity on the envelope. Equal → Ok.
/// Divergence → FailureBundle with `/failure/first_divergence` populated.
pub fn compare_bytes(
    envelope: &ExecutionEnvelope,
    fixture_id: impl Into<String>,
    repro_command: impl Into<String>,
    subject: &[u8],
    oracle: &[u8],
) -> Result<(), FailureBundle> {
    match FailureBundle::from_byte_divergence(envelope, fixture_id, repro_command, subject, oracle)
    {
        None => Ok(()),
        Some(bundle) => Err(bundle),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Verifiable / Ambiguous classification copied from Phase 2 SPEC-TAGS.
/// `existing_driver` is a census path when a live verifier already exists.
/// Absence is Uncovered, never a silent pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecTagClass {
    Verifiable,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecTagWire {
    pub tag: &'static str,
    pub class: SpecTagClass,
    pub existing_driver: Option<&'static str>,
}

impl SpecTagWire {
    pub const fn is_wired(self) -> bool {
        matches!(self.class, SpecTagClass::Verifiable) && self.existing_driver.is_some()
    }
}

/// Coverage table for Phase 3. Ambiguous tags stay uncovered.
/// `existing_driver` is a live path only. `d8c0844` deleted most drivers;
/// Verifiable + None is Uncovered, never a silent pass.
pub const SPEC_TAG_WIRES: &[SpecTagWire] = &[
    SpecTagWire {
        tag: "SPEC-TZ-TOK-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-core/model_artifact_limits.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-TOK-002",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-pulse/tokenizer_id_grammar.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-CAP-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-core/model_artifact_limits.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-CAP-002",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-core/model_artifact_limits.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-DV-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-PFX-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-recovery/prefix_stability_floor.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-ELIG-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-engine/prefix_probe_eligibility.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-ELIG-002",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-engine/prefix_probe_eligibility.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-RS-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-HR-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-SH-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-NOV-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-NOV-002",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-CONT-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-NW-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("benchmarks/test_never_worse_gate.py"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-NW-002",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("benchmarks/test_never_worse_gate.py"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-NW-003",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("benchmarks/competitor-bakeoff.sh"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-NW-004",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("benchmarks/test_never_worse_gate.py"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-HUB-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-test-support/gauntlet_oracle_smoke.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-HUB-002",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-test-support/gauntlet_oracle_smoke.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-FAIL-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-recovery/expand_fragment_oracle.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-GOLD-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-core/provider_tokenizer_goldens.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-ID-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("crates/tokenzero/tokenzero-test-support/src/gauntlet.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-ID-002",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("crates/tokenzero/tokenzero-test-support/src/gauntlet.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-METRIC-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-engine/phase4_output_contract_goldens.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-RT-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-RT-002",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("fuzz/fuzz_targets/expand_fragment_differential.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-SKIP-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-test-support/parity_taxonomy_tests.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-TELE-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-REC-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-HOT-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-OMIT-001",
        class: SpecTagClass::Verifiable,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-RB-001",
        class: SpecTagClass::Verifiable,
        existing_driver: Some("tests/unit/tokenzero-install/rollback_drift.rs"),
    },
    SpecTagWire {
        tag: "SPEC-TZ-STRICT-001",
        class: SpecTagClass::Ambiguous,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-DV-SURF-001",
        class: SpecTagClass::Ambiguous,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-RS-SURF-001",
        class: SpecTagClass::Ambiguous,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-NOV-SURF-001",
        class: SpecTagClass::Ambiguous,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-CONT-SURF-001",
        class: SpecTagClass::Ambiguous,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-HR-SURF-001",
        class: SpecTagClass::Ambiguous,
        existing_driver: None,
    },
    SpecTagWire {
        tag: "SPEC-TZ-SH-PUB-001",
        class: SpecTagClass::Ambiguous,
        existing_driver: None,
    },
];
