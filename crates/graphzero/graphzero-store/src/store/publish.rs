//! Validates and appends writable edge publisher batches to the WAL.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ContentHash;

use super::delta_log::{DeltaEntry, DeltaLog, entry_type};
use super::expand::{ExpandResolver, apply_fragment};
use super::lock::WriterLock;
use super::manifest::Manifest;
use super::query::encode_edge_with_meta;
use super::refs::{Fragment, GzRef};

pub const PUBLISH_SCHEMA_VERSION: &str = "publish/v1";
pub const MAX_EDGES: usize = 10_000;
pub const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishError {
    pub code: &'static str,
    pub message: String,
    pub field: Option<String>,
}

impl PublishError {
    pub fn to_json(&self) -> String {
        let field = self
            .field
            .as_ref()
            .map(|f| format!(",\"field\":\"{}\"", super::expand::json_escape(f)))
            .unwrap_or_default();
        format!(
            "{{\"error\":\"{}\",\"code\":\"{}\"{field}}}",
            super::expand::json_escape(&self.message),
            self.code
        )
    }
}

#[derive(Debug, Deserialize)]
pub struct Batch {
    schema_version: String,
    publisher: String,
    edges: Vec<Edge>,
}

#[derive(Debug, Deserialize)]
pub struct Edge {
    src: String,
    dst: String,
    kind: String,
    evidence_ref: String,
    confidence: f64,
    #[serde(default)]
    source: Option<String>,
}

/// Map external publish kinds to CSR kind bytes.
pub fn map_publish_kind(kind: &str) -> Option<u8> {
    use super::csr::EdgeKind;
    match kind {
        "calls" => Some(EdgeKind::CALLS),
        "imports" => Some(EdgeKind::IMPORTS),
        "refs" => Some(EdgeKind::REFS),
        "co_changed" | "ci_flake" => Some(EdgeKind::CO_CHANGED),
        "session_followed" => Some(EdgeKind::SESSION_FOLLOWED),
        "runtime_called" => Some(EdgeKind::RUNTIME_CALLED),
        "linter_smell" => Some(EdgeKind::LINTER_SMELL),
        "verification_passed" => Some(EdgeKind::VERIFICATION_PASSED),
        "verification_failed" => Some(EdgeKind::VERIFICATION_FAILED),
        "build_depends" => Some(EdgeKind::BUILD_DEPENDS),
        "schema_depends" => Some(EdgeKind::SCHEMA_DEPENDS),
        "effect_may_touch" => Some(EdgeKind::EFFECT_MAY_TOUCH),
        _ => None,
    }
}

pub fn confidence_to_u8(conf: f64) -> Result<u8, PublishError> {
    if !(0.0..=1.0).contains(&conf) || conf.is_nan() {
        return Err(PublishError {
            code: "E_CONFIDENCE",
            message: format!("confidence out of range: {conf}"),
            field: Some("edges[].confidence".into()),
        });
    }
    Ok(confidence_to_u8_clamped(conf))
}

/// Single clamped quantization core shared with the extractor indexer. Trusted extractor confidence
/// may arrive out of range or NaN: finite values clamp to `[0.0, 1.0]` before `*255` rounding, and
/// NaN maps to `0` through the float-to-`u8` cast.
pub(crate) fn confidence_to_u8_clamped(conf: f64) -> u8 {
    (conf.clamp(0.0, 1.0) * 255.0).round() as u8
}

pub fn publisher_id_valid(id: &str) -> bool {
    if id.len() < 3 || id.len() > 64 {
        return false;
    }
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
}

fn parse_schema_major(schema_version: &str) -> Option<u32> {
    let rest = schema_version.strip_prefix("publish/v")?;
    rest.parse().ok()
}

