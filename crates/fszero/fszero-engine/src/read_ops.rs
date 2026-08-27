use super::access_log::{content_hash_from_ref, rel_path_for_log_with_canon};
use super::racc::{LineEndingPolicy, range_digest_hex};
use super::target_ref::{LineWindow, parse_target_ref};
use super::*;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

#[cfg(test)]
std::thread_local! {
    static TEST_READ_BETWEEN_CAPTURE: std::cell::Cell<Option<fn(&Path)>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) struct ReadCaptureInterfereGuard(Option<fn(&Path)>);

#[cfg(test)]
impl Drop for ReadCaptureInterfereGuard {
    fn drop(&mut self) {
        TEST_READ_BETWEEN_CAPTURE.with(|hook| hook.set(self.0));
    }
}

#[cfg(test)]
pub(crate) fn test_read_between_capture(interfere: Option<fn(&Path)>) -> ReadCaptureInterfereGuard {
    let previous = TEST_READ_BETWEEN_CAPTURE.with(|hook| hook.replace(interfere));
    ReadCaptureInterfereGuard(previous)
}

fn file_len_mtime(path: &Path) -> Option<(u64, SystemTime)> {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok().map(|modified| (meta.len(), modified)))
}

/// Full-file capture that refuses a torn snapshot. If len+mtime change
/// across the read, fail closed so callers never hash mixed bytes
/// (fszero-ai-filesystem-excellence-jqf.9.2). Range/`#L` captures share the
/// same hook and identity check (jqf.9.3). Missing stats stay unverified
/// (same as the prior cache-skip path).
pub(crate) fn read_stable_file_bytes(path: &Path) -> Result<(Vec<u8>, Option<SystemTime>), String> {
    crate::path::refuse_non_regular_file(path)?;
    let before = file_len_mtime(path);
    let content = fs::read(path).map_err(|e| format!("read failed: {e}"))?;
    #[cfg(test)]
    TEST_READ_BETWEEN_CAPTURE.with(|hook| {
        if let Some(interfere) = hook.get() {
            interfere(path);
        }
    });
    let after = file_len_mtime(path);
    match (before, after) {
        (Some(before), Some(after)) if before != after => Err("file changed during read".into()),
        (Some(_), Some((_, mtime))) => Ok((content, Some(mtime))),
        _ => Ok((content, None)),
    }
}

impl FSZeroSession {
    /// Return the stable bytes backing the current complete read view.
    /// The Arc clones share session cache storage; this performs no I/O or hashing.
    pub fn last_stable_complete_read(&self) -> Option<(Arc<Vec<u8>>, Arc<str>)> {
        let view = self.views.views.get(&self.views.last_view_id)?;
        let cached = self.caches.content.get(view.path.as_path())?;
        if cached.content_ref.as_ref() != view.content_ref.as_ref() {
            return None;
        }
        Some((Arc::clone(&cached.bytes), Arc::clone(&cached.content_ref)))
    }

    pub fn do_read(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let raw_arg = arg.unwrap_or("src/main.rs");
        // Snap-to-file target refs are accepted verbatim (bead 99q7): the exact
        // string a discovery hit emitted is a valid read arg, no re-derivation.
        let line_window = super::target_ref::parse_target_ref(raw_arg).map(|(_, w)| w);
        let (path_arg, byte_range) = match parse_read_arg(raw_arg) {
            Ok(parsed) => parsed,
            Err(e) => return format!("bad read: {e}"),
        };
        // Warm read fast path: path-cache hit + content-cache hit + single
        // metadata check. When mtime/len still match, skip re-canonicalize
        // (path was already root-confined when inserted; content identity
        // is proven by the cache entry). Mutation/write paths still always
        // revalidate via resolve_existing_path_cached.
        if byte_range.is_none() && line_window.is_none() {
            if let Some(hit) = self.try_warm_read_hit(root, path_arg) {
                return hit;
            }
        }
        // Absolute reads resolve against the opt-in read-only scratch
        // allowlist so one logical read stays one op (no copy-into-root).
        // Writes never reach here and stay root-jailed.
        let full_path = if Path::new(path_arg).is_absolute() {
            match super::path::resolve_scratch_read_path(path_arg) {
                Ok(p) => p,
                Err(e) => return format!("bad path: {e}"),
            }
        } else {
            match self.resolve_existing_path_cached(root, path_arg) {
                Ok(p) => p,
                Err(e) => return format!("bad path: {}", e),
            }
        };
        if let Some(window) = line_window {
            return match line_window_capture(&full_path, window) {
                Ok((range, content)) => self.finish_range_read(root, &full_path, range, content),
                Err(e) => super::op_result::op0("read", e),
            };
        }
        if let Some(range) = byte_range {
            return self.do_read_range(root, &full_path, range);
        }
        // Content may have been cold when try_warm_read_hit ran (path miss);
        // re-check after full resolve.
        if let Some(cached) = self.caches.content.get(&full_path) {
            if content_cache_fresh(&full_path, cached) {
                let len = cached.bytes.len();
                let cref = Arc::clone(&cached.content_ref);
                // Not a consecutive sticky arg — rebuild access path metadata.
                return self.finish_warm_read(root, &full_path, len, cref, false);
            }
        }
        let (content, stable_mtime) = match read_stable_file_bytes(&full_path) {
            Ok(captured) => captured,
            Err(e) => return super::op_result::op0("read", e),
        };
        let len = content.len();
        let cref: Arc<str> = Arc::from(self.recovery.put_content_ref(&content));
        if let Some(mtime) = stable_mtime {
            self.caches.content.insert(
                full_path.clone(),
                ReadCacheEntry {
                    bytes: Arc::new(content),
                    mtime,
                    content_ref: Arc::clone(&cref),
                },
            );
        }
        self.finish_warm_read(root, &full_path, len, cref, false)
    }

