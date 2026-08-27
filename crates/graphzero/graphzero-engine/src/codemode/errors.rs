//! CodeMode error constructors.

use super::types::{CodeModeError, TimeoutEnvelope};

pub(crate) fn validation_error(message: impl Into<String>, step: Option<&str>) -> CodeModeError {
    let _ = step;
    CodeModeError {
        kind: "validation".into(),
        message: message.into(),
        retryable: false,
        timeout: None,
    }
}

pub(crate) fn policy_error(message: impl Into<String>, step: impl Into<String>) -> CodeModeError {
    let _ = step.into();
    CodeModeError {
        kind: "policy".into(),
        message: message.into(),
        retryable: false,
        timeout: None,
    }
}

pub(crate) fn substrate_error(
    message: impl Into<String>,
    step: impl Into<String>,
) -> CodeModeError {
    let _ = step.into();
    CodeModeError {
        kind: "substrate".into(),
        message: message.into(),
        retryable: false,
        timeout: None,
    }
}

pub(crate) fn cancelled_error(message: impl Into<String>) -> CodeModeError {
    CodeModeError {
        kind: "cancelled".into(),
        message: message.into(),
        retryable: true,
        timeout: None,
    }
}

/// Deadline stop carrying the bounded resume envelope.
///
/// The human message stays one short line; every machine-consumable detail
/// (code, phase, elapsed, configured deadline, index posture, resume ref, next
/// action) lives in the envelope so a harness renders one summary instead of
/// re-narrating nested JSON.
pub(crate) fn deadline_error_with_context(
    phase: &str,
    elapsed_ms: u64,
    deadline_ms: u64,
    index_state: &str,
    resume_ref: Option<String>,
) -> CodeModeError {
    let next_action = match resume_ref.as_deref() {
        Some(reference) => {
            format!("expand {reference} for partial results, then retry the remaining steps")
        }
        None if index_state == "cold" => "run graph.index first, then retry this plan".to_string(),
        None => format!("retry with a smaller plan or raise max_wall_ms above {deadline_ms}"),
    };
    CodeModeError {
        kind: "deadline_exceeded".into(),
        message: format!(
            "deadline exceeded after {elapsed_ms}ms of {deadline_ms}ms during {phase}"
        ),
        retryable: true,
        timeout: Some(TimeoutEnvelope {
            code: "GZ_CODEMODE_DEADLINE".into(),
            phase: phase.to_string(),
            elapsed_ms,
            deadline_ms,
            index_state: index_state.to_string(),
            resume_ref,
            next_action,
        }),
    }
}

/// Retryable machine-permit / backpressure busy (contract v1).
pub(crate) fn busy_error(message: impl Into<String>) -> CodeModeError {
    CodeModeError {
        kind: "busy".into(),
        message: message.into(),
        retryable: true,
        timeout: None,
    }
}

/// Host/policy approval gate (retryable until approved).
pub(crate) fn approval_error(message: impl Into<String>) -> CodeModeError {
    CodeModeError {
        kind: "approval".into(),
        message: message.into(),
        retryable: true,
        timeout: None,
    }
}

/// Domain deadline without a full timeout envelope (preserve kind + retryable).
pub(crate) fn deadline_exceeded_error(message: impl Into<String>) -> CodeModeError {
    CodeModeError {
        kind: "deadline_exceeded".into(),
        message: message.into(),
        retryable: true,
        timeout: None,
    }
}

pub(crate) fn not_found_error(message: impl Into<String>) -> CodeModeError {
    CodeModeError {
        kind: "not_found".into(),
        message: message.into(),
        retryable: false,
        timeout: None,
    }
}

pub(crate) fn runtime_error(message: impl Into<String>) -> CodeModeError {
    CodeModeError {
        kind: "runtime".into(),
        message: message.into(),
        retryable: false,
        timeout: None,
    }
}

pub(crate) fn sandbox_error(message: impl Into<String>) -> CodeModeError {
    CodeModeError {
        kind: "sandbox".into(),
        message: message.into(),
        retryable: false,
        timeout: None,
    }
}
