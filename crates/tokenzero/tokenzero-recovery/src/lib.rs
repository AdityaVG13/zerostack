#![forbid(unsafe_code)]

use fs4::{FileExt, TryLockError};
use regex::Regex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    LazyLock,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::thread;
use std::time::{Duration, Instant, SystemTime};
use thiserror::Error;
use tokenzero_core::{ContentType, count_tokens, error_block, id_for, sha256_hex, symbol_block};

use crate::shared_cas::{SharedCas, SharedCasError};
use crate::telemetry::CrossEngineTelemetry;
use zero_store::{
    AppendOutcome, FileIdentity, SESSION_WAL_DEFAULT_MAX_SEALED_SEGMENTS, SessionWal,
    SessionWalConfig, SessionWalError, SyncPolicy,
};

pub mod telemetry;

pub mod boot;
pub mod context_view;
pub mod cow_fork;
pub mod crash_inject;
pub mod dst;
pub mod embedded_store;
pub mod entity_novelty;
pub mod migration;
pub mod prefix_stability;
pub mod segment_store;
pub mod session_aliases;
pub mod shared_cas;
pub mod transparency;
pub use entity_novelty::{
    ENTITY_NOVELTY_RECORD_TYPE, ENTITY_NOVELTY_REL_DIR, ENTITY_NOVELTY_SCHEMA_VERSION,
    EntityNoveltyRecord, NoveltyError, entity_novelty_path, merge_entity_novelty, parse_entity_ref,
    read_entity_novelty, scope_digest, write_entity_novelty,
};

pub mod action_cache;
pub mod cachezero;
pub mod frecency;
pub mod memory_verbs;
pub mod store_hygiene;
pub mod store_schema;
pub mod working_set;

pub use frecency::{HALF_LIFE_SECS, burst_compress, coldest, decay, score, score_from_order};
pub use memory_verbs::{
    MemoryVerb, MemoryVerbEffect, MemoryVerbError, MemoryVerbRequest, apply_memory_verb,
    describe_memory_verb,
};

pub use action_cache::{
    ACTIONCACHE_GC_GRACE_SECS, ACTIONCACHE_REL_DIR, ActionCacheEntry, ActionCacheError,
    ActionCacheIndex, BlobEvictionPlan, EvictionSlackGuard, ServedArtifact,
    action_cache_protects_hash, artifact_full_hash,
};
pub use cachezero::{
    CACHEZERO_ENV, CACHEZERO_GRADUATION_PCT, CACHEZERO_MODE_ENV, CACHEZERO_REL_DIR,
    CACHEZERO_SHADOW_FILE, CACHEZERO_STATS_SCHEMA, CacheStatus, CachezeroMode, CachezeroStats,
    ShadowDecision, aggregate_cachezero, cachezero_stats_json, classify_would_be_status,
    live_entry_for_key, record_shadow_decision, shadow_jsonl_path, store_root_from_cache_path,
};
pub use store_schema::{
    SHADOW_JSONL_RING_CAP, STORE_SCHEMA_MAJOR, STORE_SCHEMA_MINOR, STORE_SCHEMA_NAME, SchemaAdmit,
    SchemaSkewError, StoreSchemaStamp, StoreSchemaVersion, admit_store_schema,
    admit_store_schema_against, append_shadow_jsonl, recover_actioncache_segment,
    write_actioncache_segment,
};

pub use crash_inject::{
    AFTER_JOURNAL_APPEND, AFTER_TMP_BEFORE_RENAME, AFTER_WAL_APPEND, ARM_ENV,
    BEFORE_PERSIST_UNREADABLE, BEFORE_PRUNE_UNREADABLE, maybe_crash,
};
pub use session_aliases::{
    SESSION_ALIAS_HEX_LEN, canonical_full_blob_ref, is_full_hash_blob_bare, is_session_alias_bare,
    is_session_ordinal_bare, parse_session_ordinal_bare, rewrite_full_hash_blob_refs_in_text,
    rewrite_full_hash_blob_refs_in_value, session_alias_hex_keyed, session_ordinal_ref,
    session_visible_blob_alias, session_visible_blob_alias_keyed, split_ref_fragment,
};
pub use store_hygiene::{
    BlobSidecarPruneReport, RecoveryBlobPruneReport, STALE_TMP_MAX_AGE, TmpSweepReport,
    prune_blob_sidecars, prune_recovery_blobs, recovery_blob_status, sweep_stale_tmp_files,
};

const LOCK_RETRIES: usize = 240;
const MAX_SHELL_OUTCOMES: usize = 256;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(25);
const TMP_RETRIES: usize = 16;
const REF_INDEX_MAX_BYTES: u64 = 1_048_576;
const REF_INDEX_READ_MAX_BYTES: usize = (REF_INDEX_MAX_BYTES as usize) * 16;
const REF_INDEX_COMPACT_EMERGENCY_MAX_BYTES: usize = REF_INDEX_READ_MAX_BYTES * 4;
// Two-character shards saturated in production, making every append recompact
// an irreducible multi-megabyte file. New writes use three characters while
// reads retain compatibility with the immutable two-character generation.
const REF_INDEX_SHARD_PREFIX_LEN: usize = 3;
const REF_INDEX_LEGACY_SHARD_PREFIX_LEN: usize = 2;
const REF_INDEX_DISABLE_ENV: &str = "TOKENZERO_REF_INDEX";
const REF_INDEX_PATH_ENV: &str = "TOKENZERO_REF_INDEX_PATH";

/// Profiling-only leaf spans for expand (TOKENZERO_PERF_PROFILE). No product effect when off.
fn expand_leaf_span<R>(span: &'static str, f: impl FnOnce() -> R) -> R {
    static ENABLED: AtomicU8 = AtomicU8::new(0);
    let on = match ENABLED.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let on = env::var("TOKENZERO_PERF_PROFILE")
                .ok()
                .as_deref()
                .map(|raw| {
                    matches!(
                        raw.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false);
            ENABLED.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    };
    if !on {
        return f();
    }
    let t0 = Instant::now();
    let out = f();
    let us = u64::try_from(t0.elapsed().as_micros()).unwrap_or(u64::MAX);
    let _ = writeln!(
        io::stderr(),
        r#"{{"event":"perf.profile.span_summary","span":"{span}","category":"expand.leaf","cumulative_us":{us},"count":1,"p50_us":{us},"p95_us":{us},"evidence":"recovery expand leaf Instant when TOKENZERO_PERF_PROFILE=1"}}"#
    );
    out
}

/// ZeroStack schemes accepted by expand. Full-hash portable blob refs first use the
/// configured canonical shared CAS. Legacy short blob refs retain a clearly separated
/// same-store alias tier when no shared object is available.
pub const EXPAND_REF_SCHEMES: &[&str] = &["tz://", "fz://", "gz://"];
const BLOB_REF_PREFIXES: &[(&str, &str)] = &[
    ("tz", "tz://blob/"),
    ("fz", "fz://blob/"),
    ("gz", "gz://blob/"),
];
fn blob_ref_scheme_hash(bare: &str) -> Option<(&str, &str)> {
    BLOB_REF_PREFIXES
        .iter()
        .find_map(|(scheme, prefix)| bare.strip_prefix(prefix).map(|hash| (*scheme, hash)))
}
fn blob_ref_hash(bare: &str) -> Option<&str> {
    blob_ref_scheme_hash(bare).map(|(_, hash)| hash)
}
fn is_foreign_blob_ref(ref_id: &str) -> bool {
    ref_id.starts_with("fz://blob/") || ref_id.starts_with("gz://blob/")
}
fn is_foreign_non_blob_ref(ref_id: &str) -> bool {
    (ref_id.starts_with("fz://") || ref_id.starts_with("gz://")) && blob_ref_hash(ref_id).is_none()
}

/// True when `ref_id` starts with a scheme expand can recover (`tz://`, `fz://`, `gz://`).
pub fn is_expandable_ref(ref_id: &str) -> bool {
    EXPAND_REF_SCHEMES
        .iter()
        .any(|scheme| ref_id.starts_with(scheme))
}

/// Rewrite portable `fz://blob/` / `gz://blob/` refs to `tz://blob/` for the
/// legacy same-store alias tier. Foreign non-blob refs remain engine-owned and
/// are rejected instead of being reinterpreted as TokenZero keys.
pub fn canonicalize_expand_ref(ref_id: &str) -> Option<String> {
    if ref_id.starts_with("tz://") {
        return Some(ref_id.to_string());
    }
    blob_ref_hash(ref_id).map(|hash| format!("tz://blob/{hash}"))
}

fn is_legacy_same_store_blob_ref(ref_id: &str) -> bool {
    let bare = ref_id.split_once('#').map_or(ref_id, |(bare, _)| bare);
    blob_ref_hash(bare).is_some_and(|hash| {
        hash.len() == 17
            && hash.starts_with('b')
            && hash[1..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Lazily compiled `file:line[:col]` matcher for search ingestion.
static SEARCH_PATH_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<path>(?:[\w.@+-]+/)+[\w.@+-]+):(?P<line>\d+):?(?P<col>\d+)?")
        .expect("SEARCH_PATH_LINE is a valid compile-time regex literal")
});
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

macro_rules! recovery_maps {
    (contains $s:expr, $id:expr) => {
        $s.blobs.contains_key($id)
            || $s.files.contains_key($id)
            || $s.units.contains_key($id)
            || $s.search_hits.contains_key($id)
    };
    (remove $s:expr, $id:expr) => {{
        $s.blobs.remove($id);
        $s.files.remove($id);
        $s.units.remove($id);
        $s.search_hits.remove($id);
    }};
    (keys $s:expr) => {
        $s.blobs
            .keys()
            .chain($s.files.keys())
            .chain($s.units.keys())
            .chain($s.search_hits.keys())
    };
    (copy $d:expr, $s:expr, $id:expr) => {{
        copy_map_entry(&mut $d.blobs, &$s.blobs, $id);
        copy_map_entry(&mut $d.files, &$s.files, $id);
        copy_map_entry(&mut $d.units, &$s.units, $id);
        copy_map_entry(&mut $d.search_hits, &$s.search_hits, $id);
    }};
    (merge $session:expr, $m:expr, $c:expr) => {{
        merge_map_entries($session, &mut $m.blobs, $c.blobs);
        merge_map_entries($session, &mut $m.files, $c.files);
        merge_map_entries($session, &mut $m.units, $c.units);
        merge_map_entries($session, &mut $m.search_hits, $c.search_hits);
    }};
    (evict $slf:expr) => {{
        evict_prefix(
            &mut $slf.state.blobs,
            &mut $slf.state.order,
            "tz://blob/",
            $slf.config.max_blobs,
        );
        evict_prefix(
            &mut $slf.state.files,
            &mut $slf.state.order,
            "tz://file/",
            $slf.config.max_files,
        );
        evict_prefix(
            &mut $slf.state.units,
            &mut $slf.state.order,
            "tz://unit/",
            $slf.config.max_units,
        );
        evict_prefix(
            &mut $slf.state.search_hits,
            &mut $slf.state.order,
            "tz://search/",
            $slf.config.max_search_hits,
        );
    }};
}

macro_rules! persist_after_deferred {
    ($name:ident, $deferred:ident ( $($arg:ident : $ty:ty),* $(,)? ) -> $ret:ty) => {
        pub fn $name(&mut self, $($arg : $ty),*) -> Result<$ret, RecoveryError> {
            let value = self.$deferred($($arg),*);
            self.persist_value(value)
        }
    };
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<SessionWalError> for RecoveryError {
    fn from(err: SessionWalError) -> Self {
        match err {
            SessionWalError::Io(err) => Self::Io(err),
            other => Self::Io(io::Error::new(io::ErrorKind::InvalidData, other)),
        }
    }
}

macro_rules! labeled_errors {
    ($name:ident { $($var:ident => $label:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $(
                #[error($label)]
                $var,
            )+
        }
    };
}

labeled_errors! { ZeroRefError {
    Malformed => "malformed", Unsupported => "unsupported", Missing => "missing", Io => "io",
    Corruption => "corruption", Policy => "policy", IncompatibleVersion => "incompatible_version",
    LegacyAmbiguity => "legacy_ambiguity",
}}

labeled_errors! { FragmentError {
    Malformed => "malformed", Reversed => "reversed", OutOfRange => "out_of_range",
    NonUtf8Line => "non_utf8_line_fragment", UnknownKind => "unknown_kind",
    DuplicateFragment => "duplicate_fragment",
}}

// Parsed byte or line fragment. Shared by every expand surface in this
// crate (RecoveryStore and the embedded TokenZeroStore) so the fragment
// grammar cannot diverge between dual-path expand stores.
pub(crate) enum FragmentSpec {
    /// Zero-based half-open byte range `start..end`.
    Byte { start: usize, end: usize },
    /// One-based inclusive line range `start..=end`.
    Line { start: usize, end: usize },
}

// Parse validated byte and line fragment bounds.
fn parse_fragment_bounds_core(
    value: &str,
    repeated_kind: char,
    allow_single: bool,
    require_nonzero_start: bool,
) -> Result<(usize, usize), bool> {
    if value.starts_with(repeated_kind) {
        return Err(false);
    }
    let separated = value.split_once(',').or_else(|| value.split_once('-'));
    let (start, end) = match separated {
        Some((start, end)) => (start, end),
        None if allow_single => (value, value),
        None => return Err(false),
    };
    let start = start
        .trim_start_matches(repeated_kind)
        .parse::<usize>()
        .map_err(|_| false)?;
    let end = end
        .trim_start_matches(repeated_kind)
        .parse::<usize>()
        .map_err(|_| false)?;
    if require_nonzero_start && start == 0 {
        return Err(false);
    }
    if start > end {
        return Err(true);
    }
    // `#Bn` is the single byte at n (half-open [n, n+1)). Empty selection
    // stays `#Bn-n`. Line `#Ln` is already the inclusive singleton [n, n].
    let end = if allow_single && separated.is_none() && repeated_kind == 'B' {
        start.checked_add(1).ok_or(false)?
    } else {
        end
    };
    Ok((start, end))
}

pub(crate) fn parse_fragment_spec(fragment: &str) -> Result<FragmentSpec, FragmentError> {
    if fragment.is_empty() {
        return Err(FragmentError::Malformed);
    }
    if fragment.contains('#') {
        return Err(FragmentError::DuplicateFragment);
    }
    let kind_byte = fragment.as_bytes()[0];
    if !kind_byte.is_ascii() {
        return Err(FragmentError::UnknownKind);
    }
    let kind = kind_byte as char;
    // Shared-contract legacy byte alias `B<start>+<len>`: the strict zero-ref
    // grammar accepts it, so every expand surface must too (bytes only).
    if kind == 'B'
        && let Some((start, len)) = fragment[1..].split_once('+')
    {
        let start = start
            .parse::<usize>()
            .map_err(|_| FragmentError::Malformed)?;
        let len = len.parse::<usize>().map_err(|_| FragmentError::Malformed)?;
        let end = start.checked_add(len).ok_or(FragmentError::Malformed)?;
        return Ok(FragmentSpec::Byte { start, end });
    }
    let map_err = |reversed| {
        if reversed {
            FragmentError::Reversed
        } else {
            FragmentError::Malformed
        }
    };
    let (start, end) =
        parse_fragment_bounds_core(&fragment[1..], kind, true, kind == 'L').map_err(map_err)?;
    match kind {
        'B' => Ok(FragmentSpec::Byte { start, end }),
        'L' => Ok(FragmentSpec::Line { start, end }),
        _ => Err(FragmentError::UnknownKind),
    }
}

/// Stable reason string for a [`FragmentError`] used in `ExpansionResult::reason`.
fn shared_cas_error_reason(err: SharedCasError) -> &'static str {
    match err {
        SharedCasError::Corruption => "shared-cas-corruption",
        SharedCasError::Policy => "shared-cas-policy",
        SharedCasError::Io(_) => "shared-cas-io",
        SharedCasError::InvalidHash(_) => "zeroref-malformed",
        SharedCasError::NotFound => "shared-cas-missing",
        SharedCasError::Gc(_) => "shared-cas-gc",
    }
}

/// `Err(None)` = non-UTF8 object bytes; `Err(Some(_))` = CAS error.
fn shared_cas_utf8(cas: &SharedCas, hash: &str) -> Result<String, Option<SharedCasError>> {
    match cas.resolve(hash) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| None),
        Err(err) => Err(Some(err)),
    }
}

pub(crate) fn fragment_error_reason(err: FragmentError) -> &'static str {
    match err {
        FragmentError::Malformed => "fragment-malformed",
        FragmentError::Reversed => "fragment-reversed",
        FragmentError::OutOfRange => "fragment-out-of-range",
        FragmentError::NonUtf8Line => "non_utf8_line_fragment",
        FragmentError::UnknownKind => "fragment-unknown-kind",
        FragmentError::DuplicateFragment => "fragment-duplicate",
    }
}

/// Parsed components of a ZeroRef v1 portable blob ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZeroRefBlob {
    pub scheme: String,
    pub hash: String,
    pub fragment: Option<ZeroRefFragment>,
}

/// Fragment selector for a ZeroRef v1 blob ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ZeroRefFragment {
    /// Zero-based half-open byte range `start..end`. `start == end` is allowed.
    Byte { start: usize, end: usize },
    /// One-based inclusive line range `start..=end`. Exact newline retention.
    Line { start: usize, end: usize },
}

/// Parse a portable `(tz|fz|gz)://blob/<sha256>` ZeroRef v1 reference.
/// Only full lowercase SHA-256 identities are accepted. Grammar is delegated
/// to the shared zero-ref foundation crate, which also accepts the contract's
/// legacy `#B<start>+<len>` alias; TokenZero's wider lenient fragment forms
/// (`#L2-L5`, comma separators, single line values) remain accepted as a
/// fallback. `#Bstart-end` is a zero-based half-open byte range;
/// `#Lstart-end` is a one-based inclusive line range. Legacy short IDs
/// return [`ZeroRefError::LegacyAmbiguity`].
pub fn parse_zeroref_v1_blob(
    ref_id: &str,
    byte_length: Option<usize>,
) -> Result<ZeroRefBlob, ZeroRefError> {
    let (bare, fragment) = ref_id
        .split_once('#')
        .map_or((ref_id, None), |(bare, fragment)| (bare, Some(fragment)));
    let (scheme, hash) = blob_ref_scheme_hash(bare).ok_or(ZeroRefError::Unsupported)?;
    if hash.is_empty() || hash.contains('/') {
        return Err(ZeroRefError::Malformed);
    }
    if hash.len() != 64 {
        // TokenZero-owned migration tier: short IDs fall through to the
        // legacy same-store alias resolution, not a hard parse error.
        return Err(ZeroRefError::LegacyAmbiguity);
    }
    if !zero_ref::is_full_lower_hex(hash) {
        return Err(ZeroRefError::Malformed);
    }
    let fragment = fragment
        .map(|frag| parse_portable_or_lenient_fragment(ref_id, frag))
        .transpose()?;
    if let (Some(ZeroRefFragment::Byte { end, .. }), Some(len)) = (&fragment, byte_length)
        && *end > len
    {
        return Err(ZeroRefError::Malformed);
    }
    Ok(ZeroRefBlob {
        scheme: scheme.to_string(),
        hash: hash.to_string(),
        fragment,
    })
}