    /// Path+content cache hit with one metadata check; no re-canonicalize.
    fn try_warm_read_hit(&mut self, root: Option<&Path>, path_arg: &str) -> Option<String> {
        // True only when the *previous* op already used this arg (consecutive).
        // Path-cache promote must not reuse last_access_rel from another file.
        let consecutive_arg = self.caches.last_path_arg.as_deref() == Some(path_arg);
        let full_path = if consecutive_arg {
            Arc::clone(self.caches.last_path.as_ref()?)
        } else {
            let key = if root.is_some() {
                path_arg.to_string()
            } else {
                format!("\0{path_arg}")
            };
            let p = Arc::clone(self.caches.paths.get(&key)?);
            // Promote to sticky so the next identical arg skips HashMap.
            self.caches.last_path_arg = Some(path_arg.to_string());
            self.caches.last_path = Some(Arc::clone(&p));
            p
        };
        // Lexical root confinement using session-cached root_canon (no syscall).
        if let (Some(_), Some(rc)) = (root, self.root_canon.as_deref()) {
            if !crate::path::canonical_path_within_root(rc, full_path.as_path()) {
                return None;
            }
        }
        let cached = self.caches.content.get(full_path.as_path())?;
        if !content_cache_fresh(full_path.as_path(), cached) {
            return None;
        }
        let len = cached.bytes.len();
        let cref = Arc::clone(&cached.content_ref);
        Some(self.finish_warm_read(root, full_path.as_path(), len, cref, consecutive_arg))
    }

    fn finish_warm_read(
        &mut self,
        root: Option<&Path>,
        full_path: &Path,
        len: usize,
        content_ref: Arc<str>,
        consecutive_arg: bool,
    ) -> String {
        let small_id = (self.op_count % 999) + 1;
        self.views.last_view_id = small_id;
        // Capture before store_read_view_ref may update last_read_ref.
        let same_ref = !self.latest_read_ref_changed(content_ref.as_ref());
        let path_arc = if consecutive_arg
            || self
                .caches
                .last_path
                .as_ref()
                .is_some_and(|p| p.as_path() == full_path)
        {
            Arc::clone(self.caches.last_path.as_ref().expect("path set"))
        } else {
            Arc::new(full_path.to_path_buf())
        };
        self.store_read_view_ref(small_id, path_arc, content_ref.clone(), !same_ref);
        // Reuse sticky rel/hash only on consecutive identical path args.
        if !consecutive_arg || self.caches.last_access_rel.is_none() {
            self.caches.last_access_rel = Some(Arc::from(rel_path_for_log_with_canon(
                root,
                self.root_canon.as_deref(),
                full_path,
            )));
        }
        if !(consecutive_arg && same_ref && self.caches.last_access_hash.is_some()) {
            self.caches.last_access_hash =
                Some(Arc::from(content_hash_from_ref(content_ref.as_ref())));
        }
        // Arc clone is cheap; record_access needs &str while holding &mut self.
        let rel = Arc::clone(self.caches.last_access_rel.as_ref().expect("access rel"));
        let hash = Arc::clone(self.caches.last_access_hash.as_ref().expect("access hash"));
        self.record_access("read", rel.as_ref(), hash.as_ref());
        // Response string omits view id — safe to reuse on sticky same-ref re-read.
        if consecutive_arg
            && same_ref
            && self.caches.last_warm_len == Some(len)
            && let Some(resp) = self.caches.last_warm_response.clone()
        {
            return resp;
        }
        let resp = format!("read:{} bytes ref={content_ref}", len);
        self.caches.last_warm_len = Some(len);
        self.caches.last_warm_response = Some(resp.clone());
        resp
    }

