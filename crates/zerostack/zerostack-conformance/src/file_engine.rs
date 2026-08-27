//! FileEngine contract tests: every implementation of `zero_abi::FileEngine`
//! must satisfy these. Engines call `run_all` with their concrete instance.

use zero_abi::{
    EngineErrorKind, FileEffectKind, FileEffectRequest, FileEngine, FileReadRequest,
    ReadOptions,
};
use std::path::PathBuf;

use crate::{conformance_invocation, ConformanceWorkspace, SuiteResult};

/// Run the full FileEngine conformance suite.
pub fn run_all(engine: &dyn FileEngine, root: &PathBuf, session: &str) -> SuiteResult {
    let mut result = SuiteResult::default();
    let invocation = conformance_invocation(root, session);

    // --- write → read round-trip ---
    {
        let name = "write_read_roundtrip";
        let request = FileEffectRequest {
            kind: FileEffectKind::Write,
            path: PathBuf::from("conf_roundtrip.txt"),
            content: Some(b"round-trip payload".to_vec()),
            patch: None,
            expected_preimage: None,
            expect_absent: false,
        };
        match engine.apply(&invocation, request) {
            Ok(_) => {
                let read = engine.read(
                    &invocation,
                    FileReadRequest {
                        path: PathBuf::from("conf_roundtrip.txt"),
                        options: ReadOptions::default(),
                    },
                );
                match read {
                    Ok(snapshot)
                        if snapshot.inline_utf8.as_deref() == Some("round-trip payload") =>
                    {
                        result.record_pass(name);
                    }
                    Ok(snapshot) => {
                        result.record_fail(name, format!("content mismatch: {:?}", snapshot.inline_utf8));
                    }
                    Err(e) => result.record_fail(name, format!("read after write failed: {e}")),
                }
            }
            Err(e) => result.record_fail(name, format!("apply write failed: {e}")),
        }
    }

    // --- read missing → NotFound (not silent empty) ---
    {
        let name = "read_missing_returns_not_found";
        let read = engine.read(
            &invocation,
            FileReadRequest {
                path: PathBuf::from("definitely_missing_conf_test.txt"),
                options: ReadOptions::default(),
            },
        );
        match read {
            Err(e) if e.kind == EngineErrorKind::NotFound => result.record_pass(name),
            Err(e) => result.record_fail(name, format!("wrong error kind: {:?}", e.kind)),
            Ok(_) => result.record_fail(name, "missing file returned Ok — silent data loss"),
        }
    }

    // --- digest stability: same bytes → same digest across calls ---
    {
        let name = "digest_stability";
        let request = FileEffectRequest {
            kind: FileEffectKind::Write,
            path: PathBuf::from("conf_digest_stability.txt"),
            content: Some(b"digest stability probe".to_vec()),
            patch: None,
            expected_preimage: None,
            expect_absent: false,
        };
        let _ = engine.apply(&invocation, request.clone());
        let read1 = engine.read(
            &invocation,
            FileReadRequest {
                path: PathBuf::from("conf_digest_stability.txt"),
                options: ReadOptions::default(),
            },
        );
        let read2 = engine.read(
            &invocation,
            FileReadRequest {
                path: PathBuf::from("conf_digest_stability.txt"),
                options: ReadOptions::default(),
            },
        );
        match (read1, read2) {
            (Ok(a), Ok(b)) if a.content == b.content => result.record_pass(name),
            (Ok(a), Ok(b)) => result.record_fail(
                name,
                format!("digests differ for identical content: {} vs {}", a.content, b.content),
            ),
            (e1, e2) => result.record_fail(name, format!("reads failed: {e1:?} / {e2:?}")),
        }
    }

    result
}