/// Strict shared-contract fragment grammar first (which includes the legacy
/// `+` byte alias), then TokenZero's lenient forms as a fallback.
fn parse_portable_or_lenient_fragment(
    ref_id: &str,
    fragment: &str,
) -> Result<ZeroRefFragment, ZeroRefError> {
    if let Ok(parsed) = zero_ref::ZeroRef::parse(ref_id) {
        return match parsed.fragment {
            zero_ref::ZeroFragment::Bytes { start, end } => Ok(ZeroRefFragment::Byte {
                start: usize::try_from(start).map_err(|_| ZeroRefError::Malformed)?,
                end: usize::try_from(end).map_err(|_| ZeroRefError::Malformed)?,
            }),
            zero_ref::ZeroFragment::Lines { start, end } => Ok(ZeroRefFragment::Line {
                start: usize::try_from(start).map_err(|_| ZeroRefError::Malformed)?,
                end: usize::try_from(end).map_err(|_| ZeroRefError::Malformed)?,
            }),
            zero_ref::ZeroFragment::None => Err(ZeroRefError::Malformed),
        };
    }
    let (kind, value) = match fragment.as_bytes().first() {
        Some(&b'B') => ('B', &fragment[1..]),
        Some(&b'L') => ('L', &fragment[1..]),
        _ => return Err(ZeroRefError::Malformed),
    };
    // allow_single=true for both kinds, matching parse_fragment_spec:
    // `#B0` is byte 0 (`[0, 1)`); `#L1` is line 1.
    let (start, end) = parse_fragment_bounds_core(value, kind, true, kind == 'L')
        .map_err(|_| ZeroRefError::Malformed)?;
    match kind {
        'B' => Ok(ZeroRefFragment::Byte { start, end }),
        'L' => Ok(ZeroRefFragment::Line { start, end }),
        _ => Err(ZeroRefError::Malformed),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryConfig {
    pub max_blobs: usize,
    pub max_files: usize,
    pub max_units: usize,
    pub max_search_hits: usize,
    pub max_bytes: usize,
    pub max_load_bytes: usize,
    /// When true, legacy short-ref lookups resolve through the alias tier.
    /// When false, legacy short refs fail with a typed "legacy-ref-disabled" reason.
    #[serde(default = "default_legacy_compat")]
    pub legacy_compat: bool,
    /// Optional Unix timestamp after which legacy compatibility may be removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_compat_deadline: Option<u64>,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            max_blobs: 128,
            max_files: 256,
            max_units: 2048,
            max_search_hits: 1024,
            max_bytes: 8_000_000,
            max_load_bytes: 16_000_000,
            legacy_compat: true,
            legacy_compat_deadline: None,
        }
    }
}

impl RecoveryConfig {
    /// Fail loud when a load cap cannot round-trip a snapshot.
    pub fn validate(&self) -> Result<(), RecoveryError> {
        if self.max_load_bytes == 0 {
            return Err(invalid_input("RecoveryConfig.max_load_bytes must be nonzero").into());
        }
        Ok(())
    }
}