    fn do_read_range(&mut self, root: Option<&Path>, full_path: &Path, range: ByteRange) -> String {
        match read_range_bytes(full_path, range) {
            Ok(content) => self.finish_range_read(root, full_path, range, content),
            Err(e) => super::op_result::op0("read", e),
        }
    }

    fn finish_range_read(
        &mut self,
        root: Option<&Path>,
        full_path: &Path,
        range: ByteRange,
        content: Vec<u8>,
    ) -> String {
        let small_id = (self.op_count % 999) + 1;
        self.views.last_view_id = small_id;
        self.store_read_view(small_id, full_path, &content);
        let rel = rel_path_for_log_with_canon(root, self.root_canon.as_deref(), full_path);
        let cref = self
            .views
            .views
            .get(&small_id)
            .map(|v| Arc::clone(&v.content_ref))
            .unwrap_or_else(|| Arc::from(""));
        self.record_access("read", &rel, content_hash_from_ref(cref.as_ref()));
        format!(
            "read:{} bytes range:{}-{} ref={cref}",
            content.len(),
            range.start,
            range.end
        )
    }

    fn store_read_view(&mut self, small_id: u32, full_path: &Path, content: &[u8]) {
        let cref: Arc<str> = Arc::from(self.recovery.put_content_ref(content));
        self.store_read_view_ref(small_id, Arc::new(full_path.to_path_buf()), cref, true);
    }

    fn latest_read_ref_changed(&self, content_ref: &str) -> bool {
        self.views.last_read_ref.as_deref() != Some(content_ref)
    }

    fn store_read_view_ref(
        &mut self,
        small_id: u32,
        full_path: Arc<PathBuf>,
        content_ref: Arc<str>,
        persist_numbered_aliases: bool,
    ) {
        self.views.views.insert(
            small_id,
            ReadViewMeta {
                path: Arc::clone(&full_path),
                content_ref: Arc::clone(&content_ref),
            },
        );
        // When the content ref is unchanged, skip durable alias churn (warm
        // re-read of the same bytes). expand("read") still resolves via the
        // prior read/ref keys; view small_ids only need durable keys when
        // the agent will expand a numbered view after this op.
        if !persist_numbered_aliases {
            return;
        }
        let path_bytes = full_path.to_string_lossy();
        self.recovery
            .put_key(&format!("view_{}/path", small_id), path_bytes.as_bytes());
        let ref_bytes = content_ref.as_bytes();
        self.recovery
            .put_key(&format!("view_{}/ref", small_id), ref_bytes);
        self.recovery
            .put_key(&format!("r{}/ref", small_id), ref_bytes);
        // Dual named keys: expand("read") prefers read/ref then falls back to
        // read (recovery::expand_current_store). Both must stay in sync.
        self.recovery.put_key("read", ref_bytes);
        self.recovery.put_key("read/ref", ref_bytes);
        self.views.last_read_ref = Some(content_ref);
    }
}

/// One metadata check: mtime + len still match the content-cache entry.
pub fn content_cache_fresh(path: &Path, cached: &ReadCacheEntry) -> bool {
    fs::metadata(path)
        .ok()
        .and_then(|meta| {
            meta.modified().ok().map(|mtime| {
                mtime == cached.mtime
                    && usize::try_from(meta.len()).ok() == Some(cached.bytes.len())
            })
        })
        .unwrap_or(false)
}

const MAX_READ_RANGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

