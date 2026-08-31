//! Canonical snap-to-file target grammar used by FSZero discovery and GraphZero.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Matched-line role for discovery ranking / annotation (no regex).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LineRole {
    /// Declarations (fn/struct/class/…) — ranked first.
    Definition = 0,
    /// Import / use / include lines.
    Import = 1,
    /// Everything else.
    Other = 2,
}

impl LineRole {
    pub fn as_str(self) -> &'static str {
        match self {
            LineRole::Definition => "definition",
            LineRole::Import => "import",
            LineRole::Other => "other",
        }
    }
}

/// Byte-level classification of a source line as definition / import / other.
/// Skips common visibility/async modifiers; never uses regex.
pub fn classify_line_role(line: &str) -> LineRole {
    let s = strip_leading_modifiers(line.trim_start());
    if is_import_line(s) {
        return LineRole::Import;
    }
    if is_definition_line(s) {
        return LineRole::Definition;
    }
    LineRole::Other
}

fn strip_leading_modifiers(mut s: &str) -> &str {
    // Stacked modifiers (pub/async/unsafe/…); `pub(in path)` handled specially.
    loop {
        let before = s;
        if let Some(rest) = s.strip_prefix("pub ") {
            s = rest.trim_start();
        } else if let Some(rest) = s.strip_prefix("pub(super) ") {
            s = rest.trim_start();
        } else if let Some(rest) = s.strip_prefix("pub(in ") {
            if let Some(idx) = rest.find(')') {
                s = rest[idx + 1..].trim_start();
            }
        } else if let Some(rest) = s.strip_prefix("pub ") {
            s = rest.trim_start();
        } else {
            // Visibility / async / unsafe only -- not bare const/static (those are decls).
            for m in [
                "async ",
                "unsafe ",
                "default ",
                "export ",
                "abstract ",
                "final ",
                "private ",
                "protected ",
                "public ",
                "readonly ",
                "declare ",
            ] {
                if let Some(rest) = s.strip_prefix(m) {
                    s = rest.trim_start();
                    break;
                }
            }
        }
        if s == before {
            break;
        }
    }
    s
}

fn is_import_line(s: &str) -> bool {
    s.starts_with("use ")
        || s.starts_with("import ")
        || s.starts_with("from ")
        || s.starts_with("require(")
        || s.starts_with("require \"")
        || s.starts_with("require '")
        || s.starts_with("#include")
        || s.starts_with("using ")
        || s.starts_with("extern crate ")
}

fn is_definition_line(s: &str) -> bool {
    const HEADS: &[&str] = &[
        "fn ",
        "struct ",
        "enum ",
        "impl ",
        "trait ",
        "mod ",
        "class ",
        "def ",
        "function ",
        "type ",
        "interface ",
        "protocol ",
        "extension ",
        "actor ",
        "record ",
        "namespace ",
        "module ",
        "macro_rules!",
        "const ",
        "static ",
        "typedef ",
    ];
    HEADS.iter().any(|h| s.starts_with(h))
}

/// Lines of context rendered on each side of the matched line.
pub const TARGET_CONTEXT_LINES: usize = 2;

/// Payloads at or below this size are always inlined, never preview-only.
pub const TARGET_INLINE_MAX_BYTES: usize = 4096;

/// 1-based, inclusive line window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineWindow {
    pub start: usize,
    pub end: usize,
}

/// Render the canonical target ref for `path` and `window`.
pub fn render_target(path: &str, window: LineWindow) -> String {
    format!("{path}#L{}-L{}", window.start, window.end)
}

/// Parse a canonical target ref. Returns `None` when `arg` is a plain path.
pub fn parse_target_ref(arg: &str) -> Option<(&str, LineWindow)> {
    let (path, suffix) = arg.rsplit_once("#L")?;
    if path.is_empty() {
        return None;
    }
    let (start, end) = suffix.split_once("-L")?;
    let start: usize = start.parse().ok()?;
    let end: usize = end.parse().ok()?;
    if start == 0 || end < start {
        return None;
    }
    Some((path, LineWindow { start, end }))
}

