//! Per-derived-row provenance (`zerostack.derivation-provenance`).
//!
//! Bead: `graphzero-iubq` (migrate after TokenZero freeze `04b9db5`).
//! Prior attach work: `graphzero-3wbh` / `.1` / `.2` / `.3`.
//!
//! Freeze owner: TokenZero `schemas/derivation-provenance/v1/`.
//! Orthogonal to frozen `zerostack.cas-gc.legacy` (untouched). This module emits
//! the shared contract behind `GRAPHZERO_PROVENANCE` / `ZEROSTACK_PROVENANCE`
//! opt-in:
//!
//! - durable `ProvenanceRecord` explaining WHY a derived row exists
//! - attach on the worktree-overlay edge derivation path
//! - attach on the full-index shard/global CSR edge path
//! - attach on outline/semantic/capsule derivation transforms
//! - query + doctor orphan surface (source blob missing from CAS)
//!
//! Records live under engine-private
//! `<store-root>/graphzero/provenance/<row_id>.json` so v1 GC collectors that
//! only discover `gc/roots|pins|leases` are unaffected.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fast_hex;

use super::path_safety::file_name_to_str;
use super::refs::blob_span_ref;

/// Frozen schema id (`zerostack.derivation-provenance`, TokenZero SHA `04b9db5`).
/// Replaces retired proposal tag `zerostack.cas-gc.vnext-provenance`.
pub const PROVENANCE_SCHEMA_VERSION: &str = "zerostack.derivation-provenance";

/// Record type for one derived-row provenance entry.
pub const RECORD_TYPE_DERIVATION: &str = "derivation-provenance";

/// Producing engine namespace.
pub const PRODUCING_ENGINE: &str = "graphzero";

/// Transform id for worktree-overlay scan edges (first attach path).
pub const TRANSFORM_OVERLAY_EXTRACT_EDGES: &str = "graphzero.overlay.extract_edges.v1";

/// Transform id for full-index edges folded into the snapshot graph (global CSR).
pub const TRANSFORM_INDEXER_SHARD_EDGES: &str = "graphzero.indexer.shard_edges.v1";

/// Transform id for outline/skeleton name spans derived from defs.
pub const TRANSFORM_OUTLINE_EXTRACT_SPANS: &str = "graphzero.outline.extract_spans.v1";

/// Transform id for semantic definition chunks (full block body).
pub const TRANSFORM_SEMANTIC_EXTRACT_CHUNKS: &str = "graphzero.semantic.extract_chunks.v1";

/// Transform id for durable query-capsule spills (`gz://query/<id>`).
pub const TRANSFORM_CAPSULE_BUILD: &str = "graphzero.capsule.build.v1";

/// Derived artifact kinds (beyond `graph_edge`).
pub const DERIVED_KIND_OUTLINE_SPAN: &str = "outline_span";
pub const DERIVED_KIND_SEMANTIC_CHUNK: &str = "semantic_chunk";
pub const DERIVED_KIND_QUERY_CAPSULE: &str = "query_capsule";

/// Opt-in env vars (truthy: 1|on|true|yes, case-insensitive).
pub const PROVENANCE_OPT_IN_ENVS: &[&str] = &["GRAPHZERO_PROVENANCE", "ZEROSTACK_PROVENANCE"];

/// Optional override for the producing commit pin recorded on attach.
pub const ENGINE_COMMIT_ENV: &str = "GRAPHZERO_ENGINE_COMMIT";

/// Byte span inside the source blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteSpan {
    pub start: u32,
    pub end: u32,
}

/// Optional 1-based inclusive line span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineSpan {
    pub start: u32,
    pub end: u32,
}

