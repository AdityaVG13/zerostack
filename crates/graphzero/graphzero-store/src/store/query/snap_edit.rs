//! Warm, in-memory intent/symbol to edit-anchor resolution. The index is built
//! once per Snapshot. Resolution performs no repository traversal or filesystem
//! I/O; every path, line and byte span is materialized during index construction.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::{Result, bail};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::BlobStore;
use crate::store::format::symbol_kind;
use crate::store::refs::blob_span_ref;
use crate::store::symbol_table::SymbolTable;

use super::snapshot::Snapshot;
use super::spans::span_range;

const ALTERNATE_LIMIT: usize = 5;
const AMBIGUOUS_CONFIDENCE: f64 = 0.79;

/// Byte range compatible with FSZero file anchors: half-open [start, end).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditByteSpan {
    pub start: u32,
    pub end: u32,
}

/// An edit-ready source definition. Path is repository-relative and line is one-based.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EditAnchor {
    pub path: String,
    pub line: u32,
    pub byte_span: EditByteSpan,
    pub definition_kind: String,
    pub enclosing_block_span: EditByteSpan,
    pub confidence: f64,
    pub symbol: String,
    pub evidence_ref: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SnapEditResult {
    pub query: String,
    pub best: EditAnchor,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternates: Vec<EditAnchor>,
}

#[derive(Clone, Debug)]
struct IndexedAnchor {
    anchor: EditAnchor,
    name_tokens: BTreeSet<String>,
    all_tokens: BTreeSet<String>,
    search_trigrams: BTreeSet<[u8; 3]>,
}

/// Wire form of [`IndexedAnchor`] for the published sidecar (graphzero perf).
/// Loading skips the blob reads, hashing and line-index builds entirely; the
/// lookup maps are rebuilt from the deserialized entries via `from_entries`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct SerializedAnchor {
    anchor: EditAnchor,
    name_tokens: BTreeSet<String>,
    all_tokens: BTreeSet<String>,
    search_trigrams: BTreeSet<[u8; 3]>,
}

impl From<&IndexedAnchor> for SerializedAnchor {
    fn from(e: &IndexedAnchor) -> Self {
        Self {
            anchor: e.anchor.clone(),
            name_tokens: e.name_tokens.clone(),
            all_tokens: e.all_tokens.clone(),
            search_trigrams: e.search_trigrams.clone(),
        }
    }
}

