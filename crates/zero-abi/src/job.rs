//! Canonical typed ABI for session-owned TokenZero background jobs.
//!
//! Aggregate CodeMode owns the public `zero.token.job` surface. TokenZero owns
//! execution and polling. These types freeze the value exchanged in raw-worker
//! v2 `call.request.args` and `result.value` without adding a planner, JavaScript,
//! MCP, or nested-CodeMode concept to the worker protocol.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::digest::contract_digest_hex;

pub const TOKEN_JOB_ABI_VERSION_V1: &str = "zerostack.token_job.v1";
pub const TOKEN_JOB_OPERATION_V1: &str = "job";
pub const TOKEN_JOB_DEFAULT_WAIT_MS_V1: u64 = 30_000;
pub const TOKEN_JOB_MAX_WAIT_MS_V1: u64 = 30_000;
pub const TOKEN_JOB_DEFAULT_TAIL_BYTES_V1: u64 = 8 * 1024;
pub const TOKEN_JOB_MAX_TAIL_BYTES_V1: u64 = 64 * 1024;
pub const TOKEN_JOB_MAX_ID_BYTES_V1: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenJobPollRequestV1 {
    pub id: String,
    pub wait_ms: u64,
    pub since: u64,
    pub tail_bytes: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TokenJobPollRequestWireV1 {
    id: String,
    #[serde(default = "default_wait_ms")]
    wait_ms: u64,
    #[serde(default)]
    since: u64,
    #[serde(default = "default_tail_bytes")]
    tail_bytes: u64,
}

const fn default_wait_ms() -> u64 {
    TOKEN_JOB_DEFAULT_WAIT_MS_V1
}

const fn default_tail_bytes() -> u64 {
    TOKEN_JOB_DEFAULT_TAIL_BYTES_V1
}

impl TokenJobPollRequestV1 {
    pub fn new(id: impl Into<String>) -> Result<Self, TokenJobContractError> {
        Self::with_options(
            id,
            TOKEN_JOB_DEFAULT_WAIT_MS_V1,
            0,
            TOKEN_JOB_DEFAULT_TAIL_BYTES_V1,
        )
    }

    pub fn with_options(
        id: impl Into<String>,
        wait_ms: u64,
        since: u64,
        tail_bytes: u64,
    ) -> Result<Self, TokenJobContractError> {
        let request = Self {
            id: id.into(),
            wait_ms,
            since,
            tail_bytes,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<(), TokenJobContractError> {
        validate_id(&self.id)?;
        if self.wait_ms > TOKEN_JOB_MAX_WAIT_MS_V1 {
            return Err(TokenJobContractError::WaitTooLong);
        }
        if !(1..=TOKEN_JOB_MAX_TAIL_BYTES_V1).contains(&self.tail_bytes) {
            return Err(TokenJobContractError::InvalidTailLimit);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TokenJobPollRequestV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TokenJobPollRequestWireV1::deserialize(deserializer)?;
        Self::with_options(wire.id, wire.wait_ms, wire.since, wire.tail_bytes)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenJobStatusV1 {
    Running,
    Exited,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenJobPollResultV1 {
    pub id: String,
    pub status: TokenJobStatusV1,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub tail: String,
    pub tail_utf8_lossless: bool,
    pub tail_bytes: u64,
    pub log_bytes: u64,
    pub cursor: u64,
    pub version: u64,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_poll_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TokenJobPollResultWireV1 {
    id: String,
    status: TokenJobStatusV1,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    exit_code: Option<i32>,
    tail: String,
    tail_utf8_lossless: bool,
    tail_bytes: u64,
    log_bytes: u64,
    cursor: u64,
    version: u64,
    changed: bool,
    #[serde(default)]
    next_poll_ms: Option<u64>,
}

impl TokenJobPollResultV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        status: TokenJobStatusV1,
        pid: Option<u32>,
        exit_code: Option<i32>,
        tail: impl Into<String>,
        tail_utf8_lossless: bool,
        tail_bytes: u64,
        log_bytes: u64,
        cursor: u64,
        version: u64,
        changed: bool,
        next_poll_ms: Option<u64>,
    ) -> Result<Self, TokenJobContractError> {
        let result = Self {
            id: id.into(),
            status,
            pid,
            exit_code,
            tail: tail.into(),
            tail_utf8_lossless,
            tail_bytes,
            log_bytes,
            cursor,
            version,
            changed,
            next_poll_ms,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), TokenJobContractError> {
        validate_id(&self.id)?;
        if self.tail_bytes > TOKEN_JOB_MAX_TAIL_BYTES_V1
            || self.tail.len() as u64 > TOKEN_JOB_MAX_TAIL_BYTES_V1
        {
            return Err(TokenJobContractError::TailTooLarge);
        }
        if self.cursor > self.log_bytes || self.tail_bytes > self.cursor {
            return Err(TokenJobContractError::InvalidCursor);
        }
        if self.tail.is_empty() != (self.tail_bytes == 0)
            || (self.tail_utf8_lossless && self.tail.len() as u64 != self.tail_bytes)
            || (self.tail_bytes == 0 && !self.tail_utf8_lossless)
        {
            return Err(TokenJobContractError::InconsistentTail);
        }
        if !self.changed && (self.status != TokenJobStatusV1::Running || self.tail_bytes != 0) {
            return Err(TokenJobContractError::InconsistentChange);
        }
        if self.status == TokenJobStatusV1::Running && self.exit_code.is_some() {
            return Err(TokenJobContractError::InvalidExitCode);
        }
        if (self.status == TokenJobStatusV1::Running) != self.next_poll_ms.is_some() {
            return Err(TokenJobContractError::InconsistentPollState);
        }
        if self
            .next_poll_ms
            .is_some_and(|value| value > TOKEN_JOB_MAX_WAIT_MS_V1)
        {
            return Err(TokenJobContractError::WaitTooLong);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TokenJobPollResultV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TokenJobPollResultWireV1::deserialize(deserializer)?;
        Self::new(
            wire.id,
            wire.status,
            wire.pid,
            wire.exit_code,
            wire.tail,
            wire.tail_utf8_lossless,
            wire.tail_bytes,
            wire.log_bytes,
            wire.cursor,
            wire.version,
            wire.changed,
            wire.next_poll_ms,
        )
        .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenJobContractError {
    InvalidId,
    WaitTooLong,
    InvalidTailLimit,
    TailTooLarge,
    InvalidCursor,
    InconsistentTail,
    InconsistentChange,
    InvalidExitCode,
    InconsistentPollState,
}

impl fmt::Display for TokenJobContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "job id must contain 1 to 256 non-control UTF-8 bytes",
            Self::WaitTooLong => "job wait exceeds 30000 milliseconds",
            Self::InvalidTailLimit => "job tailBytes must be between 1 and 65536",
            Self::TailTooLarge => "job result tail exceeds 65536 bytes",
            Self::InvalidCursor => "job cursor must cover tailBytes and not exceed logBytes",
            Self::InconsistentTail => "job tail encoding and emptiness must match tailBytes",
            Self::InconsistentChange => "unchanged job result must be running with an empty tail",
            Self::InvalidExitCode => "running job result cannot have an exit code",
            Self::InconsistentPollState => {
                "nextPollMs must be present exactly while a job is running"
            }
        })
    }
}

impl std::error::Error for TokenJobContractError {}

fn validate_id(id: &str) -> Result<(), TokenJobContractError> {
    if id.is_empty() || id.len() > TOKEN_JOB_MAX_ID_BYTES_V1 || id.chars().any(char::is_control) {
        Err(TokenJobContractError::InvalidId)
    } else {
        Ok(())
    }
}

pub fn token_job_contract_manifest_v1() -> Value {
    json!({
        "version": TOKEN_JOB_ABI_VERSION_V1,
        "operation": TOKEN_JOB_OPERATION_V1,
        "request": {
            "encoding": "json-object/camelCase",
            "required": ["id"],
            "normalizedFields": ["id", "waitMs", "since", "tailBytes"],
            "maxIdBytes": TOKEN_JOB_MAX_ID_BYTES_V1,
            "defaultWaitMs": TOKEN_JOB_DEFAULT_WAIT_MS_V1,
            "maxWaitMs": TOKEN_JOB_MAX_WAIT_MS_V1,
            "defaultTailBytes": TOKEN_JOB_DEFAULT_TAIL_BYTES_V1,
            "maxTailBytes": TOKEN_JOB_MAX_TAIL_BYTES_V1
        },
        "result": {
            "encoding": "json-object/camelCase",
            "required": ["id", "status", "tail", "tailUtf8Lossless", "tailBytes", "logBytes", "cursor", "version", "changed"],
            "statuses": ["running", "exited", "failed"],
            "optional": ["pid", "exitCode", "nextPollMs"],
            "maxIdBytes": TOKEN_JOB_MAX_ID_BYTES_V1,
            "maxTailBytes": TOKEN_JOB_MAX_TAIL_BYTES_V1,
            "maxNextPollMs": TOKEN_JOB_MAX_WAIT_MS_V1,
            "invariants": {
                "cursorAtMostLogBytes": true,
                "tailBytesAtMostCursor": true,
                "tailEmptinessMatchesTailBytes": true,
                "losslessTailMatchesTailBytes": true,
                "emptyTailIsLossless": true,
                "unchangedIsRunningWithEmptyTail": true,
                "runningHasNoExitCode": true,
                "nextPollPresentExactlyWhileRunning": true
            }
        }
    })
}

pub fn token_job_contract_digest_v1() -> String {
    contract_digest_hex(&token_job_contract_manifest_v1())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_defaults_and_canonical_wire_are_stable() {
        let request: TokenJobPollRequestV1 =
            serde_json::from_value(json!({"id":"tzjob-7"})).unwrap();
        assert_eq!(request.wait_ms, TOKEN_JOB_DEFAULT_WAIT_MS_V1);
        assert_eq!(request.since, 0);
        assert_eq!(request.tail_bytes, TOKEN_JOB_DEFAULT_TAIL_BYTES_V1);
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"id":"tzjob-7","waitMs":30000,"since":0,"tailBytes":8192})
        );
    }

    #[test]
    fn request_rejects_unknown_and_out_of_range_fields() {
        for mutant in [
            json!({"id":"tzjob-7","extra":true}),
            json!({"id":""}),
            json!({"id":"tzjob-7","waitMs":30001}),
            json!({"id":"tzjob-7","tailBytes":0}),
            json!({"id":"tzjob-7","tailBytes":65537}),
        ] {
            assert!(serde_json::from_value::<TokenJobPollRequestV1>(mutant).is_err());
        }
    }

    #[test]
    fn result_round_trips_and_revalidates_public_values() {
        let result = TokenJobPollResultV1::new(
            "tzjob-7",
            TokenJobStatusV1::Running,
            Some(42),
            None,
            "ok\n",
            true,
            3,
            3,
            3,
            2,
            true,
            Some(20_000),
        )
        .unwrap();
        let encoded = serde_json::to_value(&result).unwrap();
        assert!(encoded.get("exitCode").is_none());
        assert_eq!(encoded["tailUtf8Lossless"], true);
        assert_eq!(
            serde_json::from_value::<TokenJobPollResultV1>(encoded).unwrap(),
            result
        );

        let exited = TokenJobPollResultV1::new(
            "tzjob-7",
            TokenJobStatusV1::Exited,
            Some(42),
            Some(0),
            "",
            true,
            0,
            3,
            3,
            3,
            true,
            None,
        )
        .unwrap();
        assert_eq!(exited.status, TokenJobStatusV1::Exited);

        let invalid = TokenJobPollResultV1 {
            id: "tzjob-7".into(),
            status: TokenJobStatusV1::Running,
            pid: None,
            exit_code: Some(0),
            tail: String::new(),
            tail_utf8_lossless: true,
            tail_bytes: 0,
            log_bytes: 0,
            cursor: 0,
            version: 0,
            changed: false,
            next_poll_ms: None,
        };
        assert_eq!(
            invalid.validate(),
            Err(TokenJobContractError::InvalidExitCode)
        );
    }

    #[test]
    fn result_rejects_unknown_and_inconsistent_fields() {
        let base = json!({
            "id":"tzjob-7","status":"running","tail":"","tailUtf8Lossless":true,"tailBytes":0,
            "logBytes":0,"cursor":0,"version":0,"changed":false,"nextPollMs":20000
        });
        let mut unknown = base.clone();
        unknown["log"] = json!("/private/session.log");
        assert!(serde_json::from_value::<TokenJobPollResultV1>(unknown).is_err());

        let mut cursor = base.clone();
        cursor["cursor"] = json!(2);
        assert!(serde_json::from_value::<TokenJobPollResultV1>(cursor).is_err());

        let mut changed = base.clone();
        changed["tail"] = json!("hidden");
        assert!(serde_json::from_value::<TokenJobPollResultV1>(changed).is_err());

        let mut byte_mismatch = base;
        byte_mismatch["tail"] = json!("x");
        byte_mismatch["tailBytes"] = json!(2);
        byte_mismatch["cursor"] = json!(2);
        byte_mismatch["logBytes"] = json!(2);
        byte_mismatch["changed"] = json!(true);
        assert!(serde_json::from_value::<TokenJobPollResultV1>(byte_mismatch).is_err());
    }

    #[test]
    fn contract_digest_is_frozen() {
        assert_eq!(token_job_contract_manifest_v1()["operation"], "job");
        assert_eq!(
            token_job_contract_digest_v1(),
            "d9b15de5be5a4c5a2d80ffd409eb04fc796b16b377a67254016fc4f285b7a597"
        );
    }
}