fn parse_batch_value(raw: &[u8]) -> Result<Value, PublishError> {
    if raw.len() > MAX_BATCH_BYTES {
        return Err(PublishError {
            code: "E_BATCH_LIMIT",
            message: format!("batch exceeds {} bytes", MAX_BATCH_BYTES),
            field: None,
        });
    }
    let v: Value = serde_json::from_slice(raw).map_err(|e| PublishError {
        code: "E_SCHEMA",
        message: e.to_string(),
        field: None,
    })?;
    if let Some(arr) = v.get("edges").and_then(|e| e.as_array())
        && arr.len() > MAX_EDGES
    {
        return Err(PublishError {
            code: "E_BATCH_LIMIT",
            message: format!("batch exceeds {} edges", MAX_EDGES),
            field: Some("edges".into()),
        });
    }
    Ok(v)
}

fn validate_batch_header(batch: &Batch) -> Result<(), PublishError> {
    if parse_schema_major(&batch.schema_version) != Some(1) {
        return Err(PublishError {
            code: "E_SCHEMA_VERSION",
            message: format!("unsupported schema_version {}", batch.schema_version),
            field: Some("schema_version".into()),
        });
    }
    if !publisher_id_valid(&batch.publisher) {
        return Err(PublishError {
            code: "E_SCHEMA",
            message: "invalid publisher id".into(),
            field: Some("publisher".into()),
        });
    }
    if batch.edges.is_empty() {
        return Err(PublishError {
            code: "E_SCHEMA",
            message: "edges must not be empty".into(),
            field: Some("edges".into()),
        });
    }
    Ok(())
}

fn validate_edge(i: usize, e: &Edge, default_publisher: &str) -> Result<(), PublishError> {
    if map_publish_kind(&e.kind).is_none() {
        return Err(PublishError {
            code: "E_SCHEMA",
            message: format!("unknown edge kind {}", e.kind),
            field: Some(format!("edges[{i}].kind")),
        });
    }
    let src = e.source.as_deref().unwrap_or(default_publisher);
    if !publisher_id_valid(src) {
        return Err(PublishError {
            code: "E_SCHEMA",
            message: "invalid edge source".into(),
            field: Some(format!("edges[{i}].source")),
        });
    }
    confidence_to_u8(e.confidence).map_err(|mut err| {
        err.field = Some(format!("edges[{i}].confidence"));
        err
    })?;
    if !e.evidence_ref.starts_with("z://blob/") {
        return Err(PublishError {
            code: "E_SCHEMA",
            message:
                "evidence_ref must be z://blob/<sha>#B<start>-<end>; retired gz:// fails closed"
                    .into(),
            field: Some(format!("edges[{i}].evidence_ref")),
        });
    }
    GzRef::parse(&e.evidence_ref).map_err(|_| PublishError {
        code: "E_SCHEMA",
        message: "evidence_ref must be z://blob/<sha>#B<start>-<end>".into(),
        field: Some(format!("edges[{i}].evidence_ref")),
    })?;
    Ok(())
}

pub fn validate_batch_json(raw: &[u8]) -> Result<Batch, PublishError> {
    let v = parse_batch_value(raw)?;
    let batch: Batch = serde_json::from_value(v).map_err(|e| PublishError {
        code: "E_SCHEMA",
        message: e.to_string(),
        field: None,
    })?;
    validate_batch_header(&batch)?;
    for (i, e) in batch.edges.iter().enumerate() {
        validate_edge(i, e, &batch.publisher)?;
    }
    Ok(batch)
}

fn evidence_error(message: impl Into<String>) -> PublishError {
    PublishError {
        code: "E_EVIDENCE",
        message: message.into(),
        field: None,
    }
}

fn parse_blob_span(gz: &GzRef) -> Result<(&str, u32, u32), PublishError> {
    let GzRef::Blob { hash, fragment } = gz else {
        return Err(evidence_error("evidence_ref must be z://blob/..."));
    };
    let Fragment::Bytes { start, end } = fragment else {
        return Err(evidence_error("evidence_ref requires byte span fragment"));
    };
    let start = u32::try_from(*start)
        .map_err(|_| evidence_error("evidence span start exceeds u32 range"))?;
    let end =
        u32::try_from(*end).map_err(|_| evidence_error("evidence span end exceeds u32 range"))?;
    Ok((hash.as_str(), start, end))
}