impl From<SerializedAnchor> for IndexedAnchor {
    fn from(s: SerializedAnchor) -> Self {
        Self {
            anchor: s.anchor,
            name_tokens: s.name_tokens,
            all_tokens: s.all_tokens,
            search_trigrams: s.search_trigrams,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SnapEditSidecar {
    schema: u32,
    entries: Vec<SerializedAnchor>,
}

const SNAP_EDIT_SIDECAR_SCHEMA: u32 = 1;

fn snap_edit_sidecar_name(snapshot_id: u64) -> String {
    format!("snap_edit_{snapshot_id:08}.bin")
}

/// Snapshot-owned immutable index. Resolution is allocation-bounded and I/O-free.
#[derive(Clone, Debug, Default)]
pub struct SnapEditIndex {
    entries: Vec<IndexedAnchor>,
    exact: HashMap<String, Vec<usize>, FxBuildHasher>,
    qualified: HashMap<String, usize, FxBuildHasher>,
    /// Token inverted index for the fuzzy fallback. Built lazily on first
    /// fuzzy miss (graphzero perf): exact/qualified resolutions — the common
    /// snap case — never pay its ~200k-insert construction.
    token_to_entries: std::sync::OnceLock<HashMap<String, Vec<usize>, FxBuildHasher>>,
}

impl SnapEditIndex {
    pub fn build(snapshot: &Snapshot) -> Result<Self> {
        let build_t0 = std::time::Instant::now();
        let profile = std::env::var_os("GRAPHZERO_PERF_PROFILE").is_some();
        let view = snapshot.global_view()?;
        let table = SymbolTable::from_view(&view)?;
        let spans = view.spans()?;
        let blob_hashes = view.coverage()?.blob_hashes;
        let blob_store = BlobStore::open(&snapshot.store_root)?;
        if profile {
            eprintln!(
                "{{\"stage\":\"snap_edit.view_table_ms\",\"stage_ms\":{:.6}}}",
                build_t0.elapsed().as_secs_f64() * 1e3
            );
        }

        // Pre-stage every distinct blob in parallel (graphzero perf): the per-symbol loop below is
        // otherwise serialized on ~928 read+SHA-256 verifications (~350ms).
        let distinct_hashes: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            let mut out = Vec::new();
            for symbol_id in 0..table.len() as u32 {
                for span in span_range(&spans, symbol_id) {
                    let hash_hex = match crate::hex_blob_hash(blob_hashes, span.blob_idx) {
                        Ok(hex) => hex,
                        Err(_) => continue,
                    };
                    if seen.insert(hash_hex.clone()) {
                        out.push(hash_hex);
                    }
                }
            }
            out
        };
        let repo_root = snapshot.repo_root.clone();
        let stage_t = std::time::Instant::now();
        let loaded: HashMap<String, Vec<u8>> = distinct_hashes
            .par_iter()
            .map(|hash_hex| -> (String, Vec<u8>) {
                let path = snapshot.path_for_blob(hash_hex).map(|r| r.path.clone());
                let bytes = blob_store
                    .get_hex(hash_hex)
                    .ok()
                    .flatten()
                    .filter(|bytes| !bytes.is_empty())
                    .or_else(|| {
                        repo_root
                            .as_ref()
                            .and_then(|root| std::fs::read(root.join(path.as_deref()?)).ok())
                    })
                    .unwrap_or_default();
                (hash_hex.clone(), bytes)
            })
            .collect();
        let blob_cache = loaded;

        // Per-blob newline prefix indices, built in parallel alongside entry
        let line_indices: Vec<Vec<u32>> = distinct_hashes
            .par_iter()
            .map(|hash_hex| {
                blob_cache
                    .get(hash_hex)
                    .map(|bytes| build_line_index(bytes))
                    .unwrap_or_default()
            })
            .collect();
        let line_index_by_hash: HashMap<&str, &Vec<u32>> = distinct_hashes
            .iter()
            .zip(line_indices.iter())
            .map(|(hex, idx)| (hex.as_str(), idx))
            .collect();
        if profile {
            eprintln!(
                "{{\"stage\":\"snap_edit.prestage_ms\",\"stage_ms\":{:.6}}}",
                stage_t.elapsed().as_secs_f64() * 1e3
            );
        }

        let empty_line_index = Vec::new();

        // Entry construction in parallel over symbols (graphzero perf): each
        // symbol's anchors are independent; the deterministic order required by
        // `from_entries` is restored by the full sort below.
        let entries_t = std::time::Instant::now();
        let mut entries: Vec<IndexedAnchor> = (0..table.len() as u32)
            .into_par_iter()
            .filter_map(|symbol_id| {
                let symbol = table.name(symbol_id)?;
                let symbol_entry = table.entry(symbol_id)?;
                let mut out = Vec::new();
                for span in span_range(&spans, symbol_id) {
                    let hash_hex = crate::hex_blob_hash(blob_hashes, span.blob_idx).ok()?;
                    let Some(path) = snapshot.path_for_blob(&hash_hex).map(|r| r.path.clone())
                    else {
                        continue;
                    };
                    let line_starts = line_index_by_hash
                        .get(hash_hex.as_str())
                        .copied()
                        .unwrap_or(&empty_line_index);
                    let (name_start, name_end) = span.name_byte_range();
                    let (block_start, block_end) = span.outline_byte_range();
                    let anchor = EditAnchor {
                        path: path.clone(),
                        line: line_at_offset_indexed(line_starts, name_start),
                        byte_span: EditByteSpan {
                            start: name_start,
                            end: name_end,
                        },
                        definition_kind: kind_label(symbol_entry.kind).to_string(),
                        enclosing_block_span: EditByteSpan {
                            start: block_start,
                            end: block_end,
                        },
                        confidence: 0.0,
                        symbol: symbol.to_string(),
                        evidence_ref: blob_span_ref(&hash_hex, name_start, name_end),
                    };
                    let search_text = format!("{path} {symbol}");
                    out.push(IndexedAnchor {
                        name_tokens: token_set(symbol),
                        all_tokens: token_set(&search_text),
                        search_trigrams: trigrams(&search_text),
                        anchor,
                    });
                }
                Some(out)
            })
            .flatten()
            .collect();
        if profile {
            eprintln!(
                "{{\"stage\":\"snap_edit.entries_ms\",\"stage_ms\":{:.6}}}",
                entries_t.elapsed().as_secs_f64() * 1e3
            );
        }
        entries.par_sort_by(|a, b| {
            a.anchor
                .symbol
                .cmp(&b.anchor.symbol)
                .then_with(|| a.anchor.path.cmp(&b.anchor.path))
                .then_with(|| a.anchor.byte_span.start.cmp(&b.anchor.byte_span.start))
        });
        if profile {
            eprintln!(
                "{{\"stage\":\"snap_edit.sort_ms\",\"stage_ms\":{:.6}}}",
                entries_t.elapsed().as_secs_f64() * 1e3
            );
        }
        let fe_t = std::time::Instant::now();
        let out = Self::from_entries(entries);
        if profile {
            eprintln!(
                "{{\"stage\":\"snap_edit.from_entries_ms\",\"stage_ms\":{:.6}}}",
                fe_t.elapsed().as_secs_f64() * 1e3
            );
            eprintln!(
                "{{\"stage\":\"snap_edit.total_ms\",\"stage_ms\":{:.6}}}",
                build_t0.elapsed().as_secs_f64() * 1e3
            );
        }
        Ok(out)
    }

    /// Write the built index next to the shards so later opens skip the build
    /// (graphzero perf; mirrors LexicalSemanticIndex). Best-effort: a failed
    /// write only costs the rebuild next open.
    pub fn write_published(shards_dir: &Path, snapshot_id: u64, index: &Self) -> Result<()> {
        let sidecar = SnapEditSidecar {
            schema: SNAP_EDIT_SIDECAR_SCHEMA,
            entries: index.entries.iter().map(SerializedAnchor::from).collect(),
        };
        let bytes = bincode_ser(&sidecar)?;
        let path = shards_dir.join(snap_edit_sidecar_name(snapshot_id));
        let tmp = shards_dir.join(format!("{}.tmp", snap_edit_sidecar_name(snapshot_id)));
        std::fs::write(&tmp, &bytes)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| anyhow::anyhow!("snap_edit write {}: {e}", path.display()))
    }

    /// Load a published sidecar if present. `Ok(None)` when missing or from an
    /// incompatible schema — caller falls back to `build`.
    pub fn try_load_published(shards_dir: &Path, snapshot_id: u64) -> Result<Option<Self>> {
        let path = shards_dir.join(snap_edit_sidecar_name(snapshot_id));
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        let Ok(sidecar) = bincode_de(&bytes) else {
            return Ok(None);
        };
        if sidecar.schema != SNAP_EDIT_SIDECAR_SCHEMA {
            return Ok(None);
        }
        let mut entries: Vec<IndexedAnchor> = sidecar
            .entries
            .into_iter()
            .map(IndexedAnchor::from)
            .collect();
        // Sidecars are written post-sort, but don't trust file ordering.
        entries.sort_by(|a, b| {
            a.anchor
                .symbol
                .cmp(&b.anchor.symbol)
                .then_with(|| a.anchor.path.cmp(&b.anchor.path))
                .then_with(|| a.anchor.byte_span.start.cmp(&b.anchor.byte_span.start))
        });
        Ok(Some(Self::from_entries(entries)))
    }

    fn from_entries(entries: Vec<IndexedAnchor>) -> Self {
        // Pre-sized + FxHash-style hasher (graphzero perf): ~100k short-string inserts per build;
        // SipHash's cost dominates these internal maps. Iteration order is never output (resolution
        // sorts), so the non-cryptographic, seed-free hasher is safe here.
        let mut exact: HashMap<String, Vec<usize>, FxBuildHasher> =
            HashMap::with_capacity_and_hasher(entries.len(), FxBuildHasher::default());
        let mut qualified: HashMap<String, usize, FxBuildHasher> =
            HashMap::with_capacity_and_hasher(entries.len() * 2, FxBuildHasher::default());
        for (index, entry) in entries.iter().enumerate() {
            let symbol = entry.anchor.symbol.to_ascii_lowercase();
            let path = entry.anchor.path.to_ascii_lowercase();
            exact.entry(symbol.clone()).or_default().push(index);
            qualified.insert(format!("{path}::{symbol}"), index);
            qualified.insert(format!("{path}/{symbol}"), index);
        }
        Self {
            entries,
            exact,
            qualified,
            token_to_entries: std::sync::OnceLock::new(),
        }
    }

    /// Token inverted index over `entries`, built on first fuzzy-miss use.
    fn token_map(&self) -> &HashMap<String, Vec<usize>, FxBuildHasher> {
        self.token_to_entries.get_or_init(|| {
            let mut map: HashMap<String, Vec<usize>, FxBuildHasher> =
                HashMap::with_capacity_and_hasher(self.entries.len() * 4, FxBuildHasher::default());
            for (index, entry) in self.entries.iter().enumerate() {
                for token in &entry.all_tokens {
                    map.entry(token.clone()).or_default().push(index);
                }
            }
            map
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Resolve an exact name, qualified path/name, or short natural-language intent.
    pub fn resolve(&self, query: &str) -> Result<SnapEditResult> {
        let raw = query.trim();
        if raw.is_empty() {
            bail!("snap-to-edit query must not be empty");
        }
        let query_tokens = token_set(raw);
        let query_trigrams = trigrams(raw);
        let raw_lower = raw.to_ascii_lowercase();
        let candidate_indexes: BTreeSet<usize> = if let Some(index) = self.qualified.get(&raw_lower)
        {
            [*index].into_iter().collect()
        } else if let Some(indexes) = self.exact.get(&raw_lower) {
            indexes.iter().copied().collect()
        } else {
            let matched: BTreeSet<usize> = query_tokens
                .iter()
                .filter_map(|token| self.token_map().get(token))
                .flatten()
                .copied()
                .collect();
            if matched.is_empty() {
                (0..self.entries.len()).collect()
            } else {
                matched
            }
        };
        let mut ranked: Vec<(f64, &IndexedAnchor)> = candidate_indexes
            .into_iter()
            .filter_map(|index| {
                let entry = &self.entries[index];
                let score = score_entry(entry, &raw_lower, &query_tokens, &query_trigrams);
                (score > 0.0).then_some((score, entry))
            })
            .collect();
        ranked.sort_by(|(sa, a), (sb, b)| {
            sb.total_cmp(sa)
                .then_with(|| a.anchor.path.cmp(&b.anchor.path))
                .then_with(|| a.anchor.byte_span.start.cmp(&b.anchor.byte_span.start))
        });
        let Some((mut best_score, best_entry)) = ranked.first().copied() else {
            bail!("no indexed edit anchor matched {raw:?}");
        };
        if best_score >= 0.999 && ranked.get(1).is_some_and(|(score, _)| *score >= 0.999) {
            best_score = AMBIGUOUS_CONFIDENCE;
        }
        let best = with_confidence(&best_entry.anchor, best_score);
        let alternates = if best_score < 0.8 {
            ranked
                .iter()
                .skip(1)
                .take(ALTERNATE_LIMIT)
                .map(|(score, entry)| with_confidence(&entry.anchor, (*score).min(best_score)))
                .collect()
        } else {
            Vec::new()
        };
        Ok(SnapEditResult {
            query: raw.to_string(),
            best,
            alternates,
        })
    }
}

/// Resolve a natural-language edit query to the best anchor in a snapshot.
pub fn snap_to_edit(snapshot: &Snapshot, query: &str) -> Result<SnapEditResult> {
    snapshot.snap_edit_index()?.resolve(query)
}

fn with_confidence(anchor: &EditAnchor, confidence: f64) -> EditAnchor {
    let mut out = anchor.clone();
    out.confidence = (confidence * 1000.0).round() / 1000.0;
    out
}

fn score_entry(
    entry: &IndexedAnchor,
    raw_lower: &str,
    query: &BTreeSet<String>,
    query_trigrams: &BTreeSet<[u8; 3]>,
) -> f64 {
    let symbol_lower = entry.anchor.symbol.to_ascii_lowercase();
    if raw_lower == symbol_lower {
        return 1.0;
    }
    let path_lower = entry.anchor.path.to_ascii_lowercase();
    if raw_lower == format!("{path_lower}::{symbol_lower}")
        || raw_lower == format!("{path_lower}/{symbol_lower}")
    {
        return 0.99;
    }
    if raw_lower.ends_with(&format!("::{symbol_lower}")) {
        let prefix = raw_lower.trim_end_matches(&format!("::{symbol_lower}"));
        if !prefix.is_empty() && path_lower.contains(prefix) {
            return 0.98;
        }
    }
    if query.is_empty() {
        return 0.0;
    }
    let name_hits = query.intersection(&entry.name_tokens).count() as f64;
    let all_hits = query.intersection(&entry.all_tokens).count() as f64;
    if all_hits == 0.0 {
        return trigram_similarity_sets(query_trigrams, &entry.search_trigrams) * 0.55;
    }
    let n = query.len() as f64;
    let mut score = 0.72 * (name_hits / n)
        + 0.23 * (all_hits / n)
        + 0.05 * trigram_similarity_sets(query_trigrams, &entry.search_trigrams);
    if name_hits == n && query.len() > 1 {
        score = score.max(0.9);
    } else if all_hits == n {
        score = score.max(0.82);
    }
    score.min(0.95)
}

fn token_set(text: &str) -> BTreeSet<String> {
    let mut normalized = String::with_capacity(text.len() + 8);
    let mut previous_lower_or_digit = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            if ch.is_ascii_uppercase() && previous_lower_or_digit {
                normalized.push(' ');
            }
            normalized.push(ch.to_ascii_lowercase());
            previous_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            normalized.push(' ');
            previous_lower_or_digit = false;
        }
    }
    normalized
        .split_whitespace()
        .map(|token| match token {
            "renderer" => "render",
            "resolver" => "resolve",
            "reader" => "read",
            "writer" => "write",
            "loader" => "load",
            "builder" => "build",
            "handler" => "handle",
            "matcher" => "match",
            "parser" => "parse",
            "router" => "route",
            "formatter" => "format",
            "collector" => "collect",
            "reporter" => "report",
            other => other,
        })
        .map(str::to_string)
        .collect()
}

fn trigram_similarity_sets(a: &BTreeSet<[u8; 3]>, b: &BTreeSet<[u8; 3]>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let common = a.intersection(b).count() as f64;
    (2.0 * common) / (a.len() + b.len()) as f64
}

fn trigrams(text: &str) -> BTreeSet<[u8; 3]> {
    let compact: Vec<u8> = text
        .bytes()
        .filter(|b| b.is_ascii_alphanumeric())
        .map(|b| b.to_ascii_lowercase())
        .collect();
    compact.windows(3).map(|w| [w[0], w[1], w[2]]).collect()
}

/// Newline prefix index for one blob: `line_starts[i]` is the byte offset of
/// the start of line `i+1`. Built in one pass; queries binary-search.
fn build_line_index(bytes: &[u8]) -> Vec<u32> {
    let mut line_starts = Vec::with_capacity(1024);
    line_starts.push(0u32);
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            line_starts.push(i as u32 + 1);
        }
    }
    line_starts
}

