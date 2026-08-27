use super::search_prefilter_eval::{self, EvalHit, LazyBigramIndex};
use super::session::IndexedLine;
use super::*;
use std::path::Path;

const DEFAULT_LIMIT: usize = 16;

/// Production literal prefilter (fszero-kbo default-on).
/// Default: lazy bigram + memmem. Escape hatch: `FSZERO_SEARCH_PREFILTER=contains`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralPrefilter {
    Contains,
    BigramMemmem,
}

pub fn literal_prefilter_from_env() -> LiteralPrefilter {
    match std::env::var("FSZERO_SEARCH_PREFILTER").ok().as_deref() {
        Some("contains") | Some("off") | Some("0") => LiteralPrefilter::Contains,
        _ => LiteralPrefilter::BigramMemmem,
    }
}

#[derive(Debug, Clone)]
struct RankedHit {
    score: f64,
    line: String,
    dedup: String,
}

/// Parse `ast-sgrep:...` or `asgrep:...` query body.
pub fn parse_ast_sgrep_query(arg: Option<&str>) -> Option<String> {
    let q = arg?.trim();
    q.strip_prefix("ast-sgrep:")
        .or_else(|| q.strip_prefix("asgrep:"))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Direct literal scanner: reads files in lexicographic key order, scans
/// lines in parallel, returns up to `limit` deterministic hits.
/// Ignores unreadable / non-UTF8 files; case-sensitive substring match.
///
/// When `bigrams` is `Some` and the active prefilter is `BigramMemmem` (default
/// after fszero-kbo; escape with `FSZERO_SEARCH_PREFILTER=contains`), uses
/// the lazy incremental bigram+memmem path measured in fszero-9ot. The session
/// freshness boundary already validated known files, so this fills only misses.
pub fn direct_literal_scan(
    root: &Path,
    index: &super::subsystems::IndexState,
    terms: &[String],
    limit: usize,
    bigrams: Option<&mut LazyBigramIndex>,
) -> Vec<IndexedLine> {
    direct_literal_scan_keys(root, &index.indexed_file_keys, terms, limit, bigrams)
}

/// Scan exactly the supplied workspace-relative keys. Scope selection happens
/// before this call, so the hit cap and completeness apply only to that scope.
pub fn direct_literal_scan_keys(
    root: &Path,
    keys: &std::collections::HashSet<String>,
    terms: &[String],
    limit: usize,
    bigrams: Option<&mut LazyBigramIndex>,
) -> Vec<IndexedLine> {
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    if literal_prefilter_from_env() == LiteralPrefilter::BigramMemmem {
        if let Some(index_bg) = bigrams {
            return to_indexed_lines(search_prefilter_eval::scan_bigram_memmem_prevalidated(
                root, keys, terms, limit, index_bg,
            ));
        }
    }
    to_indexed_lines(search_prefilter_eval::scan_contains_literal(
        root, keys, terms, limit,
    ))
}

fn to_indexed_lines(hits: impl IntoIterator<Item = EvalHit>) -> Vec<IndexedLine> {
    hits.into_iter()
        .map(|h| IndexedLine {
            file_key: h.file_key,
            line_no: h.line_no,
            text: h.text,
        })
        .collect()
}

#[cfg(test)]
fn scan_contains(
    root: &Path,
    index: &super::subsystems::IndexState,
    terms: &[String],
    limit: usize,
) -> Vec<IndexedLine> {
    to_indexed_lines(search_prefilter_eval::scan_contains_literal(
        root,
        &index.indexed_file_keys,
        terms,
        limit,
    ))
}

#[cfg(test)]
fn scan_bigram_memmem_indexed(
    root: &Path,
    index: &super::subsystems::IndexState,
    terms: &[String],
    limit: usize,
    bigrams: &mut LazyBigramIndex,
) -> Vec<IndexedLine> {
    to_indexed_lines(search_prefilter_eval::scan_bigram_memmem_prevalidated(
        root,
        &index.indexed_file_keys,
        terms,
        limit,
        bigrams,
    ))
}

pub fn ast_sgrep_payload_parts(
    root: &Path,
    index: &super::subsystems::IndexState,
    recovery: &RecoveryStore,
    query: &str,
    bigrams: Option<&mut LazyBigramIndex>,
) -> String {
    let terms = tokenize(query);
    if terms.is_empty() {
        return String::new();
    }
    let limit = env_usize("FSZERO_AST_SGREP_LIMIT").unwrap_or(DEFAULT_LIMIT);
    let version = index.ast_generation as i64;

    let mut hits: Vec<RankedHit> = Vec::new();
    let mut renderer = super::target_ref::HitRenderer::new(root);

    let ranked = direct_literal_scan(root, index, &terms, limit, bigrams);
    for (rank, line) in ranked.into_iter().enumerate() {
        let score = rrf_score(rank, 1.0);
        let excerpt = truncate_excerpt(&line.text, 120);
        let dedup = format!("{}:{}", line.file_key, line.line_no);
        let legacy = format!(
            "ASGREP: {}:{}-{}: {}",
            line.file_key, line.line_no, line.line_no, excerpt
        );
        hits.push(RankedHit {
            score,
            line: format!(
                "{}\n{legacy}",
                renderer.render_hit(&line.file_key, line.line_no, "asgrep")
            ),
            dedup,
        });
    }

    for sym in index.symbols.iter().map(|(n, _)| n).collect::<Vec<_>>() {
        let sym_score = score_symbol(sym, &terms);
        if sym_score <= 0.0 {
            continue;
        }
        for (fk, name, start, end) in recovery.query_fns_like(sym, version) {
            if !symbol_matches_terms(&name, &terms) && sym_score < 2.0 {
                continue;
            }
            let boost = sym_score * 2.0;
            let excerpt = excerpt_for_span(root, &fk);
            let start_line = byte_span_to_line(root, &fk, start);
            let end_line = byte_span_to_line(root, &fk, end);
            let dedup = format!("def:{fk}:{name}");
            hits.push(RankedHit {
                score: boost + 3.0,
                line: {
                    let legacy = format!(
                        "DEF: {}: {} span={}..{} | {}",
                        fk,
                        name,
                        start_line,
                        end_line,
                        truncate_excerpt(&excerpt, 100)
                    );
                    format!(
                        "{}\n{legacy}",
                        renderer.render_hit_for_symbol(&fk, start_line, "def", &name)
                    )
                },
                dedup,
            });

            for (cfk, caller) in recovery.ast.query_callers(&name, version) {
                let dedup_c = format!("caller:{cfk}:{caller}->{name}");
                let caller_line = recovery
                    .ast
                    .fn_span(&caller, version)
                    .filter(|(f, _, _)| *f == cfk)
                    .map(|(_, s, _)| byte_span_to_line(root, &cfk, s))
                    .unwrap_or(1);
                let legacy = format!("CALLER: {cfk}: {caller} -> {name}");
                hits.push(RankedHit {
                    score: boost + 1.5,
                    line: format!(
                        "{}\n{legacy}",
                        renderer.render_hit_for_symbol(&cfk, caller_line, "caller", &caller)
                    ),
                    dedup: dedup_c,
                });
            }
        }
    }

    for sym in exact_symbol_hits(query, &index.symbols) {
        for (fk, caller) in recovery.ast.query_callers(&sym, version) {
            let dedup = format!("graph:{fk}:{caller}:{sym}");
            hits.push(RankedHit {
                score: 5.0,
                line: format!("GRAPH: {fk}: {caller} calls {sym}"),
                dedup,
            });
        }
        for (fk, name, start, end) in recovery.query_fns_like(&sym, version) {
            if name != sym {
                continue;
            }
            let excerpt = excerpt_for_span(root, &fk);
            let dedup = format!("anchor:{fk}:{sym}");
            hits.push(RankedHit {
                score: 6.0,
                line: format!(
                    "ANCHOR: {}:{}-{}: {}",
                    fk,
                    byte_span_to_line(root, &fk, start),
                    byte_span_to_line(root, &fk, end),
                    truncate_excerpt(&excerpt, 120)
                ),
                dedup,
            });
        }
    }

    super::resolve_ident::sort_by_score_desc(&mut hits, |h| h.score);
    hits.truncate(limit);

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for h in hits {
        if seen.insert(h.dedup) {
            out.push(h.line);
        }
    }
    out.join("\n")
}

fn tokenize(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| s.len() >= 2 && !super::resolve_ident::STOP.contains(&s.as_str()))
        .collect()
}

