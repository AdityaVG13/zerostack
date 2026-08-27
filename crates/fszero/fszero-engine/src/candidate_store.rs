//! Candidate persistence + delta-only repair loop (bead
//! zerostack-racc-caching-output-vz89.4).
//!
//! The repair loop a model runs today costs `sum |P_i|`: every round it re-emits
//! the whole patch. This module makes it cost `|P_0| + sum |delta_i|` instead.
//!
//! * Every model-emitted candidate (patch / file / fragment) is persisted as a
//!   durable CAS blob and gets a short stable handle: `@candidate:<n>`,
//!   `@fragment:F<n>`. Handles survive process restarts (append-only index).
//! * A repair round emits only a delta: `CandidateDelta` carries ZEP/1 verbs
//!   (`REPLACE` / `INSERT` / `DELETE` / `COPY`) whose refs use the ZEP/1
//!   `FileSpan` line grammar (`#L<a>-L<b>`, 1-based inclusive).
//! * Candidates compose: a revised candidate is `base root + delta chain`,
//!   materialized here, never re-uploaded by the model.
//! * Diagnostics are returned anchored to spans inside the candidate
//!   (`@candidate:7#L4-L6`), not as full re-dumps.
//!
//! Span anchoring is **base-relative**: every ref in one delta resolves against
//! the base image the model actually saw, so a delta is not order-fragile.
//! Resolved edit ranges must therefore be non-overlapping and start at distinct
//! offsets; ambiguity is a typed error, never a silent last-writer-wins.
//!
//! Materialization is memoized under a `cache-entry`-aligned key
//! (docs/contracts/cache-entry-v1.md, ZeroStack bead vz89.3): the key locks the
//! operator id/version, canonical parameters, the *minimum exact* dependency
//! roots (base blob + delta blob only -- never a repository root), the toolchain
//! root, and a completeness witness. Re-applying the same delta to the same base
//! returns the existing handle instead of recomputing.

use super::cas::{CasError, CasStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Wire discriminator for persisted candidate index records.
pub const CANDIDATE_SCHEMA: &str = "candidate/v1";
/// Cache-entry operator identity for candidate materialization.
pub const CANDIDATE_OPERATOR_ID: &str = "fszero.candidate.materialize";
/// Locked operator version; bump on any change to delta application semantics.
pub const CANDIDATE_OPERATOR_VERSION: &str = "1";
/// Protocol tag carried by a delta, per Zero Edit Protocol v1.
pub const CANDIDATE_DELTA_PROTOCOL: &str = "zep/1";
/// Longest base->revision chain a single candidate may accumulate.
pub const CANDIDATE_MAX_CHAIN: usize = 64;

const CANDIDATE_DIR: &str = "candidates";
const INDEX_FILE: &str = "index.jsonl";

/// Typed candidate-layer failures. Nothing here is ever reported as success.
#[derive(Debug)]
pub enum CandidateError {
    /// Underlying content store failure (missing blob, corruption, I/O).
    Cas(CasError),
    /// Index file could not be read or appended.
    Io {
        context: String,
        source: std::io::Error,
    },
    /// Handle syntax is not `@candidate:<n>` or `@fragment:F<n>`.
    MalformedHandle(String),
    /// Handle syntax is valid but no such candidate is persisted.
    UnknownHandle(String),
    /// A ref did not resolve inside the base image.
    BadSpan { anchor: String, detail: String },
    /// Two resolved edits collide, so the delta has no single meaning.
    AmbiguousDelta { detail: String },
    /// Persisted index line is not a decodable record.
    CorruptIndex { line: usize, detail: String },
    /// Chain length would exceed [`CANDIDATE_MAX_CHAIN`].
    ChainTooLong { handle: String, limit: usize },
}

impl fmt::Display for CandidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CandidateError::Cas(e) => write!(f, "candidate store: {e}"),
            CandidateError::Io { context, source } => {
                write!(f, "candidate index io ({context}): {source}")
            }
            CandidateError::MalformedHandle(h) => write!(f, "malformed candidate handle: {h}"),
            CandidateError::UnknownHandle(h) => write!(f, "unknown candidate handle: {h}"),
            CandidateError::BadSpan { anchor, detail } => {
                write!(f, "unresolvable ref {anchor}: {detail}")
            }
            CandidateError::AmbiguousDelta { detail } => write!(f, "ambiguous delta: {detail}"),
            CandidateError::CorruptIndex { line, detail } => {
                write!(f, "corrupt candidate index at line {line}: {detail}")
            }
            CandidateError::ChainTooLong { handle, limit } => {
                write!(f, "delta chain for {handle} exceeds {limit} links")
            }
        }
    }
}

