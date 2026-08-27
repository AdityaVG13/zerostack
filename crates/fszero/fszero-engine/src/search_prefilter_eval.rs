//! Bigram/memmem prefilter for `direct_literal_scan` (fszero-9yq / 9ot / up8 / kbo).
//!
//! Production path (default `bigram_memmem` after fszero-kbo; escape
//! `FSZERO_SEARCH_PREFILTER=contains`) reuses `scan_bigram_memmem` +
//! `LazyBigramIndex` from here. Scanners share the
//! same hit contract as `ast_sgrep::direct_literal_scan`: lexicographic file
//! order, UTF-8 lines, case-sensitive substring, one hit per matching line,
//! early stop at `limit`.
//!
//! `measure_ingest_bigram_cost` times `BigramBitset::from_bytes` on the same
//! bytes already loaded for AST extract (lazy incremental path), not a bulk
//! rebuild-vs-read-all proxy.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
#[cfg(any(test, feature = "search-eval"))]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
#[cfg(any(test, feature = "search-eval"))]
use std::time::{Duration, Instant};

use memchr::memmem;
use rayon::prelude::*;

// Kept outside `#[cfg(test)]` so dependent crates' test suites (fs-zero) can
// read per-thread scan-count deltas against the engine.
std::thread_local! {
    static LITERAL_PHYSICAL_SCAN_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Per-test literal physical scan counter for this thread (measured,
/// read-only). Kept outside `#[cfg(test)]` so dependent crates' test suites
/// (fs-zero) can assert scan-count deltas against the engine.
pub fn literal_physical_scan_count() -> u64 {
    LITERAL_PHYSICAL_SCAN_COUNT.with(std::cell::Cell::get)
}

/// Per-thread measured scan counter (kept outside `#[cfg(test)]` so
/// dependent crates' test suites can assert scan-count deltas).
fn note_literal_physical_scan() {
    LITERAL_PHYSICAL_SCAN_COUNT.with(|count| count.set(count.get().saturating_add(1)));
}

#[cfg(any(test, feature = "search-eval"))]
use crate::ast::walk::looks_binary_bytes;
#[cfg(any(test, feature = "search-eval"))]
use crate::ast::{extract_structural, is_structural_path};

/// One grep-style hit (mirrors `IndexedLine` without pulling session internals).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalHit {
    pub file_key: Arc<str>,
    pub line_no: usize,
    pub text: String,
}

/// Byte-bigram presence bitset (65536 bits = 8 KiB) per file.
#[derive(Debug, Clone)]
pub struct BigramBitset {
    bits: Box<[u64; 1024]>,
}

impl BigramBitset {
    pub fn empty() -> Self {
        Self {
            bits: Box::new([0u64; 1024]),
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut bits = Box::new([0u64; 1024]);
        if bytes.len() < 2 {
            return Self { bits };
        }
        for w in bytes.windows(2) {
            let bg = u16::from_be_bytes([w[0], w[1]]);
            let idx = (bg as usize) >> 6;
            let mask = 1u64 << (bg as u64 & 63);
            bits[idx] |= mask;
        }
        Self { bits }
    }

    #[inline]
    fn contains_bigram(&self, bg: u16) -> bool {
        let idx = (bg as usize) >> 6;
        let mask = 1u64 << (bg as u64 & 63);
        self.bits[idx] & mask != 0
    }

    /// True when every byte-bigram of `needle` is present (or needle len < 2).
    pub fn may_contain(&self, needle: &[u8]) -> bool {
        if needle.len() < 2 {
            return true;
        }
        for w in needle.windows(2) {
            let bg = u16::from_be_bytes([w[0], w[1]]);
            if !self.contains_bigram(bg) {
                return false;
            }
        }
        true
    }

    pub fn approx_bytes(&self) -> usize {
        std::mem::size_of_val(self.bits.as_ref())
    }
}

/// On-disk identity paired with a bigram bitset. A bitset may reject a file
/// only while this stamp still matches; missing or unstable entries fall back
/// to the full content scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        }
    }
}

#[derive(Debug, Clone)]
struct BigramEntry {
    bitset: BigramBitset,
    stamp: Option<FileStamp>,
}