/// Durable WHY record for one derived graph row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    pub schema_version: String,
    pub record_type: String,
    /// Stable identity for this derivation (64-hex SHA-256).
    pub row_id: String,
    /// Kind of derived artifact (`graph_edge`, …).
    pub derived_kind: String,
    /// Expandable evidence ref for the derived span (`gz://blob/<hash>#B…`).
    pub derived_ref: String,
    /// Source blob digest (lowercase 64-hex SHA-256).
    pub source_blob_digest: String,
    pub byte_span: ByteSpan,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_span: Option<LineSpan>,
    pub producing_engine: String,
    /// Engine commit / package pin that produced the row.
    pub producing_commit: String,
    pub transform_id: String,
    pub created_at: String,
    /// Optional edge endpoints when `derived_kind == "graph_edge"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_src: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_dst: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_kind: Option<u8>,
}

/// Doctor / verify summary of orphaned derivations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrphanedDerivation {
    pub row_id: String,
    pub derived_ref: String,
    pub source_blob_digest: String,
    pub transform_id: String,
    pub reason: String,
}

/// Doctor payload for provenance health.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceDoctorReport {
    pub schema_version: String,
    pub enabled: bool,
    pub record_count: usize,
    pub orphaned_derivations: Vec<OrphanedDerivation>,
}

impl ProvenanceRecord {
    /// Build a provenance record for one graph-edge derivation.
    pub fn for_graph_edge(
        transform_id: &str,
        source_blob_digest: &str,
        start: u32,
        end: u32,
        src: &str,
        dst: &str,
        kind: u8,
        content: Option<&[u8]>,
    ) -> Self {
        let digest = source_blob_digest.to_lowercase();
        let row_id = row_id_for_edge(transform_id, &digest, start, end, src, dst, kind);
        Self {
            schema_version: PROVENANCE_SCHEMA_VERSION.to_string(),
            record_type: RECORD_TYPE_DERIVATION.to_string(),
            row_id,
            derived_kind: "graph_edge".to_string(),
            derived_ref: blob_span_ref(&digest, start, end),
            source_blob_digest: digest,
            byte_span: ByteSpan { start, end },
            line_span: content.and_then(|c| line_span_for_bytes(c, start, end)),
            producing_engine: PRODUCING_ENGINE.to_string(),
            producing_commit: producing_commit(),
            transform_id: transform_id.to_string(),
            created_at: rfc3339_now(),
            edge_src: Some(src.to_string()),
            edge_dst: Some(dst.to_string()),
            edge_kind: Some(kind),
        }
    }

    /// Build a provenance record for one overlay-extracted call edge.
    pub fn for_overlay_edge(
        source_blob_digest: &str,
        start: u32,
        end: u32,
        src: &str,
        dst: &str,
        kind: u8,
        content: Option<&[u8]>,
    ) -> Self {
        Self::for_graph_edge(
            TRANSFORM_OVERLAY_EXTRACT_EDGES,
            source_blob_digest,
            start,
            end,
            src,
            dst,
            kind,
            content,
        )
    }

    /// Build a provenance record for one full-index (shard/global CSR) edge.
    pub fn for_indexer_shard_edge(
        source_blob_digest: &str,
        start: u32,
        end: u32,
        src: &str,
        dst: &str,
        kind: u8,
        content: Option<&[u8]>,
    ) -> Self {
        Self::for_graph_edge(
            TRANSFORM_INDEXER_SHARD_EDGES,
            source_blob_digest,
            start,
            end,
            src,
            dst,
            kind,
            content,
        )
    }

    /// Build a provenance record for one span-backed derivation (outline/semantic).
    pub fn for_span_derivation(
        transform_id: &str,
        derived_kind: &str,
        source_blob_digest: &str,
        start: u32,
        end: u32,
        symbol: Option<&str>,
        content: Option<&[u8]>,
    ) -> Self {
        let digest = source_blob_digest.to_lowercase();
        let row_id = row_id_for_span(transform_id, &digest, start, end, symbol);
        Self {
            schema_version: PROVENANCE_SCHEMA_VERSION.to_string(),
            record_type: RECORD_TYPE_DERIVATION.to_string(),
            row_id,
            derived_kind: derived_kind.to_string(),
            derived_ref: blob_span_ref(&digest, start, end),
            source_blob_digest: digest,
            byte_span: ByteSpan { start, end },
            line_span: content.and_then(|c| line_span_for_bytes(c, start, end)),
            producing_engine: PRODUCING_ENGINE.to_string(),
            producing_commit: producing_commit(),
            transform_id: transform_id.to_string(),
            created_at: rfc3339_now(),
            edge_src: None,
            edge_dst: None,
            edge_kind: None,
        }
    }