impl std::error::Error for CandidateError {}

impl From<CasError> for CandidateError {
    fn from(e: CasError) -> Self {
        CandidateError::Cas(e)
    }
}

impl CandidateError {
    /// Stable error class, mirroring the ZEP/1 error-class vocabulary.
    pub fn class(&self) -> &'static str {
        match self {
            CandidateError::Cas(_) => "zeroref",
            CandidateError::Io { .. } => "io",
            CandidateError::MalformedHandle(_) => "malformed_ref",
            CandidateError::UnknownHandle(_) => "unresolved_ref",
            CandidateError::BadSpan { .. } => "unresolved_ref",
            CandidateError::AmbiguousDelta { .. } => "ambiguous_edit",
            CandidateError::CorruptIndex { .. } => "corrupt_index",
            CandidateError::ChainTooLong { .. } => "limit_exceeded",
        }
    }
}

/// 1-based inclusive line span: the ZEP/1 `FileSpan` ref slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineSpan {
    pub start: u32,
    pub end: u32,
}

impl LineSpan {
    pub fn line(n: u32) -> Self {
        Self { start: n, end: n }
    }
    pub fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    /// Parse the `#L<a>-L<b>` fragment body (`L4-L6`, or `L4` for one line).
    pub fn parse(fragment: &str) -> Option<Self> {
        let body = fragment.strip_prefix('#').unwrap_or(fragment);
        let rest = body.strip_prefix('L')?;
        match rest.split_once("-L") {
            Some((a, b)) => Some(Self {
                start: a.parse().ok()?,
                end: b.parse().ok()?,
            }),
            None => {
                let n = rest.parse().ok()?;
                Some(Self { start: n, end: n })
            }
        }
    }
}

impl fmt::Display for LineSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "L{}-L{}", self.start, self.end)
    }
}

/// Insertion side for `INSERT` / `COPY`; `after` is the ZEP/1 default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    After,
    Before,
}

/// One repair operation. Verb lives in the `v` discriminant, per ZEP/1 section 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "v")]
pub enum DeltaOp {
    #[serde(rename = "REPLACE")]
    Replace { r: LineSpan, text: String },
    #[serde(rename = "INSERT")]
    Insert {
        at: LineSpan,
        #[serde(default)]
        side: Side,
        text: String,
    },
    #[serde(rename = "DELETE")]
    Delete { r: LineSpan },
    /// Splice a previously persisted fragment in without regenerating it.
    #[serde(rename = "COPY")]
    Copy {
        from: String,
        at: LineSpan,
        #[serde(default)]
        side: Side,
    },
}

/// The only thing a repair round emits: an ordered op list against one base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDelta {
    #[serde(default = "default_protocol")]
    pub p: String,
    pub ops: Vec<DeltaOp>,
}

fn default_protocol() -> String {
    CANDIDATE_DELTA_PROTOCOL.to_string()
}

impl CandidateDelta {
    pub fn new(ops: Vec<DeltaOp>) -> Self {
        Self {
            p: default_protocol(),
            ops,
        }
    }
}

/// A persisted candidate or fragment. `base`/`delta_root` are set only for a
/// revision produced by [`CandidateStore::apply_delta`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub schema: String,
    pub handle: String,
    /// Content root of the materialized bytes (`fz://blob/<sha256>`).
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_hash: Option<String>,
    /// Set for a fragment: the candidate and span it was sliced from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<LineSpan>,
}