/// Lazy incremental bigram index: built/updated per file, never required at
/// cold index time. Create/modify upsert; delete removes.
#[derive(Debug, Default, Clone)]
pub struct LazyBigramIndex {
    by_file: HashMap<String, BigramEntry>,
}

impl LazyBigramIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Upsert bytes whose disk identity is not known to this component. The
    /// next query validates them from disk before using them to reject a file.
    pub fn upsert(&mut self, file_key: &str, content: &[u8]) {
        self.insert(file_key, content, None);
    }

    /// Upsert bytes already read by the owning file index, without rereading
    /// them on the next query.
    pub fn upsert_file(&mut self, file_key: &str, content: &[u8], meta: &std::fs::Metadata) {
        self.insert(file_key, content, Some(FileStamp::of(meta)));
    }

    pub fn upsert_stable(
        &mut self,
        file_key: &str,
        content: &[u8],
        modified: SystemTime,
        len: u64,
    ) {
        self.insert(
            file_key,
            content,
            Some(FileStamp {
                modified: Some(modified),
                len,
            }),
        );
    }

    fn insert(&mut self, file_key: &str, content: &[u8], stamp: Option<FileStamp>) {
        self.by_file.insert(
            file_key.to_string(),
            BigramEntry {
                bitset: BigramBitset::from_bytes(content),
                stamp,
            },
        );
    }

    pub fn remove(&mut self, file_key: &str) {
        self.by_file.remove(file_key);
    }

    pub fn get(&self, file_key: &str) -> Option<&BigramBitset> {
        self.by_file.get(file_key).map(|entry| &entry.bitset)
    }

    /// Return whether a metadata-stable indexed file may contain any needle.
    /// `None` means the entry is absent or stale and callers must scan.
    pub fn may_contain_stable(
        &self,
        root: &Path,
        file_key: &str,
        needles: &[&[u8]],
    ) -> Option<bool> {
        let entry = self.by_file.get(file_key)?;
        let current = std::fs::metadata(root.join(file_key))
            .ok()
            .filter(|meta| meta.is_file())
            .map(|meta| FileStamp::of(&meta));
        current
            .filter(|stamp| entry.stamp == Some(*stamp))
            .map(|_| {
                needles
                    .iter()
                    .any(|needle| entry.bitset.may_contain(needle))
            })
    }

    pub fn file_count(&self) -> usize {
        self.by_file.len()
    }

    pub fn approx_bytes(&self) -> usize {
        self.by_file
            .values()
            .map(|entry| entry.bitset.approx_bytes() + std::mem::size_of::<FileStamp>() + 32)
            .sum::<usize>()
            + self.by_file.len() * 64
    }

    /// Refresh entries whose on-disk identity changed. A stable refresh opens
    /// once and verifies metadata before/after the read. Failure removes the
    /// entry so candidate selection falls back instead of risking a false
    /// negative.
    pub fn ensure_files(&mut self, root: &Path, keys: &[&str]) {
        for key in keys {
            let path = root.join(key);
            let current = std::fs::metadata(&path)
                .ok()
                .filter(|meta| meta.is_file())
                .map(|meta| FileStamp::of(&meta));
            if current.is_some()
                && self
                    .by_file
                    .get(*key)
                    .is_some_and(|entry| entry.stamp == current)
            {
                continue;
            }
            self.load_file(key, &path);
        }
    }

    /// Fill only absent entries after the owning index has already performed
    /// its per-op freshness pass.
    fn ensure_missing_files(&mut self, root: &Path, keys: &[&str]) {
        for key in keys {
            if !self.by_file.contains_key(*key) {
                self.load_file(key, &root.join(key));
            }
        }
    }

    fn load_file(&mut self, key: &str, path: &Path) {
        match read_stable(path) {
            Some((bytes, stamp)) => self.insert(key, &bytes, Some(stamp)),
            None => {
                self.by_file.remove(key);
            }
        }
    }
}