    /// Outline/skeleton name-span derivation.
    pub fn for_outline_span(
        source_blob_digest: &str,
        start: u32,
        end: u32,
        symbol: &str,
        content: Option<&[u8]>,
    ) -> Self {
        Self::for_span_derivation(
            TRANSFORM_OUTLINE_EXTRACT_SPANS,
            DERIVED_KIND_OUTLINE_SPAN,
            source_blob_digest,
            start,
            end,
            Some(symbol),
            content,
        )
    }

    /// Semantic definition-chunk derivation (full block body).
    pub fn for_semantic_chunk(
        source_blob_digest: &str,
        start: u32,
        end: u32,
        symbol: &str,
        content: Option<&[u8]>,
    ) -> Self {
        Self::for_span_derivation(
            TRANSFORM_SEMANTIC_EXTRACT_CHUNKS,
            DERIVED_KIND_SEMANTIC_CHUNK,
            source_blob_digest,
            start,
            end,
            Some(symbol),
            content,
        )
    }

    /// Durable query-capsule spill derivation (`gz://query/<id>`).
    pub fn for_query_capsule(
        source_blob_digest: &str,
        capsule_id: &str,
        byte_len: u32,
        content: Option<&[u8]>,
    ) -> Self {
        let digest = source_blob_digest.to_lowercase();
        let start = 0u32;
        let end = byte_len;
        let row_id = row_id_for_span(
            TRANSFORM_CAPSULE_BUILD,
            &digest,
            start,
            end,
            Some(capsule_id),
        );
        Self {
            schema_version: PROVENANCE_SCHEMA_VERSION.to_string(),
            record_type: RECORD_TYPE_DERIVATION.to_string(),
            row_id,
            derived_kind: DERIVED_KIND_QUERY_CAPSULE.to_string(),
            derived_ref: format!("gz://query/{capsule_id}"),
            source_blob_digest: digest,
            byte_span: ByteSpan { start, end },
            line_span: content.and_then(|c| line_span_for_bytes(c, start, end)),
            producing_engine: PRODUCING_ENGINE.to_string(),
            producing_commit: producing_commit(),
            transform_id: TRANSFORM_CAPSULE_BUILD.to_string(),
            created_at: rfc3339_now(),
            edge_src: None,
            edge_dst: None,
            edge_kind: None,
        }
    }
}

/// Whether provenance attach is enabled via opt-in env.
pub fn provenance_enabled() -> bool {
    for key in PROVENANCE_OPT_IN_ENVS {
        if let Ok(v) = std::env::var(key)
            && is_truthy(&v)
        {
            return true;
        }
    }
    false
}

/// Directory holding provenance JSON records for this store.
pub fn provenance_dir(store_root: &Path) -> PathBuf {
    store_root.join("graphzero").join("provenance")
}

/// Path for one provenance record.
pub fn provenance_record_path(store_root: &Path, row_id: &str) -> PathBuf {
    provenance_dir(store_root).join(format!("{row_id}.json"))
}

/// Deterministic row id for an edge derivation.
pub fn row_id_for_edge(
    transform_id: &str,
    source_blob: &str,
    start: u32,
    end: u32,
    src: &str,
    dst: &str,
    kind: u8,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"graphzero.provenance.edge.v1\0");
    hasher.update(transform_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(source_blob.as_bytes());
    hasher.update(b"\0");
    hasher.update(start.to_le_bytes());
    hasher.update(end.to_le_bytes());
    hasher.update(b"\0");
    hasher.update(src.as_bytes());
    hasher.update(b"\0");
    hasher.update(dst.as_bytes());
    hasher.update(b"\0");
    hasher.update([kind]);
    fast_hex(&hasher.finalize())
}