fn resolve_evidence_blob_bytes(
    store_root: &Path,
    repo_root: Option<&Path>,
    hash_hex: &str,
    evidence_ref: &str,
) -> Result<Vec<u8>, PublishError> {
    let resolver =
        ExpandResolver::new(store_root, repo_root).map_err(|e| evidence_error(e.to_string()))?;
    resolver
        .resolve_blob(hash_hex, evidence_ref)
        .map(|resolution| resolution.bytes)
        .map_err(|_| evidence_error("evidence_ref does not expand"))
}

fn validate_evidence_span(bytes: &[u8], start: u32, end: u32) -> Result<(), PublishError> {
    if start >= end || end as usize > bytes.len() {
        return Err(evidence_error("evidence span out of range"));
    }
    apply_fragment(
        bytes,
        &Fragment::Bytes {
            start: start as u64,
            end: end as u64,
        },
    )
    .map(|_| ())
    .map_err(|_| evidence_error("evidence span invalid"))
}

fn parse_evidence_hash(hash_hex: &str) -> Result<ContentHash, PublishError> {
    ContentHash::from_hex(hash_hex)
        .ok_or_else(|| evidence_error("invalid blob hash in evidence_ref"))
}

fn evidence_expands(
    store_root: &Path,
    repo_root: Option<&Path>,
    evidence_ref: &str,
) -> Result<(ContentHash, u32, u32), PublishError> {
    let gz = GzRef::parse(evidence_ref).map_err(|_| evidence_error("malformed evidence_ref"))?;
    let (hash_hex, start, end) = parse_blob_span(&gz)?;
    let bytes = resolve_evidence_blob_bytes(store_root, repo_root, hash_hex, evidence_ref)?;
    validate_evidence_span(&bytes, start, end)?;
    let hash = parse_evidence_hash(hash_hex)?;
    Ok((hash, start, end))
}

pub struct PublishOptions<'a> {
    pub store_root: &'a Path,
    pub repo_root: Option<&'a Path>,
    pub capability: Option<&'a str>,
    pub allow_anonymous: bool,
}

fn token_path(store_root: &Path) -> PathBuf {
    store_root.join("publish_tokens")
}

fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    crate::fast_hex(h.finalize().as_slice())
}

pub fn capability_ok(
    store_root: &Path,
    token: Option<&str>,
    allow_anonymous: bool,
) -> Result<(), PublishError> {
    if allow_anonymous {
        return Ok(());
    }
    let Some(tok) = token else {
        return Err(PublishError {
            code: "E_AUTH",
            message: "missing capability token (--capability or GRAPHZERO_PUBLISH_TOKEN)".into(),
            field: None,
        });
    };
    let path = token_path(store_root);
    let Ok(bytes) = fs::read_to_string(&path) else {
        return Err(PublishError {
            code: "E_AUTH",
            message: "no publish_tokens allowlist configured".into(),
            field: None,
        });
    };
    let want = hash_token(tok);
    if bytes.lines().any(|l| l.trim() == want) {
        Ok(())
    } else {
        Err(PublishError {
            code: "E_AUTH",
            message: "capability token not in allowlist".into(),
            field: None,
        })
    }
}

pub fn install_capability_token(store_root: &Path, token: &str) -> Result<()> {
    fs::create_dir_all(store_root)?;
    let path = token_path(store_root);
    let line = hash_token(token);
    let mut existing = fs::read_to_string(&path).unwrap_or_default();
    if !existing.lines().any(|l| l.trim() == line) {
        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }
        existing.push_str(&line);
        existing.push('\n');
        fs::write(&path, existing)?;
    }
    Ok(())
}

struct ValidatedPublishEdge {
    src: String,
    dst: String,
    kind: u8,
    confidence: u8,
    blob: ContentHash,
    start: u32,
    end: u32,
    source: String,
}

