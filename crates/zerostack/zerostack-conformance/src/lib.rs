//! Shared conformance suite for ZeroStack-family engines. Each module contains contract tests
//! written against a zero-abi trait. Engines prove conformance by calling the `run_all` function
//! with their concrete implementation.

#![forbid(unsafe_code)]

pub mod file_engine;
pub mod token_engine;

use std::path::{Path, PathBuf};
use zero_abi::{CancellationProbe, EngineCallContext, EngineInvocation, KernelBudget};

/// Noop cancellation probe shared by all conformance runners.
#[derive(Debug)]
pub struct NoopProbe;
impl CancellationProbe for NoopProbe {
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Standard conformance invocation: hermetic workspace, generous budget,
/// noop cancellation. Callers provide the root and identity strings.
pub fn conformance_invocation(root: &Path, session: &str) -> EngineInvocation {
    EngineInvocation {
        context: EngineCallContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: session.to_owned(),
            cell_id: "conformance".into(),
            trace_id: format!("{session}-conformance"),
            deadline_unix_ms: u64::MAX,
            budget: KernelBudget {
                wall_ms: 30_000,
                cpu_ms: 30_000,
                memory_bytes: 128 * 1024 * 1024,
                call_limit: 1_024,
                task_limit: 16,
                output_byte_limit: 256 * 1024,
            },
        },
        cancellation: std::sync::Arc::new(NoopProbe),
    }
}

/// A hermetic test workspace with store directory pre-created.
pub struct ConformanceWorkspace {
    dir: tempfile::TempDir,
}

impl ConformanceWorkspace {
    pub fn new(tag: &str) -> std::io::Result<Self> {
        let dir = tempfile::Builder::new().prefix(tag).tempdir()?;
        std::fs::create_dir_all(dir.path().join(".zerostack"))?;
        Ok(Self { dir })
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn store(&self) -> PathBuf {
        self.root().join(".zerostack")
    }
}

/// Result of running a conformance suite. Every check that fails is
/// recorded with its name so the caller knows exactly which contracts
/// are violated — not just a boolean pass/fail.
#[derive(Debug, Default)]
pub struct SuiteResult {
    pub passed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl SuiteResult {
    pub fn record_pass(&mut self, name: &str) {
        self.passed.push(name.to_owned());
    }
    pub fn record_fail(&mut self, name: &str, detail: impl Into<String>) {
        self.failed.push((name.to_owned(), detail.into()));
    }
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
    /// Panic with a report naming every violated contract.
    pub fn require_clean(&self, suite_name: &str) {
        if !self.failed.is_empty() {
            let details: Vec<String> = self
                .failed
                .iter()
                .map(|(name, detail)| format!("  FAILED {}: {}", name, detail))
                .collect();
            panic!("{} suite violations:\n{}", suite_name, details.join("\n"));
        }
    }
}