/// Deterministic row id for a span-backed derivation (outline/semantic/capsule).
pub fn row_id_for_span(
    transform_id: &str,
    source_blob: &str,
    start: u32,
    end: u32,
    symbol: Option<&str>,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"graphzero.provenance.span.v1\0");
    hasher.update(transform_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(source_blob.as_bytes());
    hasher.update(b"\0");
    hasher.update(start.to_le_bytes());
    hasher.update(end.to_le_bytes());
    hasher.update(b"\0");
    hasher.update(symbol.unwrap_or("").as_bytes());
    fast_hex(&hasher.finalize())
}

/// Persist a provenance record atomically (same-filesystem rename).
pub fn write_provenance_record(store_root: &Path, record: &ProvenanceRecord) -> Result<PathBuf> {
    let dir = provenance_dir(store_root);
    fs::create_dir_all(&dir).with_context(|| format!("create provenance dir {}", dir.display()))?;
    let text = serde_json::to_string_pretty(record).context("serialize provenance record")?;
    let dest = provenance_record_path(store_root, &record.row_id);
    let tmp = dir.join(format!(".{}.tmp", process_nonce()));
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)
            .with_context(|| format!("create temp provenance {}", tmp.display()))?;
        f.write_all(text.as_bytes())
            .with_context(|| format!("write temp provenance {}", tmp.display()))?;
        f.sync_data()
            .with_context(|| format!("sync temp provenance {}", tmp.display()))?;
    }
    fs::rename(&tmp, &dest)
        .with_context(|| format!("publish provenance {} -> {}", tmp.display(), dest.display()))?;
    Ok(dest)
}

/// Attach provenance for one overlay edge when opt-in is enabled.
///
/// No-op (returns `Ok(None)`) when provenance is disabled.
pub fn attach_overlay_edge_provenance(
    store_root: &Path,
    source_blob_digest: &str,
    start: u32,
    end: u32,
    src: &str,
    dst: &str,
    kind: u8,
    content: Option<&[u8]>,
) -> Result<Option<ProvenanceRecord>> {
    attach_graph_edge_provenance(
        store_root,
        TRANSFORM_OVERLAY_EXTRACT_EDGES,
        source_blob_digest,
        start,
        end,
        src,
        dst,
        kind,
        content,
    )
}

/// Attach provenance for one full-index shard/global CSR edge when opt-in is enabled.
///
/// No-op (returns `Ok(None)`) when provenance is disabled.
pub fn attach_indexer_shard_edge_provenance(
    store_root: &Path,
    source_blob_digest: &str,
    start: u32,
    end: u32,
    src: &str,
    dst: &str,
    kind: u8,
    content: Option<&[u8]>,
) -> Result<Option<ProvenanceRecord>> {
    attach_graph_edge_provenance(
        store_root,
        TRANSFORM_INDEXER_SHARD_EDGES,
        source_blob_digest,
        start,
        end,
        src,
        dst,
        kind,
        content,
    )
}

/// Attach outline + semantic provenance for one def/span when opt-in is enabled.
///
/// Outline uses the identifier/name span; semantic uses the full definition block
/// (falls back to the name span when block bounds are absent).
pub fn attach_def_span_provenance(
    store_root: &Path,
    source_blob_digest: &str,
    name_start: u32,
    name_end: u32,
    block_start: u32,
    block_end: u32,
    symbol: &str,
    content: Option<&[u8]>,
) -> Result<(Option<ProvenanceRecord>, Option<ProvenanceRecord>)> {
    if !provenance_enabled() {
        return Ok((None, None));
    }
    let outline = attach_outline_span_provenance(
        store_root,
        source_blob_digest,
        name_start,
        name_end,
        symbol,
        content,
    )?;
    let (chunk_start, chunk_end) = if block_end > block_start {
        (block_start, block_end)
    } else {
        (name_start, name_end)
    };
    let semantic = attach_semantic_chunk_provenance(
        store_root,
        source_blob_digest,
        chunk_start,
        chunk_end,
        symbol,
        content,
    )?;
    Ok((outline, semantic))
}

