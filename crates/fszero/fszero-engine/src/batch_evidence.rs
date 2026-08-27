//! Compact evidence carried by every batch row and by the batch envelope
//! (bead fszero-5kgu).
//!
//! A result is independently verifiable only if it says what it was computed
//! against. Every row and the batch envelope therefore carry one fixed-shape
//! object under the short key `ev`:
//!
//! * `sh` -- snapshot digest of the physical pass the row was computed against
//!   (digest over the `input -> content digest` map the pass actually read)
//! * `op` / `ov` -- operator id and locked operator version
//! * `ah` -- normalized-args digest of this row's own parameters
//! * `sp` -- `[start, end]` byte span the payload covers in its source image
//! * `tr` -- truncation state
//! * `cs` -- cache status: `hit` | `miss` | `cold`
//! * `us` -- measured execution microseconds
//!
//! The key set is closed and every value is an identifier, number, or boolean:
//! no prose, so envelope size cannot drift with messages. Digests come from the
//! shared `zero_abi` canonical-JSON + SHA-256 helpers that `candidate_store`
//! hashes cache-entry keys with, and are carried as the leading
//! [`EVIDENCE_DIGEST_HEX`] hex chars; a payload's full content digest stays
//! reachable through its `ref` / `source_ref`.
//!
//! Row `us` is row-local materialization time. Shared batch work (one capture,
//! one traversal, one parse per file) is attributed to the envelope, never
//! multiplied across rows.

use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::time::Instant;

/// Row/envelope evidence key.
pub const EVIDENCE_KEY: &str = "ev";
/// Hex chars of each digest carried inline.
pub const EVIDENCE_DIGEST_HEX: usize = 16;

pub const MULTI_READ_OPERATOR: (&str, &str) = ("fszero.multi_read", "1");
pub const MULTI_STAT_OPERATOR: (&str, &str) = ("fszero.multi_stat", "1");
pub const MULTI_LIST_OPERATOR: (&str, &str) = ("fszero.multi_list", "1");
pub const MULTI_SEARCH_OPERATOR: (&str, &str) = ("fszero.multi_search", "1");
pub const MULTI_AST_SEARCH_OPERATOR: (&str, &str) = ("fszero.multi_ast_search", "1");

fn short(mut hex: String) -> String {
    hex.truncate(EVIDENCE_DIGEST_HEX);
    hex
}

/// Content digest of observed bytes, as carried inline.
pub fn short_digest(bytes: &[u8]) -> String {
    short(zero_abi::sha256_hex(bytes))
}

/// Normalized-args identity: a cache-entry-aligned key over the operator
/// identity plus the canonical parameter form of one request.
pub fn args_digest(operator: (&str, &str), params: &Value) -> String {
    short(zero_abi::contract_digest_hex(&json!({
        "operator": { "id": operator.0, "version": operator.1 },
        "canonical_parameters": params,
    })))
}

/// Snapshot identity of one physical pass: a digest over the map of inputs the
/// pass actually read to their content digests. Inputs skipped by a prefilter
/// are absent -- their bytes were never an input to the result.
pub fn snapshot_digest(inputs: &BTreeMap<String, String>) -> String {
    short(zero_abi::contract_digest_hex(&json!({ "inputs": inputs })))
}

/// Normalize a kernel-local cache label onto the reportable states. An input
/// served from a populated cache is a hit; any label that means the kernel did
/// physical work for this row ("miss", "cold" trie, "uncached") is a miss.
/// `cold` is reserved for a pass that consulted no cached input at all.
pub fn cache_status_of(label: &str) -> &'static str {
    match label {
        "hit" | "warm" | "cached_metadata_match" => "hit",
        _ => "miss",
    }
}

/// Cache status of a whole pass: any physical read makes it a miss, otherwise a
/// pass served entirely from cache is a hit, and a pass that observed nothing
/// is cold -- there is no cached evidence to report either way.
pub fn pass_cache_status(cached_inputs: usize, read_inputs: usize) -> &'static str {
    if read_inputs > 0 {
        "miss"
    } else if cached_inputs > 0 {
        "hit"
    } else {
        "cold"
    }
}