fn read_stable(path: &Path) -> Option<(Vec<u8>, FileStamp)> {
    // File::open on a FIFO/socket blocks; refuse from metadata, then skip.
    crate::path::refuse_non_regular_file(path).ok()?;
    let mut file = std::fs::File::open(path).ok()?;
    let before = FileStamp::of(&file.metadata().ok()?);
    let mut bytes = Vec::with_capacity(usize::try_from(before.len).ok()?);
    file.read_to_end(&mut bytes).ok()?;
    let after = FileStamp::of(&file.metadata().ok()?);
    (before == after).then_some((bytes, after))
}

fn sorted_keys(keys: &HashSet<String>) -> Vec<&str> {
    let mut v: Vec<&str> = keys.iter().map(String::as_str).collect();
    v.sort_unstable();
    v
}

fn line_hits_filtered(
    file_key: &str,
    content: &str,
    remaining: usize,
    mut line_matches: impl FnMut(&str) -> bool,
) -> Vec<EvalHit> {
    let mut hits = Vec::new();
    for (line_no, line) in content.lines().enumerate() {
        if hits.len() >= remaining {
            break;
        }
        if line_matches(line) {
            hits.push(EvalHit {
                file_key: Arc::from(file_key),
                line_no: line_no + 1,
                text: line.to_string(),
            });
        }
    }
    hits
}

fn line_hits_contains(
    file_key: &str,
    content: &str,
    terms: &[String],
    remaining: usize,
) -> Vec<EvalHit> {
    line_hits_filtered(file_key, content, remaining, |line| {
        terms.iter().any(|term| line.contains(term.as_str()))
    })
}

fn line_hits_memmem(
    file_key: &str,
    content: &str,
    finders: &[memmem::Finder<'_>],
    remaining: usize,
) -> Vec<EvalHit> {
    line_hits_filtered(file_key, content, remaining, |line| {
        let bytes = line.as_bytes();
        finders.iter().any(|finder| finder.find(bytes).is_some())
    })
}

/// Parallel chunked file scan shared by baseline / memmem / bigram paths.
fn scan_key_chunks<'a, F>(keys: &[&'a str], limit: usize, map_file: F) -> Vec<EvalHit>
where
    F: Fn(&str, usize) -> Vec<EvalHit> + Sync,
{
    let mut accumulated = Vec::with_capacity(limit.min(256));
    for key_chunk in keys.chunks(256) {
        let remaining = limit.saturating_sub(accumulated.len());
        if remaining == 0 {
            break;
        }
        let chunk_hits: Vec<Vec<EvalHit>> = key_chunk
            .par_iter()
            .map(|file_key| map_file(file_key, remaining))
            .collect();
        for file_hits in chunk_hits {
            accumulated.extend(file_hits);
            if accumulated.len() >= limit {
                accumulated.truncate(limit);
                return accumulated;
            }
        }
    }
    accumulated
}

fn read_utf8_hits(
    root: &Path,
    file_key: &str,
    remaining: usize,
    hit_fn: impl FnOnce(&str, usize) -> Vec<EvalHit>,
) -> Vec<EvalHit> {
    let path = root.join(file_key);
    // read_to_string on a FIFO/socket blocks; skip as empty hits.
    if crate::path::refuse_non_regular_file(&path).is_err() {
        return Vec::new();
    }
    match std::fs::read_to_string(path) {
        Ok(content) => hit_fn(&content, remaining),
        Err(_) => Vec::new(),
    }
}

/// Baseline: reread every file, `str::contains` per line (eval/spike only).
#[cfg(any(test, feature = "search-eval"))]
pub fn scan_baseline(
    root: &Path,
    keys: &HashSet<String>,
    terms: &[String],
    limit: usize,
) -> Vec<EvalHit> {
    scan_contains_literal(root, keys, terms, limit)
}

/// Candidate A: same walk, `memchr::memmem` instead of `str::contains` (eval).
#[cfg(any(test, feature = "search-eval"))]
pub fn scan_memmem(
    root: &Path,
    keys: &HashSet<String>,
    terms: &[String],
    limit: usize,
) -> Vec<EvalHit> {
    let keys = sorted_keys(keys);
    scan_memmem_keys(root, &keys, terms, limit)
}

#[inline]
fn scan_idle(terms: &[String], limit: usize) -> bool {
    terms.is_empty() || limit == 0
}