fn score_symbol(sym: &str, terms: &[String]) -> f64 {
    let sym_l = sym.to_lowercase();
    let mut score = 0.0;
    for t in terms {
        if sym_l == *t {
            score += 5.0;
        } else if sym_l.contains(t.as_str()) {
            score += 2.0;
        }
    }
    score
}

fn symbol_matches_terms(sym: &str, terms: &[String]) -> bool {
    let sym_l = sym.to_lowercase();
    terms.iter().any(|t| sym_l.contains(t.as_str()))
}

pub fn byte_span_to_line(root: &Path, file_key: &str, byte: i64) -> usize {
    let path = root.join(file_key);
    // read_to_string on a FIFO/socket blocks; skip as line 1.
    if crate::path::refuse_non_regular_file(&path).is_err() {
        return 1;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        let target = byte.max(0) as usize;
        let mut acc = 0usize;
        for (i, line) in content.lines().enumerate() {
            let line_end = acc + line.len() + 1; // +1 for newline
            if target >= acc && target < line_end {
                return i + 1;
            }
            acc = line_end;
        }
    }
    1
}

fn excerpt_for_span(root: &Path, file_key: &str) -> String {
    let path = root.join(file_key);
    // read_to_string on a FIFO/socket blocks; skip as empty excerpt.
    if crate::path::refuse_non_regular_file(&path).is_err() {
        return String::new();
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if let Some(first) = content.lines().next() {
            return first.trim().to_string();
        }
    }
    String::new()
}

