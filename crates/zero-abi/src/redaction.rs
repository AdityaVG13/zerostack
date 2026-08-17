//! Secret redaction and undeclared-effect tracing (ZS-SEC-004, ZS-STORE-004).
//!
//! [`Redactor`] is the only sanctioned path for secrets crossing authority
//! boundaries: provider prompts, UI exports, benchmark traces, and error
//! strings. It fails closed -- every occurrence of every configured secret is
//! replaced by the redaction token, in keys, string fields, array elements,
//! and nested objects; a redaction is only complete when the output contains
//! no configured secret substring.
//!
//! [`EffectTrace`] compares the effects a candidate DECLARED against the
//! effects OBSERVED during execution. Any undeclared effect (network,
//! process, environment, or unlisted mutation) yields
//! [`crate::SafetyVerdict::Unknown`] -- execution is blocked or Unsafe,
//! never silently admitted.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::effect::TypedEffectOperation;
use crate::verdict::SafetyVerdict;

pub const SECRETS_CONTRACT_VERSION: u16 = 1;
pub const DEFAULT_REDACTION_TOKEN: &str = "[REDACTED]";

/// Fail-closed error for redaction and effect-trace construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecretsError {
    InvalidPolicy(String),
    InvalidTrace(String),
    RedactionLeak(String),
}

impl fmt::Display for SecretsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy(detail) => write!(formatter, "invalid redaction policy: {detail}"),
            Self::InvalidTrace(detail) => write!(formatter, "invalid effect trace: {detail}"),
            Self::RedactionLeak(detail) => write!(formatter, "redaction leak: {detail}"),
        }
    }
}

impl Error for SecretsError {}

/// A redaction policy: exact secret strings (never regex, to avoid
/// catastrophic patterns) and the token that replaces them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RedactionPolicy {
    pub policy_version: u16,
    pub secrets: Vec<String>,
    pub redaction_token: String,
}

impl RedactionPolicy {
    pub fn new(secrets: Vec<String>, redaction_token: impl Into<String>) -> Result<Self, SecretsError> {
        let policy = Self {
            policy_version: SECRETS_CONTRACT_VERSION,
            secrets,
            redaction_token: redaction_token.into(),
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), SecretsError> {
        if self.policy_version != SECRETS_CONTRACT_VERSION {
            return Err(SecretsError::InvalidPolicy(format!(
                "unsupported policy version {}",
                self.policy_version
            )));
        }
        if self.redaction_token.is_empty() {
            return Err(SecretsError::InvalidPolicy(
                "redaction_token must be nonempty".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for secret in &self.secrets {
            if secret.is_empty() {
                return Err(SecretsError::InvalidPolicy(
                    "secrets must be nonempty strings".into(),
                ));
            }
            if secret == &self.redaction_token {
                return Err(SecretsError::InvalidPolicy(format!(
                    "secret {secret:?} equals the redaction token; redaction could not be total or would loop"
                )));
            }
            if !seen.insert(secret.clone()) {
                return Err(SecretsError::InvalidPolicy(format!(
                    "duplicate secret pattern {secret:?}"
                )));
            }
        }
        Ok(())
    }

    /// Whether the policy declares any secrets.
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty()
    }
}

/// The fail-closed redactor. Redaction is total: every string field, key,
/// and array element is scanned; any configured secret substring is replaced.
#[derive(Clone, Debug)]
pub struct Redactor {
    policy: RedactionPolicy,
}

impl Redactor {
    pub fn new(policy: RedactionPolicy) -> Result<Self, SecretsError> {
        policy.validate()?;
        Ok(Self { policy })
    }

    pub fn policy(&self) -> &RedactionPolicy {
        &self.policy
    }

    /// Redact one JSON value. String values and object keys are scanned;
    /// nested containers are walked depth-first.
    pub fn redact(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.redact_text(text)),
            Value::Array(items) => Value::Array(items.iter().map(|item| self.redact(item)).collect()),
            Value::Object(fields) => {
                let redacted = fields
                    .iter()
                    .map(|(key, value)| {
                        (self.redact_text(key), self.redact(value))
                    })
                    .collect();
                Value::Object(redacted)
            }
            other => other.clone(),
        }
    }

    fn redact_text(&self, text: &str) -> String {
        let mut output = text.to_owned();
        for secret in &self.policy.secrets {
            while output.contains(secret.as_str()) {
                output = output.replace(secret.as_str(), &self.policy.redaction_token);
            }
        }
        output
    }

    /// Redact one plain string (error messages, single-line emissions) and
    /// fail closed: if any configured secret survives, the caller gets
    /// `RedactionLeak` and MUST NOT emit the output (ZS-SEC-004).
    pub fn redact_text_checked(&self, text: &str) -> Result<String, SecretsError> {
        let redacted = self.redact_text(text);
        for secret in &self.policy.secrets {
            if redacted.contains(secret.as_str()) {
                return Err(SecretsError::RedactionLeak(format!(
                    "redacted text still contains secret {secret:?}"
                )));
            }
        }
        Ok(redacted)
    }

    /// Fail-closed completeness check: the redacted output must contain NO
    /// configured secret substring anywhere (string fields, keys, or
    /// serialized form). A leak here is an error, never a warning.
    pub fn check_no_leak(&self, redacted: &Value) -> Result<(), SecretsError> {
        let serialized = serde_json::to_string(redacted)
            .map_err(|error| SecretsError::RedactionLeak(error.to_string()))?;
        for secret in &self.policy.secrets {
            if serialized.contains(secret.as_str()) {
                return Err(SecretsError::RedactionLeak(format!(
                    "redacted output still contains secret {secret:?}"
                )));
            }
        }
        Ok(())
    }
}

/// A declared-vs-observed effect trace. `declared` is what the candidate
/// promised; `observed` is what execution actually performed. An undeclared
/// observed effect is a sandbox violation: blocked or Unsafe, never silently
/// admitted (ZS-STORE-004).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectTrace {
    pub trace_version: u16,
    pub declared: Vec<TypedEffectOperation>,
    pub observed: Vec<TypedEffectOperation>,
}

