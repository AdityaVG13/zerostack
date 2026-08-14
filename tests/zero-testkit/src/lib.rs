//! Shared, transport-neutral promise suites for ZeroStack engines.
//!
//! Adapters implement EngineHarness and retain ownership of process transport,
//! binary discovery, and engine-specific protocol framing. The testkit passes typed
//! arguments and environment entries only; it never constructs a shell command.
//!
//! Promise-to-adapter contract:
//! - packaging_lifecycle: install v1, upgrade v2, rollback to the original digest,
//!   then uninstall without leaving install state.
//! - packaging_e2e: exercise packaged CodeMode and expose the engine artifact.
//! - racc_durability_matrix: write, reopen, and read while preserving its digest.
//! - readme_claims: validate named claims and expose a non-empty README and digest.
//! - readme_command_audit: audit documented commands without G* transport or a shell.
//!
//! Unsupported platforms must return Support::Unsupported with a reason. Such cases
//! make a report partial, never complete.

// deny (not forbid): the `env` module needs `unsafe` for edition-2024
// `std::env::{set_var,remove_var}` under its process-wide lock and opts in
// with a module-level `#![allow(unsafe_code)]`. Everything else stays safe.
#![deny(unsafe_code)]

#[cfg(feature = "full")]
pub mod aggregate_broker_gate;
#[cfg(feature = "full")]
pub mod assembly_kat;
#[cfg(feature = "full")]
pub mod authority;
#[cfg(feature = "full")]
pub mod bench_exec;
pub mod env;
#[cfg(feature = "full")]
pub mod invalidation_contract;
#[cfg(feature = "full")]
pub mod journal_fault_matrix;
#[cfg(feature = "full")]
pub mod kernel_fixture;
#[cfg(feature = "full")]
pub mod ledger_conservation;
#[cfg(feature = "full")]
pub mod raw_v2_slice;
#[cfg(feature = "full")]
pub mod robust_snap_model;
#[cfg(feature = "full")]
pub mod v6_conformance;
#[cfg(feature = "full")]
pub mod zero_bench_r;

use serde::{Deserialize, Serialize};
use std::fmt;
use zero_abi::{DEFAULT_MAX_FRAME_BYTES, FrameCodecError, WorkerResponseFrame};

/// Decode non-empty NDJSON responses through the canonical raw-worker codec.
pub fn decode_worker_transcript(bytes: &[u8]) -> Result<Vec<WorkerResponseFrame>, FrameCodecError> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| zero_abi::decode_response_frame(line, DEFAULT_MAX_FRAME_BYTES))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PromiseId(pub String);
impl PromiseId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CaseId(pub String);
impl CaseId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineIdentity {
    pub name: String,
    pub version: String,
}