/// Byte offsets of a line window inside `content`, clamped to the content.
pub fn window_byte_range(content: &str, window: LineWindow) -> (u64, u64) {
    let mut offset = 0u64;
    let mut start = None;
    let mut end = content.len() as u64;
    for (idx, line) in content.split_inclusive('\n').enumerate() {
        let line_no = idx + 1;
        if line_no == window.start {
            start = Some(offset);
        }
        offset += line.len() as u64;
        if line_no == window.end {
            end = offset;
            break;
        }
    }
    let start = start.unwrap_or(content.len() as u64);
    (start, end.max(start))
}

/// Enclosing symbol for `line_no`, or `None` at file scope.
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

/// Renders canonical hit records, reading each file at most once.
pub struct HitRenderer {
    root: PathBuf,
    cache: HashMap<String, Option<String>>,
    /// When set (multi-keyword search), inline windows keep only
    /// matching lines plus `...` gaps ( progressive disclosure).
    keywords: Vec<String>,
}

impl HitRenderer {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            cache: HashMap::new(),
            keywords: Vec::new(),
        }
    }

    pub fn with_keywords(mut self, keywords: Vec<String>) -> Self {
        self.keywords = keywords;
        self
    }

    fn content(&mut self, file_key: &str) -> Option<&String> {
        let root = &self.root;
        self.cache
            .entry(file_key.to_string())
            .or_insert_with(|| std::fs::read_to_string(root.join(file_key)).ok())
            .as_ref()
    }

    /// One canonical hit record: target ref + intent metadata + inlined window.
    /// The enclosing symbol is inferred from the file text.
    pub fn render_hit(&mut self, file_key: &str, line_no: usize, kind: &str) -> String {
        self.render_hit_inner(file_key, line_no, kind, None)
    }

    /// Same record, but with the symbol the discovery route already knows
    /// (structural DEF/CALLER/IMPORT rows and AST-sgrep hits carry one).
    pub fn render_hit_for_symbol(
        &mut self,
        file_key: &str,
        line_no: usize,
        kind: &str,
        symbol: &str,
    ) -> String {
        self.render_hit_inner(file_key, line_no, kind, Some(symbol))
    }

    fn render_hit_inner(
        &mut self,
        file_key: &str,
        line_no: usize,
        kind: &str,
        symbol: Option<&str>,
    ) -> String {
        let keywords = self.keywords.clone();
        let multi_kw = keywords.len() > 1;
        let Some(content) = self.content(file_key) else {
            let window = LineWindow {
                start: line_no.max(1),
                end: line_no.max(1),
            };
            return format!(
                "HIT {} kind={kind} role=other sym={}",
                render_target(file_key, window),
                symbol.unwrap_or("(unreadable)")
            );
        };
        let lines: Vec<&str> = content.lines().collect();
        let line_no = line_no.clamp(1, lines.len().max(1));
        let window = LineWindow {
            start: line_no.saturating_sub(TARGET_CONTEXT_LINES).max(1),
            end: (line_no + TARGET_CONTEXT_LINES).min(lines.len().max(1)),
        };
        let role = lines
            .get(line_no - 1)
            .map(|l| classify_line_role(l))
            .unwrap_or(LineRole::Other);
        let symbol = symbol.map(str::to_string).unwrap_or_else(|| {
            enclosing_symbol(&lines, line_no).unwrap_or_else(|| "(file-scope)".to_string())
        });
        let mut out = format!(
            "HIT {} kind={kind} role={} sym={symbol}",
            render_target(file_key, window),
            role.as_str(),
        );
        if multi_kw {
            // Matched-line snippet only: ellipsis between non-matching context rows.
            let mut pending_ellipsis = false;
            let mut emitted = false;
            for n in window.start..=window.end {
                let Some(text) = lines.get(n - 1) else {
                    continue;
                };
                let hit = keywords
                    .iter()
                    .any(|k| !k.is_empty() && text.contains(k.as_str()));
                // Always keep the matched line itself even if keywords miss (parity).
                let keep = hit || n == line_no;
                if keep {
                    if pending_ellipsis && emitted {
                        out.push_str("\n| ...");
                    }
                    out.push_str(&format!("\n| {n}: {text}"));
                    emitted = true;
                    pending_ellipsis = false;
                } else if emitted {
                    pending_ellipsis = true;
                }
            }
        } else {
            for n in window.start..=window.end {
                if let Some(text) = lines.get(n - 1) {
                    out.push_str(&format!("\n| {n}: {text}"));
                }
            }
        }
        out
    }
}
