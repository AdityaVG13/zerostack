//! Snap-to-file target grammar (bead graphzero-snap-to-file-targets-5htnw).
//!
//! ONE grammar, defined by FSZero in `docs/design/target-ref-grammar.md` and
//! `src/core/target_ref.rs`; adopted here verbatim. Do not invent a second one.
//!
//! Canonical target ref: `<path>#L<start>-L<end>` (1-based, inclusive).
//! Canonical hit record:
//! ```text
//! HIT <path>#L<start>-L<end> kind=<def|ref|blast> sym=<enclosing symbol>
//! | <line-no>: <line text>
//! ```

use super::snapshot::Snapshot;

/// Lines of context rendered on each side of the matched span.
pub const TARGET_CONTEXT_LINES: usize = 2;

/// Number of leading destinations whose content window is inlined.
pub const TARGET_INLINE_TOP_HITS: usize = 3;

/// Render the canonical target ref for `path` and a 1-based inclusive window.
pub fn render_target(path: &str, start: usize, end: usize) -> String {
    format!("{path}#L{start}-L{end}")
}

/// A resolved file target: canonical ref, intent metadata, and content window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTargetHit {
    pub target: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub kind: String,
    pub symbol: String,
    /// Inlined window, one `| <line-no>: <text>` per line.
    pub content: String,
}

impl FileTargetHit {
    /// The `HIT ...` header alone.
    pub fn header(&self) -> String {
        format!("HIT {} kind={} sym={}", self.target, self.kind, self.symbol)
    }

    /// The full canonical hit record: header plus inlined window.
    pub fn render(&self) -> String {
        if self.content.is_empty() {
            self.header()
        } else {
            format!("{}\n{}", self.header(), self.content)
        }
    }
}

fn parse_blob_span(evidence_ref: &str) -> Option<(&str, usize, usize)> {
    let raw = evidence_ref.strip_prefix("gz://blob/")?;
    let (hash, span) = raw.split_once("#B")?;
    let (start, end) = span.split_once('-')?;
    Some((hash, start.parse().ok()?, end.parse().ok()?))
}

fn line_of_byte(bytes: &[u8], offset: usize) -> usize {
    bytes[..offset.min(bytes.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// Enclosing symbol for `line_no`, or `None` at file scope.
/// Same declarator scan as FSZero `target_ref::enclosing_symbol`.
fn enclosing_symbol(lines: &[&str], line_no: usize) -> Option<String> {
    const DECLARATORS: &[&str] = &[
        "fn ",
        "pub fn ",
        "async fn ",
        "struct ",
        "enum ",
        "impl ",
        "trait ",
        "mod ",
        "class ",
        "def ",
        "function ",
        "type ",
        "const ",
        "static ",
    ];
    for line in lines[..line_no.min(lines.len())].iter().rev() {
        let trimmed = line.trim();
        if DECLARATORS.iter().any(|d| trimmed.starts_with(d)) {
            let head = trimmed.trim_end_matches(['{', ' ']);
            return Some(head.chars().take(80).collect());
        }
    }
    None
}

/// Resolve a `gz://blob/<hash>#B<start>-<end>` evidence ref into a canonical
/// file target with intent metadata and an inlined content window.
///
/// Returns `None` when the ref is not a blob span, the blob is absent, or the
/// snapshot has no repo-relative path for it.
pub fn file_target_for_evidence(
    snapshot: &Snapshot,
    evidence_ref: &str,
    kind: &str,
    symbol: Option<&str>,
    inline_content: bool,
) -> Option<FileTargetHit> {
    let (hash, span_start, span_end) = parse_blob_span(evidence_ref)?;
    let path = snapshot.path_for_blob(hash)?.path.clone();
    let bytes = snapshot.blob_bytes(hash)?;
    if span_start >= bytes.len() || span_start >= span_end {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len().max(1);
    let first = line_of_byte(text.as_bytes(), span_start).clamp(1, total);
    let last = line_of_byte(text.as_bytes(), span_end.saturating_sub(1)).clamp(first, total);
    let start_line = first.saturating_sub(TARGET_CONTEXT_LINES).max(1);
    let end_line = (last + TARGET_CONTEXT_LINES).min(total);

    let symbol = symbol
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| enclosing_symbol(&lines, first))
        .unwrap_or_else(|| "(file-scope)".to_string());

    let mut content = String::new();
    if inline_content {
        for n in start_line..=end_line {
            if let Some(line) = lines.get(n - 1) {
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(&format!("| {n}: {line}"));
            }
        }
    }

    Some(FileTargetHit {
        target: render_target(&path, start_line, end_line),
        path,
        start_line,
        end_line,
        kind: kind.to_string(),
        symbol,
        content,
    })
}