/// Production contains-scan (escape hatch / baseline path for `direct_literal_scan`).
pub fn scan_contains_literal(
    root: &Path,
    keys: &HashSet<String>,
    terms: &[String],
    limit: usize,
) -> Vec<EvalHit> {
    if scan_idle(terms, limit) {
        return Vec::new();
    }
    note_literal_physical_scan();
    let keys = sorted_keys(keys);
    scan_key_chunks(&keys, limit, |file_key, remaining| {
        read_utf8_hits(root, file_key, remaining, |content, rem| {
            line_hits_contains(file_key, content, terms, rem)
        })
    })
}

/// Memmem line hits over an explicit key list (eval full-walk + bigram survivors).
fn scan_memmem_keys(root: &Path, keys: &[&str], terms: &[String], limit: usize) -> Vec<EvalHit> {
    if scan_idle(terms, limit) {
        return Vec::new();
    }
    let finders: Vec<memmem::Finder<'_>> = terms
        .iter()
        .map(|t| memmem::Finder::new(t.as_bytes()))
        .collect();
    scan_key_chunks(keys, limit, |file_key, remaining| {
        read_utf8_hits(root, file_key, remaining, |content, rem| {
            line_hits_memmem(file_key, content, &finders, rem)
        })
    })
}

/// Candidate B: lazy bigram candidate filter, then memmem on survivors.
///
/// Files missing from the index are filled lazily from disk on first query.
/// Missing or unstable entries remain candidates: the prefilter may reduce
/// work only when it can prove a stable bitset lacks every query term.
pub fn scan_bigram_memmem(
    root: &Path,
    keys: &HashSet<String>,
    terms: &[String],
    limit: usize,
    index: &mut LazyBigramIndex,
) -> Vec<EvalHit> {
    scan_bigram_memmem_impl(root, keys, terms, limit, index, true)
}

/// Production path after `refresh_stale_index_files`: avoids a duplicate
/// metadata pass while retaining lazy fills for files without a bitset.
pub fn scan_bigram_memmem_prevalidated(
    root: &Path,
    keys: &HashSet<String>,
    terms: &[String],
    limit: usize,
    index: &mut LazyBigramIndex,
) -> Vec<EvalHit> {
    scan_bigram_memmem_impl(root, keys, terms, limit, index, false)
}

fn scan_bigram_memmem_impl(
    root: &Path,
    keys: &HashSet<String>,
    terms: &[String],
    limit: usize,
    index: &mut LazyBigramIndex,
    validate_stamps: bool,
) -> Vec<EvalHit> {
    if scan_idle(terms, limit) {
        return Vec::new();
    }
    note_literal_physical_scan();
    let sorted = sorted_keys(keys);
    if validate_stamps {
        index.ensure_files(root, &sorted);
    } else {
        index.ensure_missing_files(root, &sorted);
    }

    let term_bytes: Vec<&[u8]> = terms.iter().map(|t| t.as_bytes()).collect();
    let mut candidates: Vec<&str> = Vec::new();
    for key in &sorted {
        if index
            .get(key)
            .is_none_or(|bs| term_bytes.iter().any(|needle| bs.may_contain(needle)))
        {
            candidates.push(key);
        }
    }
    scan_memmem_keys(root, &candidates, terms, limit)
}

#[cfg(any(test, feature = "search-eval"))]
pub struct IngestBigramCost {
    pub files: usize,
    pub bytes: u64,
    pub baseline_ingest_ms: f64,
    pub with_bigram_ingest_ms: f64,
    pub from_bytes_wall_ms: f64,
    pub from_bytes_sum_ms: f64,
    pub from_bytes_p50_us: f64,
    pub from_bytes_p95_us: f64,
    pub from_bytes_p99_us: f64,
    pub from_bytes_samples_us: Vec<f64>,
    pub cold_ingest_regress_pct: f64,
    pub index_approx_bytes: usize,
}