/// Path-read range suffix `path#Bstart-end`. This addresses LIVE files, not
/// immutable blobs, and is NOT a ZeroRef: its ranges clamp to file length
/// (`read_range_bytes`), whereas `#B` on `(fz|gz|tz)://blob/…` refs is the
/// strict v1 fragment algebra (`core/zeroref.rs`) and never clamps
/// (docs/design/zeroref-v1-annex.md §9.6).
pub fn parse_read_arg(arg: &str) -> Result<(&str, Option<ByteRange>), String> {
    // Canonical snap-to-file target refs (`path#Lstart-Lend`, bead 99q7) are
    // accepted verbatim: the discovery hit's ref is the read arg, unedited.
    if let Some((path, _window)) = super::target_ref::parse_target_ref(arg) {
        return Ok((path, None));
    }
    let Some((path, suffix)) = arg.rsplit_once("#B") else {
        return Ok((arg, None));
    };
    if path.is_empty() {
        return Err("empty path rejected".to_string());
    }
    let (start, end) = suffix
        .split_once('-')
        .ok_or_else(|| "bad byte range".to_string())?;
    let start = start
        .parse::<u64>()
        .map_err(|_| "bad range start".to_string())?;
    let end = end
        .parse::<u64>()
        .map_err(|_| "bad range end".to_string())?;
    if end < start {
        return Err("range end before start".to_string());
    }
    Ok((path, Some(ByteRange { start, end })))
}

/// Byte range covering a 1-based inclusive line window of `full_path`.
pub fn line_window_range(
    full_path: &Path,
    window: super::target_ref::LineWindow,
) -> Result<ByteRange, String> {
    line_window_capture(full_path, window).map(|(range, _)| range)
}

/// One capture for `#L` windows: offsets and bytes come from the same snapshot.
fn line_window_capture(
    full_path: &Path,
    window: super::target_ref::LineWindow,
) -> Result<(ByteRange, Vec<u8>), String> {
    let (content, _) = read_stable_file_bytes(full_path)?;
    let text = std::str::from_utf8(&content).map_err(|e| format!("read failed: {e}"))?;
    let (start, end) = super::target_ref::window_byte_range(text, window);
    let sliced = content
        .get(start as usize..end as usize)
        .ok_or_else(|| "bad range".to_string())?
        .to_vec();
    Ok((ByteRange { start, end }, sliced))
}

pub fn read_range_bytes(full_path: &Path, range: ByteRange) -> Result<Vec<u8>, String> {
    crate::path::refuse_non_regular_file(full_path)?;
    let before = file_len_mtime(full_path);
    let mut file = fs::File::open(full_path).map_err(|e| format!("open failed: {e}"))?;
    let len = file
        .metadata()
        .map_err(|e| format!("metadata failed: {e}"))?
        .len();
    let start = range.start.min(len);
    let end = range.end.min(len);
    if end < start {
        return Err("bad range".to_string());
    }
    let read_len = end - start;
    if read_len > MAX_READ_RANGE_BYTES as u64 {
        return Err(format!("range too large: {read_len}"));
    }
    file.seek(SeekFrom::Start(start))
        .map_err(|e| format!("seek failed: {e}"))?;
    let mut content = vec![0u8; read_len as usize];
    file.read_exact(&mut content)
        .map_err(|e| format!("read failed: {e}"))?;
    #[cfg(test)]
    TEST_READ_BETWEEN_CAPTURE.with(|hook| {
        if let Some(interfere) = hook.get() {
            interfere(full_path);
        }
    });
    let after = file_len_mtime(full_path);
    match (before, after) {
        (Some(before), Some(after)) if before != after => Err("file changed during read".into()),
        _ => Ok(content),
    }
}

// ---------------------------------------------------------------------------
// resolve_span op (V6-F3): ONE resolver operation binding EvidencePage +
// AsofJournal against a snapshot root.
// ---------------------------------------------------------------------------

/// Errors from the `resolve_span` resolver operation (V6-F3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveSpanError {
    /// Arg is not a canonical `<path>#Lx-Ly` target ref.
    NotATargetRef(String),
    /// The ref's path is not covered by the snapshot root.
    NotInSnapshot(String),
    /// Journal as-of read failure (delegated `AsofJournal` error).
    Asof(AsofError),
    /// File bytes at the as-of ordinal do not match the snapshot root's
    /// record for that path (stale/foreign root -- fails loud).
    StaleFile {
        path: String,
        expected: String,
        actual: String,
    },
    /// The line window addresses no bytes (start beyond the last line).
    EmptyWindow { path: String, window: LineWindow },
    /// Range or digest verification failure on the derived evidence page.
    Evidence(EvidencePageError),
}