fn default_legacy_compat() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredFile {
    pub ref_id: String,
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_identity: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub source_backed: bool,
    pub text: String,
    pub content_type: String,
    pub source_fingerprint: Option<SourceFingerprint>,
    pub source_start_line: Option<usize>,
    pub source_end_line: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredUnit {
    pub ref_id: String,
    pub text: String,
    pub content_type: String,
    pub source_ref: Option<String>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFingerprint {
    pub size: u64,
    pub mtime_ns: u128,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredPayload {
    pub blob_ref: String,
    pub file_ref: String,
    pub unit_refs: Vec<String>,
    pub raw_tokens: usize,
    pub source_start_line: Option<usize>,
    pub source_end_line: Option<usize>,
}

#[derive(Debug, Clone)]
struct PayloadMemo {
    text: String,
    content_type: ContentType,
    path: Option<PathBuf>,
    source_start_line: Option<usize>,
    source_end_line: Option<usize>,
    source_backed: bool,
    stored: StoredPayload,
}

impl PayloadMemo {
    fn matches(
        &self,
        text: &str,
        content_type: ContentType,
        path: Option<&Path>,
        source_start_line: Option<usize>,
        source_end_line: Option<usize>,
        source_backed: bool,
    ) -> bool {
        self.text == text
            && self.content_type == content_type
            && self.path.as_deref() == path
            && self.source_start_line == source_start_line
            && self.source_end_line == source_end_line
            && self.source_backed == source_backed
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpansionResult {
    #[serde(rename = "ref")]
    pub ref_id: String,
    pub selector: Option<String>,
    pub content: String,
    pub tokens: usize,
    pub found: bool,
    pub reason: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub clamped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_start_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returned_end_line: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_count: Option<usize>,
}

impl ExpansionResult {
    pub fn ok(ref_id: String, selector: Option<String>, content: String) -> Self {
        let tokens = count_tokens(&content);
        Self::ok_with_tokens(ref_id, selector, content, tokens)
    }

    /// Like [`Self::ok`] but reuses a precomputed token count (expand success path).
    pub fn ok_with_tokens(
        ref_id: String,
        selector: Option<String>,
        content: String,
        tokens: usize,
    ) -> Self {
        Self {
            ref_id,
            selector,
            content,
            tokens,
            found: true,
            reason: "ok".to_string(),
            clamped: false,
            returned_start_line: None,
            returned_end_line: None,
            line_count: None,
        }
    }

    pub fn missing(ref_id: String, selector: Option<String>, reason: impl Into<String>) -> Self {
        Self {
            ref_id,
            selector,
            content: String::new(),
            tokens: 0,
            found: false,
            reason: reason.into(),
            clamped: false,
            returned_start_line: None,
            returned_end_line: None,
            line_count: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BlobEntry {
    /// Full text stored directly in recovery state. Legacy string-valued caches
    /// deserialize into this variant and serialize back to the same JSON shape.
    Inline(String),
    /// Pointer to an exact, one-based inclusive source line range.
    FileRef {
        path: PathBuf,
        source_start_line: usize,
        source_end_line: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct RecoveryState {
    pub version: u32,
    pub max_blobs: usize,
    pub max_files: usize,
    pub max_units: usize,
    pub max_search_hits: usize,
    pub max_bytes: usize,
    pub blobs: BTreeMap<String, BlobEntry>,
    pub files: BTreeMap<String, StoredFile>,
    pub units: BTreeMap<String, StoredUnit>,
    pub search_hits: BTreeMap<String, StoredUnit>,
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    #[serde(default = "initial_ordinal_generation")]
    pub ordinal_generation: u64,
    #[serde(default = "initial_next_ordinal")]
    pub next_ordinal: u64,
    pub order: Vec<String>,
    #[serde(default)]
    pub shell_outcomes: BTreeMap<String, ShellOutcome>,
    #[serde(default)]
    pub shell_outcome_seq: u64,
    /// Generation for `shell_outcome_seq`. Incremented on checked overflow so
    /// a rebased seq cannot look older than pre-rollover entries.
    #[serde(default)]
    pub shell_outcome_epoch: u64,
    /// Short refs whose 16-hex prefix maps to multiple distinct full hashes.
    #[serde(default)]
    pub ambiguous_aliases: BTreeSet<String>,
    /// Append-only audit commitment for acknowledged mint and alias-CAS mutations.
    #[serde(default)]
    pub transparency: crate::transparency::MmrLog,
    /// Hex-encoded 32-byte HMAC key for opaque session alias derivation
    /// (W4-OPAQUE-CAS-ALIAS). Generated lazily on first alias mint, persisted
    /// with the store so every engine sharing this store agrees on aliases.
    /// Internal-only: the key never appears in visible transcripts.
    #[serde(default)]
    pub alias_key: Option<String>,
}

// Capped shell-result index; blob payloads remain content-addressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ShellOutcome {
    pub combined_sha: String,
    pub exit_code: Option<i32>,
    pub seen: u32,
    pub seq: u64,
    #[serde(default)]
    pub epoch: u64,
}

/// Repeat verdict for the command just recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellRepeat {
    pub unchanged: bool,
    pub seen: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinalRange {
    pub generation: u64,
    pub start: u64,
    pub end_exclusive: u64,
}

impl OrdinalRange {
    pub fn len(self) -> u64 {
        self.end_exclusive.saturating_sub(self.start)
    }
    pub fn is_empty(self) -> bool {
        self.start == self.end_exclusive
    }
    pub fn ref_for(self, offset: u64) -> Option<String> {
        (offset < self.len()).then(|| session_ordinal_ref(self.generation, self.start + offset))
    }
}

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "PascalCase")]
pub enum ContentClass {
    SourceFile,
    Diff,
    ShellOutput,
    SearchHits,
    Doc,
    BinaryPreview,
    #[default]
    Unknown,
}

// Infer the ref-index content class.
fn classify_ref(ref_id: &str, content_type: Option<ContentType>) -> ContentClass {
    let Some(parsed) = parse_ref(ref_id) else {
        return ContentClass::Unknown;
    };
    match parsed.kind {
        "file" => ContentClass::SourceFile,
        "search" => ContentClass::SearchHits,
        "unit" => match content_type {
            Some(ContentType::Diff) => ContentClass::Diff,
            Some(ContentType::ShellOutput) => ContentClass::ShellOutput,
            _ => ContentClass::Unknown,
        },
        "blob" => match content_type {
            Some(ContentType::Code) => ContentClass::SourceFile,
            Some(ContentType::Diff) => ContentClass::Diff,
            Some(ContentType::ShellOutput) => ContentClass::ShellOutput,
            Some(ContentType::Markdown)
            | Some(ContentType::Logs)
            | Some(ContentType::Tree)
            | Some(ContentType::JsonConfig) => ContentClass::Doc,
            Some(ContentType::SearchResult) => ContentClass::SearchHits,
            _ => ContentClass::BinaryPreview,
        },
        _ => ContentClass::Unknown,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefIndexEntry {
    ref_id: String,
    store_path: String,
    ts: u128,
    #[serde(default)]
    content_class: ContentClass,
    #[serde(default)]
    expanded: bool,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    expansion_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_expanded_ts: Option<u128>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    metadata_migrated: bool,
    /// FNV-1a of the JSON without this field. Torn or truncated lines fail it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    commit: Option<u32>,
}

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

const fn initial_ordinal_generation() -> u64 {
    1
}
const fn initial_next_ordinal() -> u64 {
    1
}

fn ordinal_generation_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".ordinal-generation");
    PathBuf::from(value)
}

fn read_ordinal_generation(path: &Path) -> Result<u64, RecoveryError> {
    let sidecar = ordinal_generation_path(path);
    match fs::read_to_string(&sidecar) {
        Ok(value) => value.trim().parse::<u64>().map_err(|_| {
            invalid_data(format!(
                "ordinal generation sidecar is unreadable: {}",
                sidecar.display()
            ))
            .into()
        }),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err.into()),
    }
}

fn write_ordinal_generation(path: &Path, generation: u64) -> Result<(), RecoveryError> {
    let destination = ordinal_generation_path(path);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut collision = None;
    for _ in 0..TMP_RETRIES {
        let tmp = recovery_tmp_path(&destination);
        match create_private_new(&tmp) {
            Ok(mut file) => {
                let write_result = (|| {
                    writeln!(file, "{generation}")?;
                    file.sync_all()
                })();
                if let Err(error) = write_result {
                    let _ = fs::remove_file(&tmp);
                    return Err(error.into());
                }
                if let Err(error) = fs::rename(&tmp, &destination) {
                    let _ = fs::remove_file(&tmp);
                    return Err(error.into());
                }
                return Ok(());
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                collision = Some(error);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(collision
        .unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "ordinal generation temp collision",
            )
        })
        .into())
}

fn ensure_ordinal_generation_floor(path: &Path, generation: u64) -> Result<(), RecoveryError> {
    if read_ordinal_generation(path)? < generation {
        write_ordinal_generation(path, generation)?;
    }
    Ok(())
}

fn next_ordinal_generation(path: &Path) -> Result<u64, RecoveryError> {
    let current = read_ordinal_generation(path)?;
    // Generation one predates the durable sidecar. Never allocate it again, so
    // an upgraded cache cannot ABA-reuse a legacy ordinal after snapshot loss.
    let next = if current == 0 {
        2
    } else {
        current
            .checked_add(1)
            .ok_or_else(|| io::Error::other("ordinal generation counter exhausted"))?
    };
    write_ordinal_generation(path, next)?;
    Ok(next)
}

impl RecoveryState {
    pub(crate) fn empty(config: &RecoveryConfig) -> Self {
        Self {
            version: 1,
            max_blobs: config.max_blobs,
            max_files: config.max_files,
            max_units: config.max_units,
            max_search_hits: config.max_search_hits,
            max_bytes: config.max_bytes,
            blobs: BTreeMap::new(),
            files: BTreeMap::new(),
            units: BTreeMap::new(),
            search_hits: BTreeMap::new(),
            aliases: BTreeMap::new(),
            ordinal_generation: initial_ordinal_generation(),
            next_ordinal: initial_next_ordinal(),
            order: Vec::new(),
            shell_outcomes: BTreeMap::new(),
            shell_outcome_seq: 0,
            shell_outcome_epoch: 0,
            ambiguous_aliases: BTreeSet::new(),
            transparency: crate::transparency::MmrLog::default(),
            alias_key: None,
        }
    }

    fn configure(&mut self, config: &RecoveryConfig) {
        self.max_blobs = config.max_blobs;
        self.max_files = config.max_files;
        self.max_units = config.max_units;
        self.max_search_hits = config.max_search_hits;
        self.max_bytes = config.max_bytes;
    }
}

#[derive(Debug)]
pub struct RecoveryStore {
    pub config: RecoveryConfig,
    pub persistence_path: Option<PathBuf>,
    pub(crate) state: RecoveryState,
    session_refs: Vec<String>,
    /// Transient mapping from ref id to the content class inferred at store time.
    /// Used only to seed ref-index entries with a class before the state is persisted;
    /// it is not itself persisted and re-derives from `classify_ref` when absent.
    ref_classes: BTreeMap<String, ContentClass>,
    /// Identity of the cache file as last written by this store, captured
    /// while still holding the persist lock. `None` until the first persist;
    /// also reset to `None` whenever a write fails, so the next persist must
    /// take the full reload+merge path.
    disk_identity: Option<FileIdentity>,
    /// Identity of the session WAL sibling at our last write (`None` = we left
    /// no WAL). Checked together with `disk_identity`: a foreign append to the
    /// WAL must force the reload+merge path just like a foreign snapshot rewrite.
    journal_identity: Option<FileIdentity>,
    /// Canonical immutable store shared with FSZero/GraphZero. Attached only
    /// for unified `<store-root>/tokenzero/...` cache paths whose `blobs/`
    /// directory already exists.
    shared_cas: Option<SharedCas>,
    pub recovery_count: usize,
    /// Expand debit for this store instance. Not thread-local: a moved
    /// expand still charges the store that performed it (tokenzero-73yc).
    pub recovery_tokens: usize,
    /// Count of legacy short-ref lookups resolved via alias this session.
    pub legacy_read_count: usize,
    pub telemetry: CrossEngineTelemetry,
    /// Transient set of blob refs pending deletion. Applied by persist() and
    /// cleared only after successful authoritative snapshot write.
    pending_blob_deletions: BTreeSet<String>,
    /// Transient set of alias short refs pending deletion. Applied by
    /// persist() and cleared only after successful authoritative snapshot write.
    pending_alias_deletions: BTreeSet<String>,
    /// Last exact payload admitted by this engine. One entry bounds retained
    /// memory while covering the repeated MCP read/find hot path.
    payload_memo: Option<PayloadMemo>,
    /// A memo hit can make the immediately following persist provably empty.
    /// Any real ref mutation clears this flag through `remember_ref`.
    skip_empty_persist: bool,
    /// Construction saw an existing unreadable snapshot. Expand must not
    /// pretend the store is empty; persist re-checks disk and still refuses.
    unreadable_snapshot: bool,
    /// Hashes of blobs stored via `put_blob` since the last
    /// `publish_pending_cas` call. Tracked in-memory only; the finalizer's
    /// `commit()` publishes these post-durable-commit so CAS publication
    /// fsync barriers stay off the staging critical path (zerostack-5u7).
    pending_cas_hashes: BTreeSet<String>,
}

fn cache_identities(path: &Path) -> (Option<FileIdentity>, Option<FileIdentity>) {
    match SessionWal::new(path, SessionWalConfig::default()) {
        Ok(wal) => (wal.snapshot_identity(), wal.wal_identity()),
        Err(_) => (FileIdentity::capture(path), None),
    }
}

fn recovery_session_wal(path: &Path, config: &RecoveryConfig) -> Result<SessionWal, RecoveryError> {
    Ok(SessionWal::new(
        path,
        SessionWalConfig {
            max_replay_bytes: config.max_load_bytes as u64,
            ..SessionWalConfig::default()
        },
    )?)
}

fn snapshot_bytes(state: &RecoveryState) -> Result<Vec<u8>, RecoveryError> {
    let mut bytes = serde_json::to_vec(state)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RefResolve {
    Found(String),
    /// Content bytes already integrity-checked against lowercase hex `sha256`.
    FoundVerified {
        content: String,
        sha256: String,
    },
    NotFound,
    Stale,
    DecodeFailed,
}

impl RefResolve {
    fn verified_sha256(&self) -> Option<&str> {
        match self {
            Self::FoundVerified { sha256, .. } => Some(sha256.as_str()),
            _ => None,
        }
    }
}

/// Absorb fsync failures that mean "this filesystem cannot fsync", not
/// "this write failed".
///
/// Network mounts do not implement the durability primitives POSIX advertises.
/// On macOS SMB, `sync_all` on a DIRECTORY returns ENOTSUP (45), and on a file
/// opened read-only it returns EPERM (13) -- both verified against a live
/// smbfs mount. Propagating those aborted the entire CodeMode plan AFTER its
/// work had already run, so a read-only plan against a network-mounted repo
/// failed nondeterministically depending on whether a commit happened to be
/// triggered.
///
/// The data is already written and `persist()` has already succeeded; what is
/// lost is only the ORDERING guarantee against power loss, which the mount was
/// never able to provide. Refusing to proceed does not recover that guarantee,
/// it just denies service on filesystems that cannot offer it.
///
/// Deliberately narrow: only "the operation itself is unavailable here" codes
/// are absorbed. ENOSPC, EIO and friends still fail loudly, because those mean
/// the write really is in doubt.
pub(crate) fn tolerate_unsupported_sync(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(err) if sync_unsupported(&err) => Ok(()),
        other => other,
    }
}

fn sync_unsupported(err: &io::Error) -> bool {
    if matches!(
        err.kind(),
        io::ErrorKind::Unsupported | io::ErrorKind::PermissionDenied
    ) {
        return true;
    }
    // ErrorKind::Unsupported does not cover every platform spelling: macOS
    // smbfs reports raw ENOTSUP/EOPNOTSUPP as Uncategorized, which no stable
    // ErrorKind matches. Match the raw codes so the check works there.
    // The numeric values differ per platform (ENOTSUP is 45 on macOS but 95 on
    // Linux), so they must be spelled per target rather than hardcoded once.
    #[cfg(target_vendor = "apple")]
    const UNSUPPORTED_CODES: &[i32] = &[45, 102]; // ENOTSUP, EOPNOTSUPP
    #[cfg(all(unix, not(target_vendor = "apple")))]
    const UNSUPPORTED_CODES: &[i32] = &[95, 524]; // EOPNOTSUPP, ENOTSUP
    #[cfg(not(unix))]
    const UNSUPPORTED_CODES: &[i32] = &[];

    err.raw_os_error()
        .is_some_and(|code| UNSUPPORTED_CODES.contains(&code))
}

impl RecoveryStore {
    pub fn new(persistence_path: Option<PathBuf>) -> Self {
        Self::with_config(persistence_path, RecoveryConfig::default())
    }

    pub fn with_config(persistence_path: Option<PathBuf>, config: RecoveryConfig) -> Self {
        let (loaded, unreadable_snapshot) = match persistence_path.as_ref() {
            Some(path) => match load_state_if_present(path, &config) {
                Ok(state) => (state, false),
                Err(_) => (None, true),
            },
            None => (None, false),
        };
        let (disk_identity, journal_identity) = if unreadable_snapshot {
            (None, None)
        } else {
            loaded
                .as_ref()
                .and(persistence_path.as_deref())
                .map(cache_identities)
                .unwrap_or_default()
        };
        let state = loaded.unwrap_or_else(|| RecoveryState::empty(&config));
        let shared_cas = persistence_path
            .as_deref()
            .map(SharedCas::attach_for_cache_path);
        Self {
            config,
            persistence_path,
            state,
            session_refs: Vec::new(),
            ref_classes: BTreeMap::new(),
            disk_identity,
            journal_identity,
            shared_cas,
            recovery_count: 0,
            recovery_tokens: 0,
            legacy_read_count: 0,
            telemetry: CrossEngineTelemetry::default(),
            pending_blob_deletions: BTreeSet::new(),
            pending_alias_deletions: BTreeSet::new(),
            payload_memo: None,
            skip_empty_persist: false,
            unreadable_snapshot,
            pending_cas_hashes: BTreeSet::new(),
        }
    }

    /// Persist exact bytes as a durable content-addressed blob without
    /// creating file/unit index entries. Used when prompt spans are paged out.
    pub fn store_blob(
        &mut self,
        text: &str,
        content_type: ContentType,
    ) -> Result<String, RecoveryError> {
        let ref_id = self.put_blob(text, content_type);
        self.persist_evicted(ref_id)
    }

    /// Stage exact bytes in the current recovery transaction. Callers batching
    /// related blobs and indexes must finish with `persist_pending()`.
    pub fn store_blob_deferred(&mut self, text: &str, content_type: ContentType) -> String {
        self.put_blob(text, content_type)
    }

    /// Persist a blob as a pointer to an exact source-file line range.
    ///
    /// This is an explicit opt-in path; ordinary blob writers remain inline so
    /// ephemeral stdin, shell, and slice content survives source deletion.
    pub fn store_file_backed_blob(
        &mut self,
        path: &Path,
        source_start_line: usize,
        source_end_line: usize,
        content_type: ContentType,
    ) -> Result<String, RecoveryError> {
        refuse_unexpanded_tilde_store_path(path)?;
        let source = fs::read_to_string(path)?;
        let line_count = content_line_count(&source);
        if line_range_out_of_bounds(source_start_line, source_end_line, line_count) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid source line range {source_start_line}..={source_end_line}; file has {line_count} lines"),
            )
            .into());
        }
        let text = line_slice_exact(&source, source_start_line, source_end_line);
        let ref_id = self.put_file_backed_blob(
            &text,
            path,
            source_start_line,
            source_end_line,
            content_type,
        );
        self.persist_evicted(ref_id)
    }

    persist_after_deferred!(
        store_payload,
        store_payload_deferred(
            text: &str,
            content_type: ContentType,
            path: Option<&Path>,
            source_start_line: Option<usize>,
            source_end_line: Option<usize>,
        ) -> StoredPayload
    );

    pub fn store_payload_deferred(
        &mut self,
        text: &str,
        content_type: ContentType,
        path: Option<&Path>,
        source_start_line: Option<usize>,
        source_end_line: Option<usize>,
    ) -> StoredPayload {
        let stored = self.store_payload_deferred_batch(
            text,
            content_type,
            path,
            source_start_line,
            source_end_line,
        );
        if !self.skip_empty_persist {
            self.evict();
        }
        stored
    }

    pub fn store_payload_deferred_batch(
        &mut self,
        text: &str,
        content_type: ContentType,
        path: Option<&Path>,
        source_start_line: Option<usize>,
        source_end_line: Option<usize>,
    ) -> StoredPayload {
        if let Some(stored) = self.admit_memoized_payload(
            text,
            content_type,
            path,
            source_start_line,
            source_end_line,
            false,
        ) {
            return stored;
        }
        let blob_ref = self.put_blob(text, content_type);
        let file_ref = self.put_file(text, content_type, path, source_start_line, source_end_line);
        let stored = self.finish_payload(
            blob_ref,
            file_ref,
            text,
            content_type,
            source_start_line,
            source_end_line,
        );
        self.memoize_payload(
            text,
            content_type,
            path,
            (source_start_line, source_end_line),
            false,
            &stored,
        );
        stored
    }

    /// Admit an already-read complete source file without duplicating its payload.
    pub fn store_source_backed_payload_deferred_batch(
        &mut self,
        text: &str,
        content_type: ContentType,
        path: &Path,
    ) -> StoredPayload {
        if let Some(stored) =
            self.admit_memoized_payload(text, content_type, Some(path), None, None, true)
        {
            return stored;
        }
        let source_sha256 = sha256_hex(text);
        let blob_ref = self.put_file_backed_blob_hashed(
            path,
            1,
            content_line_count(text),
            content_type,
            &source_sha256,
        );
        let file_ref = self.put_source_backed_file(text, content_type, path, &source_sha256);
        let stored = self.finish_payload(blob_ref, file_ref, text, content_type, None, None);
        self.memoize_payload(text, content_type, Some(path), (None, None), true, &stored);
        stored
    }

    fn memoized_payload(
        &self,
        text: &str,
        content_type: ContentType,
        path: Option<&Path>,
        source_start_line: Option<usize>,
        source_end_line: Option<usize>,
        source_backed: bool,
    ) -> Option<StoredPayload> {
        let memo = self
            .payload_memo
            .as_ref()?
            .matches(
                text,
                content_type,
                path,
                source_start_line,
                source_end_line,
                source_backed,
            )
            .then_some(self.payload_memo.as_ref()?)?;
        let refs_live = self.state.files.contains_key(&memo.stored.file_ref)
            && memo
                .stored
                .unit_refs
                .iter()
                .all(|ref_id| self.state.units.contains_key(ref_id))
            && self.has_ref(&memo.stored.blob_ref);
        refs_live.then(|| memo.stored.clone())
    }

    fn admit_memoized_payload(
        &mut self,
        text: &str,
        content_type: ContentType,
        path: Option<&Path>,
        source_start_line: Option<usize>,
        source_end_line: Option<usize>,
        source_backed: bool,
    ) -> Option<StoredPayload> {
        let stored = self.memoized_payload(
            text,
            content_type,
            path,
            source_start_line,
            source_end_line,
            source_backed,
        )?;
        self.skip_empty_persist = true;
        Some(stored)
    }

    fn memoize_payload(
        &mut self,
        text: &str,
        content_type: ContentType,
        path: Option<&Path>,
        source_lines: (Option<usize>, Option<usize>),
        source_backed: bool,
        stored: &StoredPayload,
    ) {
        let (source_start_line, source_end_line) = source_lines;
        self.payload_memo = Some(PayloadMemo {
            text: text.to_owned(),
            content_type,
            path: path.map(Path::to_path_buf),
            source_start_line,
            source_end_line,
            source_backed,
            stored: stored.clone(),
        });
    }

    fn finish_payload(
        &mut self,
        blob_ref: String,
        file_ref: String,
        text: &str,
        content_type: ContentType,
        source_start_line: Option<usize>,
        source_end_line: Option<usize>,
    ) -> StoredPayload {
        let unit_refs = self.index_units(text, content_type, &file_ref);
        StoredPayload {
            blob_ref,
            file_ref,
            unit_refs,
            raw_tokens: count_tokens(text),
            source_start_line,
            source_end_line,
        }
    }

    fn persist_value<T>(&mut self, value: T) -> Result<T, RecoveryError> {
        self.persist()?;
        Ok(value)
    }

    fn persist_evicted<T>(&mut self, value: T) -> Result<T, RecoveryError> {
        self.evict();
        self.persist_value(value)
    }

    pub fn persist_pending(&mut self) -> Result<(), RecoveryError> {
        self.persist()
    }

    /// Publish all deferred mutations as one recovery entry and make that
    /// publication durable before returning to a caller that will acknowledge it.
    ///
    /// Hub `SessionWal::publish_snapshot` owns the snapshot rewrite + WAL
    /// retirement. `SyncPolicy::Required` is tried first; mounts that cannot
    /// fsync fall back to `TolerateUnsupported`.
    pub fn persist_pending_durable(&mut self) -> Result<(), RecoveryError> {
        let Some(path) = self.persistence_path.clone() else {
            return self.persist();
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Hold PersistLock across merge/write and the durability republish.
        // Dropping it after persist() let prune/GC observe a snapshot that
        // this method then overwrote without a lock.
        let _lock = PersistLock::acquire(recovery_lock_path(&path))?;
        self.persist_assuming_locked()?;
        let bytes = snapshot_bytes(&self.state)?;
        let mut cfg = SessionWalConfig {
            max_replay_bytes: self.config.max_load_bytes as u64,
            publish_sync: SyncPolicy::Required,
            ..SessionWalConfig::default()
        };
        match SessionWal::new(&path, cfg)?.publish_snapshot(&bytes) {
            Ok(()) => {
                self.journal_identity = None;
                self.disk_identity = FileIdentity::capture(&path);
                Ok(())
            }
            Err(SessionWalError::Io(err)) if sync_unsupported(&err) => {
                cfg.publish_sync = SyncPolicy::TolerateUnsupported;
                SessionWal::new(&path, cfg)?.publish_snapshot(&bytes)?;
                self.journal_identity = None;
                self.disk_identity = FileIdentity::capture(&path);
                self.sync_durable_publication()
            }
            Err(err) => Err(err.into()),
        }
    }

    fn sync_durable_publication(&self) -> Result<(), RecoveryError> {
        let Some(path) = self.persistence_path.as_deref() else {
            return Ok(());
        };
        let wal = recovery_session_wal(path, &self.config)?;
        let wal_path = wal.wal_path();
        let published = if wal_path.exists() {
            wal_path.as_path()
        } else {
            path
        };
        Self::sync_published_file(published)?;
        Self::sync_published_directory(path)
    }

    fn sync_published_file(published: &Path) -> Result<(), RecoveryError> {
        if published.exists() {
            tolerate_unsupported_sync(fs::File::open(published)?.sync_all())?;
        }
        Ok(())
    }

    fn sync_published_directory(path: &Path) -> Result<(), RecoveryError> {
        #[cfg(unix)]
        {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            tolerate_unsupported_sync(fs::File::open(parent)?.sync_all())?;
        }
        #[cfg(not(unix))]
        let _ = path;
        Ok(())
    }

    /// Publish pending inline blobs to hub CAS after the recovery root is
    /// durably committed (zerostack-5u7 / tokenzero-cas-fsync-ovn).
    ///
    /// `put_blob` keeps bodies inline until this call. Every successful
    /// publish lands the object in CAS (full-hash expand still needs that).
    /// Only blobs ≥ [`BLOB_EXTERNALIZE_MIN_BYTES`] replace the snapshot
    /// inline with a `\0tzx:v1:` marker so the root no longer carries
    /// megabytes. Smaller bodies stay inline (crash authority). A failed
    /// publish leaves that blob inline and is reported.
    pub fn publish_pending_cas(&mut self) -> Result<(), RecoveryError> {
        let Some(cas) = self.shared_cas.clone() else {
            self.pending_cas_hashes.clear();
            return Ok(());
        };
        if self.pending_cas_hashes.is_empty() {
            return Ok(());
        }
        let path = self.persistence_path.clone();
        // PersistLock first so prune cannot rewrite the snapshot between CAS
        // publication and marker commit. Leased publish then covers the GC
        // window until the snapshot names the object.
        let _lock = match path.as_ref() {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                Some(PersistLock::acquire(recovery_lock_path(path))?)
            }
            None => None,
        };
        self.publish_pending_cas_locked(&cas)
    }

    fn inline_blob_len(&self, hash: &str) -> Option<usize> {
        match self.state.blobs.get(&format!("tz://blob/{hash}")) {
            Some(BlobEntry::Inline(text)) if !text.starts_with(BLOB_MARKER_PREFIX) => {
                Some(text.len())
            }
            _ => None,
        }
    }

    /// CAS-publish pending blobs ≥ [`BLOB_EXTERNALIZE_MIN_BYTES`] and marker
    /// them. Small hashes stay queued for an explicit [`publish_pending_cas`]
    /// (TokenZeroStore full-hash expand still needs CAS for tiny descriptors).
    /// WAL/snapshot is already crash authority; CAS miss must not look like
    /// persist failure.
    fn externalize_large_pending_cas_locked(&mut self) {
        let Some(cas) = self.shared_cas.clone() else {
            return;
        };
        let small: Vec<String> = self
            .pending_cas_hashes
            .iter()
            .filter(|hash| {
                self.inline_blob_len(hash)
                    .is_none_or(|len| len < BLOB_EXTERNALIZE_MIN_BYTES)
            })
            .cloned()
            .collect();
        if small.len() == self.pending_cas_hashes.len() {
            return;
        }
        for hash in &small {
            self.pending_cas_hashes.remove(hash);
        }
        let _ = self.publish_pending_cas_locked(&cas);
        self.pending_cas_hashes.extend(small);
    }

    fn publish_pending_cas_locked(&mut self, cas: &SharedCas) -> Result<(), RecoveryError> {
        let hashes: Vec<String> = std::mem::take(&mut self.pending_cas_hashes)
            .into_iter()
            .collect();
        if hashes.is_empty() {
            return Ok(());
        }
        let path = self.persistence_path.clone();
        let project = crate::shared_cas::project_id(cas.root())
            .map_err(|err| io::Error::other(format!("CAS project id for leased publish: {err}")))?;
        let mut failed = 0usize;
        let mut shrunk = false;
        let mut leased_ops: Vec<String> = Vec::new();
        for hash in hashes {
            let ref_id = format!("tz://blob/{hash}");
            let published_len = match self.state.blobs.get(&ref_id) {
                Some(BlobEntry::Inline(text)) if !text.starts_with(BLOB_MARKER_PREFIX) => {
                    match cas.publish_leased(text.as_bytes(), &project, &hash, 300) {
                        Ok(_) => {
                            leased_ops.push(hash.clone());
                            Some(text.len())
                        }
                        Err(_) => {
                            failed = failed.saturating_add(1);
                            self.pending_cas_hashes.insert(hash.clone());
                            None
                        }
                    }
                }
                _ => None,
            };
            let Some(len) = published_len else {
                continue;
            };
            if len < BLOB_EXTERNALIZE_MIN_BYTES {
                continue;
            }
            self.state
                .blobs
                .insert(ref_id, BlobEntry::Inline(blob_cas_marker(&hash, len)));
            shrunk = true;
        }
        if shrunk && let Some(path) = path.as_ref() {
            // persist() can journal-append an empty session delta and leave
            // the snapshot carrying the old inline bodies. Rewrite the root.
            // Keep leases on persist failure so GC cannot collect objects
            // the in-memory markers still name.
            if let Err(err) = self.publish_snapshot(path) {
                return Err(err);
            }
        }
        if path.is_some() {
            let mut release_err = None;
            for op in &leased_ops {
                if let Err(err) = cas.release_lease(&project, op) {
                    release_err = release_err.or(Some((op.clone(), err)));
                }
            }
            if let Some((op, err)) = release_err {
                return Err(
                    io::Error::other(format!("release CAS publish lease {op}: {err}")).into(),
                );
            }
        }
        if failed > 0 {
            return Err(io::Error::other(format!(
                "publish_pending_cas failed for {failed} blob(s)"
            ))
            .into());
        }
        Ok(())
    }

    /// Non-blocking variant of [`publish_pending_cas`]: extracts the pending
    /// blob texts and publishes them on a background thread. Does not shrink
    /// the snapshot (the store is not `Send`); only the sync path replaces
    /// published inlines with markers.
    pub fn publish_pending_cas_background(&mut self) {
        let Some(cas) = self.shared_cas.clone() else {
            self.pending_cas_hashes.clear();
            return;
        };
        let hashes: Vec<String> = std::mem::take(&mut self.pending_cas_hashes)
            .into_iter()
            .collect();
        // Collect blob texts before spawning: RecoveryState is not Send, but
        // the extracted String values are.
        let mut entries = Vec::new();
        for hash in &hashes {
            let ref_id = format!("tz://blob/{hash}");
            if let Some(BlobEntry::Inline(text)) = self.state.blobs.get(&ref_id)
                && !text.starts_with(BLOB_MARKER_PREFIX)
            {
                entries.push(text.clone());
            }
        }
        if !entries.is_empty() {
            std::thread::Builder::new()
                .name("tz-cas-publish".into())
                .spawn(move || {
                    for text in entries {
                        let _ = cas.publish(text.as_bytes());
                    }
                })
                .ok();
        }
    }

    pub fn reserve_ordinal_range(&mut self, count: u64) -> Result<OrdinalRange, RecoveryError> {
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ordinal range must be non-empty",
            )
            .into());
        }
        let Some(path) = self.persistence_path.clone() else {
            let start = self.state.next_ordinal;
            let end_exclusive = start
                .checked_add(count)
                .ok_or_else(|| io::Error::other("ordinal counter overflow"))?;
            self.state.next_ordinal = end_exclusive;
            return Ok(OrdinalRange {
                generation: self.state.ordinal_generation,
                start,
                end_exclusive,
            });
        };
        refuse_unexpanded_tilde_store_path(&path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _lock = PersistLock::acquire(recovery_lock_path(&path))?;
        let existing = match load_state_if_present(&path, &self.config)? {
            Some(existing) => {
                ensure_ordinal_generation_floor(&path, existing.ordinal_generation)?;
                existing
            }
            None => {
                let generation = next_ordinal_generation(&path)?;
                self.state.ordinal_generation = generation;
                self.state.next_ordinal = initial_next_ordinal();
                let mut empty = RecoveryState::empty(&self.config);
                empty.ordinal_generation = generation;
                empty
            }
        };
        let current = std::mem::replace(&mut self.state, RecoveryState::empty(&self.config));
        self.state = merge_states(existing, current, &self.session_refs, &self.config);
        let start = self.state.next_ordinal;
        let end_exclusive = start
            .checked_add(count)
            .ok_or_else(|| io::Error::other("ordinal counter overflow"))?;
        let range = OrdinalRange {
            generation: self.state.ordinal_generation,
            start,
            end_exclusive,
        };
        self.state.next_ordinal = end_exclusive;
        self.publish_snapshot(&path)?;
        Ok(range)
    }

    pub fn store_ordinal_alias_deferred(
        &mut self,
        range: OrdinalRange,
        offset: u64,
        target_ref: &str,
    ) -> Result<String, RecoveryError> {
        let alias = range.ref_for(offset).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "ordinal offset outside reserved range",
            )
        })?;
        let target =
            canonical_full_blob_ref(split_ref_fragment(target_ref).0).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "ordinal target must be a full-hash blob ref",
                )
            })?;
        if self
            .state
            .aliases
            .get(&alias)
            .is_some_and(|existing| existing != &target)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "ordinal alias already targets another ref",
            )
            .into());
        }
        self.store_alias_deferred(&alias, &target);
        Ok(alias)
    }

    pub fn store_alias(&mut self, alias: &str, target_ref: &str) -> Result<(), RecoveryError> {
        if alias.is_empty() || target_ref.is_empty() {
            return Err(invalid_input("alias and target ref must be non-empty").into());
        }
        self.store_alias_deferred(alias, target_ref);
        self.persist()
    }

    /// Store an alias without persisting. Caller must call `persist_pending()`.
    ///
    /// A conflicting target never replaces the first mapping. The alias is
    /// marked ambiguous so later expansion fails loudly instead.
    pub fn store_alias_deferred(&mut self, alias: &str, target_ref: &str) {
        if alias.is_empty() || target_ref.is_empty() {
            return;
        }
        self.skip_empty_persist = false;
        match self.state.aliases.get(alias) {
            Some(current) if current == target_ref => return,
            Some(_) => {
                self.state.ambiguous_aliases.insert(alias.to_string());
                return;
            }
            None => {}
        }
        self.state
            .transparency
            .append(format!("alias-cas\0{alias}\0{target_ref}").as_bytes());
        self.state
            .aliases
            .insert(alias.to_string(), target_ref.to_string());
    }

    /// Current MMR transparency commitment for recovery mutations.
    pub fn transparency_root(&self) -> String {
        self.state.transparency.root()
    }
    pub fn transparency_len(&self) -> usize {
        self.state.transparency.len()
    }
    pub fn transparency_inclusion_proof(
        &self,
        leaf_index: usize,
    ) -> Option<crate::transparency::InclusionProof> {
        self.state.transparency.inclusion_proof(leaf_index)
    }
    pub fn transparency_consistency_proof(
        &self,
        old_size: usize,
    ) -> Option<crate::transparency::ConsistencyProof> {
        self.state.transparency.consistency_proof(old_size)
    }

    /// Remove an alias after the next authoritative persist.
    pub(crate) fn remove_alias(&mut self, alias: &str) {
        self.skip_empty_persist = false;
        self.state.aliases.remove(alias);
        self.state.ambiguous_aliases.remove(alias);
        self.pending_alias_deletions.insert(alias.to_string());
    }

    /// Remove a blob after the next authoritative persist.
    pub(crate) fn remove_blob(&mut self, ref_id: &str) {
        self.skip_empty_persist = false;
        self.state.blobs.remove(ref_id);
        self.pending_blob_deletions.insert(ref_id.to_string());
    }

    /// Mark a short ref as ambiguous (maps to multiple full hashes).
    pub fn mark_ambiguous(&mut self, short_ref: &str) {
        self.skip_empty_persist = false;
        self.state.ambiguous_aliases.insert(short_ref.to_string());
    }

    /// Check whether a short ref has been marked as ambiguous.
    pub fn is_alias_ambiguous(&self, short_ref: &str) -> bool {
        self.state.ambiguous_aliases.contains(short_ref)
    }

    /// Return the target ref for an existing alias, if any.
    pub fn alias_target(&self, alias: &str) -> Option<String> {
        self.state.aliases.get(alias).cloned()
    }

    /// Return all blob ref IDs currently in the store (for migration scanning).
    pub fn blob_ref_ids(&self) -> Vec<String> {
        self.state.blobs.keys().cloned().collect()
    }

    /// Resolve a blob's content by its full ref ID.
    /// Returns None if not found or if the stored value cannot be resolved.
    pub(crate) fn resolve_blob_content(&self, ref_id: &str) -> Option<String> {
        self.state.blobs.get(ref_id).and_then(|value| {
            match resolve_blob_value(self.persistence_path.as_deref(), ref_id, value) {
                RefResolve::Found(content) | RefResolve::FoundVerified { content, .. } => {
                    Some(content)
                }
                RefResolve::NotFound | RefResolve::Stale | RefResolve::DecodeFailed => None,
            }
        })
    }

    // Resolve foreign blobs from a sibling engine store under the same root.
    fn expand_in_sibling_engine_store(
        &self,
        requested_ref: &str,
        selector: Option<&str>,
        start_line: Option<usize>,
        end_line: Option<usize>,
        anchor_kind: Option<&str>,
        symbol: Option<&str>,
    ) -> Option<ExpansionResult> {
        let engine = match requested_ref.split_once("://")?.0 {
            "fz" => "fszero",
            "gz" => "graphzero",
            _ => return None,
        };
        let self_cache = self.persistence_path.as_deref()?;
        let sibling_cache = SharedCas::sibling_engine_cache_path(self_cache, engine)?;
        if sibling_cache == self_cache || !sibling_cache.is_file() {
            return None;
        }
        let canonical = canonicalize_expand_ref(requested_ref)?;
        let mut sibling_store = RecoveryStore::new(Some(sibling_cache));
        let result = sibling_store.expand(
            &canonical,
            selector,
            start_line,
            end_line,
            anchor_kind,
            symbol,
        );
        result.found.then(|| {
            ExpansionResult::ok(requested_ref.to_string(), result.selector, result.content)
        })
    }

    /// Return migration/compatibility state for doctor JSON output.
    /// Contains no payload content or filesystem paths.
    pub fn migration_state(&self) -> serde_json::Value {
        serde_json::json!({
            "legacy_compat_enabled": self.config.legacy_compat,
            "legacy_compat_deadline": self.config.legacy_compat_deadline,
            "legacy_compat_supported_until": "tokenzero-v2.0",
            "legacy_blob_count": self.state.blobs.keys()
                .filter(|k| crate::migration::is_legacy_blob_ref(k))
                .count(),
            "canonical_blob_count": self.state.blobs.keys()
                .filter(|k| k.starts_with("tz://blob/") && k.len() == 74)
                .count(),
            "alias_count": self.state.aliases.len(),
            "ambiguous_alias_count": self.state.ambiguous_aliases.len(),
            "shared_cas_attached": self.shared_cas.is_some(),
            "legacy_read_count_session": self.legacy_read_count,
        })
    }
    pub fn expected_refs(text: &str, path: Option<&Path>) -> (String, String) {
        let blob_ref = format!("tz://blob/{}", sha256_hex(text));
        let file_ref = recovery_file_ref(text, path);
        (blob_ref, file_ref)
    }

    persist_after_deferred!(
        store_search_output,
        store_search_output_deferred(output: &str, query: Option<&str>) -> Vec<String>
    );

    pub fn store_search_output_deferred(
        &mut self,
        output: &str,
        query: Option<&str>,
    ) -> Vec<String> {
        let path_line = &*SEARCH_PATH_LINE;
        let mut refs = Vec::new();
        for (idx, line) in output.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if query.is_some_and(|q| !line.contains(q)) && !path_line.is_match(line) {
                continue;
            }
            let hit_id = id_for('h', &format!("search:{idx}:{line}"));
            let ref_id = format!("tz://search/{hit_id}");
            refs.push(self.insert_stored_unit(
                true,
                ref_id,
                line,
                ContentType::SearchResult,
                None,
                (Some(idx + 1), Some(idx + 1)),
            ));
        }
        self.evict();
        refs
    }

    pub fn expand(
        // Validate routing and fragments before resolving CAS/local content and selectors.
        &mut self,
        ref_id: &str,
        selector: Option<&str>,
        start_line: Option<usize>,
        end_line: Option<usize>,
        anchor_kind: Option<&str>,
        symbol: Option<&str>,
    ) -> ExpansionResult {
        self.recovery_count += 1;
        let requested_ref = ref_id.to_string();
        let selector_owned = selector.map(str::to_string);
        macro_rules! miss {
            ($reason:expr) => {
                ExpansionResult::missing(requested_ref.clone(), selector_owned.clone(), $reason)
            };
        }
        // Unreadable snapshot is not an empty store: persist refuses overwrite;
        // expand must not look like a clean miss.
        if self.unreadable_snapshot {
            return miss!("unreadable-snapshot");
        }
        let early_fragment = ref_id.split_once('#').map(|(_, fragment)| fragment);
        let early_fragment_spec = match early_fragment.map(parse_fragment_spec).transpose() {
            Err(err) => return miss!(fragment_error_reason(err)),
            Ok(spec) => spec,
        };
        let portable = match parse_expand_portable(ref_id) {
            Ok(parsed) => parsed,
            Err(reason) => return miss!(reason),
        };
        if portable.is_none() && is_foreign_non_blob_ref(ref_id) {
            return miss!("unsupported-ref-kind");
        }
        let Some(lookup_ref) = canonicalize_expand_ref(ref_id) else {
            return miss!("invalid-ref");
        };
        if let Some(reason) = self.note_legacy_expand(&lookup_ref) {
            return miss!(reason);
        }
        let ordinal_bare = split_ref_fragment(&lookup_ref).0;
        if self.state.ambiguous_aliases.contains(ordinal_bare) {
            return miss!("ambiguous-alias");
        }
        let requested_alias = self.state.aliases.contains_key(ordinal_bare);
        if let Some((generation, _)) = parse_session_ordinal_bare(ordinal_bare) {
            if generation != self.state.ordinal_generation {
                return miss!("stale-ref");
            }
            if !self.state.aliases.contains_key(ordinal_bare) {
                return miss!("dangling-ref");
            }
        }
        let resolved_ref = self.resolve_alias_chain(&lookup_ref).unwrap_or(lookup_ref);
        let portable_resolved = parse_zeroref_v1_blob(&resolved_ref, None).ok();
        let shared_content = match (&portable_resolved, &self.shared_cas) {
            (Some(portable), Some(cas)) => {
                let hash = &portable.hash;
                match expand_leaf_span("expand.leaf.shared_cas_utf8", || shared_cas_utf8(cas, hash))
                {
                    Ok(content) => Some(content),
                    Err(None) => return miss!("shared-cas-non-utf8"),
                    Err(Some(SharedCasError::NotFound)) => {
                        if !requested_ref.starts_with("tz://")
                            && let Some(result) = self.expand_in_sibling_engine_store(
                                &requested_ref,
                                selector,
                                start_line,
                                end_line,
                                anchor_kind,
                                symbol,
                            )
                        {
                            return result;
                        }
                        // Same-store fz/gz aliases and unpublished inline
                        // bodies still live in the recovery snapshot.
                        None
                    }
                    Err(Some(err)) => return miss!(shared_cas_error_reason(err)),
                }
            }
            _ => None,
        };
        let ref_id = resolved_ref;
        let Some(parsed) = parse_ref(&ref_id) else {
            return miss!("invalid-ref");
        };
        let mut selected_start = start_line;
        let mut selected_end = end_line;
        // Reuse the early parse when the resolved ref kept the same fragment text.
        let fragment_spec = match (early_fragment, early_fragment_spec, parsed.fragment) {
            (Some(early), Some(spec), Some(pf)) if early == pf => Some(Ok(spec)),
            (_, _, Some(pf)) => Some(parse_fragment_spec(pf)),
            _ => None,
        };
        if let Some(Err(err)) = &fragment_spec {
            return miss!(fragment_error_reason(*err));
        }
        if let Some(Ok(FragmentSpec::Line { start, end })) = &fragment_spec {
            selected_start = Some(*start);
            selected_end = Some(*end);
        }
        resolve_selector_line_window(selector, &mut selected_start, &mut selected_end);
        let mut index_store_path = None;
        // When content load already verified a digest, portable integrity can compare
        // hex digests instead of re-hashing the full body (R1).
        let mut content_verified_sha256: Option<String> = None;
        let content = if let Some(content) = shared_content {
            // SharedCas::resolve already checked content_sha256_hex == lookup hash.
            content_verified_sha256 = portable_resolved.as_ref().map(|p| p.hash.clone());
            content
        } else {
            let load = expand_leaf_span("expand.leaf.local_resolve", || {
                let (resolved, source_store_path) =
                    self.resolve_ref_with_index(parsed.kind, parsed.bare);
                (resolved, source_store_path)
            });
            index_store_path = load.1;
            let resolved = load.0;
            if let Some(h) = resolved.verified_sha256() {
                content_verified_sha256 = Some(h.to_string());
            }
            match resolve_to_expand_content(resolved, &requested_ref, parsed.kind) {
                Ok(content) => content,
                Err(reason) if requested_alias && reason.starts_with("ref-not-found") => {
                    return miss!("dangling-ref");
                }
                Err(reason)
                    if self.shared_cas.is_some()
                        && !requested_ref.starts_with("tz://")
                        && reason.starts_with("ref-not-found") =>
                {
                    return miss!("shared-cas-missing");
                }
                Err(reason) => return miss!(reason),
            }
        };
        if portable.as_ref().is_some_and(|portable| {
            expand_leaf_span("expand.leaf.portable_integrity", || {
                if content_verified_sha256.as_deref() == Some(portable.hash.as_str()) {
                    return false;
                }
                sha256_hex(&content) != portable.hash
            })
        }) {
            return miss!("zeroref-corruption");
        }
        if parsed.kind == "file" && self.file_ref_is_stale(parsed.bare) {
            return miss!("stale-ref");
        }
        let line_window = if matches!(fragment_spec, Some(Ok(FragmentSpec::Byte { .. }))) {
            None
        } else {
            match clamp_line_window(&content, selected_start, &mut selected_end) {
                Ok(window) => window,
                Err(reason) => {
                    let reason = if matches!(fragment_spec, Some(Ok(FragmentSpec::Line { .. }))) {
                        reason.replacen(
                            "window-out-of-range",
                            fragment_error_reason(FragmentError::OutOfRange),
                            1,
                        )
                    } else {
                        reason
                    };
                    return miss!(reason);
                }
            }
        };
        match expand_selected_content(
            content,
            &fragment_spec,
            selector,
            selected_start,
            selected_end,
            anchor_kind,
            symbol,
        ) {
            Ok(selected) => {
                let mut result = expand_leaf_span("expand.leaf.expand_ok", || {
                    self.expand_ok(
                        requested_ref,
                        selector_owned,
                        &ref_id,
                        selected,
                        index_store_path.as_deref(),
                    )
                });
                if let Some((clamped, start, end, line_count)) = line_window {
                    result.clamped = clamped;
                    result.returned_start_line = Some(start);
                    result.returned_end_line = Some(end);
                    result.line_count = Some(line_count);
                }
                result
            }
            Err(reason) => miss!(reason),
        }
    }

    fn note_legacy_expand(&mut self, lookup_ref: &str) -> Option<&'static str> {
        if !is_legacy_same_store_blob_ref(lookup_ref) {
            return None;
        }
        if !self.config.legacy_compat {
            return Some("legacy-ref-disabled");
        }
        let bare = split_ref_fragment(lookup_ref).0;
        if self.state.ambiguous_aliases.contains(bare) {
            return Some("legacy-ambiguous");
        }
        self.legacy_read_count += 1;
        None
    }

    fn expand_ok(
        &mut self,
        requested_ref: String,
        selector: Option<String>,
        ref_id: &str,
        content: String,
        index_store_path: Option<&Path>,
    ) -> ExpansionResult {
        // Single lexical pass: recovery_tokens and ExpansionResult.tokens share one count.
        let tokens = expand_leaf_span("expand.leaf.expand_ok_count_tokens", || {
            count_tokens(&content)
        });
        self.note_expand_with_tokens(ref_id, tokens, index_store_path);
        ExpansionResult::ok_with_tokens(requested_ref, selector, content, tokens)
    }

    fn note_expand_with_tokens(
        &mut self,
        ref_id: &str,
        tokens: usize,
        index_store_path: Option<&Path>,
    ) {
        self.recovery_tokens += tokens;
        if let Some(store_path) = index_store_path.or(self.persistence_path.as_deref()) {
            let content_class = self
                .ref_classes
                .get(ref_id)
                .copied()
                .unwrap_or_else(|| classify_ref(ref_id, None));
            expand_leaf_span("expand.leaf.record_ref_index_expanded", || {
                record_ref_index_expanded(store_path, ref_id, content_class);
            });
        }
    }

    fn resolve_alias_chain(&self, ref_id: &str) -> Option<String> {
        let (bare, frag) = split_ref_fragment(ref_id);
        let mut current = bare;
        let mut advanced = false;
        for _ in 0..8 {
            let Some(next) = self.state.aliases.get(current) else {
                if !advanced {
                    return None;
                }
                return Some(match frag {
                    Some(f) => format!("{current}#{f}"),
                    None => current.to_string(),
                });
            };
            current = next;
            advanced = true;
        }
        None
    }

    /// The store's alias derivation key, generating and persisting it into
    /// the state on first use (lazily; in-memory stores get an ephemeral key).
    fn alias_key(&mut self) -> [u8; crate::session_aliases::ALIAS_KEY_BYTES] {
        if let Some(hex_key) = self.state.alias_key.as_deref() {
            let mut key = [0u8; crate::session_aliases::ALIAS_KEY_BYTES];
            if hex_key.len() == 64
                && hex_key
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                for (i, chunk) in hex_key.as_bytes().chunks_exact(2).enumerate() {
                    let hi =
                        hex_value(chunk[0]).expect("alias key already validated as lowercase hex");
                    let lo =
                        hex_value(chunk[1]).expect("alias key already validated as lowercase hex");
                    key[i] = (hi << 4) | lo;
                }
                return key;
            }
            // Corrupt key field: fall through and regenerate (aliases minted
            // under the corrupt key remain in the alias table and still
            // resolve; only the derivation input changes).
        }
        let mut key = [0u8; crate::session_aliases::ALIAS_KEY_BYTES];
        getrandom::getrandom(&mut key).expect("OS entropy for alias key");
        let mut hex_key = String::with_capacity(64);
        for byte in key {
            hex_key.push_str(&format!("{byte:02x}"));
        }
        self.state.alias_key = Some(hex_key);
        key
    }

    /// Register `tz://s/<16hex>` → full-hash blob alias and return the short form
    /// for visible capsules. Non-full-hash refs pass through unchanged.
    /// The short form is the keyed (opaque) derivation: visible alias bytes
    /// are independent of the payload content hash (W4-OPAQUE-CAS-ALIAS).
    /// Register a session-visible short alias without flushing to disk.
    /// Callers that batch many aliases should finish with `persist_pending`.
    pub fn register_session_visible_alias(&mut self, ref_id: &str) -> String {
        let key = self.alias_key();
        let Some(short) = session_aliases::session_visible_blob_alias_keyed(&key, ref_id) else {
            return ref_id.to_string();
        };
        let (short_bare, _) = split_ref_fragment(&short);
        if let Some(full_bare) = canonical_full_blob_ref(split_ref_fragment(ref_id).0)
            && self.alias_target(short_bare).as_deref() != Some(full_bare.as_str())
        {
            self.store_alias_deferred(short_bare, &full_bare);
        }
        // Collision marks the short form ambiguous; expand will refuse it.
        // Never advertise a handle that cannot be recovered.
        if self.is_alias_ambiguous(short_bare) {
            return ref_id.to_string();
        }
        short
    }

    /// Ensure a full-hash blob ref has a durable session-visible short alias.
    /// Persists immediately so a subsequent process restart can expand the short form.
    pub fn ensure_session_visible_alias(&mut self, ref_id: &str) -> String {
        let short = self.register_session_visible_alias(ref_id);
        match self.persist_pending() {
            Ok(()) => short,
            Err(_) => ref_id.to_string(),
        }
    }

    /// Rewrite full-hash blob refs in text to session-visible short aliases,
    /// registering each short → full mapping in the alias table (deferred).
    /// Single keyed pass: the emitted short form is the opaque alias produced
    /// by registration, never the content-derived legacy form.
    pub fn apply_session_visible_aliases_in_text(&mut self, text: &str) -> String {
        // Skip the char-by-char scan when the payload has no full-hash blob refs.
        if !text.contains("tz://blob/")
            && !text.contains("fz://blob/")
            && !text.contains("gz://blob/")
        {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut cursor = 0usize;
        while cursor < text.len() {
            if let Some((end, full)) = crate::session_aliases::take_full_hash_blob_at(text, cursor)
            {
                let short = self.register_session_visible_alias(&full);
                out.push_str(&short);
                cursor = end;
            } else {
                // Advance one full character (refs are pure ASCII; mid-char
                // slicing would mojibake or panic).
                let next = (cursor + 1..=text.len())
                    .find(|&index| text.is_char_boundary(index))
                    .unwrap_or(text.len());
                out.push_str(&text[cursor..next]);
                cursor = next;
            }
        }
        out
    }

    /// Shorten full-hash blob ref strings inside a JSON value.
    pub fn apply_session_visible_aliases_in_value(&mut self, value: &mut serde_json::Value) {
        fn walk(store: &mut RecoveryStore, value: &mut serde_json::Value) {
            match value {
                serde_json::Value::String(text) => {
                    // Shape check only; the registered short form is keyed.
                    let key_ready = session_visible_blob_alias(text).is_some();
                    if key_ready {
                        *text = store.register_session_visible_alias(text);
                    } else if text.contains("://blob/") {
                        *text = store.apply_session_visible_aliases_in_text(text);
                    }
                }
                serde_json::Value::Array(items) => {
                    for item in items {
                        walk(store, item);
                    }
                }
                serde_json::Value::Object(map) => {
                    for item in map.values_mut() {
                        walk(store, item);
                    }
                }
                _ => {}
            }
        }
        walk(self, value);
    }

    pub fn has_ref(&self, ref_id: &str) -> bool {
        let Some(lookup) = canonicalize_expand_ref(ref_id) else {
            return false;
        };
        let lookup = self.resolve_alias_chain(&lookup).unwrap_or(lookup);
        let Some(parsed) = parse_ref(&lookup) else {
            return false;
        };
        match parsed.kind {
            "blob" => self.blob_reachable(parsed.bare),
            "file" => self.state.files.contains_key(parsed.bare),
            "unit" | "search" => recovery_unit_map(&self.state, parsed.kind)
                .is_some_and(|m| m.contains_key(parsed.bare)),
            _ => false,
        }
    }

    /// Local/CAS reachability only — never opens sibling stores via ref-index.
    ///
    /// Session resume must use this: calling [`Self::has_ref`] per persisted
    /// record can reload multi-MB journals thousands of times and peg a core.
    pub fn has_ref_local(&self, ref_id: &str) -> bool {
        let Some(lookup) = canonicalize_expand_ref(ref_id) else {
            return false;
        };
        let lookup = self.resolve_alias_chain(&lookup).unwrap_or(lookup);
        let Some(parsed) = parse_ref(&lookup) else {
            return false;
        };
        match parsed.kind {
            "blob" => {
                self.state.blobs.contains_key(parsed.bare)
                    || self
                        .shared_cas
                        .as_ref()
                        .and_then(|cas| {
                            ref_index_id_part(parsed.bare).map(|hash| cas.contains(hash))
                        })
                        .unwrap_or(false)
            }
            "file" => self.state.files.contains_key(parsed.bare),
            "unit" | "search" => recovery_unit_map(&self.state, parsed.kind)
                .is_some_and(|m| m.contains_key(parsed.bare)),
            _ => false,
        }
    }

    /// Durability check for internal reuse of a ref as a diff/ack base: the
    /// ref must be reachable from PERSISTED state (shared CAS or a fresh read
    /// of the store file), not merely from this process's in-memory state.
    /// An external prune (cache file and/or CAS object removed) invalidates
    /// memory-only blobs so served output never references a base the agent
    /// cannot expand later (bxqo.1 / F-021). Without a persistence path the
    /// in-memory state is the whole truth (tests, embedded handles).
    pub fn has_ref_durable(&self, ref_id: &str) -> bool {
        let Some(lookup) = canonicalize_expand_ref(ref_id) else {
            return false;
        };
        let lookup = self.resolve_alias_chain(&lookup).unwrap_or(lookup);
        let Some(parsed) = parse_ref(&lookup) else {
            return false;
        };
        if parsed.kind == "blob"
            && let Some(cas) = &self.shared_cas
            && let Some(hash) = ref_index_id_part(parsed.bare)
            && cas.contains(hash)
        {
            return true;
        }
        let Some(path) = &self.persistence_path else {
            return self.has_ref_local(ref_id);
        };
        if !path.exists() {
            return false;
        }
        RecoveryStore::new(Some(path.clone())).has_ref_local(ref_id)
    }

    fn blob_reachable(&self, bare: &str) -> bool {
        self.state.blobs.contains_key(bare)
            || self
                .shared_cas
                .as_ref()
                .and_then(|cas| ref_index_id_part(bare).map(|hash| cas.contains(hash)))
                .unwrap_or(false)
    }

    pub fn export_status(&self) -> serde_json::Value {
        serde_json::json!({
            "schema_version": "tokenzero.recovery.v1",
            "blobs": self.state.blobs.len(),
            "files": self.state.files.len(),
            "units": self.state.units.len(),
            "search_hits": self.state.search_hits.len(),
            "max_blobs": self.config.max_blobs,
            "max_files": self.config.max_files,
            "max_units": self.config.max_units,
            "max_search_hits": self.config.max_search_hits,
            "approx_bytes": self.approx_bytes(),
            "max_bytes": self.config.max_bytes,
            "recovery_count": self.recovery_count,
            "recovery_tokens": self.recovery_tokens,
            "persistent": self.persistence_path.is_some(),
            "persistence_path": self.persistence_path.as_ref().map(|p| p.display().to_string()),
        })
    }

    pub fn prune_stale(&mut self, dry_run: bool) -> Result<serde_json::Value, RecoveryError> {
        let stale: Vec<String> = self
            .state
            .files
            .keys()
            .filter(|ref_id| self.file_ref_is_stale(ref_id))
            .cloned()
            .collect();
        if !dry_run {
            for ref_id in &stale {
                self.drop_ref(ref_id);
            }
            self.persist()?;
        }
        Ok(serde_json::json!({
            "schema_version": "tokenzero.cache.v1",
            "status": "ok",
            "dry_run": dry_run,
            "candidates": stale.iter().map(|ref_id| {
                serde_json::json!({"category": "exact", "ref": ref_id, "reason": "stale-source"})
            }).collect::<Vec<_>>(),
            "reclaimed_bytes": if dry_run { 0 } else { stale.len() },
        }))
    }

    // Record the outcome of a shell command and report whether it repeated
    // the previous run byte-for-byte (same combined output, same exit code).
    // Callers may render verified-unchanged successes as a tiny delta
    // envelope; the content-addressed blob ref still recovers exact bytes.
    persist_after_deferred!(
        record_shell_outcome,
        record_shell_outcome_deferred(
            scope: Option<&str>,
            command: &str,
            combined: &str,
            exit_code: Option<i32>,
        ) -> ShellRepeat
    );

    pub fn record_shell_outcome_deferred(
        &mut self,
        scope: Option<&str>,
        command: &str,
        combined: &str,
        exit_code: Option<i32>,
    ) -> ShellRepeat {
        self.skip_empty_persist = false;
        let key = id_for('s', &format!("{}\u{0}{command}", scope.unwrap_or("")));
        let combined_sha = sha256_hex(combined);
        let (epoch, seq) = next_shell_outcome_clock(&mut self.state);
        let (unchanged, seen) = match self.state.shell_outcomes.get(&key) {
            Some(prev) if prev.combined_sha == combined_sha && prev.exit_code == exit_code => {
                (true, prev.seen.saturating_add(1))
            }
            _ => (false, 1),
        };
        self.state.shell_outcomes.insert(
            key,
            ShellOutcome {
                combined_sha,
                exit_code,
                seen,
                seq,
                epoch,
            },
        );
        trim_shell_outcomes(&mut self.state.shell_outcomes);
        ShellRepeat { unchanged, seen }
    }

    fn put_file_backed_blob(
        &mut self,
        text: &str,
        path: &Path,
        source_start_line: usize,
        source_end_line: usize,
        content_type: ContentType,
    ) -> String {
        self.put_file_backed_blob_hashed(
            path,
            source_start_line,
            source_end_line,
            content_type,
            &sha256_hex(text),
        )
    }

    fn put_file_backed_blob_hashed(
        &mut self,
        path: &Path,
        source_start_line: usize,
        source_end_line: usize,
        content_type: ContentType,
        full_hash: &str,
    ) -> String {
        self.register_blob(
            full_hash,
            format!("tz://blob/b{}", &full_hash[..16]),
            content_type,
            Some(BlobEntry::FileRef {
                path: path.to_path_buf(),
                source_start_line,
                source_end_line,
            }),
        )
    }

    fn track_ref_class(&mut self, ref_id: &str, content_type: ContentType) {
        self.ref_classes
            .insert(ref_id.to_string(), classify_ref(ref_id, Some(content_type)));
    }

    fn register_blob(
        &mut self,
        full_hash: &str,
        legacy_ref: String,
        content_type: ContentType,
        value: Option<BlobEntry>,
    ) -> String {
        let ref_id = format!("tz://blob/{full_hash}");
        self.track_ref_class(&ref_id, content_type);
        if legacy_ref != ref_id {
            self.store_alias_deferred(&legacy_ref, &ref_id);
        }
        if let Some(value) = value {
            self.state.blobs.insert(ref_id.clone(), value);
        }
        self.remember_ref(&ref_id);
        ref_id
    }

    fn put_blob(&mut self, text: &str, content_type: ContentType) -> String {
        let full_hash = sha256_hex(text);
        let canonical_ref = format!("tz://blob/{full_hash}");
        if !self.state.blobs.contains_key(&canonical_ref) {
            self.state
                .transparency
                .append(format!("mint\0{canonical_ref}").as_bytes());
        }
        // Deferred CAS publication (zerostack-5u7 / tokenzero-cas-fsync-ovn):
        // Per-object CAS fsync barriers dominated CodeMode latency (~24ms of
        // 38ms per plan). The recovery root's durable commit already makes
        // inline bodies crash-safe, and the expand path falls back to inline
        // when CAS returns NotFound. CAS publication happens post-commit in
        // `publish_pending_cas`.
        //
        // Durable stores always attach a local hub CAS. Keep the body inline
        // until `publish_pending_cas` (zerostack-5u7). Snapshot marker
        // replacement is gated at `BLOB_EXTERNALIZE_MIN_BYTES`; smaller
        // bodies stay inline. Never write the private `<cache>.blobs/` tree;
        // leftover sidecars stay read-only.
        let value = Some(BlobEntry::Inline(text.to_string()));
        if self.shared_cas.is_some() {
            self.pending_cas_hashes.insert(full_hash.clone());
        }
        self.register_blob(
            &full_hash,
            format!("tz://blob/{}", id_for('b', text)),
            content_type,
            value,
        )
    }

    fn put_file(
        &mut self,
        text: &str,
        content_type: ContentType,
        path: Option<&Path>,
        source_start_line: Option<usize>,
        source_end_line: Option<usize>,
    ) -> String {
        self.put_file_entry(
            text,
            content_type,
            path,
            false,
            || fingerprint_for_stored_payload(path, source_start_line, source_end_line),
            source_start_line,
            source_end_line,
        )
    }

    fn put_source_backed_file(
        &mut self,
        text: &str,
        content_type: ContentType,
        path: &Path,
        source_sha256: &str,
    ) -> String {
        self.put_file_entry(
            text,
            content_type,
            Some(path),
            true,
            || source_fingerprint_from_sha256(path, source_sha256),
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn put_file_entry<F: FnOnce() -> Option<SourceFingerprint>>(
        &mut self,
        text: &str,
        content_type: ContentType,
        path: Option<&Path>,
        source_backed: bool,
        source_fingerprint: F,
        source_start_line: Option<usize>,
        source_end_line: Option<usize>,
    ) -> String {
        let ref_id = recovery_file_ref(text, path);
        self.track_ref_class(&ref_id, content_type);
        self.state.files.insert(
            ref_id.clone(),
            StoredFile {
                ref_id: ref_id.clone(),
                path: path.map(|path| path.to_string_lossy().into_owned()),
                path_identity: path.map(path_identity_text),
                source_backed,
                text: if source_backed {
                    String::new()
                } else {
                    text.to_string()
                },
                content_type: content_type.to_string(),
                source_fingerprint: source_fingerprint(),
                source_start_line,
                source_end_line,
            },
        );
        self.remember_ref(&ref_id);
        ref_id
    }

    fn index_units(
        &mut self,
        text: &str,
        content_type: ContentType,
        source_ref: &str,
    ) -> Vec<String> {
        let mut refs = Vec::new();
        for (idx, line) in text.lines().enumerate() {
            let stripped = line.trim();
            if stripped.len() >= 12 {
                refs.push(self.put_unit(
                    stripped,
                    content_type,
                    Some(source_ref),
                    Some(idx + 1),
                    Some(idx + 1),
                ));
            }
            if refs.len() >= 64 {
                break;
            }
        }
        refs
    }

    fn put_unit(
        &mut self,
        text: &str,
        content_type: ContentType,
        source_ref: Option<&str>,
        start_line: Option<usize>,
        end_line: Option<usize>,
    ) -> String {
        self.insert_stored_unit(
            false,
            format!("tz://unit/{}", id_for('u', text)),
            text,
            content_type,
            source_ref,
            (start_line, end_line),
        )
    }

    fn insert_stored_unit(
        &mut self,
        search_hit: bool,
        ref_id: String,
        text: &str,
        content_type: ContentType,
        source_ref: Option<&str>,
        source_lines: (Option<usize>, Option<usize>),
    ) -> String {
        let (start_line, end_line) = source_lines;
        self.track_ref_class(&ref_id, content_type);
        let unit = StoredUnit {
            ref_id: ref_id.clone(),
            text: text.to_string(),
            content_type: content_type.to_string(),
            source_ref: source_ref.map(str::to_string),
            start_line,
            end_line,
        };
        if search_hit {
            self.state.search_hits.insert(ref_id.clone(), unit);
        } else {
            self.state.units.entry(ref_id.clone()).or_insert(unit);
        }
        self.remember_ref(&ref_id);
        ref_id
    }

    fn resolve_ref(&self, kind: &str, bare: &str) -> RefResolve {
        match kind {
            "blob" => self
                .state
                .blobs
                .get(bare)
                .map_or(RefResolve::NotFound, |value| {
                    resolve_blob_value(self.persistence_path.as_deref(), bare, value)
                }),
            "file" => self
                .state
                .files
                .get(bare)
                .map(resolve_file_value)
                .unwrap_or(RefResolve::NotFound),
            "unit" | "search" => recovery_unit_map(&self.state, kind)
                .and_then(|units| units.get(bare))
                .map_or(RefResolve::NotFound, |u| RefResolve::Found(u.text.clone())),
            _ => RefResolve::NotFound,
        }
    }

    fn resolve_ref_with_index(&self, kind: &str, bare: &str) -> (RefResolve, Option<PathBuf>) {
        (self.resolve_ref(kind, bare), None)
    }

    fn file_ref_is_stale(&self, bare: &str) -> bool {
        let Some(stored) = self.state.files.get(bare) else {
            return false;
        };
        if is_ephemeral_source_path(stored.path.as_deref().unwrap_or_default()) {
            return false;
        }
        let Some(expected) = stored.source_fingerprint.as_ref() else {
            return false;
        };
        let Some(source_path) = stored_source_path(stored) else {
            return false;
        };
        source_fingerprint(&source_path).is_none_or(|actual| actual != *expected)
    }

    /// FIFO eviction contract (docs/racc.md): `state.order` is an insertion
    /// queue scanned from the front, so the oldest entry is evicted first.
    /// Re-putting a live ref appends a duplicate entry, but `compact_order`
    /// and the concurrent-session `merge_states` path retain each ref's FIRST
    /// occurrence, and reads never touch the order, so neither a re-put nor a
    /// read refreshes an eviction position. Duplicates are harmless to
    /// eviction because victims are de-duplicated before removal.
    fn remember_ref(&mut self, ref_id: &str) {
        self.skip_empty_persist = false;
        self.state.order.push(ref_id.to_string());
        self.session_refs.push(ref_id.to_string());
    }

    fn evict(&mut self) {
        recovery_maps!(evict self);
        while self.approx_bytes() > self.config.max_bytes {
            // Byte pressure evicts the coldest live ref (frecency over
            // `order`). Per-kind prefix caps above stay FIFO. Reads still
            // do not append to `order`.
            let Some(victim) = crate::frecency::coldest(&self.state.order, |ref_id| {
                self.local_entry_present(ref_id)
            })
            .map(str::to_string) else {
                break;
            };
            self.drop_ref(&victim);
        }
        self.compact_order();
    }

    fn local_entry_present(&self, ref_id: &str) -> bool {
        state_entry_present(&self.state, ref_id)
    }

    /// Frecency of one recovery ref from the existing `order` log.
    pub fn frecency_of(&self, ref_id: &str) -> f64 {
        crate::frecency::score_from_order(&self.state.order, ref_id)
    }

    /// Hottest live file-ref whose stored path matches `path`.
    pub fn frecency_for_path(&self, path: &Path) -> f64 {
        let want = path.to_string_lossy();
        self.state
            .files
            .values()
            .filter(|file| file.path.as_deref() == Some(want.as_ref()))
            .map(|file| self.frecency_of(&file.ref_id))
            .fold(0.0, f64::max)
    }

    fn drop_ref(&mut self, ref_id: &str) {
        self.skip_empty_persist = false;
        recovery_maps!(remove self.state, ref_id);
    }

    /// Collapse duplicate order entries to each ref's FIRST occurrence while
    /// dropping refs no longer in state. First-occurrence retention is the
    /// FIFO contract: eviction position equals first insertion time.
    fn compact_order(&mut self) {
        let live: HashSet<String> = recovery_maps!(keys self.state).cloned().collect();
        let mut seen = HashSet::new();
        self.state
            .order
            .retain(|ref_id| live.contains(ref_id) && seen.insert(ref_id.clone()));
    }

    fn approx_bytes(&self) -> usize {
        // Externalized blob markers account at their original payload size so
        // eviction pressure reflects real content, not marker bytes.
        // Saturate: overflow is over budget, never under (tokenzero-gbsh).
        saturating_usize_sum(self.state.blobs.values().map(blob_value_len))
            .saturating_add(saturating_usize_sum(self.state.files.values().map(|v| {
                v.text
                    .len()
                    .saturating_add(v.path.as_deref().unwrap_or_default().len())
            })))
            .saturating_add(saturating_usize_sum(
                self.state
                    .units
                    .values()
                    .chain(self.state.search_hits.values())
                    .map(|v| v.text.len()),
            ))
    }

    fn persist(&mut self) -> Result<(), RecoveryError> {
        self.persist_inner(true)
    }

    /// Persist while the caller already holds `PersistLock` for this cache.
    /// Nested `persist_pending` would unlock on drop before prune unlinks.
    pub(crate) fn persist_assuming_locked(&mut self) -> Result<(), RecoveryError> {
        self.persist_inner(false)
    }

    fn persist_inner(&mut self, acquire_lock: bool) -> Result<(), RecoveryError> {
        let storage_unchanged = self.persistence_path.as_deref().is_none_or(|path| {
            let (disk_identity, journal_identity) = cache_identities(path);
            disk_identity == self.disk_identity && journal_identity == self.journal_identity
        });
        if self.persist_skip_empty(storage_unchanged) {
            self.skip_empty_persist = false;
            return Ok(());
        }
        self.skip_empty_persist = false;
        // The persist lock covers identity checks, merge, journal append, and snapshot publication.
        let Some(path) = self.persistence_path.clone() else {
            self.evict();
            return Ok(());
        };
        self.config.validate()?;
        refuse_unexpanded_tilde_store_path(&path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _lock = if acquire_lock {
            Some(PersistLock::acquire(recovery_lock_path(&path))?)
        } else {
            None
        };
        let wal = recovery_session_wal(&path, &self.config)?;
        let unchanged_since_last_write = self.disk_identity.is_some()
            && !wal.foreign_write_since(self.disk_identity, self.journal_identity);
        if !unchanged_since_last_write {
            let existing = match load_state_if_present(&path, &self.config) {
                Ok(Some(existing)) => {
                    self.unreadable_snapshot = false;
                    ensure_ordinal_generation_floor(&path, existing.ordinal_generation)?;
                    existing
                }
                Ok(None) => {
                    self.unreadable_snapshot = false;
                    let generation = next_ordinal_generation(&path)?;
                    self.state.ordinal_generation = generation;
                    self.state.next_ordinal = initial_next_ordinal();
                    let mut empty = RecoveryState::empty(&self.config);
                    empty.ordinal_generation = generation;
                    empty
                }
                Err(err) => {
                    crash_inject::maybe_crash(crash_inject::BEFORE_PERSIST_UNREADABLE);
                    return Err(err);
                }
            };
            let current = std::mem::replace(&mut self.state, RecoveryState::empty(&self.config));
            self.state = merge_states(existing, current, &self.session_refs, &self.config);
        }
        self.evict();
        let has_pending_deletions =
            !self.pending_alias_deletions.is_empty() || !self.pending_blob_deletions.is_empty();
        apply_deletions(
            &mut self.state,
            self.pending_blob_deletions.iter().map(String::as_str),
            self.pending_alias_deletions.iter().map(String::as_str),
        );
        let result = if self.try_append_session_journal(
            &path,
            unchanged_since_last_write,
            has_pending_deletions,
        ) {
            Ok(())
        } else {
            self.publish_snapshot(&path)
        };
        result?;
        crash_inject::maybe_crash(crash_inject::AFTER_TMP_BEFORE_RENAME);
        // PersistLock is already held (`acquire_lock` or caller). Nested
        // `publish_pending_cas` would deadlock on the exclusive file lock.
        self.externalize_large_pending_cas_locked();
        Ok(())
    }

    fn persist_skip_empty(&self, storage_unchanged: bool) -> bool {
        self.skip_empty_persist
            && self.session_refs.is_empty()
            && self.pending_blob_deletions.is_empty()
            && self.pending_alias_deletions.is_empty()
            && storage_unchanged
    }

    // True when journal append published the delta. Restores session_refs on compaction/append failure.
    fn try_append_session_journal(
        &mut self,
        path: &Path,
        unchanged_since_last_write: bool,
        has_pending_deletions: bool,
    ) -> bool {
        if !(unchanged_since_last_write && !has_pending_deletions) {
            return false;
        }
        let delta = session_delta(&self.state, &self.session_refs, &self.config);
        let entry = JournalEntry {
            refs: std::mem::take(&mut self.session_refs),
            state: delta,
            deleted_blob_refs: self.pending_blob_deletions.iter().cloned().collect(),
            deleted_aliases: self.pending_alias_deletions.iter().cloned().collect(),
        };
        let Ok(record) = serde_json::to_vec(&entry) else {
            self.session_refs = entry.refs;
            return false;
        };
        let Ok(wal) = recovery_session_wal(path, &self.config) else {
            self.session_refs = entry.refs;
            return false;
        };
        match wal.append(&record) {
            Ok(AppendOutcome::Appended) => {
                self.journal_identity = wal.wal_identity();
                crash_inject::maybe_crash(crash_inject::AFTER_WAL_APPEND);
                crash_inject::maybe_crash(crash_inject::AFTER_JOURNAL_APPEND);
                append_blob_refs_to_ref_index(path, &entry.refs, Some(&self.ref_classes));
                self.clear_pending_deletions();
                true
            }
            Ok(AppendOutcome::NeedsCompaction) | Err(_) => {
                self.session_refs = entry.refs;
                false
            }
        }
    }

    fn publish_snapshot(&mut self, path: &Path) -> Result<(), RecoveryError> {
        self.disk_identity = None;
        let wal = recovery_session_wal(path, &self.config)?;
        wal.publish_snapshot(&snapshot_bytes(&self.state)?)?;
        crash_inject::maybe_crash(crash_inject::AFTER_WAL_APPEND);
        self.journal_identity = None;
        self.disk_identity = wal.snapshot_identity();
        append_blob_refs_to_ref_index(path, &self.session_refs, Some(&self.ref_classes));
        self.session_refs.clear();
        self.clear_pending_deletions();
        Ok(())
    }

    fn clear_pending_deletions(&mut self) {
        self.pending_blob_deletions.clear();
        self.pending_alias_deletions.clear();
    }
}

#[derive(Debug)]
struct ParsedRef<'a> {
    kind: &'a str,
    bare: &'a str,
    fragment: Option<&'a str>,
}

fn parse_ref(ref_id: &str) -> Option<ParsedRef<'_>> {
    let (bare, fragment) = ref_id
        .split_once('#')
        .map_or((ref_id, None), |(bare, fragment)| (bare, Some(fragment)));
    let rest = bare.strip_prefix("tz://")?;
    let (kind, id) = rest.split_once('/')?;
    if id.is_empty() || !matches!(kind, "blob" | "file" | "unit" | "search" | "codemode" | "s") {
        return None;
    }
    if kind == "codemode" {
        let mut parts = id.split('/');
        if parts.next() != Some("execution")
            || parts.next().is_none()
            || !matches!(
                parts.next(),
                Some("code" | "steps" | "telemetry" | "result" | "error")
            )
            || parts.next().is_some()
        {
            return None;
        }
    }
    if kind == "s" && !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(ParsedRef {
        kind,
        bare,
        fragment,
    })
}

fn parse_line_fragment(fragment: &str) -> (Option<usize>, Option<usize>) {
    let value = fragment.trim().trim_start_matches('L');
    let (start, end) = value.split_once('-').unwrap_or((value, value));
    let start = start.trim_start_matches('L').parse::<usize>().ok();
    let end = end.trim_start_matches('L').parse::<usize>().ok();
    // Line windows are one-based. Zero is malformed, not "slice from line 1".
    match (start, end) {
        (Some(0), _) | (_, Some(0)) => (None, None),
        other => other,
    }
}

fn parse_around_selector(value: &str) -> (Option<usize>, Option<usize>) {
    let (line_text, radius_text) = value
        .split_once(':')
        .or_else(|| value.split_once(','))
        .unwrap_or((value, "3"));
    let Ok(line) = line_text.trim().trim_start_matches('L').parse::<usize>() else {
        return (None, None);
    };
    if line == 0 {
        return (None, None);
    }
    let Ok(radius) = radius_text.trim().parse::<usize>() else {
        return (None, None);
    };
    (
        Some(line.saturating_sub(radius).max(1)),
        Some(line.saturating_add(radius)),
    )
}

// Line count matching exact split-inclusive slicing.
// Empty text is 0 lines: `split_inclusive` yields one empty segment, which
// would make a 0-byte blob look like it has line 1 (tokenzero-ubs-p9).
pub(crate) fn content_line_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.split_inclusive('\n').count()
    }
}