fn validate_publish_edges(
    batch: &Batch,
    opts: &PublishOptions<'_>,
) -> Result<Vec<ValidatedPublishEdge>, PublishError> {
    let mut validated = Vec::with_capacity(batch.edges.len());
    for (i, e) in batch.edges.iter().enumerate() {
        let (blob, start, end) = evidence_expands(opts.store_root, opts.repo_root, &e.evidence_ref)
            .map_err(|mut err| {
                if err.field.is_none() {
                    err.field = Some(format!("edges[{i}].evidence_ref"));
                }
                err
            })?;
        validated.push(ValidatedPublishEdge {
            src: e.src.clone(),
            dst: e.dst.clone(),
            kind: map_publish_kind(&e.kind).expect("validated"),
            confidence: confidence_to_u8(e.confidence).expect("validated"),
            blob,
            start,
            end,
            source: e.source.as_deref().unwrap_or(&batch.publisher).to_string(),
        });
    }
    Ok(validated)
}

fn io_publish_error(err: impl std::fmt::Display) -> PublishError {
    PublishError {
        code: "E_IO",
        message: err.to_string(),
        field: None,
    }
}

fn append_published_edge(
    log: &mut DeltaLog,
    edge: &ValidatedPublishEdge,
) -> Result<(), PublishError> {
    log.append(DeltaEntry {
        entry_type: entry_type::EDGE,
        blob_hash: edge.blob.0,
        payload: encode_edge_with_meta(
            &edge.src,
            &edge.dst,
            edge.kind,
            edge.confidence,
            edge.start,
            edge.end,
            Some(&edge.source),
        )
        .map_err(io_publish_error)?,
    })
    .map_err(io_publish_error)?;
    log.append(DeltaEntry {
        entry_type: entry_type::COVERAGE,
        blob_hash: edge.blob.0,
        payload: vec![0b100], // tier C bit for publisher edges
    })
    .map_err(io_publish_error)
}

#[derive(Debug)]
pub struct PublishAck {
    pub edges_accepted: usize,
    pub segment_id: u64,
    pub snapshot_id: u64,
}

pub fn publish_batch(raw: &[u8], opts: &PublishOptions<'_>) -> Result<PublishAck, PublishError> {
    publish_batch_inner(raw, opts, |_, _| Ok(()))
}

fn publish_batch_inner(
    raw: &[u8],
    opts: &PublishOptions<'_>,
    mut after_append: impl FnMut(usize, &ValidatedPublishEdge) -> Result<(), PublishError>,
) -> Result<PublishAck, PublishError> {
    capability_ok(opts.store_root, opts.capability, opts.allow_anonymous)?;
    let batch = validate_batch_json(raw)?;
    let validated = validate_publish_edges(&batch, opts)?;

    let _lock = WriterLock::acquire(opts.store_root).map_err(io_publish_error)?;

    fs::create_dir_all(opts.store_root.join("wal")).map_err(io_publish_error)?;
    let mut log = DeltaLog::open(opts.store_root).map_err(io_publish_error)?;
    for (index, edge) in validated.iter().enumerate() {
        append_published_edge(&mut log, edge)?;
        after_append(index, edge)?;
    }
    log.commit().map_err(io_publish_error)?;

    let segment_id = DeltaLog::segment_ids(log.wal_dir())
        .ok()
        .and_then(|ids| ids.last().copied())
        .unwrap_or(0);

    // Wal segments stay "unfolded" until compaction; Snapshot::open merges them as pending.
    let snapshot_id = Manifest::load(opts.store_root)
        .ok()
        .and_then(|m| m.latest().map(|s| s.snapshot_id))
        .unwrap_or(0);

    Ok(PublishAck {
        edges_accepted: validated.len(),
        segment_id,
        snapshot_id,
    })
}

pub fn wal_edge_count(wal_dir: &Path) -> Result<usize> {
    use super::delta_log::read_all_segments;
    let mut n = 0;
    if wal_dir.is_dir() {
        for (_, entries) in read_all_segments(wal_dir)? {
            n += entries
                .iter()
                .filter(|e| e.entry_type == entry_type::EDGE)
                .count();
        }
    }
    Ok(n)
}

pub fn publish_schema_json() -> &'static str {
    include_str!("../../schemas/publish.schema.json")
}