/// Attach provenance for one outline name span when opt-in is enabled.
pub fn attach_outline_span_provenance(
    store_root: &Path,
    source_blob_digest: &str,
    start: u32,
    end: u32,
    symbol: &str,
    content: Option<&[u8]>,
) -> Result<Option<ProvenanceRecord>> {
    if !provenance_enabled() {
        return Ok(None);
    }
    let record =
        ProvenanceRecord::for_outline_span(source_blob_digest, start, end, symbol, content);
    write_provenance_record(store_root, &record)?;
    Ok(Some(record))
}

/// Attach provenance for one semantic definition chunk when opt-in is enabled.
pub fn attach_semantic_chunk_provenance(
    store_root: &Path,
    source_blob_digest: &str,
    start: u32,
    end: u32,
    symbol: &str,
    content: Option<&[u8]>,
) -> Result<Option<ProvenanceRecord>> {
    if !provenance_enabled() {
        return Ok(None);
    }
    let record =
        ProvenanceRecord::for_semantic_chunk(source_blob_digest, start, end, symbol, content);
    write_provenance_record(store_root, &record)?;
    Ok(Some(record))
}

/// Attach provenance for one durable query-capsule spill when opt-in is enabled.
pub fn attach_capsule_build_provenance(
    store_root: &Path,
    source_blob_digest: &str,
    capsule_id: &str,
    byte_len: u32,
    content: Option<&[u8]>,
) -> Result<Option<ProvenanceRecord>> {
    if !provenance_enabled() {
        return Ok(None);
    }
    let record =
        ProvenanceRecord::for_query_capsule(source_blob_digest, capsule_id, byte_len, content);
    write_provenance_record(store_root, &record)?;
    Ok(Some(record))
}

fn attach_graph_edge_provenance(
    store_root: &Path,
    transform_id: &str,
    source_blob_digest: &str,
    start: u32,
    end: u32,
    src: &str,
    dst: &str,
    kind: u8,
    content: Option<&[u8]>,
) -> Result<Option<ProvenanceRecord>> {
    if !provenance_enabled() {
        return Ok(None);
    }
    let record = ProvenanceRecord::for_graph_edge(
        transform_id,
        source_blob_digest,
        start,
        end,
        src,
        dst,
        kind,
        content,
    );
    write_provenance_record(store_root, &record)?;
    Ok(Some(record))
}

/// Read one provenance record by row id.
pub fn read_provenance_record(store_root: &Path, row_id: &str) -> Result<Option<ProvenanceRecord>> {
    let path = provenance_record_path(store_root, row_id);
    if !path.exists() {
        return Ok(None);
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("read provenance {}", path.display()))?;
    let record: ProvenanceRecord = serde_json::from_str(&text)
        .with_context(|| format!("parse provenance {}", path.display()))?;
    Ok(Some(record))
}

/// Look up provenance by derived evidence ref (`gz://blob/…#B…`).
pub fn lookup_by_derived_ref(
    store_root: &Path,
    derived_ref: &str,
) -> Result<Option<ProvenanceRecord>> {
    for record in list_provenance_records(store_root)? {
        if record.derived_ref == derived_ref {
            return Ok(Some(record));
        }
    }
    Ok(None)
}

/// List every durable provenance record under the store.
pub fn list_provenance_records(store_root: &Path) -> Result<Vec<ProvenanceRecord>> {
    let dir = provenance_dir(store_root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)
        .with_context(|| format!("read provenance dir {}", dir.display()))?
        .flatten()
    {
        let file_name = entry.file_name();
        let name = match file_name_to_str(&file_name, "provenance file") {
            Ok(n) => n,
            Err(_) => continue,
        };
        if !name.ends_with(".json") || name.starts_with('.') {
            continue;
        }
        let text = match fs::read_to_string(entry.path()) {
            Ok(t) => t,
            Err(_) => continue,
        };
        match serde_json::from_str::<ProvenanceRecord>(&text) {
            Ok(r) => out.push(r),
            Err(_) => continue,
        }
    }
    out.sort_by(|a, b| a.row_id.cmp(&b.row_id));
    Ok(out)
}