// Return an exact one-based inclusive line slice.
fn line_slice_exact(text: &str, start: usize, end: usize) -> String {
    let start = start.max(1);
    text.split_inclusive('\n')
        .skip(start - 1)
        .take(end.max(start) - start + 1)
        .collect()
}

// Resolve a selector line window in place.
fn resolve_selector_line_window(
    selector: Option<&str>,
    selected_start: &mut Option<usize>,
    selected_end: &mut Option<usize>,
) {
    let Some(selector) = selector else { return };
    let window = ["range:", "lines:", "line:"]
        .into_iter()
        .find_map(|prefix| selector.strip_prefix(prefix).map(parse_line_fragment))
        .or_else(|| selector.strip_prefix("around:").map(parse_around_selector));
    // Only a parsed start may replace the caller's line window. `(None, None)`
    // is a malformed selector, not "clear the window and serve everything".
    if let Some((Some(start), end)) = window {
        (*selected_start, *selected_end) = (Some(start), end);
    }
}

fn select_content<'a>(
    content: String,
    selector: Option<&'a str>,
    start_line: Option<usize>,
    end_line: Option<usize>,
    anchor_kind: Option<&str>,
    symbol: Option<&'a str>,
) -> String {
    match selector {
        Some("error_block") => return error_block(&content, 3),
        Some("summary") => return tokenzero_core::summarize_lines(&content, 12, 8, ""),
        _ => {}
    }
    let (mut selected_start, mut selected_end) = (start_line, end_line);
    resolve_selector_line_window(selector, &mut selected_start, &mut selected_end);
    if let Some(start) = selected_start.filter(|&line| line > 0) {
        return line_slice_exact(&content, start, selected_end.unwrap_or(start));
    }
    if let Some(symbol) = selector
        .and_then(|value| value.strip_prefix("symbol:"))
        .or(symbol)
    {
        return symbol_block(&content, symbol);
    }
    if anchor_kind.is_some() || selector.is_some_and(|value| value.starts_with("anchor:")) {
        return content
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                [
                    "fn ", "def ", "class ", "struct ", "impl ", "use ", "import ",
                ]
                .iter()
                .any(|prefix| line.starts_with(prefix))
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    content
}
fn ref_not_found_reason(kind: &str) -> String {
    if kind == "blob" {
        "ref-not-found; tiers tried: explicit/env cache, current-root store, shared CAS".to_string()
    } else {
        "ref-not-found; tiers tried: explicit/env cache, current-root store".to_string()
    }
}

fn resolve_to_expand_content(
    resolve: RefResolve,
    requested_ref: &str,
    kind: &str,
) -> Result<String, String> {
    match resolve {
        RefResolve::Found(content) | RefResolve::FoundVerified { content, .. } => Ok(content),
        RefResolve::Stale => Err("stale-ref".into()),
        RefResolve::DecodeFailed => Err("decode-failed".into()),
        RefResolve::NotFound if is_foreign_blob_ref(requested_ref) => Err("ref-not-found".into()),
        RefResolve::NotFound => Err(ref_not_found_reason(kind)),
    }
}
fn parse_expand_portable(ref_id: &str) -> Result<Option<ZeroRefBlob>, String> {
    match parse_zeroref_v1_blob(ref_id, None) {
        Ok(parsed) => Ok(Some(parsed)),
        Err(ZeroRefError::Unsupported) => Ok(None),
        Err(ZeroRefError::LegacyAmbiguity) if is_legacy_same_store_blob_ref(ref_id) => Ok(None),
        Err(err) => Err(format!("zeroref-{err}")),
    }
}
fn expand_selected_content(
    content: String,
    fragment_spec: &Option<Result<FragmentSpec, FragmentError>>,
    selector: Option<&str>,
    selected_start: Option<usize>,
    selected_end: Option<usize>,
    anchor_kind: Option<&str>,
    symbol: Option<&str>,
) -> Result<String, String> {
    if let Some(Ok(FragmentSpec::Byte { start, end })) = fragment_spec {
        let bytes = content.as_bytes();
        if *end > bytes.len() {
            return Err(format!(
                "fragment-out-of-range; start={start} end={end} len={}",
                bytes.len()
            ));
        }
        // An empty range carries no bytes, so a char boundary is irrelevant;
        // byte-addressable stores (TokenZeroStore) already allow this.
        if start == end {
            return Ok(String::new());
        }
        return content.get(*start..*end).map(str::to_owned).ok_or_else(|| {
            format!(
                "fragment-not-utf8-boundary; start={start} end={end} len={}",
                bytes.len()
            )
        });
    }
    Ok(select_content(
        content,
        selector,
        selected_start,
        selected_end,
        anchor_kind,
        symbol,
    ))
}

fn clamp_line_window(
    content: &str,
    selected_start: Option<usize>,
    selected_end: &mut Option<usize>,
) -> Result<Option<(bool, usize, usize, usize)>, String> {
    let Some(start) = selected_start else {
        return Ok(None);
    };
    let requested_end = selected_end.unwrap_or(start);
    let line_count = content_line_count(content);
    if start == 0 || start > requested_end || start > line_count {
        return Err(format!(
            "window-out-of-range; start={start} end={requested_end} line_count={line_count}"
        ));
    }
    let returned_end = requested_end.min(line_count);
    *selected_end = Some(returned_end);
    Ok(Some((
        returned_end != requested_end,
        start,
        returned_end,
        line_count,
    )))
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum RefIndexOverride {
    /// Use env/`HOME` (production default).
    Ambient,
    /// Keep the enabled-bit from env, but never open `HOME`/env roots.
    Isolated,
    /// Force the index off on this thread.
    Disabled,
    Path(PathBuf),
}

const fn default_ref_index_override() -> RefIndexOverride {
    #[cfg(test)]
    {
        RefIndexOverride::Isolated
    }
    #[cfg(not(test))]
    {
        RefIndexOverride::Ambient
    }
}

fn ref_index_enabled() -> bool {
    match REF_INDEX_OVERRIDE.with(|slot| slot.borrow().clone()) {
        RefIndexOverride::Disabled => return false,
        RefIndexOverride::Path(_) => return true,
        RefIndexOverride::Ambient | RefIndexOverride::Isolated => {}
    }
    env::var(REF_INDEX_DISABLE_ENV)
        .map(|value| value.trim() != "0")
        .unwrap_or(true)
}

use std::sync::OnceLock;

/// Test-only hook: override the ref index root directory on the current thread.
/// Call with `Some(path)` to redirect, `None` to restore the compile-time default.
#[doc(hidden)]
pub fn set_ref_index_root_override(path: Option<PathBuf>) {
    let _ = replace_ref_index_override(match path {
        Some(path) => RefIndexOverride::Path(path),
        None => default_ref_index_override(),
    });
}

pub(crate) fn replace_ref_index_override(value: RefIndexOverride) -> RefIndexOverride {
    REF_INDEX_OVERRIDE.with(|slot| std::mem::replace(&mut *slot.borrow_mut(), value))
}

std::thread_local! {
    static REF_INDEX_OVERRIDE: std::cell::RefCell<RefIndexOverride> =
        const { std::cell::RefCell::new(default_ref_index_override()) };
}
static REF_INDEX_DISABLED_OVERRIDE: OnceLock<std::sync::atomic::AtomicBool> = OnceLock::new();

/// Test-only hook: disable the per-user ref-index (stats/pointer log) so
/// stores do not write HOME shards regardless of ambient state.
#[doc(hidden)]
pub fn set_ref_index_disabled_override(disabled: bool) {
    REF_INDEX_DISABLED_OVERRIDE
        .get_or_init(|| std::sync::atomic::AtomicBool::new(false))
        .store(disabled, std::sync::atomic::Ordering::SeqCst);
}

fn ref_index_root() -> Option<PathBuf> {
    if let Some(flag) = REF_INDEX_DISABLED_OVERRIDE.get()
        && flag.load(std::sync::atomic::Ordering::SeqCst)
    {
        return None;
    }
    match REF_INDEX_OVERRIDE.with(|slot| slot.borrow().clone()) {
        RefIndexOverride::Disabled | RefIndexOverride::Isolated => return None,
        RefIndexOverride::Path(path) => return Some(path),
        RefIndexOverride::Ambient => {}
    }
    if !ref_index_enabled() {
        return None;
    }
    if let Some(path) = env::var_os(REF_INDEX_PATH_ENV).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        return (!unexpanded_tilde_path(&path)).then_some(path);
    }
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|home| PathBuf::from(home).join(".tokenzero").join("ref-index"))
        .filter(|path| !unexpanded_tilde_path(path))
}