#[cfg(any(test, feature = "search-eval"))]
fn percentile_f64(samples: &[f64], p: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(any(test, feature = "search-eval"))]
fn dur_ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Cold ingest proxy matching `build_index` parallel_ingest work per file.
///
/// `with_bigram=false` times read+AST extract only. `with_bigram=true` adds
/// `LazyBigramIndex::upsert` (i.e. `BigramBitset::from_bytes`) on the same
/// bytes already held for extract — the lazy incremental accounting gate.
#[cfg(any(test, feature = "search-eval"))]
pub fn measure_ingest_bigram_cost(root: &Path, keys: &HashSet<String>) -> IngestBigramCost {
    let sorted = sorted_keys(keys);
    let paths: Vec<(String, PathBuf)> = sorted
        .iter()
        .map(|k| ((*k).to_string(), root.join(k)))
        .collect();

    // Warm page cache once so both arms measure CPU, not disk cold-start noise.
    for (_, path) in &paths {
        let _ = std::fs::read(path);
    }

    let run_arm = |with_bigram: bool| -> (Duration, u64, usize, Vec<f64>, Duration) {
        let mut index = LazyBigramIndex::new();
        let mut total_bytes = 0u64;
        let mut per_file_us: Vec<f64> = Vec::with_capacity(paths.len());
        let mut from_bytes_sum = Duration::ZERO;
        let wall0 = Instant::now();
        for (key, path) in &paths {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            if looks_binary_bytes(&bytes) {
                continue;
            }
            // Same ownership shape as ingest_one_file: UTF-8 String owns the
            // bytes already loaded for extract; bigram upserts from that buffer.
            let Ok(txt) = String::from_utf8(bytes) else {
                continue;
            };
            total_bytes += txt.len() as u64;
            if is_structural_path(path) {
                let _ = extract_structural(path, &txt);
            }
            if with_bigram {
                let t0 = Instant::now();
                index.upsert(key, txt.as_bytes());
                let elapsed = t0.elapsed();
                from_bytes_sum += elapsed;
                per_file_us.push(elapsed.as_secs_f64() * 1_000_000.0);
            }
        }
        let wall = wall0.elapsed();
        (
            wall,
            total_bytes,
            index.approx_bytes(),
            per_file_us,
            from_bytes_sum,
        )
    };

    let (baseline_wall, bytes, _, _, _) = run_arm(false);
    let (with_wall, _, index_bytes, per_file_us, from_bytes_sum) = run_arm(true);

    let baseline_ms = dur_ms(baseline_wall);
    let with_ms = dur_ms(with_wall);
    let from_sum_ms = dur_ms(from_bytes_sum);
    // Prefer instrumented from_bytes sum over wall delta: on multi-second AST
    // extract walls, ±tens of ms noise can flip the sign of (with - baseline).
    let cold_regress = if baseline_ms > 0.0 {
        from_sum_ms / baseline_ms * 100.0
    } else {
        0.0
    };

    IngestBigramCost {
        files: paths.len(),
        bytes,
        baseline_ingest_ms: baseline_ms,
        with_bigram_ingest_ms: with_ms,
        from_bytes_wall_ms: (with_ms - baseline_ms).max(0.0),
        from_bytes_sum_ms: from_sum_ms,
        from_bytes_p50_us: percentile_f64(&per_file_us, 50.0),
        from_bytes_p95_us: percentile_f64(&per_file_us, 95.0),
        from_bytes_p99_us: percentile_f64(&per_file_us, 99.0),
        from_bytes_samples_us: per_file_us,
        cold_ingest_regress_pct: cold_regress,
        index_approx_bytes: index_bytes,
    }
}

/// Deterministic incremental parity helper for tests/eval: create/modify/delete
/// then compare bigram+memmem hits to baseline.
#[cfg(any(test, feature = "search-eval"))]
pub fn apply_incremental(
    index: &mut LazyBigramIndex,
    root: &Path,
    op: &str,
    rel: &str,
    content: Option<&[u8]>,
) -> PathBuf {
    let path = root.join(rel);
    match op {
        "create" | "modify" => {
            let bytes = content.unwrap_or(b"");
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&path, bytes).expect("write");
            index.upsert(rel, bytes);
        }
        "delete" => {
            let _ = std::fs::remove_file(&path);
            index.remove(rel);
        }
        _ => panic!("unknown op {op}"),
    }
    path
}