/// Adapter-owned opaque temporary workspace handle. It contains no host path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    id: String,
}
impl Workspace {
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A single argument. This crate never parses or interpolates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arg(pub String);
impl Arg {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

/// Semantic operation mapped to engine transport by the adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineOperation {
    PackagingInstall,
    PackagingUpgrade,
    PackagingRollback,
    PackagingUninstall,
    PackagingE2e,
    RaccWrite,
    RaccReopen,
    RaccRead,
    ReadmeClaims,
    ReadmeCommandAudit,
}

/// Transport-neutral command with typed argv and environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invocation {
    pub operation: EngineOperation,
    pub argv: Vec<Arg>,
    pub env: Vec<EnvVar>,
}
impl Invocation {
    pub fn new(operation: EngineOperation, argv: impl IntoIterator<Item = Arg>) -> Self {
        Self {
            operation,
            argv: argv.into_iter().collect(),
            env: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResult {
    pub exit_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Relative logical filesystem path inside an adapter-owned workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbePath(pub String);
impl ProbePath {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactDigest(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    Packaging,
    RaccDurability,
    ReadmeAudit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Support {
    Supported,
    Unsupported { reason: String },
}

/// Object-safe boundary. Implementations own process and protocol transport.
pub trait EngineHarness {
    fn identity(&self) -> EngineIdentity;
    fn platform(&self) -> Platform;
    fn support(&self, capability: Capability) -> Support;
    fn create_workspace(&mut self, suite: &PromiseId) -> Result<Workspace, HarnessError>;
    fn invoke(
        &mut self,
        workspace: &Workspace,
        invocation: Invocation,
    ) -> Result<CommandResult, HarnessError>;
    fn path_exists(
        &mut self,
        workspace: &Workspace,
        path: &ProbePath,
    ) -> Result<bool, HarnessError>;
    fn read_utf8(
        &mut self,
        workspace: &Workspace,
        path: &ProbePath,
    ) -> Result<String, HarnessError>;
    fn artifact_digest(
        &mut self,
        workspace: &Workspace,
        path: &ProbePath,
    ) -> Result<ArtifactDigest, HarnessError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessError {
    pub message: String,
}
impl HarnessError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for HarnessError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CaseStatus {
    Pass,
    Fail { reason: String },
    Skip { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Evidence {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseResult {
    pub id: CaseId,
    pub status: CaseStatus,
    pub evidence: Vec<Evidence>,
}
impl CaseResult {
    pub fn new(id: CaseId, status: CaseStatus, mut evidence: Vec<Evidence>) -> Self {
        evidence.sort();
        Self {
            id,
            status,
            evidence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuiteStatus {
    Complete,
    Partial,
    Failed,
}

/// Serializable report consumed directly by conformance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuiteReport {
    pub promise: PromiseId,
    pub engine: EngineIdentity,
    pub platform: Platform,
    pub status: SuiteStatus,
    pub cases: Vec<CaseResult>,
}
impl SuiteReport {
    pub fn new(
        promise: PromiseId,
        engine: EngineIdentity,
        platform: Platform,
        mut cases: Vec<CaseResult>,
    ) -> Self {
        cases.sort_by(|left, right| left.id.cmp(&right.id));
        let status = if cases.is_empty()
            || cases
                .iter()
                .any(|case| matches!(&case.status, CaseStatus::Fail { .. }))
        {
            SuiteStatus::Failed
        } else if cases
            .iter()
            .any(|case| matches!(&case.status, CaseStatus::Skip { .. }))
        {
            SuiteStatus::Partial
        } else {
            SuiteStatus::Complete
        };
        Self {
            promise,
            engine,
            platform,
            status,
            cases,
        }
    }
}

fn args(values: &[&str]) -> Vec<Arg> {
    values.iter().map(|value| Arg::new(*value)).collect()
}

fn invoke_ok(
    harness: &mut dyn EngineHarness,
    workspace: &Workspace,
    operation: EngineOperation,
    argv: &[&str],
) -> Result<CommandResult, String> {
    let result = harness
        .invoke(workspace, Invocation::new(operation, args(argv)))
        .map_err(|error| error.message)?;
    if result.exit_code == Some(0) {
        Ok(result)
    } else {
        Err(format!(
            "{operation:?} exited {:?}: {}",
            result.exit_code,
            String::from_utf8_lossy(&result.stderr)
        ))
    }
}

fn run_case(
    harness: &mut dyn EngineHarness,
    promise: &str,
    capability: Capability,
    case: &str,
    body: impl FnOnce(&mut dyn EngineHarness, &Workspace) -> Result<Vec<Evidence>, String>,
) -> SuiteReport {
    let promise = PromiseId::new(promise);
    let engine = harness.identity();
    let platform = harness.platform();
    if let Support::Unsupported { reason } = harness.support(capability) {
        return SuiteReport::new(
            promise,
            engine,
            platform,
            vec![CaseResult::new(
                CaseId::new(case),
                CaseStatus::Skip { reason },
                Vec::new(),
            )],
        );
    }
    let result = match harness.create_workspace(&promise) {
        Ok(workspace) => match body(harness, &workspace) {
            Ok(evidence) => CaseResult::new(CaseId::new(case), CaseStatus::Pass, evidence),
            Err(reason) => {
                CaseResult::new(CaseId::new(case), CaseStatus::Fail { reason }, Vec::new())
            }
        },
        Err(error) => CaseResult::new(
            CaseId::new(case),
            CaseStatus::Fail {
                reason: error.message,
            },
            Vec::new(),
        ),
    };
    SuiteReport::new(promise, engine, platform, vec![result])
}

/// Install v1, upgrade v2, roll back exactly, and uninstall cleanly.
pub fn packaging_lifecycle(harness: &mut dyn EngineHarness) -> SuiteReport {
    run_case(
        harness,
        "packaging.lifecycle",
        Capability::Packaging,
        "lifecycle",
        |harness, workspace| {
            let state = ProbePath::new("install/state.json");
            invoke_ok(
                harness,
                workspace,
                EngineOperation::PackagingInstall,
                &["--version", "v1"],
            )?;
            if !harness
                .path_exists(workspace, &state)
                .map_err(|error| error.message)?
            {
                return Err("install state missing after install".into());
            }
            let installed = harness
                .artifact_digest(workspace, &state)
                .map_err(|error| error.message)?;
            invoke_ok(
                harness,
                workspace,
                EngineOperation::PackagingUpgrade,
                &["--version", "v2"],
            )?;
            let upgraded = harness
                .artifact_digest(workspace, &state)
                .map_err(|error| error.message)?;
            if upgraded == installed {
                return Err("upgrade did not change install state digest".into());
            }
            invoke_ok(
                harness,
                workspace,
                EngineOperation::PackagingRollback,
                &["--version", "v1"],
            )?;
            let rolled_back = harness
                .artifact_digest(workspace, &state)
                .map_err(|error| error.message)?;
            if rolled_back != installed {
                return Err("rollback did not restore install state digest".into());
            }
            invoke_ok(harness, workspace, EngineOperation::PackagingUninstall, &[])?;
            if harness
                .path_exists(workspace, &state)
                .map_err(|error| error.message)?
            {
                return Err("install state remained after uninstall".into());
            }
            Ok(vec![Evidence {
                name: "restored_digest".into(),
                value: rolled_back.0,
            }])
        },
    )
}

/// Exercise packaged CodeMode and digest its installed artifact.
pub fn packaging_e2e(harness: &mut dyn EngineHarness) -> SuiteReport {
    run_case(
        harness,
        "packaging.e2e",
        Capability::Packaging,
        "packaged_codemode",
        |harness, workspace| {
            invoke_ok(
                harness,
                workspace,
                EngineOperation::PackagingE2e,
                &["--surface", "codemode"],
            )?;
            let artifact = ProbePath::new("package/engine");
            if !harness
                .path_exists(workspace, &artifact)
                .map_err(|error| error.message)?
            {
                return Err("packaged engine artifact missing".into());
            }
            let digest = harness
                .artifact_digest(workspace, &artifact)
                .map_err(|error| error.message)?;
            Ok(vec![Evidence {
                name: "artifact_digest".into(),
                value: digest.0,
            }])
        },
    )
}

/// Write, reopen, and read durable state without changing its digest.
pub fn racc_durability_matrix(harness: &mut dyn EngineHarness) -> SuiteReport {
    run_case(
        harness,
        "racc.durability_matrix",
        Capability::RaccDurability,
        "write_reopen_read",
        |harness, workspace| {
            let store = ProbePath::new("racc/store");
            invoke_ok(
                harness,
                workspace,
                EngineOperation::RaccWrite,
                &["fixture", "durable-value"],
            )?;
            let before = harness
                .artifact_digest(workspace, &store)
                .map_err(|error| error.message)?;
            invoke_ok(harness, workspace, EngineOperation::RaccReopen, &[])?;
            let read = invoke_ok(harness, workspace, EngineOperation::RaccRead, &["fixture"])?;
            if read.stdout != b"durable-value" {
                return Err("reopened store returned the wrong durable value".into());
            }
            let after = harness
                .artifact_digest(workspace, &store)
                .map_err(|error| error.message)?;
            if before != after {
                return Err("store artifact digest changed across reopen".into());
            }
            Ok(vec![Evidence {
                name: "store_digest".into(),
                value: after.0,
            }])
        },
    )
}

/// Validate named README claims and record the README digest.
pub fn readme_claims(harness: &mut dyn EngineHarness) -> SuiteReport {
    run_case(
        harness,
        "readme.claims",
        Capability::ReadmeAudit,
        "documented_claims",
        |harness, workspace| {
            let readme = ProbePath::new("README.md");
            let text = harness
                .read_utf8(workspace, &readme)
                .map_err(|error| error.message)?;
            if text.trim().is_empty() {
                return Err("README is empty".into());
            }
            invoke_ok(
                harness,
                workspace,
                EngineOperation::ReadmeClaims,
                &["--all-named-promises"],
            )?;
            let digest = harness
                .artifact_digest(workspace, &readme)
                .map_err(|error| error.message)?;
            Ok(vec![Evidence {
                name: "readme_digest".into(),
                value: digest.0,
            }])
        },
    )
}

/// Audit documented commands using typed adapter argv.
pub fn readme_command_audit(harness: &mut dyn EngineHarness) -> SuiteReport {
    run_case(
        harness,
        "readme.command_audit",
        Capability::ReadmeAudit,
        "documented_commands",
        |harness, workspace| {
            let readme = ProbePath::new("README.md");
            if !harness
                .path_exists(workspace, &readme)
                .map_err(|error| error.message)?
            {
                return Err("README is missing".into());
            }
            let result = invoke_ok(
                harness,
                workspace,
                EngineOperation::ReadmeCommandAudit,
                &["--typed-argv"],
            )?;
            Ok(vec![Evidence {
                name: "audited_stdout_bytes".into(),
                value: result.stdout.len().to_string(),
            }])
        },
    )
}

/// Run all shared suites, ordered by promise identifier.
pub fn run_all(harness: &mut dyn EngineHarness) -> Vec<SuiteReport> {
    let mut reports = vec![
        packaging_lifecycle(harness),
        packaging_e2e(harness),
        racc_durability_matrix(harness),
        readme_claims(harness),
        readme_command_audit(harness),
    ];
    reports.sort_by(|left, right| left.promise.cmp(&right.promise));
    reports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_decoder_rejects_unknown_fields() {
        let canonical = b"{\"kind\":\"shutdown_ack\"}\n";
        assert!(matches!(
            decode_worker_transcript(canonical).as_deref(),
            Ok([WorkerResponseFrame::ShutdownAck])
        ));
        let mutant = b"{\"kind\":\"shutdown_ack\",\"extra\":true}\n";
        assert_eq!(
            decode_worker_transcript(mutant).unwrap_err().kind(),
            "invalid_frame"
        );
    }

    #[derive(Default)]
    struct FakeHarness {
        unsupported: Option<Capability>,
        fail: Option<EngineOperation>,
        invocations: Vec<Invocation>,
        install: Option<&'static str>,
        racc_written: bool,
    }

    impl EngineHarness for FakeHarness {
        fn identity(&self) -> EngineIdentity {
            EngineIdentity {
                name: "fakezero".into(),
                version: "1".into(),
            }
        }
        fn platform(&self) -> Platform {
            Platform::Linux
        }
        fn support(&self, capability: Capability) -> Support {
            if self.unsupported == Some(capability) {
                Support::Unsupported {
                    reason: "not available on fake platform".into(),
                }
            } else {
                Support::Supported
            }
        }
        fn create_workspace(&mut self, suite: &PromiseId) -> Result<Workspace, HarnessError> {
            Ok(Workspace::new(format!("workspace:{}", suite.0)))
        }
        fn invoke(
            &mut self,
            _workspace: &Workspace,
            invocation: Invocation,
        ) -> Result<CommandResult, HarnessError> {
            let operation = invocation.operation;
            self.invocations.push(invocation);
            if self.fail == Some(operation) {
                return Ok(CommandResult {
                    exit_code: Some(9),
                    stdout: Vec::new(),
                    stderr: b"injected".to_vec(),
                });
            }
            match operation {
                EngineOperation::PackagingInstall | EngineOperation::PackagingRollback => {
                    self.install = Some("v1")
                }
                EngineOperation::PackagingUpgrade => self.install = Some("v2"),
                EngineOperation::PackagingUninstall => self.install = None,
                EngineOperation::RaccWrite => self.racc_written = true,
                _ => {}
            }
            let stdout = if operation == EngineOperation::RaccRead {
                b"durable-value".to_vec()
            } else {
                b"ok".to_vec()
            };
            Ok(CommandResult {
                exit_code: Some(0),
                stdout,
                stderr: Vec::new(),
            })
        }
        fn path_exists(
            &mut self,
            _workspace: &Workspace,
            path: &ProbePath,
        ) -> Result<bool, HarnessError> {
            Ok(match path.0.as_str() {
                "install/state.json" => self.install.is_some(),
                "package/engine" | "README.md" => true,
                "racc/store" => self.racc_written,
                _ => false,
            })
        }
        fn read_utf8(
            &mut self,
            _workspace: &Workspace,
            path: &ProbePath,
        ) -> Result<String, HarnessError> {
            if path.0 == "README.md" {
                Ok("# fakezero claims".into())
            } else {
                Err(HarnessError::new("missing text"))
            }
        }
        fn artifact_digest(
            &mut self,
            _workspace: &Workspace,
            path: &ProbePath,
        ) -> Result<ArtifactDigest, HarnessError> {
            let value = match path.0.as_str() {
                "install/state.json" => self.install.unwrap_or("missing"),
                "package/engine" => "package-digest",
                "racc/store" if self.racc_written => "racc-digest",
                "README.md" => "readme-digest",
                _ => return Err(HarnessError::new("missing artifact")),
            };
            Ok(ArtifactDigest(value.into()))
        }
    }

    #[test]
    fn zero_testkit_passing_fake_harness_completes_every_suite() {
        let reports = run_all(&mut FakeHarness::default());
        assert_eq!(reports.len(), 5);
        assert!(
            reports
                .iter()
                .all(|report| report.status == SuiteStatus::Complete)
        );
    }

    type SuiteRunner = fn(&mut dyn EngineHarness) -> SuiteReport;

    #[test]
    fn zero_testkit_each_suite_owns_its_injected_failure() {
        let suites: [(EngineOperation, SuiteRunner); 5] = [
            (EngineOperation::PackagingInstall, packaging_lifecycle),
            (EngineOperation::PackagingE2e, packaging_e2e),
            (EngineOperation::RaccWrite, racc_durability_matrix),
            (EngineOperation::ReadmeClaims, readme_claims),
            (EngineOperation::ReadmeCommandAudit, readme_command_audit),
        ];
        for (operation, suite) in suites {
            let mut harness = FakeHarness {
                fail: Some(operation),
                ..FakeHarness::default()
            };
            assert_eq!(
                suite(&mut harness).status,
                SuiteStatus::Failed,
                "{operation:?}"
            );
        }
    }

    #[test]
    fn zero_testkit_skip_is_partial_not_complete() {
        let mut harness = FakeHarness {
            unsupported: Some(Capability::Packaging),
            ..FakeHarness::default()
        };
        let report = packaging_e2e(&mut harness);
        assert_eq!(report.status, SuiteStatus::Partial);
        assert!(matches!(report.cases[0].status, CaseStatus::Skip { .. }));
    }

    #[test]
    fn zero_testkit_argv_is_not_shell_interpolated() {
        let mut harness = FakeHarness::default();
        let workspace = Workspace::new("argv");
        let dangerous = "$(touch forbidden); * | echo changed";
        harness
            .invoke(
                &workspace,
                Invocation::new(
                    EngineOperation::ReadmeCommandAudit,
                    vec![Arg::new(dangerous)],
                ),
            )
            .expect("fake invocation");
        assert_eq!(harness.invocations[0].argv, vec![Arg::new(dangerous)]);
    }

    #[test]
    fn zero_testkit_case_and_suite_ordering_is_deterministic() {
        let engine = EngineIdentity {
            name: "fakezero".into(),
            version: "1".into(),
        };
        let report = SuiteReport::new(
            PromiseId::new("z"),
            engine,
            Platform::Linux,
            vec![
                CaseResult::new(CaseId::new("z"), CaseStatus::Pass, Vec::new()),
                CaseResult::new(CaseId::new("a"), CaseStatus::Pass, Vec::new()),
            ],
        );
        assert_eq!(
            report
                .cases
                .iter()
                .map(|case| case.id.0.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "z"]
        );
        let reports = run_all(&mut FakeHarness::default());
        assert!(
            reports
                .windows(2)
                .all(|pair| pair[0].promise <= pair[1].promise)
        );
    }
}
