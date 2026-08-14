//! ZS-SEC-004 fixtures: secret-leak surfaces at the zero-codemode host
//! boundary. Worker stderr (which may echo environment values) and remote
//! error messages are the model-visible error strings of the adapter; the
//! redacted emission helper must scrub them before they leave the host.

use super::*;

use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use zero_abi::{DEFAULT_REDACTION_TOKEN, RedactionPolicyV1, RedactorV1};

fn redactor(secrets: &[&str]) -> RedactorV1 {
    RedactorV1::new(
        RedactionPolicyV1::new(
            secrets.iter().map(|value| value.to_string()).collect(),
            DEFAULT_REDACTION_TOKEN,
        )
        .unwrap(),
    )
    .unwrap()
}

fn crash_with_stderr(text: &str) -> WorkerAdapterError {
    WorkerAdapterError::Crash {
        status: Some(ExitStatus::from_raw(1)),
        stderr: StderrCapture {
            text: text.to_string(),
            observed_bytes: text.len() as u64,
            complete: true,
            truncated: false,
        },
    }
}

/// The raw Display surface still carries the secret (proving the surface is
/// live), while the sanctioned emission helper removes every occurrence.
#[test]
fn crash_stderr_secret_is_redacted_at_emission() {
    let secret = "ZEROSTACK_SESSION_TOKEN=sk-live-abc123";
    let error = crash_with_stderr(&format!(
        "fixture panic: leaked env {secret} into stderr"
    ));
    let raw = error.to_string();
    assert!(raw.contains(secret), "raw Display must carry the secret");

    let redacted = error.redacted_message(&redactor(&[secret])).unwrap();
    assert!(!redacted.contains(secret));
    assert!(redacted.contains(DEFAULT_REDACTION_TOKEN));
}

/// A remote worker error whose message echoes a secret is scrubbed the same
/// way; the redactor also covers nested occurrences, not just the first.
#[test]
fn remote_error_message_secret_is_redacted_at_emission() {
    let secret = "ghp_1234567890abcdef";
    let error = WorkerAdapterError::Remote {
        request_id: Some("req-1".into()),
        kind: "provider_error".into(),
        message: format!("upstream said {secret} and again {secret}"),
        retryable: false,
        details: None,
        trace: None,
    };
    let raw = error.to_string();
    assert!(raw.contains(secret));
    let redacted = error.redacted_message(&redactor(&[secret])).unwrap();
    assert!(!redacted.contains(secret));
    assert_eq!(redacted.matches(DEFAULT_REDACTION_TOKEN).count(), 2);
}

/// The host boundary strips session capability env vars BEFORE the child
/// environment is assembled: raw session env is never forwarded, so it can
/// never be echoed by a worker or land in a crash/remote error string.
#[test]
fn session_capability_env_vars_are_stripped_before_child_env_assembly() {
    let mut env = BTreeMap::new();
    env.insert("ZEROSTACK_SESSION_TOKEN".into(), "sk-live-abc123".into());
    env.insert("ZEROSTACK_SESSION_SHUTDOWN_TOKEN".into(), "shutdown-token".into());
    env.insert("GRAPHZERO_REPO".into(), "/work/repo".into());
    let stripped = strip_session_env(&env);
    assert!(stripped.iter().all(|(key, _)| {
        key != "ZEROSTACK_SESSION_TOKEN" && key != "ZEROSTACK_SESSION_SHUTDOWN_TOKEN"
    }));
    assert_eq!(stripped, vec![("GRAPHZERO_REPO".into(), "/work/repo".into())]);
}