impl std::fmt::Display for ResolveSpanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotATargetRef(arg) => write!(f, "not a canonical target ref: {arg}"),
            Self::NotInSnapshot(path) => write!(f, "path not in snapshot root: {path}"),
            Self::Asof(e) => write!(f, "as-of read: {e}"),
            Self::StaleFile {
                path,
                expected,
                actual,
            } => write!(
                f,
                "stale source: {path} bytes do not match snapshot root (expected {expected}, actual {actual})"
            ),
            Self::EmptyWindow { path, window } => write!(
                f,
                "line window L{}-L{} of {path} addresses no bytes",
                window.start, window.end
            ),
            Self::Evidence(e) => write!(f, "evidence: {e}"),
        }
    }
}
impl std::error::Error for ResolveSpanError {}

/// Outcome of `resolve_span`: a root-verified evidence page plus the raw
/// (byte-exact) window digest so canonical and raw identity stay comparable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSpan {
    pub page: EvidencePage,
    /// Raw byte digest of the exact window bytes (byte-exact identity).
    pub raw_digest_hex: String,
}

/// ONE resolver operation (V6-F3): given a canonical target ref
/// (`<path>#Lx-Ly`) plus a snapshot root and an as-of journal, resolve the
/// span to exact bytes and an EvidencePage verified against that root.
///
/// - `snapshot` is the committed tree identity; `journal` supplies exact
///   bytes as of `as_of`. This is the production caller of `AsofJournal`
///   (previously exported with no caller).
/// - Line windows address logical lines over raw bytes; the evidence digest
///   is derived over canonical-LF window bytes under an explicit
///   [`LineEndingPolicy::Lf`] declared in the page identity, so CRLF and LF
///   checkouts of the same file share one evidence identity while raw byte
///   digests stay distinct.
/// - Stale roots fail loud: journal bytes that do not match the snapshot's
///   file record for the path return [`ResolveSpanError::StaleFile`].
pub fn resolve_span(
    target: &str,
    snapshot: &ExactSnapshot,
    journal: &AsofJournal,
    as_of: i64,
) -> Result<ResolvedSpan, ResolveSpanError> {
    let (path, window) = parse_target_ref(target)
        .ok_or_else(|| ResolveSpanError::NotATargetRef(target.to_string()))?;
    let record = snapshot
        .get(path)
        .ok_or_else(|| ResolveSpanError::NotInSnapshot(path.to_string()))?;
    let bytes = journal
        .read_as_of(&record.path, as_of)
        .map_err(ResolveSpanError::Asof)?;
    // Bind the as-of bytes to the snapshot root: the file record digest must
    // match, or the root is stale/foreign for these bytes (fails loud).
    let actual = raw_content_digest_hex(&bytes);
    if actual != record.digest_hex {
        return Err(ResolveSpanError::StaleFile {
            path: record.path.clone(),
            expected: record.digest_hex.clone(),
            actual,
        });
    }
    let (start, end) = window_byte_range_bytes(&bytes, window);
    if start == end {
        return Err(ResolveSpanError::EmptyWindow {
            path: record.path.clone(),
            window,
        });
    }
    let range = ExactRange::new(start, end).map_err(ResolveSpanError::Evidence)?;
    let page = EvidencePage::extract_line_addressed(
        snapshot.root_digest_hex(),
        &record.path,
        &bytes,
        range,
        LineEndingPolicy::Lf,
    )
    .map_err(ResolveSpanError::Evidence)?;
    page.verify_against_source(snapshot.root_digest_hex(), &bytes)
        .map_err(ResolveSpanError::Evidence)?;
    let raw_digest_hex = range_digest_hex(&page.bytes);
    Ok(ResolvedSpan {
        page,
        raw_digest_hex,
    })
}

/// Byte offsets of a 1-based inclusive line window over raw bytes (no UTF-8
/// interpretation). A line is any run terminated by `\n`; a final
/// unterminated run counts as a line. Semantics match
/// `target_ref::window_byte_range` for text content, byte-exact for all input.
fn window_byte_range_bytes(content: &[u8], window: LineWindow) -> (u64, u64) {
    let mut offset = 0u64;
    let mut start = None;
    let mut end = content.len() as u64;
    for (idx, chunk) in content.split_inclusive(|&b| b == b'\n').enumerate() {
        let line_no = idx + 1;
        if line_no == window.start {
            start = Some(offset);
        }
        offset += chunk.len() as u64;
        if line_no == window.end {
            end = offset;
            break;
        }
    }
    let start = start.unwrap_or(content.len() as u64);
    (start, end.max(start))
}

/// Plain SHA-256 over raw bytes -- the `ExactSnapshot` file-record digest
/// derivation (no domain tag), hex-encoded.
fn raw_content_digest_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode_lower(h.finalize().as_slice())
}