fn create_ref_index_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn ref_index_id_part(ref_id: &str) -> Option<&str> {
    ref_id
        .rsplit_once('/')
        .map(|(_, id)| id)
        .filter(|id| !id.is_empty())
}

fn ref_index_shard_prefix(ref_id: &str, prefix_len: usize) -> String {
    let id = ref_index_id_part(ref_id).unwrap_or(ref_id);
    let mut prefix: String = id.chars().take(prefix_len).collect();
    while prefix.chars().count() < prefix_len {
        prefix.push('x');
    }
    if prefix.chars().all(|c| c.is_ascii_alphanumeric()) {
        return prefix;
    }
    // Untrusted ref ids are used as shard filenames. Non-alphanumeric
    // prefixes (`../`, `/`, `\`) must not join outside the index root.
    crate::shared_cas::content_sha256_hex(id.as_bytes())
        .chars()
        .take(prefix_len)
        .collect()
}

fn ref_index_shard_path_with_prefix(root: &Path, ref_id: &str, prefix_len: usize) -> PathBuf {
    root.join(format!(
        "{}.ndjson",
        ref_index_shard_prefix(ref_id, prefix_len)
    ))
}

fn ref_index_shard_path(root: &Path, ref_id: &str) -> PathBuf {
    ref_index_shard_path_with_prefix(root, ref_id, REF_INDEX_SHARD_PREFIX_LEN)
}