impl EffectTrace {
    pub fn new(
        declared: Vec<TypedEffectOperation>,
        observed: Vec<TypedEffectOperation>,
    ) -> Result<Self, SecretsError> {
        let trace = Self {
            trace_version: SECRETS_CONTRACT_VERSION,
            declared,
            observed,
        };
        trace.validate()?;
        Ok(trace)
    }

    pub fn validate(&self) -> Result<(), SecretsError> {
        if self.trace_version != SECRETS_CONTRACT_VERSION {
            return Err(SecretsError::InvalidTrace(format!(
                "unsupported trace version {}",
                self.trace_version
            )));
        }
        Ok(())
    }

    fn canonical(op: &TypedEffectOperation) -> String {
        match serde_json::to_value(op) {
            Ok(value) => crate::canonical_json(&value),
            Err(_) => String::new(),
        }
    }

    /// Observed effects with no declared counterpart, in deterministic
    /// order. Identity is canonical JSON of the operation, so a declared
    /// operation with different arguments is undeclared too.
    pub fn undeclared_effects(&self) -> Vec<&TypedEffectOperation> {
        let declared: std::collections::BTreeSet<String> =
            self.declared.iter().map(Self::canonical).collect();
        self.observed
            .iter()
            .filter(|operation| !declared.contains(&Self::canonical(operation)))
            .collect()
    }

    /// The fail-closed sandbox verdict: `Safe` only when every observed
    /// effect was declared; any undeclared effect is `Unknown` (blocked or
    /// Unsafe, never promotable).
    pub fn verdict(&self) -> SafetyVerdict {
        let undeclared = self.undeclared_effects();
        if undeclared.is_empty() {
            return SafetyVerdict::Safe;
        }
        SafetyVerdict::Unknown {
            reasons: undeclared
                .iter()
                .map(|operation| format!("undeclared_effect:{:?}", operation.effect_class()))
                .collect(),
        }
    }
}

