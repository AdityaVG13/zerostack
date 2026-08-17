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

pub const TOKEN_JOB_ABI_VERSION: &str = "zerostack.token_job.v1";
pub const TOKEN_JOB_OPERATION: &str = "job";
pub const TOKEN_JOB_DEFAULT_WAIT_MS: u64 = 30_000;
pub const TOKEN_JOB_MAX_WAIT_MS: u64 = 30_000;
pub const TOKEN_JOB_DEFAULT_TAIL_BYTES: u64 = 8 * 1024;
pub const TOKEN_JOB_MAX_TAIL_BYTES: u64 = 64 * 1024;
pub const TOKEN_JOB_MAX_ID_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenJobPollRequest {
    pub id: String,
    pub wait_ms: u64,
    pub since: u64,
    pub tail_bytes: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TokenJobPollRequestWire {
    id: String,
    #[serde(default = "default_wait_ms")]
    wait_ms: u64,
    #[serde(default)]
    since: u64,
    #[serde(default = "default_tail_bytes")]
    tail_bytes: u64,
}

const fn default_wait_ms() -> u64 {
    TOKEN_JOB_DEFAULT_WAIT_MS
}

const fn default_tail_bytes() -> u64 {
    TOKEN_JOB_DEFAULT_TAIL_BYTES
}

impl TokenJobPollRequest {
    pub fn new(id: impl Into<String>) -> Result<Self, TokenJobContractError> {
        Self::with_options(
            id,
            TOKEN_JOB_DEFAULT_WAIT_MS,
            0,
            TOKEN_JOB_DEFAULT_TAIL_BYTES,
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
        if self.wait_ms > TOKEN_JOB_MAX_WAIT_MS {
            return Err(TokenJobContractError::WaitTooLong);
        }
        if !(1..=TOKEN_JOB_MAX_TAIL_BYTES).contains(&self.tail_bytes) {
            return Err(TokenJobContractError::InvalidTailLimit);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TokenJobPollRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TokenJobPollRequestWire::deserialize(deserializer)?;
        Self::with_options(wire.id, wire.wait_ms, wire.since, wire.tail_bytes)
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenJobStatus {
    Running,
    Exited,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenJobPollResult {
    pub id: String,
    pub status: TokenJobStatus,
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
struct TokenJobPollResultWire {
    id: String,
    status: TokenJobStatus,
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

impl TokenJobPollResult {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        status: TokenJobStatus,
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
        if self.tail_bytes > TOKEN_JOB_MAX_TAIL_BYTES
            || self.tail.len() as u64 > TOKEN_JOB_MAX_TAIL_BYTES
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
        if !self.changed && (self.status != TokenJobStatus::Running || self.tail_bytes != 0) {
            return Err(TokenJobContractError::InconsistentChange);
        }
        if self.status == TokenJobStatus::Running && self.exit_code.is_some() {
            return Err(TokenJobContractError::InvalidExitCode);
        }
        if (self.status == TokenJobStatus::Running) != self.next_poll_ms.is_some() {
            return Err(TokenJobContractError::InconsistentPollState);
        }
        if self
            .next_poll_ms
            .is_some_and(|value| value > TOKEN_JOB_MAX_WAIT_MS)
        {
            return Err(TokenJobContractError::WaitTooLong);
        }
        Ok(())
    }
}

impl<'de> Deserialize<'de> for TokenJobPollResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TokenJobPollResultWire::deserialize(deserializer)?;
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
    if id.is_empty() || id.len() > TOKEN_JOB_MAX_ID_BYTES || id.chars().any(char::is_control) {
        Err(TokenJobContractError::InvalidId)
    } else {
        Ok(())
    }
}

pub fn token_job_contract_manifest() -> Value {
    json!({
        "version": TOKEN_JOB_ABI_VERSION,
        "operation": TOKEN_JOB_OPERATION,
        "request": {
            "encoding": "json-object/camelCase",
            "required": ["id"],
            "normalizedFields": ["id", "waitMs", "since", "tailBytes"],
            "maxIdBytes": TOKEN_JOB_MAX_ID_BYTES,
            "defaultWaitMs": TOKEN_JOB_DEFAULT_WAIT_MS,
            "maxWaitMs": TOKEN_JOB_MAX_WAIT_MS,
            "defaultTailBytes": TOKEN_JOB_DEFAULT_TAIL_BYTES,
            "maxTailBytes": TOKEN_JOB_MAX_TAIL_BYTES
        },
        "result": {
            "encoding": "json-object/camelCase",
            "required": ["id", "status", "tail", "tailUtf8Lossless", "tailBytes", "logBytes", "cursor", "version", "changed"],
            "statuses": ["running", "exited", "failed"],
            "optional": ["pid", "exitCode", "nextPollMs"],
            "maxIdBytes": TOKEN_JOB_MAX_ID_BYTES,
            "maxTailBytes": TOKEN_JOB_MAX_TAIL_BYTES,
            "maxNextPollMs": TOKEN_JOB_MAX_WAIT_MS,
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

pub fn token_job_contract_digest() -> String {
    contract_digest_hex(&token_job_contract_manifest())
}