fn legacy_ref_index_shard_path(root: &Path, ref_id: &str) -> PathBuf {
    ref_index_shard_path_with_prefix(root, ref_id, REF_INDEX_LEGACY_SHARD_PREFIX_LEN)
}

fn ref_index_read_shard_paths(root: &Path, ref_id: &str) -> Vec<PathBuf> {
    let current = ref_index_shard_path(root, ref_id);
    let legacy = legacy_ref_index_shard_path(root, ref_id);
    if current == legacy {
        vec![current]
    } else {
        vec![current, legacy]
    }
}

fn ref_index_lock_path(shard: &Path) -> PathBuf {
    append_file_name_suffix(shard, ".lock")
}

fn ref_index_timestamp() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn ref_index_text(path: &Path) -> Option<String> {
    read_limited_utf8(fs::File::open(path).ok()?, REF_INDEX_READ_MAX_BYTES)
        .ok()
        .flatten()
}

fn parsed_ref_index_entries(text: &str) -> impl Iterator<Item = RefIndexEntry> + '_ {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let entry: RefIndexEntry = serde_json::from_str(line).ok()?;
            match entry.commit {
                None => Some(entry),
                Some(commit) if commit == ref_index_line_commit(&entry) => Some(entry),
                Some(_) => None,
            }
        })
}

fn ref_index_store_path(store_path: &Path) -> Option<PathBuf> {
    store_path
        .canonicalize()
        .or_else(|_| Ok::<_, std::io::Error>(store_path.to_path_buf()))
        .ok()
}

fn ref_index_root_store(store_path: &Path) -> Option<(PathBuf, PathBuf)> {
    Some((ref_index_root()?, ref_index_store_path(store_path)?))
}

fn locked_ref_index_shard(root: &Path, ref_id: &str) -> Option<(PathBuf, PersistLock)> {
    let shard = ref_index_shard_path(root, ref_id);
    PersistLock::acquire_with_retries(ref_index_lock_path(&shard), LOCK_RETRIES)
        .ok()
        .map(|lock| (shard, lock))
}

fn compact_ref_index_if_needed(shard: &Path) -> Result<(), RecoveryError> {
    let Ok(meta) = fs::metadata(shard) else {
        return Ok(());
    };
    if meta.len() <= REF_INDEX_MAX_BYTES {
        return Ok(());
    }
    // Below the stale-ratio gate a shard can still grow toward the read
    // ceiling, after which ref_index_text returns None and the shard vanishes.
    let near_unread = meta.len() >= (REF_INDEX_READ_MAX_BYTES as u64 / 2);
    if !near_unread {
        let Some(text) = ref_index_text(shard) else {
            return Ok(());
        };
        let lines = text.lines().filter(|line| !line.trim().is_empty()).count();
        if lines == 0 {
            return Ok(());
        }
        let live = newest_ref_index_entries(&text, None).len();
        let stale = lines.saturating_sub(live);
        // Append-only until most of the shard is superseded. Rewrite-compaction
        // at the 1MiB threshold is what a kill mid-rename used to risk.
        if (stale as f64) / (lines as f64) < REF_INDEX_RECLAIM_STALE_RATIO {
            return Ok(());
        }
    }
    compact_ref_index_shard(shard)
}

/// Compact rewrites a secondary index. WAL/snapshot is already the crash
/// authority, and [`write_ref_index_entries`] leaves dest intact on tmp
/// failure. Persist and expand must not panic on disk-full / EACCES here.
fn compact_ref_index_best_effort(shard: &Path) {
    let _ = compact_ref_index_if_needed(shard);
}

const REF_INDEX_RECLAIM_STALE_RATIO: f64 = 0.75;