fn hex_encode_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// Parse the `resolve_span` op arg: JSON `{"ref","root","as_of"}` or a plain
/// canonical target ref. Returns (target, claimed root hex, as_of).
fn parse_resolve_span_arg(arg: &str) -> (String, Option<String>, Option<i64>) {
    if let Ok(v) = serde_json::from_str::<Value>(arg) {
        let target = v
            .get("ref")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let root = v.get("root").and_then(Value::as_str).map(|r| {
            r.strip_prefix("fz://snapshot/")
                .unwrap_or(r)
                .to_ascii_lowercase()
        });
        let as_of = v.get("as_of").and_then(Value::as_i64);
        return (target, root, as_of);
    }
    (arg.to_string(), None, None)
}

/// Recursive scan of `root` into (relative path, bytes) pairs. Symlinks are
/// never followed (loop-safe); zerostack store dirs (`.fszero`) are skipped.
fn scan_tree(root: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), String> {
        let rd = fs::read_dir(dir).map_err(|e| format!("scan {}: {e}", dir.display()))?;
        for entry in rd.flatten() {
            let ft = entry
                .file_type()
                .map_err(|e| format!("stat {}: {e}", entry.path().display()))?;
            if ft.is_dir() {
                if entry.file_name().to_string_lossy() == ".fszero" {
                    continue;
                }
                walk(&entry.path(), root, out)?;
            } else if ft.is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_| format!("path escape: {}", entry.path().display()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = fs::read(entry.path())
                    .map_err(|e| format!("read {}: {e}", entry.path().display()))?;
                out.push((rel, bytes));
            }
        }
        Ok(())
    }
    walk(root, root, out)
}

impl FSZeroSession {
    /// `resolve_span` op (V6-F3): resolve a canonical target ref against a
    /// snapshot root derived from the current workspace tree, returning exact
    /// bytes (minted `fz://blob` ref) + a root-verified `EvidencePage`.
    ///
    /// Arg forms:
    /// - `<path>#L<start>-L<end>` -- resolve against the scanned tree at its
    ///   latest ordinal; the snapshot root is the scanned tree's digest.
    /// - JSON `{"ref":"<path>#Lx-Ly","root":"<hex|fz://snapshot/<hex>>","as_of":<i64>}`
    ///   -- explicit root commitment and ordinal; a root that does not match
    ///   the current tree fails loud, as does an out-of-range ordinal.
    pub fn do_resolve_span(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let Some(root) = root else {
            return super::op_result::op0("resolve_span", "requires a workspace root");
        };
        let raw = arg.unwrap_or("");
        let (target, claimed_root, as_of_opt) = parse_resolve_span_arg(raw);
        let target = target.trim();
        if target.is_empty() {
            return super::op_result::op0("resolve_span", "missing target ref");
        }
        // Scan the tree once: journal (as-of replay) + snapshot (root digest)
        // derive from the same bytes.
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        if let Err(e) = scan_tree(root, &mut files) {
            return super::op_result::op0("resolve_span", e);
        }
        if files.is_empty() {
            return super::op_result::op0("resolve_span", "no files under root");
        }
        let snapshot = match ExactSnapshot::from_files(files.iter().cloned()) {
            Ok(s) => s,
            Err(e) => return super::op_result::op0("resolve_span", e),
        };
        if let Some(claimed) = claimed_root {
            if claimed != snapshot.root_digest_hex() {
                return super::op_result::op0(
                    "resolve_span",
                    EvidencePageError::StaleSource {
                        expected_root: claimed,
                        actual_root: snapshot.root_digest_hex().to_string(),
                    },
                );
            }
        }
        let mut journal = AsofJournal::new();
        for (i, (path, bytes)) in files.iter().enumerate() {
            journal.apply(AsofMutation::Put {
                seq: (i + 1) as i64,
                path: path.clone(),
                bytes: bytes.clone(),
            });
        }
        let max = journal.max_seq();
        let as_of = as_of_opt.unwrap_or(max);
        match resolve_span(target, &snapshot, &journal, as_of) {
            Ok(span) => {
                let cref = self.recovery.put_content_ref(&span.page.bytes);
                format!(
                    "resolve_span:{} bytes range:{}-{} root={} ref={cref}",
                    span.page.bytes.len(),
                    span.page.range.start,
                    span.page.range.end,
                    snapshot.root_digest_hex()
                )
            }
            Err(e) => super::op_result::op0("resolve_span", e),
        }
    }
}