/// Find derivations whose source blob is no longer present in the store.
pub fn find_orphaned_derivations(store_root: &Path) -> Result<Vec<OrphanedDerivation>> {
    let records = list_provenance_records(store_root)?;
    let mut orphans = Vec::new();
    for record in records {
        if source_blob_present(store_root, &record.source_blob_digest) {
            continue;
        }
        orphans.push(OrphanedDerivation {
            row_id: record.row_id,
            derived_ref: record.derived_ref,
            source_blob_digest: record.source_blob_digest,
            transform_id: record.transform_id,
            reason: "source_blob_missing".to_string(),
        });
    }
    Ok(orphans)
}

/// Doctor report: enablement, counts, and orphaned derivations.
pub fn provenance_doctor_report(store_root: &Path) -> Result<ProvenanceDoctorReport> {
    let records = list_provenance_records(store_root)?;
    let orphaned = find_orphaned_derivations(store_root)?;
    Ok(ProvenanceDoctorReport {
        schema_version: PROVENANCE_SCHEMA_VERSION.to_string(),
        enabled: provenance_enabled(),
        record_count: records.len(),
        orphaned_derivations: orphaned,
    })
}

/// Resolve WHY for a verify evidence ref (best-effort; never fails the claim).
pub fn why_for_evidence_ref(store_root: &Path, evidence_ref: &str) -> Option<ProvenanceRecord> {
    lookup_by_derived_ref(store_root, evidence_ref)
        .ok()
        .flatten()
}

fn source_blob_present(store_root: &Path, digest: &str) -> bool {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    let lower = digest.to_lowercase();
    let cas = store_root
        .join("blobs")
        .join("sha256")
        .join(&lower[..2])
        .join(&lower);
    if cas.is_file() {
        return true;
    }
    let legacy = store_root.join("blobs").join(&lower);
    legacy.is_file()
}

fn producing_commit() -> String {
    if let Ok(v) = std::env::var(ENGINE_COMMIT_ENV)
        && !v.trim().is_empty()
    {
        return v.trim().to_string();
    }
    format!("graphzero-store@{}", env!("CARGO_PKG_VERSION"))
}

fn is_truthy(v: &str) -> bool {
    matches!(
        v.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes"
    )
}

fn line_span_for_bytes(content: &[u8], start: u32, end: u32) -> Option<LineSpan> {
    if start > end {
        return None;
    }
    let start = start as usize;
    let end = end as usize;
    if end > content.len() {
        return None;
    }
    let mut line: u32 = 1;
    let mut start_line = None;
    let mut end_line = None;
    for (i, b) in content.iter().enumerate() {
        if i == start {
            start_line = Some(line);
        }
        if i == end.saturating_sub(1).max(start) {
            end_line = Some(line);
        }
        if *b == b'\n' {
            line = line.saturating_add(1);
        }
    }
    if end == content.len() && end_line.is_none() {
        end_line = Some(line);
    }
    Some(LineSpan {
        start: start_line?,
        end: end_line.or(start_line)?,
    })
}

fn rfc3339_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let nanos = now.subsec_nanos();
    let days = secs / 86_400;
    let (y, m, d) = days_to_ymd(days as i64);
    let rem = secs % 86_400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{nanos:09}Z")
}

fn days_to_ymd(mut days: i64) -> (i64, u8, u8) {
    days += 719_468;
    let era = if days >= 0 {
        days / 146_097
    } else {
        (days - 146_096) / 146_097
    };
    let doe = days - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + if m <= 2 { 1 } else { 0 }, m as u8, d as u8)
}

fn process_nonce() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let pid = std::process::id() as u64;
    (pid << 32) | seq
}