fn rrf_score(rank: usize, weight: f64) -> f64 {
    weight / (60.0 + rank as f64 + 1.0)
}

fn exact_symbol_hits(query: &str, symbols: &[(String, String)]) -> Vec<String> {
    let q = query.trim();
    let mut out = Vec::new();
    for (name, _) in symbols {
        if q.contains(name.as_str()) {
            out.push(name.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn truncate_excerpt(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.len() <= max {
        return flat;
    }
    format!("{}…", &flat[..max.saturating_sub(1)])
}

#[cfg(all(test, unix))]
mod fifo_tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::FileTypeExt;
    use std::path::Path;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    const HANG_BUDGET: Duration = Duration::from_millis(1500);

    fn mkfifo(path: &Path) {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("spawn mkfifo");
        assert!(
            status.success(),
            "mkfifo {} failed: {status}",
            path.display()
        );
    }

    fn within_timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ast-sgrep-fifo".into())
            .spawn(move || {
                let _ = tx.send(f());
            })
            .expect("spawn timeout worker");
        match rx.recv_timeout(HANG_BUDGET) {
            Ok(value) => value,
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "timed out after {HANG_BUDGET:?}: AST excerpt/line FIFO op hung instead of skipping"
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("timeout worker panicked before returning a skip result")
            }
        }
    }

    fn assert_still_fifo(path: &Path) {
        let meta = fs::symlink_metadata(path).expect("fifo metadata");
        assert!(
            meta.file_type().is_fifo(),
            "{} must remain a FIFO",
            path.display()
        );
    }

    #[test]
    fn ast_span_helpers_skip_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let fifo = root.join("hang.rs");
        mkfifo(&fifo);

        let (line, excerpt) = within_timeout({
            let root = root.clone();
            move || {
                (
                    byte_span_to_line(&root, "hang.rs", 0),
                    excerpt_for_span(&root, "hang.rs"),
                )
            }
        });
        assert_eq!(line, 1, "FIFO span mapping must skip as line 1");
        assert!(
            excerpt.is_empty(),
            "FIFO excerpt must skip empty, got {excerpt:?}"
        );
        assert_still_fifo(&fifo);
    }
}