fn ref_index_line_commit(entry: &RefIndexEntry) -> u32 {
    let mut hashed = entry.clone();
    hashed.commit = None;
    let bytes = serde_json::to_vec(&hashed).unwrap_or_default();
    let mut hash = 2_166_136_261u32;
    for byte in bytes {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

fn append_blob_refs_to_ref_index(
    store_path: &Path,
    refs: &[String],
    classes: Option<&BTreeMap<String, ContentClass>>,
) {
    let Some((root, store_path)) = ref_index_root_store(store_path) else {
        return;
    };
    let mut refs = refs
        .iter()
        .filter(|ref_id| ref_id.starts_with("tz://blob/"))
        .peekable();
    if refs.peek().is_none() || create_ref_index_dir(&root).is_err() {
        return;
    }
    let ts = ref_index_timestamp();
    for ref_id in refs {
        let Some((shard, _lock)) = locked_ref_index_shard(&root, ref_id) else {
            continue;
        };
        if newest_ref_index_store_path(&shard, ref_id).as_deref()
            == Some(store_path.to_string_lossy().as_ref())
        {
            continue;
        }
        let class = classes
            .and_then(|classes| classes.get(ref_id))
            .copied()
            .unwrap_or_else(|| classify_ref(ref_id, None));
        if append_ref_index_line(&shard, ref_id, &store_path, ts, class, false, 0, None).is_ok() {
            compact_ref_index_best_effort(&shard);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn append_ref_index_line(
    shard: &Path,
    ref_id: &str,
    store_path: &Path,
    ts: u128,
    content_class: ContentClass,
    expanded: bool,
    expansion_count: u64,
    last_expanded_ts: Option<u128>,
) -> Result<(), RecoveryError> {
    let Some(parent) = shard.parent() else {
        return Ok(());
    };
    create_ref_index_dir(parent)?;
    let entry = RefIndexEntry {
        ref_id: ref_id.to_string(),
        store_path: store_path.to_string_lossy().into_owned(),
        ts,
        content_class,
        expanded,
        expansion_count,
        last_expanded_ts,
        metadata_migrated: expanded
            && shard
                .parent()
                .is_some_and(|root| shard == ref_index_shard_path(root, ref_id)),
        commit: None,
    };
    let mut entry = entry;
    entry.commit = Some(ref_index_line_commit(&entry));
    let mut line = serde_json::to_string(&entry)?;
    line.push('\n');
    private_open_options()
        .create(true)
        .append(true)
        .open(shard)?
        .write_all(line.as_bytes())?;
    Ok(())
}

fn open_optional_file(path: &Path) -> Result<Option<fs::File>, RecoveryError> {
    match fs::File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn compact_ref_index_shard(shard: &Path) -> Result<(), RecoveryError> {
    let Some(file) = open_optional_file(shard)? else {
        return Ok(());
    };
    let file_len = file.metadata()?.len();
    let cap = if file_len > REF_INDEX_READ_MAX_BYTES as u64 {
        (file_len
            .saturating_add(1)
            .min(REF_INDEX_COMPACT_EMERGENCY_MAX_BYTES as u64)) as usize
    } else {
        REF_INDEX_READ_MAX_BYTES
    };
    let Some(text) = read_limited_utf8(file, cap)? else {
        if file_len > cap as u64 {
            return Err(invalid_data(format!(
                "ref index shard exceeds emergency compact cap ({cap} bytes): {}",
                shard.display()
            ))
            .into());
        }
        return Ok(());
    };
    write_ref_index_entries(shard, newest_ref_index_entries(&text, None).values())
}

fn newest_ref_index_store_path(shard: &Path, ref_id: &str) -> Option<String> {
    parsed_ref_index_entries(&ref_index_text(shard)?)
        .filter(|entry| entry.ref_id == ref_id)
        .fold(None, |newest, entry| {
            if newest
                .as_ref()
                .is_none_or(|current: &RefIndexEntry| entry.ts > current.ts)
            {
                Some(entry)
            } else {
                newest
            }
        })
        .map(|entry| entry.store_path)
}

fn ref_index_entries_for_ref(text: &str, ref_id: &str) -> Vec<RefIndexEntry> {
    let mut entries: Vec<_> = parsed_ref_index_entries(text)
        .filter(|entry| entry.ref_id == ref_id)
        .collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.ts));
    entries
}

fn newest_ref_index_entries(text: &str, skip_ref: Option<&str>) -> BTreeMap<String, RefIndexEntry> {
    let mut entries = BTreeMap::new();
    for mut entry in
        parsed_ref_index_entries(text).filter(|entry| skip_ref != Some(entry.ref_id.as_str()))
    {
        match entries.entry(entry.ref_id.clone()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(entry);
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let existing = slot.get_mut();
                if entry.ts >= existing.ts {
                    entry.expanded |= existing.expanded;
                    entry.expansion_count = entry.expansion_count.max(existing.expansion_count);
                    entry.last_expanded_ts = entry.last_expanded_ts.max(existing.last_expanded_ts);
                    entry.metadata_migrated |= existing.metadata_migrated;
                    slot.insert(entry);
                } else {
                    existing.expanded |= entry.expanded;
                    existing.expansion_count = existing.expansion_count.max(entry.expansion_count);
                    existing.last_expanded_ts =
                        existing.last_expanded_ts.max(entry.last_expanded_ts);
                    existing.metadata_migrated |= entry.metadata_migrated;
                }
            }
        }
    }
    entries
}

fn write_ref_index_entries<'a>(
    shard: &Path,
    entries: impl IntoIterator<Item = &'a RefIndexEntry>,
) -> Result<(), RecoveryError> {
    create_ref_index_dir(shard.parent().unwrap_or_else(|| Path::new(".")))?;
    let tmp = recovery_tmp_path(shard);
    let mut created = false;
    let result = (|| {
        let mut file = create_private_new(&tmp)?;
        created = true;
        for entry in entries {
            let mut stamped = entry.clone();
            stamped.commit = None;
            stamped.commit = Some(ref_index_line_commit(&stamped));
            serde_json::to_writer(&mut file, &stamped)?;
            file.write_all(b"\n")?;
        }
        file.sync_all()?;
        drop(file);
        crash_inject::maybe_crash(crash_inject::AFTER_TMP_BEFORE_RENAME);
        fs::rename(&tmp, shard)?;
        Ok(())
    })();
    if result.is_err() && created {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub(crate) fn ref_index_blob_lru(ref_id: &str) -> (u64, u128) {
    ref_index_blob_entries(ref_id)
        .map(|(_, entries)| {
            entries.into_iter().fold((0_u64, 0_u128), |acc, item| {
                (
                    acc.0
                        .max(item.expansion_count.max(u64::from(item.expanded))),
                    acc.1.max(item.last_expanded_ts.unwrap_or(0)),
                )
            })
        })
        .unwrap_or_default()
}

fn ref_index_blob_entries(ref_id: &str) -> Option<(PathBuf, Vec<RefIndexEntry>)> {
    let root = ref_index_root()?;
    let mut by_store = BTreeMap::<String, RefIndexEntry>::new();
    for shard in ref_index_read_shard_paths(&root, ref_id) {
        let Some(text) = ref_index_text(&shard) else {
            continue;
        };
        for mut entry in ref_index_entries_for_ref(&text, ref_id) {
            match by_store.entry(entry.store_path.clone()) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(entry);
                }
                std::collections::btree_map::Entry::Occupied(mut slot) => {
                    let existing = slot.get_mut();
                    if entry.ts >= existing.ts {
                        entry.expanded |= existing.expanded;
                        entry.expansion_count = entry.expansion_count.max(existing.expansion_count);
                        entry.last_expanded_ts =
                            entry.last_expanded_ts.max(existing.last_expanded_ts);
                        entry.metadata_migrated |= existing.metadata_migrated;
                        slot.insert(entry);
                    } else {
                        existing.expanded |= entry.expanded;
                        existing.expansion_count =
                            existing.expansion_count.max(entry.expansion_count);
                        existing.last_expanded_ts =
                            existing.last_expanded_ts.max(entry.last_expanded_ts);
                        existing.metadata_migrated |= entry.metadata_migrated;
                    }
                }
            }
        }
    }
    let mut entries: Vec<_> = by_store.into_values().collect();
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.ts));
    Some((root, entries))
}

fn record_ref_index_expanded(store_path: &Path, ref_id: &str, fallback: ContentClass) {
    let Some((root, request_store_path)) = ref_index_root_store(store_path) else {
        return;
    };
    let Some((shard, _lock)) = locked_ref_index_shard(&root, ref_id) else {
        return;
    };
    let mut existing = ref_index_text(&shard)
        .and_then(|text| newest_ref_index_entries(&text, None).remove(ref_id));
    if !existing
        .as_ref()
        .is_some_and(|entry| entry.metadata_migrated)
    {
        let legacy = legacy_ref_index_shard_path(&root, ref_id);
        let legacy_entry = if legacy != shard && legacy.exists() {
            let Some(text) = ref_index_text(&legacy) else {
                return;
            };
            newest_ref_index_entries(&text, None).remove(ref_id)
        } else {
            None
        };
        if let Some(legacy_entry) = legacy_entry {
            if let Some(current) = existing.as_mut() {
                current.expanded |= legacy_entry.expanded;
                current.expansion_count = current.expansion_count.max(legacy_entry.expansion_count);
                current.last_expanded_ts =
                    current.last_expanded_ts.max(legacy_entry.last_expanded_ts);
            } else {
                existing = Some(legacy_entry);
            }
        }
    }
    let class = existing
        .as_ref()
        .map_or(fallback, |entry| entry.content_class);
    let expansion_count = existing.as_ref().map_or(1, |entry| {
        entry
            .expansion_count
            .max(u64::from(entry.expanded))
            .saturating_add(1)
    });
    let now = ref_index_timestamp();
    let _ = append_ref_index_line(
        &shard,
        ref_id,
        &request_store_path,
        now,
        class,
        true,
        expansion_count,
        Some(now),
    );
    compact_ref_index_best_effort(&shard);
}

/// Export per-content-class expansion rates from the per-user ref index.
/// Returns a JSON summary with total refs, expanded refs, and the expansion
/// rate for each content class. The `expanded` flag is sticky across sessions.
pub fn export_class_stats() -> serde_json::Value {
    const SCHEMA: &str = "tokenzero.recovery.class-stats.v1";
    let empty = || {
        serde_json::json!({
            "schema_version": SCHEMA,
            "classes": Vec::<serde_json::Value>::new(),
            "total_refs": 0,
            "total_expanded": 0,
        })
    };
    let Some(root) = ref_index_root() else {
        return empty();
    };
    let Ok(shards) = fs::read_dir(root) else {
        return empty();
    };
    let mut per_ref: BTreeMap<String, (u128, ContentClass, bool)> = BTreeMap::new();
    for shard in shards
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("ndjson"))
    {
        let Some(text) = ref_index_text(&shard) else {
            continue;
        };
        for entry in parsed_ref_index_entries(&text) {
            let current = per_ref
                .entry(entry.ref_id)
                .or_insert((0, entry.content_class, false));
            current.2 |= entry.expanded;
            if entry.ts > current.0 {
                current.0 = entry.ts;
                current.1 = entry.content_class;
            }
        }
    }
    let mut totals: BTreeMap<ContentClass, (usize, usize)> = BTreeMap::new();
    for (_, class, expanded) in per_ref.values() {
        let counts = totals.entry(*class).or_default();
        counts.0 += 1;
        counts.1 += usize::from(*expanded);
    }
    let mut total_refs = 0usize;
    let mut total_expanded = 0usize;
    let classes = [
        ContentClass::SourceFile,
        ContentClass::Diff,
        ContentClass::ShellOutput,
        ContentClass::SearchHits,
        ContentClass::Doc,
        ContentClass::BinaryPreview,
        ContentClass::Unknown,
    ]
    .into_iter()
    .map(|class| {
        let (total, expanded) = totals.remove(&class).unwrap_or_default();
        total_refs += total;
        total_expanded += expanded;
        let rate = if total == 0 {
            0.0
        } else {
            expanded as f64 / total as f64
        };
        serde_json::json!({
            "content_class": class,
            "total": total,
            "expanded": expanded,
            "rate": rate,
        })
    })
    .collect::<Vec<_>>();
    serde_json::json!({
        "schema_version": SCHEMA,
        "classes": classes,
        "total_refs": total_refs,
        "total_expanded": total_expanded,
    })
}

pub(crate) fn load_state(
    path: &Path,
    config: &RecoveryConfig,
) -> Result<Option<RecoveryState>, RecoveryError> {
    config.validate()?;
    refuse_unexpanded_tilde_store_path(path)?;
    let state = match open_optional_file(path)? {
        Some(file) => {
            let meta = file.metadata()?;
            // Compare as u64 so a file larger than usize can't truncate and slip past
            // the load-size guard on 32-bit targets (which would risk an OOM on read).
            if !meta.is_file() || meta.len() > config.max_load_bytes as u64 {
                return Err(unreadable_snapshot_error(path));
            }
            let Some(text) = read_limited_utf8(file, config.max_load_bytes)? else {
                return Err(unreadable_snapshot_error(path));
            };
            let mut state = serde_json::from_str::<RecoveryState>(&text)
                .map_err(|_| unreadable_snapshot_error(path))?;
            state.configure(config);
            Some(state)
        }
        None => None,
    };
    match state {
        Some(state) => Ok(Some(apply_session_wal(state, path, config)?)),
        None if !session_journal_present(path) => Ok(None),
        None => {
            // Snapshot gone, WAL still present: replay complete records onto
            // empty rather than treating the store as missing and later
            // publish_snapshot/clear_wal dropping committed journal bytes.
            Ok(Some(apply_session_wal(
                RecoveryState::empty(config),
                path,
                config,
            )?))
        }
    }
}

fn unreadable_snapshot_error(path: &Path) -> RecoveryError {
    invalid_data(format!(
        "recovery snapshot is unreadable: {}",
        path.display()
    ))
    .into()
}

fn session_journal_present(path: &Path) -> bool {
    let Ok(wal) = SessionWal::new(path, SessionWalConfig::default()) else {
        return false;
    };
    let active = wal.wal_path();
    if active.is_file() {
        return true;
    }
    (1..=SESSION_WAL_DEFAULT_MAX_SEALED_SEGMENTS).any(|generation| {
        let mut sibling = active.as_os_str().to_os_string();
        sibling.push(format!(".{generation}"));
        PathBuf::from(sibling).is_file()
    })
}

/// `None` only when the snapshot file and session WAL are both absent. An
/// existing unreadable, unparseable, or oversized snapshot is an error so
/// persist/prune cannot treat it as empty and overwrite or delete dependents.
pub(crate) fn load_state_if_present(
    path: &Path,
    config: &RecoveryConfig,
) -> Result<Option<RecoveryState>, RecoveryError> {
    match load_state(path, config)? {
        Some(state) => Ok(Some(state)),
        None if !path.exists() && !session_journal_present(path) => Ok(None),
        None => Err(unreadable_snapshot_error(path)),
    }
}

// Large blobs use verified content-addressed CAS objects + snapshot markers.
const BLOB_EXTERNALIZE_MIN_BYTES: usize = 64 * 1024;
const STREAM_READ_BUFFER_BYTES: usize = 64 * 1024;
const BLOB_MARKER_PREFIX: &str = "\u{0}tzx:v1:";

pub(crate) fn blob_sidecar_dir(cache_path: &Path) -> PathBuf {
    append_file_name_suffix(cache_path, ".blobs")
}

/// Prove blob reachability from on-disk sidecar / SharedCas without loading
/// the multi-MB recovery snapshot and journal.
///
/// Session-memory resume used to call [`RecoveryStore::new`] solely for
/// [`RecoveryStore::has_ref_local`], which re-parsed the full cache on every
/// one-shot CLI expand (~20+ ms on the S4_whole corpus). Large externalized
/// blobs already have a content-addressed sidecar; SharedCas has the same
/// full-hash objects. When either proves the ref, the seen-set record is
/// safe to restore without touching the recovery JSON.
///
/// Contract:
/// - `true` means presence is proven on disk (isomorphic to a successful
///   `has_ref_local` for that blob).
/// - `false` does **not** prove absence from the snapshot (small inline-only
///   blobs, file/unit/search refs, aliases). Callers must fall back to
///   [`RecoveryStore::has_ref_local`] for those cases.
pub fn blob_ref_proven_on_disk(cache_path: &Path, ref_id: &str) -> bool {
    let Some(lookup) = canonicalize_expand_ref(ref_id) else {
        return false;
    };
    let Some(parsed) = parse_ref(&lookup) else {
        return false;
    };
    if parsed.kind != "blob" {
        return false;
    }
    let Some(hash) = ref_index_id_part(parsed.bare) else {
        return false;
    };
    // Externalized large-blob sidecar: `<cache>.blobs/<full-hash>.txt`.
    if zero_ref::is_full_lower_hex(hash) {
        let sidecar = blob_sidecar_dir(cache_path).join(format!("{hash}.txt"));
        if sidecar.is_file() {
            return true;
        }
    }
    // Unified / sibling SharedCas object store.
    if let Some(cas) = SharedCas::detect_from_cache_path(cache_path)
        && cas.contains(hash)
    {
        return true;
    }
    false
}

fn blob_cas_marker(hash: &str, len: usize) -> String {
    format!("{BLOB_MARKER_PREFIX}{hash}:{len}:")
}

pub(crate) fn parse_blob_marker(value: &str) -> Option<(&str, usize)> {
    let rest = value.strip_prefix(BLOB_MARKER_PREFIX)?;
    let (hash, rest) = rest.split_at_checked(64)?;
    if !zero_ref::is_full_lower_hex(hash) {
        return None;
    }
    let len: usize = rest.strip_prefix(':')?.strip_suffix(':')?.parse().ok()?;
    Some((hash, len))
}

fn blob_value_len(value: &BlobEntry) -> usize {
    match value {
        BlobEntry::Inline(text) => parse_blob_marker(text).map_or(text.len(), |(_, len)| len),
        BlobEntry::FileRef { path, .. } => {
            std::mem::size_of::<BlobEntry>() + path.as_os_str().len()
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    crate::shared_cas::lower_hex(bytes)
}

fn digest_hex(hasher: Sha256) -> String {
    encode_hex(hasher.finalize().as_ref())
}

fn invalid_data(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.into())
}

fn invalid_input(msg: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, msg.into())
}

/// True when `path` is a literal unexpanded `~` store root (`~/…`), matching
/// the hub `literal_tilde_root` rule. Never create or persist under it.
pub(crate) fn unexpanded_tilde_path(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(std::path::Component::Normal(component)) if component == "~"
    )
}

fn refuse_unexpanded_tilde_store_path(path: &Path) -> Result<(), RecoveryError> {
    if unexpanded_tilde_path(path) {
        return Err(invalid_input(format!("unexpanded ~ store path: {}", path.display())).into());
    }
    Ok(())
}

fn finalize_utf8_digest(bytes: Vec<u8>, hasher: Sha256) -> std::io::Result<(String, String)> {
    Ok((
        String::from_utf8(bytes).map_err(|err| invalid_data(err.to_string()))?,
        digest_hex(hasher),
    ))
}

fn line_range_out_of_bounds(start: usize, end: usize, line_count: usize) -> bool {
    start == 0 || start > end || end > line_count
}

fn read_file_chunks<R: Read>(
    reader: &mut R,
    mut on_chunk: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut buffer = [0_u8; STREAM_READ_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        on_chunk(&buffer[..read])?;
    }
    Ok(())
}

fn read_utf8_hashed(path: &Path, expected_len: Option<usize>) -> std::io::Result<(String, String)> {
    let mut file = fs::File::open(path)?;
    let capacity = expected_len
        .or_else(|| file.metadata().ok()?.len().try_into().ok())
        .unwrap_or(STREAM_READ_BUFFER_BYTES);
    let mut bytes = Vec::with_capacity(capacity);
    let mut hasher = Sha256::new();
    read_file_chunks(&mut file, |chunk| {
        if expected_len.is_some_and(|len| bytes.len().saturating_add(chunk.len()) > len) {
            return Err(invalid_data("streamed payload exceeds its recorded length"));
        }
        hasher.update(chunk);
        bytes.extend_from_slice(chunk);
        Ok(())
    })?;
    if expected_len.is_some_and(|len| bytes.len() != len) {
        return Err(invalid_data(
            "streamed payload does not match its recorded length",
        ));
    }
    finalize_utf8_digest(bytes, hasher)
}

fn read_utf8_line_range_hashed(
    path: &Path,
    start_line: usize,
    end_line: usize,
) -> std::io::Result<(String, String)> {
    let mut file = fs::File::open(path)?;
    let mut selected = Vec::new();
    let mut hasher = Sha256::new();
    let mut line = 1_usize;
    let mut bytes_seen = 0_usize;
    let mut newline_count = 0_usize;
    let mut last_byte = None;
    read_file_chunks(&mut file, |chunk| {
        bytes_seen += chunk.len();
        let mut selected_from = None;
        for (index, byte) in chunk.iter().copied().enumerate() {
            if line >= start_line && line <= end_line && selected_from.is_none() {
                selected_from = Some(index);
            }
            if byte == b'\n' {
                newline_count += 1;
                if line == end_line
                    && let Some(from) = selected_from.take()
                {
                    hasher.update(&chunk[from..=index]);
                    selected.extend_from_slice(&chunk[from..=index]);
                }
                line += 1;
            }
            last_byte = Some(byte);
        }
        if let Some(from) = selected_from {
            hasher.update(&chunk[from..]);
            selected.extend_from_slice(&chunk[from..]);
        }
        Ok(())
    })?;
    let line_count = if bytes_seen == 0 {
        0
    } else {
        newline_count + usize::from(last_byte != Some(b'\n'))
    };
    if line_range_out_of_bounds(start_line, end_line, line_count) {
        return Err(invalid_data("streamed line range is outside the source"));
    }
    finalize_utf8_digest(selected, hasher)
}