fn line_at_offset_indexed(line_starts: &[u32], offset: u32) -> u32 {
    // Number of newline starts at or before `offset` + 1 == 1-based line.
    let off = offset.min(u32::MAX - 1);
    let count = line_starts.partition_point(|&start| start <= off);
    count.max(1) as u32
}

/// FxHash-style hasher for the index's internal lookup maps (graphzero perf). Keys are short ASCII
/// strings; SipHash's 2-3 cycles/byte dominates `from_entries`.
#[derive(Default)]
pub(crate) struct FxHasher {
    hash: u64,
}

const FX_SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl FxHasher {
    #[inline]
    fn add_to_hash(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(FX_SEED);
    }
}

impl std::hash::Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut chunks = bytes.chunks_exact(8);
        for c in &mut chunks {
            self.add_to_hash(u64::from_le_bytes(c.try_into().unwrap()));
        }
        let rem = chunks.remainder();
        if !rem.is_empty() {
            let mut buf = [0u8; 8];
            buf[..rem.len()].copy_from_slice(rem);
            self.add_to_hash(u64::from_le_bytes(buf));
        }
    }
    #[inline]
    fn write_u8(&mut self, b: u8) {
        self.add_to_hash(b as u64);
    }
    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub(crate) type FxBuildHasher = std::hash::BuildHasherDefault<FxHasher>;