/// Outcome of materializing `base + delta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializeOutcome {
    pub record: CandidateRecord,
    /// True when the cache-entry key already had a materialized output root.
    pub reused: bool,
    /// `sha256(canonical_json(key))`, the cache-entry lookup identity.
    pub key_hash: String,
    /// Bytes the model actually had to emit for this round.
    pub delta_bytes: usize,
}

/// Output-token accounting for one candidate chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateCost {
    pub base_bytes: usize,
    pub delta_bytes: usize,
    /// Size of the final materialized candidate.
    pub materialized_bytes: usize,
    /// What re-emitting every revision in full would have cost.
    pub full_rewrite_bytes: usize,
}

impl CandidateCost {
    /// `|P_0| + sum |delta_i|`.
    pub fn emitted_bytes(&self) -> usize {
        self.base_bytes + self.delta_bytes
    }
}

/// A compiler/test diagnostic anchored inside a candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDiagnostic {
    pub class: String,
    /// `@candidate:<n>#L<a>-L<b>` -- a ZEP/1 ref, not a file re-dump.
    pub anchor: String,
    pub message: String,
}

/// One diagnostic per line, `<class> <anchor> <message>`. No source re-dump:
/// the producer already holds the candidate under its handle.
pub fn render_diagnostics(diags: &[CandidateDiagnostic]) -> String {
    diags
        .iter()
        .map(|d| format!("{} {} {}", d.class, d.anchor, d.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Durable candidate store: CAS blobs for content, append-only index for handles.
pub struct CandidateStore {
    cas: CasStore,
    index_path: PathBuf,
    records: Vec<CandidateRecord>,
    by_handle: BTreeMap<String, usize>,
    by_root: BTreeMap<String, String>,
    by_key: BTreeMap<String, String>,
    next_candidate: u64,
    next_fragment: u64,
    /// Index-file `sync_all` count since open (fszero-60mx). Per-record durability
    /// policy keeps one fsync per append; counters make the small-write tax visible.
    index_fsync_count: u64,
    /// Cumulative wall microseconds spent in index `sync_all` since open.
    index_fsync_us: u64,
}

fn blob_ref(hash: &str) -> String {
    format!("fz://blob/{hash}")
}

fn hash_of_ref(root: &str) -> Result<&str, CandidateError> {
    super::cas::full_blob_hash(root)
        .ok_or_else(|| CandidateError::MalformedHandle(root.to_string()))
}

fn parse_handle(handle: &str) -> Result<(bool, u64), CandidateError> {
    let malformed = || CandidateError::MalformedHandle(handle.to_string());
    if let Some(n) = handle.strip_prefix("@candidate:") {
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed());
        }
        return Ok((false, n.parse().map_err(|_| malformed())?));
    }
    if let Some(n) = handle.strip_prefix("@fragment:F") {
        if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed());
        }
        return Ok((true, n.parse().map_err(|_| malformed())?));
    }
    Err(malformed())
}

/// Byte offset of the first byte of every 1-based line.
fn line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = vec![0usize];
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' && i + 1 < bytes.len() {
            starts.push(i + 1);
        }
    }
    starts
}

/// Resolve a span to a byte range. Start is strict, end clamps to the last
/// line (ZeroRef v1 clamp policy, docs/zeroref.md).
fn resolve_span(
    bytes: &[u8],
    span: LineSpan,
    anchor: &str,
) -> Result<(usize, usize), CandidateError> {
    let starts = line_starts(bytes);
    let lines = starts.len() as u32;
    if span.start == 0 || span.start > lines {
        return Err(CandidateError::BadSpan {
            anchor: anchor.to_string(),
            detail: format!("start line {} outside 1..={lines}", span.start),
        });
    }
    if span.end < span.start {
        return Err(CandidateError::BadSpan {
            anchor: anchor.to_string(),
            detail: format!("end line {} precedes start line {}", span.end, span.start),
        });
    }
    let end_line = span.end.min(lines);
    let start = starts[(span.start - 1) as usize];
    let end = starts
        .get(end_line as usize)
        .copied()
        .unwrap_or(bytes.len());
    Ok((start, end))
}

struct ResolvedEdit {
    start: usize,
    end: usize,
    text: Vec<u8>,
}