fn stored_source_path(stored: &StoredFile) -> Option<PathBuf> {
    let path_text = stored.path.as_deref()?;
    Some(
        stored
            .path_identity
            .as_deref()
            .and_then(path_from_identity_text)
            .unwrap_or_else(|| PathBuf::from(path_text)),
    )
}

fn resolve_found_if(ok: bool, text: String, fail: RefResolve) -> RefResolve {
    if ok { RefResolve::Found(text) } else { fail }
}

fn resolve_found_verified(ok: bool, text: String, sha256: String, fail: RefResolve) -> RefResolve {
    if ok {
        RefResolve::FoundVerified {
            content: text,
            sha256,
        }
    } else {
        fail
    }
}

fn blob_ref_digest_matches(ref_id: &str, text: &str, sha256: &str) -> bool {
    ref_id.strip_prefix("tz://blob/").is_some_and(|hash| {
        if hash.len() == 64 {
            sha256 == hash
        } else {
            id_for('b', text) == hash
        }
    })
}

fn resolve_file_value(stored: &StoredFile) -> RefResolve {
    if !stored.source_backed {
        return RefResolve::Found(stored.text.clone());
    }
    let Some(path) = stored_source_path(stored) else {
        return RefResolve::DecodeFailed;
    };
    let Some(expected) = stored.source_fingerprint.as_ref() else {
        return RefResolve::DecodeFailed;
    };
    let Ok(expected_len) = usize::try_from(expected.size) else {
        return RefResolve::Stale;
    };
    let Ok((text, sha256)) = read_utf8_hashed(&path, Some(expected_len)) else {
        return RefResolve::Stale;
    };
    resolve_found_if(
        source_fingerprint_from_sha256(&path, &sha256).as_ref() == Some(expected),
        text,
        RefResolve::Stale,
    )
}

fn resolve_blob_value(cache_path: Option<&Path>, ref_id: &str, value: &BlobEntry) -> RefResolve {
    match value {
        BlobEntry::Inline(value) => {
            if value.starts_with(BLOB_MARKER_PREFIX) {
                let Some((hash, expected_len)) = parse_blob_marker(value) else {
                    return RefResolve::DecodeFailed;
                };
                if let Some(cache_path) = cache_path
                    && let Some(cas) = SharedCas::detect_from_cache_path(cache_path)
                {
                    match shared_cas_utf8(&cas, hash) {
                        Ok(text) if text.len() == expected_len => {
                            return resolve_found_verified(
                                true,
                                text,
                                hash.to_string(),
                                RefResolve::DecodeFailed,
                            );
                        }
                        Ok(_) | Err(Some(SharedCasError::Corruption)) => {
                            return RefResolve::DecodeFailed;
                        }
                        Err(None) => return RefResolve::DecodeFailed,
                        Err(Some(SharedCasError::NotFound)) => {}
                        Err(Some(_)) => return RefResolve::DecodeFailed,
                    }
                }
                let Some(cache_path) = cache_path else {
                    return RefResolve::DecodeFailed;
                };
                let path = blob_sidecar_dir(cache_path).join(format!("{hash}.txt"));
                let Ok((text, actual_hash)) = read_utf8_hashed(&path, Some(expected_len)) else {
                    return RefResolve::DecodeFailed;
                };
                let ok = actual_hash == hash;
                resolve_found_verified(ok, text, actual_hash, RefResolve::DecodeFailed)
            } else {
                RefResolve::Found(value.clone())
            }
        }
        BlobEntry::FileRef {
            path,
            source_start_line,
            source_end_line,
        } => {
            let Ok((text, sha256)) =
                read_utf8_line_range_hashed(path, *source_start_line, *source_end_line)
            else {
                return RefResolve::Stale;
            };
            let ok = blob_ref_digest_matches(ref_id, &text, &sha256);
            resolve_found_verified(ok, text, sha256, RefResolve::Stale)
        }
    }
}

// Persisted session delta. Framing is hub SessionWal; payload stays engine-owned.
#[derive(Debug, Serialize, Deserialize)]
struct JournalEntry {
    refs: Vec<String>,
    state: RecoveryState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deleted_blob_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deleted_aliases: Vec<String>,
}

fn apply_deletions<'a>(
    state: &mut RecoveryState,
    blob_refs: impl IntoIterator<Item = &'a str>,
    aliases: impl IntoIterator<Item = &'a str>,
) {
    for alias in aliases {
        state.aliases.remove(alias);
        state.ambiguous_aliases.remove(alias);
    }
    for ref_id in blob_refs {
        state.blobs.remove(ref_id);
    }
}

fn recovery_unit_map<'a>(
    state: &'a RecoveryState,
    kind: &str,
) -> Option<&'a BTreeMap<String, StoredUnit>> {
    match kind {
        "unit" => Some(&state.units),
        "search" => Some(&state.search_hits),
        _ => None,
    }
}

fn state_entry_present(state: &RecoveryState, ref_id: &str) -> bool {
    recovery_maps!(contains state, ref_id)
}

fn session_delta(
    state: &RecoveryState,
    session_refs: &[String],
    config: &RecoveryConfig,
) -> RecoveryState {
    let mut delta = RecoveryState::empty(config);
    for ref_id in session_refs {
        recovery_maps!(copy delta, state, ref_id);
    }
    // These capped indexes are merged wholesale because they can change after
    // the persist that carried their target and have no session-ref identity.
    delta.aliases = state.aliases.clone();
    delta.shell_outcomes = state.shell_outcomes.clone();
    delta.shell_outcome_seq = state.shell_outcome_seq;
    delta.shell_outcome_epoch = state.shell_outcome_epoch;
    delta.ambiguous_aliases = state.ambiguous_aliases.clone();
    delta.transparency = state.transparency.clone();
    // The alias derivation key aliases share semantics with the alias table:
    // it must survive journal-append persists, not only snapshots.
    delta.alias_key = state.alias_key.clone();
    delta.order = session_refs
        .iter()
        .filter(|ref_id| state_entry_present(&delta, ref_id))
        .cloned()
        .collect();
    delta
}

fn copy_map_entry<T: Clone>(
    destination: &mut BTreeMap<String, T>,
    source: &BTreeMap<String, T>,
    ref_id: &str,
) {
    if let Some(value) = source.get(ref_id) {
        destination.insert(ref_id.to_string(), value.clone());
    }
}

// Replay complete SessionWal records. Torn tails are fail-open inside
// `SessionWal::replay` (complete prefix kept). IO errors must not look like
// a clean snapshot-only load: persist would then `publish_snapshot` and
// `clear_wal`, dropping committed journal bytes.
fn apply_session_wal(
    mut state: RecoveryState,
    path: &Path,
    config: &RecoveryConfig,
) -> Result<RecoveryState, RecoveryError> {
    let wal = recovery_session_wal(path, config)?;
    let replay = wal.replay()?;
    for record in replay.records {
        let Ok(entry) = serde_json::from_slice::<JournalEntry>(&record) else {
            // Complete frame, not a torn tail: stop at the last good record.
            break;
        };
        let JournalEntry {
            refs,
            state: delta,
            deleted_blob_refs,
            deleted_aliases,
        } = entry;
        let accumulated = std::mem::replace(&mut state, RecoveryState::empty(config));
        state = merge_states(accumulated, delta, &refs, config);
        apply_deletions(
            &mut state,
            deleted_blob_refs.iter().map(String::as_str),
            deleted_aliases.iter().map(String::as_str),
        );
    }
    Ok(state)
}

fn read_limited_utf8<R: Read>(
    reader: R,
    max_load_bytes: usize,
) -> Result<Option<String>, RecoveryError> {
    let mut limited = reader.take((max_load_bytes as u64).saturating_add(1));
    let mut bytes = Vec::new();
    limited.read_to_end(&mut bytes)?;
    if bytes.len() > max_load_bytes {
        return Ok(None);
    }
    Ok(String::from_utf8(bytes).ok())
}

fn merge_map_entries<T>(
    session: &HashSet<&str>,
    dst: &mut BTreeMap<String, T>,
    src: BTreeMap<String, T>,
) {
    for (ref_id, value) in src {
        if session.contains(ref_id.as_str()) || dst.contains_key(&ref_id) {
            dst.insert(ref_id, value);
        }
    }
}

fn saturating_usize_sum<I: IntoIterator<Item = usize>>(iter: I) -> usize {
    iter.into_iter().fold(0usize, usize::saturating_add)
}

fn shell_outcome_rank(outcome: &ShellOutcome) -> (u64, u64) {
    (outcome.epoch, outcome.seq)
}

fn rebase_shell_outcomes_dense(state: &mut RecoveryState) {
    let mut keys: Vec<(u64, u64, String)> = state
        .shell_outcomes
        .iter()
        .map(|(key, outcome)| (outcome.epoch, outcome.seq, key.clone()))
        .collect();
    keys.sort_unstable();
    for (index, (_, _, key)) in keys.iter().enumerate() {
        if let Some(outcome) = state.shell_outcomes.get_mut(key) {
            outcome.epoch = state.shell_outcome_epoch;
            outcome.seq = (index as u64).saturating_add(1);
        }
    }
    state.shell_outcome_seq = keys.len() as u64;
}

fn next_shell_outcome_clock(state: &mut RecoveryState) -> (u64, u64) {
    if let Some(seq) = state.shell_outcome_seq.checked_add(1) {
        state.shell_outcome_seq = seq;
        return (state.shell_outcome_epoch, seq);
    }
    if let Some(epoch) = state.shell_outcome_epoch.checked_add(1) {
        state.shell_outcome_epoch = epoch;
        state.shell_outcome_seq = 1;
        return (epoch, 1);
    }
    rebase_shell_outcomes_dense(state);
    if let Some(seq) = state.shell_outcome_seq.checked_add(1) {
        state.shell_outcome_seq = seq;
        return (state.shell_outcome_epoch, seq);
    }
    let seq = (state.shell_outcomes.len() as u64).saturating_add(1);
    state.shell_outcome_seq = seq;
    (state.shell_outcome_epoch, seq)
}

fn merge_shell_outcome_clock(merged: &mut RecoveryState, current: &RecoveryState) {
    if current.shell_outcome_epoch > merged.shell_outcome_epoch {
        merged.shell_outcome_epoch = current.shell_outcome_epoch;
        merged.shell_outcome_seq = current.shell_outcome_seq;
    } else if current.shell_outcome_epoch == merged.shell_outcome_epoch {
        merged.shell_outcome_seq = merged.shell_outcome_seq.max(current.shell_outcome_seq);
    }
}

fn trim_shell_outcomes(outcomes: &mut BTreeMap<String, ShellOutcome>) {
    while outcomes.len() > MAX_SHELL_OUTCOMES {
        let Some(victim) = outcomes
            .iter()
            .min_by_key(|(_, outcome)| shell_outcome_rank(outcome))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        outcomes.remove(&victim);
    }
}

fn merge_states(
    existing: RecoveryState,
    current: RecoveryState,
    session_refs: &[String],
    config: &RecoveryConfig,
) -> RecoveryState {
    let session: HashSet<&str> = session_refs.iter().map(String::as_str).collect();
    let mut merged = existing;
    merge_shell_outcome_clock(&mut merged, &current);
    recovery_maps!(merge & session, merged, current);
    for (alias, target) in current.aliases {
        if merged
            .aliases
            .get(&alias)
            .is_some_and(|existing| existing != &target)
        {
            merged.ambiguous_aliases.insert(alias);
        } else {
            merged.aliases.insert(alias, target);
        }
    }
    if current.ordinal_generation > merged.ordinal_generation {
        merged.ordinal_generation = current.ordinal_generation;
        merged.next_ordinal = current.next_ordinal;
    } else if current.ordinal_generation == merged.ordinal_generation {
        merged.next_ordinal = merged.next_ordinal.max(current.next_ordinal);
    }
    merged.order.extend(session_refs.iter().cloned());
    let mut seen = HashSet::new();
    merged.order.retain(|ref_id| seen.insert(ref_id.clone()));
    for (key, incoming) in current.shell_outcomes {
        match merged.shell_outcomes.get(&key) {
            Some(existing) if shell_outcome_rank(existing) > shell_outcome_rank(&incoming) => {}
            _ => {
                merged.shell_outcomes.insert(key, incoming);
            }
        }
    }
    trim_shell_outcomes(&mut merged.shell_outcomes);
    merged.ambiguous_aliases.extend(current.ambiguous_aliases);
    merged.transparency.merge_concurrent(&current.transparency);
    // Alias derivation key: prefer the existing (already-shared) key so all
    // engines keep agreeing; adopt the current key only when none exists yet.
    if merged.alias_key.is_none() {
        merged.alias_key = current.alias_key;
    }
    merged.configure(config);
    merged
}

fn evict_prefix<T>(
    items: &mut BTreeMap<String, T>,
    order: &mut Vec<String>,
    prefix: &str,
    limit: usize,
) {
    let excess = items.len().saturating_sub(limit);
    if excess == 0 {
        return;
    }
    let mut victims = HashSet::with_capacity(excess);
    for ref_id in order
        .iter()
        .filter(|ref_id| ref_id.starts_with(prefix) && items.contains_key(*ref_id))
    {
        victims.insert(ref_id.clone());
        if victims.len() == excess {
            break;
        }
    }
    if victims.len() < excess {
        for ref_id in items.keys() {
            victims.insert(ref_id.clone());
            if victims.len() == excess {
                break;
            }
        }
    }
    items.retain(|ref_id, _| !victims.contains(ref_id));
    order.retain(|ref_id| !victims.contains(ref_id));
}
fn private_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn create_private_new(path: &Path) -> std::io::Result<fs::File> {
    private_open_options()
        .write(true)
        .create_new(true)
        .open(path)
}

fn recovery_file_ref(text: &str, path: Option<&Path>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.map(path_identity_text).unwrap_or_default());
    hasher.update(b":");
    hasher.update(text.as_bytes());
    format!("tz://file/f{}", &digest_hex(hasher)[..16])
}

macro_rules! path_identity_platform {
    (unix) => {
        fn path_identity_text(path: &Path) -> String {
            use std::os::unix::ffi::OsStrExt;
            format!("unix:{}", encode_hex(path.as_os_str().as_bytes()))
        }
        fn path_from_identity_text(identity: &str) -> Option<PathBuf> {
            use std::os::unix::ffi::OsStringExt;
            Some(PathBuf::from(OsString::from_vec(decode_hex_bytes(
                identity.strip_prefix("unix:")?,
            )?)))
        }
    };
    (windows) => {
        fn path_identity_text(path: &Path) -> String {
            use std::os::windows::ffi::OsStrExt;
            let mut bytes = Vec::new();
            for unit in path.as_os_str().encode_wide() {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
            format!("windows:{}", encode_hex(&bytes))
        }
        fn path_from_identity_text(identity: &str) -> Option<PathBuf> {
            use std::os::windows::ffi::OsStringExt;
            let bytes = decode_hex_bytes(identity.strip_prefix("windows:")?)?;
            let mut chunks = bytes.chunks_exact(2);
            let units: Vec<u16> = chunks
                .by_ref()
                .map(|p| u16::from_be_bytes([p[0], p[1]]))
                .collect();
            if !chunks.remainder().is_empty() {
                return None;
            }
            Some(PathBuf::from(OsString::from_wide(&units)))
        }
    };
    (other) => {
        fn path_identity_text(path: &Path) -> String {
            format!("display:{}", path.to_string_lossy())
        }
        fn path_from_identity_text(identity: &str) -> Option<PathBuf> {
            Some(PathBuf::from(identity.strip_prefix("display:")?))
        }
    };
}
#[cfg(unix)]
path_identity_platform!(unix);
#[cfg(windows)]
path_identity_platform!(windows);
#[cfg(not(any(unix, windows)))]
path_identity_platform!(other);

fn decode_hex_bytes(hex: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    let mut chunks = hex.as_bytes().chunks_exact(2);
    for pair in chunks.by_ref() {
        let high = hex_value(pair[0])?;
        let low = hex_value(pair[1])?;
        bytes.push((high << 4) | low);
    }
    if !chunks.remainder().is_empty() {
        return None;
    }
    Some(bytes)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn recovery_lock_path(path: &Path) -> PathBuf {
    append_file_name_suffix(path, ".lock")
}

fn recovery_tmp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp_name = OsString::from(".");
    tmp_name.push(
        path.file_name()
            .map(OsString::from)
            .unwrap_or_else(|| OsString::from("recovery")),
    );
    let nonce = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    tmp_name.push(format!(".{}.{nonce}.tmp", std::process::id()));
    parent.join(tmp_name)
}

fn append_file_name_suffix(path: &Path, suffix: &str) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut file_name = path
        .file_name()
        .map(OsString::from)
        .unwrap_or_else(|| OsString::from("recovery"));
    file_name.push(suffix);
    parent.join(file_name)
}

pub(crate) struct PersistLock {
    file: fs::File,
}

impl PersistLock {
    pub(crate) fn acquire(path: PathBuf) -> Result<Self, RecoveryError> {
        Self::acquire_with_retries(path, LOCK_RETRIES)
    }

    fn acquire_with_retries(path: PathBuf, retries: usize) -> Result<Self, RecoveryError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)?;
        for attempt in 0..retries {
            match FileExt::try_lock(&file) {
                Ok(()) => {
                    file.set_len(0)?;
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self { file });
                }
                Err(TryLockError::Error(err)) if err.kind() != std::io::ErrorKind::WouldBlock => {
                    return Err(err.into());
                }
                Err(_) if attempt + 1 < retries => thread::sleep(LOCK_RETRY_DELAY),
                Err(_) => {}
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("timed out waiting for lock {}", path.display()),
        )
        .into())
    }
}

impl Drop for PersistLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn is_ephemeral_source_path(path_text: &str) -> bool {
    path_text.starts_with("shell:") || path_text.starts_with("search:")
}

fn fingerprint_for_stored_payload(
    path: Option<&Path>,
    source_start_line: Option<usize>,
    source_end_line: Option<usize>,
) -> Option<SourceFingerprint> {
    if source_start_line.is_some() || source_end_line.is_some() {
        return None;
    }
    let path = path?;
    if is_ephemeral_source_path(&path.to_string_lossy()) {
        return None;
    }
    source_fingerprint(path)
}

fn fingerprint_from_meta(meta: &fs::Metadata, sha256: String) -> SourceFingerprint {
    SourceFingerprint {
        size: meta.len(),
        mtime_ns: mtime_ns(meta),
        sha256,
    }
}

fn source_fingerprint_from_sha256(path: &Path, sha256: &str) -> Option<SourceFingerprint> {
    Some(fingerprint_from_meta(&file_meta(path)?, sha256.to_string()))
}

fn hash_file_sha256(path: &Path) -> Option<(fs::Metadata, String)> {
    let meta = file_meta(path)?;
    let mut file = fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    read_file_chunks(&mut file, |chunk| {
        hasher.update(chunk);
        Ok(())
    })
    .ok()?;
    Some((meta, digest_hex(hasher)))
}

fn source_fingerprint(path: &Path) -> Option<SourceFingerprint> {
    let (meta, sha256) = hash_file_sha256(path)?;
    Some(fingerprint_from_meta(&meta, sha256))
}

fn file_meta(path: &Path) -> Option<fs::Metadata> {
    let meta = fs::metadata(path).ok()?;
    meta.is_file().then_some(meta)
}

fn mtime_ns(meta: &fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}