/// Pass-level evidence shared by every row of one batch: what the rows were
/// computed against, and which operator computed them.
pub struct PassEvidence {
    pub operator: (&'static str, &'static str),
    pub snapshot: String,
    /// Fallback cache status for rows with no per-row cache identity.
    pub cache_status: &'static str,
}

impl PassEvidence {
    pub fn new(operator: (&'static str, &'static str)) -> Self {
        Self {
            operator,
            snapshot: String::new(),
            cache_status: "cold",
        }
    }

    /// Evidence for a produced row.
    pub fn attach(
        &self,
        fields: &mut Map<String, Value>,
        params: &Value,
        started: Instant,
        span: Value,
        truncated: bool,
        cache_status: &'static str,
    ) {
        self.insert(fields, params, started, span, truncated, cache_status);
    }

    /// Evidence for a row that failed: the pass identity and timing are still
    /// real, and an empty span with no truncation is the honest description of
    /// a row that produced no bytes.
    pub fn attach_error(&self, fields: &mut Map<String, Value>, params: &Value, started: Instant) {
        self.insert(
            fields,
            params,
            started,
            json!([0, 0]),
            false,
            self.cache_status,
        );
    }

    fn insert(
        &self,
        fields: &mut Map<String, Value>,
        params: &Value,
        started: Instant,
        span: Value,
        truncated: bool,
        cache_status: &'static str,
    ) {
        fields.insert(
            EVIDENCE_KEY.into(),
            json!({
                "sh": self.snapshot,
                "op": self.operator.0,
                "ov": self.operator.1,
                "ah": args_digest(self.operator, params),
                "sp": span,
                "tr": truncated,
                "cs": cache_status,
                "us": started.elapsed().as_micros() as u64,
            }),
        );
    }
}

/// Operator identity for a batch operation id.
pub fn operator_for(op_id: &str) -> (&'static str, &'static str) {
    match op_id {
        "fs.multiRead" => MULTI_READ_OPERATOR,
        "fs.multiStat" => MULTI_STAT_OPERATOR,
        "fs.multiList" => MULTI_LIST_OPERATOR,
        "fs.multiSearch" => MULTI_SEARCH_OPERATOR,
        "fs.multiAstSearch" => MULTI_AST_SEARCH_OPERATOR,
        _ => ("fszero.batch", "1"),
    }
}

/// Batch-envelope evidence: the pass identity the rows share, the batch's own
/// normalized args, and the folded row states. `us` is measured kernel time,
/// so shared work is reported once here instead of per row.
pub fn batch_evidence(
    op_id: &str,
    args: &Value,
    rows: &[Value],
    payload_len: usize,
    execution_us: u64,
) -> Value {
    let operator = operator_for(op_id);
    let field = |row: &Value, key: &str| row.pointer(&format!("/{EVIDENCE_KEY}/{key}")).cloned();
    let snapshot = rows
        .iter()
        .find_map(|row| field(row, "sh"))
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let truncated = rows
        .iter()
        .any(|row| field(row, "tr").and_then(|v| v.as_bool()) == Some(true));
    let mut saw_hit = false;
    let mut cache_status = "cold";
    for row in rows {
        match field(row, "cs").as_ref().and_then(Value::as_str) {
            Some("miss") => {
                cache_status = "miss";
                break;
            }
            Some("hit") => saw_hit = true,
            _ => {}
        }
    }
    if cache_status != "miss" && saw_hit {
        cache_status = "hit";
    }
    json!({
        "sh": snapshot,
        "op": operator.0,
        "ov": operator.1,
        "ah": args_digest(operator, args),
        "sp": [0, payload_len],
        "tr": truncated,
        "cs": cache_status,
        "us": execution_us,
    })
}