/// Compact binary wire format for the sidecar (length-prefixed serde via
/// postcard-style varints is overkill; a small hand-rolled framing keeps deps
/// at zero while staying unambiguous).
fn bincode_ser(value: &SnapEditSidecar) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(64 * 1024);
    // magic + schema
    out.extend_from_slice(b"GZSE");
    out.extend_from_slice(&value.schema.to_le_bytes());
    out.extend_from_slice(&(value.entries.len() as u64).to_le_bytes());
    for e in &value.entries {
        let json = serde_json::to_vec(e)?;
        out.extend_from_slice(&(json.len() as u64).to_le_bytes());
        out.extend_from_slice(&json);
    }
    Ok(out)
}

fn bincode_de(bytes: &[u8]) -> Result<SnapEditSidecar> {
    if bytes.len() < 16 || &bytes[0..4] != b"GZSE" {
        bail!("snap_edit sidecar: bad magic");
    }
    let schema = u32::from_le_bytes(bytes[4..8].try_into()?);
    let n = u64::from_le_bytes(bytes[8..16].try_into()?) as usize;
    let mut pos = 16usize;
    let mut entries = Vec::with_capacity(n.min(1 << 20));
    for _ in 0..n {
        if pos + 8 > bytes.len() {
            bail!("snap_edit sidecar: truncated");
        }
        let len = u64::from_le_bytes(bytes[pos..pos + 8].try_into()?) as usize;
        pos += 8;
        if pos + len > bytes.len() {
            bail!("snap_edit sidecar: truncated entry");
        }
        let entry: SerializedAnchor = serde_json::from_slice(&bytes[pos..pos + len])?;
        pos += len;
        entries.push(entry);
    }
    Ok(SnapEditSidecar { schema, entries })
}

fn kind_label(kind: u8) -> &'static str {
    match kind {
        symbol_kind::FUNCTION => "function",
        symbol_kind::TYPE => "type",
        symbol_kind::MODULE => "module",
        _ => "symbol",
    }
}