impl CandidateStore {
    /// Open (or create) the candidate layer under a ZeroStack store root and
    /// replay the persisted index so handles stay stable across processes.
    pub fn open(store_root: &Path) -> Result<Self, CandidateError> {
        let dir = store_root.join(CANDIDATE_DIR);
        fs::create_dir_all(&dir).map_err(|source| CandidateError::Io {
            context: format!("create {}", dir.display()),
            source,
        })?;
        let index_path = dir.join(INDEX_FILE);
        let mut store = Self {
            cas: CasStore::for_store_root(store_root),
            index_path,
            records: Vec::new(),
            by_handle: BTreeMap::new(),
            by_root: BTreeMap::new(),
            by_key: BTreeMap::new(),
            next_candidate: 1,
            next_fragment: 1,
            index_fsync_count: 0,
            index_fsync_us: 0,
        };
        store.replay()?;
        Ok(store)
    }

    fn replay(&mut self) -> Result<(), CandidateError> {
        let text = match fs::read_to_string(&self.index_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(CandidateError::Io {
                    context: format!("read {}", self.index_path.display()),
                    source,
                });
            }
        };
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let record: CandidateRecord =
                serde_json::from_str(line).map_err(|e| CandidateError::CorruptIndex {
                    line: i + 1,
                    detail: e.to_string(),
                })?;
            if record.schema != CANDIDATE_SCHEMA {
                return Err(CandidateError::CorruptIndex {
                    line: i + 1,
                    detail: format!("unknown schema {}", record.schema),
                });
            }
            let (is_fragment, n) =
                parse_handle(&record.handle).map_err(|e| CandidateError::CorruptIndex {
                    line: i + 1,
                    detail: e.to_string(),
                })?;
            if is_fragment {
                self.next_fragment = self.next_fragment.max(n + 1);
            } else {
                self.next_candidate = self.next_candidate.max(n + 1);
            }
            self.remember(record);
        }
        Ok(())
    }

    fn remember(&mut self, record: CandidateRecord) {
        let index = self.records.len();
        if record.base.is_none() && record.source.is_none() {
            self.by_root
                .entry(record.root.clone())
                .or_insert_with(|| record.handle.clone());
        }
        if let Some(key) = &record.key_hash {
            self.by_key
                .entry(key.clone())
                .or_insert_with(|| record.handle.clone());
        }
        self.by_handle.insert(record.handle.clone(), index);
        self.records.push(record);
    }

    /// Append one index record and fsync.
    ///
    /// **Durability policy (fszero-60mx):** per-record `sync_all` is intentional.
    /// Handles must survive process kill between puts (append-only JSONL is the
    /// handle namespace across restarts). Group-commit would risk losing the
    /// newest handle after a crash while CAS blobs already exist -- a silent
    /// handle/CAS gap. Cost is visible via [`index_fsync_stats`].
    fn append(&mut self, record: CandidateRecord) -> Result<CandidateRecord, CandidateError> {
        let mut line =
            serde_json::to_string(&record).map_err(|e| CandidateError::CorruptIndex {
                line: 0,
                detail: e.to_string(),
            })?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.index_path)
            .map_err(|source| CandidateError::Io {
                context: format!("open {}", self.index_path.display()),
                source,
            })?;
        file.write_all(line.as_bytes())
            .map_err(|source| CandidateError::Io {
                context: "append record".into(),
                source,
            })?;
        let t0 = std::time::Instant::now();
        file.sync_all().map_err(|source| CandidateError::Io {
            context: "sync index".into(),
            source,
        })?;
        let us = t0.elapsed().as_micros() as u64;
        self.index_fsync_count = self.index_fsync_count.saturating_add(1);
        self.index_fsync_us = self.index_fsync_us.saturating_add(us);
        self.remember(record.clone());
        Ok(record)
    }

    /// Index-file fsync count and cumulative wall microseconds since open (fszero-60mx).
    pub fn index_fsync_stats(&self) -> (u64, u64) {
        (self.index_fsync_count, self.index_fsync_us)
    }

    /// Persist a model-emitted candidate (`P_0`). Identical bytes reuse the
    /// existing handle, so handles stay stable and reusable in later outputs.
    pub fn put_candidate(&mut self, bytes: &[u8]) -> Result<CandidateRecord, CandidateError> {
        let root = blob_ref(&self.cas.put(bytes)?.hash);
        if let Some(handle) = self.by_root.get(&root) {
            let index = self.by_handle[handle];
            return Ok(self.records[index].clone());
        }
        let handle = format!("@candidate:{}", self.next_candidate);
        self.next_candidate += 1;
        self.append(CandidateRecord {
            schema: CANDIDATE_SCHEMA.to_string(),
            handle,
            root,
            base: None,
            delta_root: None,
            key_hash: None,
            source: None,
            span: None,
        })
    }

    /// Persist a span of an existing candidate under a short fragment handle so
    /// later rounds can `COPY` it instead of regenerating the text.
    pub fn put_fragment(
        &mut self,
        source: &str,
        span: LineSpan,
    ) -> Result<CandidateRecord, CandidateError> {
        let bytes = self.bytes(source)?;
        let anchor = format!("{source}#{span}");
        let (start, end) = resolve_span(&bytes, span, &anchor)?;
        let root = blob_ref(&self.cas.put(&bytes[start..end])?.hash);
        if let Some(existing) = self
            .records
            .iter()
            .find(|r| r.source.as_deref() == Some(source) && r.span == Some(span) && r.root == root)
        {
            return Ok(existing.clone());
        }
        let handle = format!("@fragment:F{}", self.next_fragment);
        self.next_fragment += 1;
        self.append(CandidateRecord {
            schema: CANDIDATE_SCHEMA.to_string(),
            handle,
            root,
            base: None,
            delta_root: None,
            key_hash: None,
            source: Some(source.to_string()),
            span: Some(span),
        })
    }

    pub fn record(&self, handle: &str) -> Result<&CandidateRecord, CandidateError> {
        parse_handle(handle)?;
        self.by_handle
            .get(handle)
            .map(|i| &self.records[*i])
            .ok_or_else(|| CandidateError::UnknownHandle(handle.to_string()))
    }

    /// Materialized bytes behind a handle, read back through the CAS (so a
    /// corrupt blob is a typed error, never silently served).
    pub fn bytes(&self, handle: &str) -> Result<Vec<u8>, CandidateError> {
        let root = self.record(handle)?.root.clone();
        Ok(self.cas.get(hash_of_ref(&root)?)?)
    }

    /// Validated ZEP/1 ref into a candidate, for anchoring diagnostics.
    pub fn anchor(&self, handle: &str, span: LineSpan) -> Result<String, CandidateError> {
        let bytes = self.bytes(handle)?;
        let anchor = format!("{handle}#{span}");
        resolve_span(&bytes, span, &anchor)?;
        Ok(anchor)
    }

    /// Anchor a compiler/test diagnostic to one line of a candidate.
    pub fn diagnostic(
        &self,
        handle: &str,
        class: &str,
        line: u32,
        message: &str,
    ) -> Result<CandidateDiagnostic, CandidateError> {
        Ok(CandidateDiagnostic {
            class: class.to_string(),
            anchor: self.anchor(handle, LineSpan::line(line))?,
            message: message.to_string(),
        })
    }

    /// Handles from the root candidate through `handle`, in application order.
    pub fn chain(&self, handle: &str) -> Result<Vec<String>, CandidateError> {
        let mut out = Vec::new();
        let mut cursor = self.record(handle)?;
        loop {
            out.push(cursor.handle.clone());
            if out.len() > CANDIDATE_MAX_CHAIN {
                return Err(CandidateError::ChainTooLong {
                    handle: handle.to_string(),
                    limit: CANDIDATE_MAX_CHAIN,
                });
            }
            match &cursor.base {
                Some(base) => cursor = self.record(base)?,
                None => break,
            }
        }
        out.reverse();
        Ok(out)
    }

    /// Output-token accounting: what the chain cost to emit vs what re-emitting
    /// every revision in full would have cost.
    pub fn cost(&self, handle: &str) -> Result<CandidateCost, CandidateError> {
        let chain = self.chain(handle)?;
        let mut cost = CandidateCost {
            base_bytes: 0,
            delta_bytes: 0,
            materialized_bytes: 0,
            full_rewrite_bytes: 0,
        };
        for (i, link) in chain.iter().enumerate() {
            let record = self.record(link)?;
            let size = self.cas.get(hash_of_ref(&record.root)?)?.len();
            cost.full_rewrite_bytes += size;
            cost.materialized_bytes = size;
            if i == 0 {
                cost.base_bytes = size;
            }
            if let Some(delta_root) = &record.delta_root {
                cost.delta_bytes += self.cas.get(hash_of_ref(delta_root)?)?.len();
            }
        }
        Ok(cost)
    }

    /// True only when every `fz://blob/<sha256>` root still resolves in the
    /// current CAS. A malformed root is a typed error, not a silent miss; a
    /// merely absent object is a resolvable gap, so the caller re-runs.
    fn roots_resolve(&self, roots: &[String]) -> Result<bool, CandidateError> {
        for root in roots {
            if !self.cas.contains(hash_of_ref(root)?) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Materialize `base + delta` into a new candidate. Memoized on a
    /// `cache-entry`-aligned key, so a repeated delta is a hit, not a re-run.
    pub fn apply_delta(
        &mut self,
        base: &str,
        delta: &CandidateDelta,
    ) -> Result<MaterializeOutcome, CandidateError> {
        let base_record = self.record(base)?.clone();
        if self.chain(base)?.len() >= CANDIDATE_MAX_CHAIN {
            return Err(CandidateError::ChainTooLong {
                handle: base.to_string(),
                limit: CANDIDATE_MAX_CHAIN,
            });
        }
        let delta_value =
            serde_json::to_value(delta).map_err(|e| CandidateError::AmbiguousDelta {
                detail: e.to_string(),
            })?;
        let delta_bytes = zero_abi::canonical_json(&delta_value).into_bytes();
        let delta_root = blob_ref(&self.cas.put(&delta_bytes)?.hash);

        let toolchain_root = blob_ref(
            &self
                .cas
                .put(format!("{CANDIDATE_OPERATOR_ID}@{CANDIDATE_OPERATOR_VERSION}").as_bytes())?
                .hash,
        );
        let mut dependency_roots = vec![base_record.root.clone(), delta_root.clone()];
        for op in &delta.ops {
            if let DeltaOp::Copy { from, .. } = op {
                dependency_roots.push(self.record(from)?.root.clone());
            }
        }
        dependency_roots.sort();
        dependency_roots.dedup();
        let witness_root = blob_ref(
            &self
                .cas
                .put(
                    zero_abi::canonical_json(&json!({
                        "checked_roots": dependency_roots.clone(),
                        "operator": format!("{CANDIDATE_OPERATOR_ID}@{CANDIDATE_OPERATOR_VERSION}"),
                    }))
                    .as_bytes(),
                )?
                .hash,
        );
        let key = candidate_cache_key(&dependency_roots, &toolchain_root, &witness_root);
        let key_hash = zero_abi::contract_digest_hex(&key);

        if let Some(handle) = self.by_key.get(&key_hash).cloned() {
            let record = self.record(&handle)?.clone();
            // Gap-check against the CURRENT CAS before reusing: every key root
            // plus the cached output root must still resolve. A gap (GC,
            // partially synced store, damaged shard) makes the entry unreusable,
            // so re-run the operator rather than hand back a handle whose bytes
            // cannot be served -- cache-entry invalidation semantics: never
            // treat missing evidence as unchanged.
            let mut roots = dependency_roots.clone();
            roots.push(toolchain_root.clone());
            roots.push(witness_root.clone());
            roots.push(record.root.clone());
            if self.roots_resolve(&roots)? {
                return Ok(MaterializeOutcome {
                    record,
                    reused: true,
                    key_hash,
                    delta_bytes: delta_bytes.len(),
                });
            }
        }

        let base_image = self.bytes(base)?;
        let mut edits = Vec::with_capacity(delta.ops.len());
        for op in &delta.ops {
            edits.push(self.resolve_op(base, &base_image, op)?);
        }
        let output = splice(&base_image, edits)?;
        let root = blob_ref(&self.cas.put(&output)?.hash);
        let handle = format!("@candidate:{}", self.next_candidate);
        self.next_candidate += 1;
        let record = self.append(CandidateRecord {
            schema: CANDIDATE_SCHEMA.to_string(),
            handle,
            root,
            base: Some(base.to_string()),
            delta_root: Some(delta_root),
            key_hash: Some(key_hash.clone()),
            source: None,
            span: None,
        })?;
        Ok(MaterializeOutcome {
            record,
            reused: false,
            key_hash,
            delta_bytes: delta_bytes.len(),
        })
    }

    fn resolve_op(
        &self,
        base: &str,
        image: &[u8],
        op: &DeltaOp,
    ) -> Result<ResolvedEdit, CandidateError> {
        let point = |span: LineSpan, side: Side| -> Result<ResolvedEdit, CandidateError> {
            let anchor = format!("{base}#{span}");
            let (start, end) = resolve_span(image, span, &anchor)?;
            let at = match side {
                Side::Before => start,
                Side::After => end,
            };
            Ok(ResolvedEdit {
                start: at,
                end: at,
                text: Vec::new(),
            })
        };
        match op {
            DeltaOp::Replace { r, text } => {
                let anchor = format!("{base}#{r}");
                let (start, end) = resolve_span(image, *r, &anchor)?;
                Ok(ResolvedEdit {
                    start,
                    end,
                    text: text.as_bytes().to_vec(),
                })
            }
            DeltaOp::Delete { r } => {
                let anchor = format!("{base}#{r}");
                let (start, end) = resolve_span(image, *r, &anchor)?;
                Ok(ResolvedEdit {
                    start,
                    end,
                    text: Vec::new(),
                })
            }
            DeltaOp::Insert { at, side, text } => {
                let mut edit = point(*at, *side)?;
                edit.text = text.as_bytes().to_vec();
                Ok(edit)
            }
            DeltaOp::Copy { from, at, side } => {
                let mut edit = point(*at, *side)?;
                edit.text = self.bytes(from)?;
                Ok(edit)
            }
        }
    }
}

/// Cache key object aligned with docs/contracts/cache-entry-v1.md. The
/// dependency set is the minimum exact one (base blob, delta blob, any copied
/// fragment) -- deliberately never a repository root.
fn candidate_cache_key(
    dependency_roots: &[String],
    toolchain_root: &str,
    witness_root: &str,
) -> Value {
    // fszero-ojnv: real scope_roots (anti-dependency cone), never hardcode [].
    let scope_roots = super::negative_cache::scope_roots_for_key(dependency_roots);
    json!({
        "operator": { "id": CANDIDATE_OPERATOR_ID, "version": CANDIDATE_OPERATOR_VERSION },
        "canonical_parameters": { "anchoring": "base_relative_spans", "protocol": CANDIDATE_DELTA_PROTOCOL },
        "minimum_dependency_roots": dependency_roots,
        "environment_roots": [],
        "toolchain_roots": [toolchain_root],
        "completeness_witness": { "proof_root": witness_root, "checked_roots": dependency_roots },
        "scope_roots": scope_roots,
    })
}

/// Apply base-relative edits. Ranges must not overlap and must start at
/// distinct offsets; otherwise the delta has no single meaning.
fn splice(image: &[u8], mut edits: Vec<ResolvedEdit>) -> Result<Vec<u8>, CandidateError> {
    edits.sort_by_key(|e| (e.start, e.end));
    for pair in edits.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        if a.start == b.start {
            return Err(CandidateError::AmbiguousDelta {
                detail: format!("two edits anchored at byte {}", a.start),
            });
        }
        if b.start < a.end {
            return Err(CandidateError::AmbiguousDelta {
                detail: format!(
                    "edit at {}..{} overlaps {}..{}",
                    b.start, b.end, a.start, a.end
                ),
            });
        }
    }
    let mut out = image.to_vec();
    for edit in edits.into_iter().rev() {
        out.splice(edit.start..edit.end, edit.text);
    }
    Ok(out)
}
