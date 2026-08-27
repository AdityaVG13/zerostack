//! `fs.resolve` — identifier-aware lexical ranking + optional co-access / local semantic tiers.

use super::ast_sgrep::{
    LiteralPrefilter, byte_span_to_line, direct_literal_scan, literal_prefilter_from_env,
};
use super::resolve_ident::{score_path_segments, score_symbol_ident, tokenize_intent};
use super::search_prefilter_eval::LazyBigramIndex;
use super::subsystems::IndexState;
use super::{FSZeroSession, RecoveryStore};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

pub const RESOLVE_REF: &str = "resolve";

const MAX_CANDIDATES: usize = 5;
/// Co-access boost weight (bounded): final = lexical * (1 + w * norm), w <= 0.3
const COACCESS_WEIGHT: f64 = 0.3;
const LEXICAL_STRONG_THRESHOLD: f64 = 4.0;

#[derive(Debug, Clone)]
pub struct ResolveOpts {
    pub engine: ResolveEngine,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveEngine {
    Lexical,
    HybridLocal,
}

impl Default for ResolveOpts {
    fn default() -> Self {
        Self {
            engine: ResolveEngine::Lexical,
            limit: MAX_CANDIDATES,
        }
    }
}

#[derive(Debug, Clone)]
struct ScoredPath {
    path: String,
    score: f64,
    skeleton: Option<String>,
    tier: &'static str,
}

pub fn parse_resolve_arg(arg: Option<&str>) -> Result<(String, ResolveOpts), String> {
    let raw = arg.unwrap_or("").trim();
    if raw.is_empty() {
        return Err("missing intent".to_string());
    }
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        let intent = v
            .get("intent")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing intent".to_string())?
            .to_string();
        let engine = v
            .get("engine")
            .or_else(|| v.get("opts").and_then(|o| o.get("engine")))
            .and_then(Value::as_str)
            .map(parse_engine)
            .unwrap_or(ResolveEngine::Lexical);
        let limit = v
            .get("limit")
            .or_else(|| v.get("opts").and_then(|o| o.get("limit")))
            .and_then(Value::as_u64)
            .map(|n| n.min(MAX_CANDIDATES as u64) as usize)
            .unwrap_or(MAX_CANDIDATES);
        return Ok((intent, ResolveOpts { engine, limit }));
    }
    Ok((raw.to_string(), ResolveOpts::default()))
}

fn parse_engine(s: &str) -> ResolveEngine {
    match s.to_ascii_lowercase().as_str() {
        "hybrid-local" | "hybrid_local" | "hybrid" | "semantic" => ResolveEngine::HybridLocal,
        _ => ResolveEngine::Lexical,
    }
}

pub fn build_resolve_json(
    recovery: &mut RecoveryStore,
    index: &IndexState,
    root: &Path,
    intent: &str,
    opts: &ResolveOpts,
    bigrams: Option<&mut LazyBigramIndex>,
) -> Value {
    let terms = tokenize_intent(intent);
    let limit = opts.limit.min(MAX_CANDIDATES);
    let mut scores: HashMap<String, ScoredPath> = HashMap::new();
    let version = index.ast_generation as i64;

    for (sym, fk) in &index.symbols {
        let sym_score = score_symbol_ident(sym, &terms);
        if sym_score > 0.0 {
            bump(
                &mut scores,
                fk,
                sym_score * 3.0 + score_path_segments(fk, &terms),
                skeleton_for_file(root, index, recovery, fk, version),
            );
        }
    }

    let ranked = direct_literal_scan(root, index, &terms, limit, bigrams);
    for line in &ranked {
        let line_score =
            score_line_text(&line.text, &terms) + score_path_segments(&line.file_key, &terms);
        if line_score > 0.0 {
            bump(
                &mut scores,
                &line.file_key,
                line_score,
                skeleton_for_file(root, index, recovery, &line.file_key, version),
            );
        }
    }

    for fk in index.indexed_file_keys.iter() {
        let path_score = score_path_segments(fk, &terms);
        if path_score >= 1.5 || intent.contains(fk.as_str()) {
            bump(
                &mut scores,
                fk,
                path_score + 4.0,
                skeleton_for_file(root, index, recovery, fk, version),
            );
        }
    }

    let (engine_label, ranked) =
        finalize_ranking(recovery, root, intent, scores, opts.engine, limit);

    let mut candidates = Vec::new();
    for hit in ranked {
        let content_ref = file_content_ref(recovery, Some(root), &hit.path);
        let mut row =
            json!({ "path": hit.path, "ref": content_ref, "score": hit.score, "tier": hit.tier, });
        if let Some(sk) = hit.skeleton {
            if !sk.is_empty() {
                row["skeleton"] = json!(sk);
            }
        }
        candidates.push(row);
    }

    json!({ "candidates": candidates, "engine": engine_label, })
}

fn finalize_ranking(
    recovery: &mut RecoveryStore,
    root: &Path,
    intent: &str,
    scores: HashMap<String, ScoredPath>,
    #[allow(unused_variables)] engine: ResolveEngine,
    limit: usize,
) -> (&'static str, Vec<ScoredPath>) {
    if scores.is_empty() {
        return ("lexical", Vec::new());
    }

    let mut ranked: Vec<ScoredPath> = scores.into_values().collect();
    // Path tie-break: HashMap collect order is non-deterministic; equal scores
    // previously flipped top-N membership across runs (resolve_contract flake).
    super::resolve_ident::sort_by_score_desc_then_key(
        &mut ranked,
        |h| h.score,
        |h| h.path.as_str(),
    );

    let strong_paths: Vec<String> = ranked
        .iter()
        .filter(|h| h.score >= LEXICAL_STRONG_THRESHOLD)
        .map(|h| h.path.clone())
        .collect();

    let mut used_coaccess = false;
    if recovery.access_log_row_count() > 0 && !strong_paths.is_empty() {
        let mut max_co: HashMap<String, i64> = HashMap::new();
        for anchor in &strong_paths {
            for (other, count) in recovery.query_coaccess_for_path(anchor) {
                max_co
                    .entry(other)
                    .and_modify(|c| *c = (*c).max(count))
                    .or_insert(count);
            }
        }
        if !max_co.is_empty() {
            let norm_denom = max_co.values().copied().max().unwrap_or(1).max(1) as f64;
            for hit in &mut ranked {
                if let Some(c) = max_co.get(&hit.path) {
                    let norm = (*c as f64) / norm_denom;
                    hit.score *= 1.0 + COACCESS_WEIGHT * norm;
                    hit.tier = "coaccess";
                    used_coaccess = true;
                }
            }
            super::resolve_ident::sort_by_score_desc_then_key(
                &mut ranked,
                |h| h.score,
                |h| h.path.as_str(),
            );
        }
    }

    #[cfg(feature = "fszero-semantic-local")]
    let engine_label = {
        if engine == ResolveEngine::HybridLocal {
            apply_frankensearch_local_rerank(recovery, root, intent, &mut ranked);
            "hybrid-local"
        } else if used_coaccess {
            "lexical+coaccess"
        } else {
            "lexical"
        }
    };

    #[cfg(not(feature = "fszero-semantic-local"))]
    let engine_label = {
        let _ = (root, intent);
        if used_coaccess {
            "lexical+coaccess"
        } else {
            "lexical"
        }
    };

    ranked.truncate(limit);
    (engine_label, ranked)
}

#[cfg(feature = "fszero-semantic-local")]
fn apply_frankensearch_local_rerank(
    recovery: &mut RecoveryStore,
    root: &Path,
    intent: &str,
    ranked: &mut [ScoredPath],
) {
    let mut pairs: Vec<(String, f64)> = ranked.iter().map(|h| (h.path.clone(), h.score)).collect();
    let tiers = super::semantic_local::hybrid_local_rerank(recovery, root, intent, &mut pairs);
    for hit in ranked.iter_mut() {
        if let Some((_, score)) = pairs.iter().find(|(p, _)| p == &hit.path) {
            hit.score = *score;
        }
        if let Some(tier) = tiers.get(&hit.path) {
            hit.tier = tier.as_str();
        }
    }
    super::resolve_ident::sort_by_score_desc_then_key(ranked, |h| h.score, |h| h.path.as_str());
}

/// `fs::read` on a FIFO/socket blocks; `stat` does not. Skip, do not hang.
pub(crate) fn file_content_ref(
    recovery: &mut RecoveryStore,
    root: Option<&Path>,
    file_key: &str,
) -> String {
    if let Some(root) = root {
        let path = root.join(file_key);
        if crate::path::refuse_non_regular_file(&path).is_err() {
            return String::new();
        }
        if let Ok(bytes) = std::fs::read(&path) {
            return recovery.put_content_ref(&bytes);
        }
    }
    String::new()
}

fn skeleton_for_file(
    root: &Path,
    index: &IndexState,
    recovery: &RecoveryStore,
    file_key: &str,
    version: i64,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (name, _fk) in index.symbols.iter().filter(|(_, fk)| *fk == file_key) {
        if let Some((_, _, kind, start, _)) = recovery
            .query_symbols_like(name, version)
            .into_iter()
            .find(|(fk, n, _, _, _)| fk == file_key && n == name)
        {
            let line = byte_span_to_line(root, file_key, start);
            if kind == "method" {
                parts.push(format!("{name}:{line} kind=method"));
            } else {
                parts.push(format!("{name}:{line}"));
            }
        } else {
            parts.push(name.clone());
        }
        if parts.len() >= 8 {
            break;
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(truncate_tokens(&parts.join(" "), 30))
}

fn score_line_text(text: &str, terms: &[String]) -> f64 {
    let text_l = text.to_lowercase();
    let mut score = 0.0;
    for t in terms {
        if text_l.contains(t.as_str()) {
            score += 1.0 + (t.len() as f64 * 0.1);
        }
    }
    score
}

fn truncate_tokens(s: &str, max_tokens: usize) -> String {
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() <= max_tokens {
        return s.to_string();
    }
    words[..max_tokens].join(" ")
}

fn bump(map: &mut HashMap<String, ScoredPath>, path: &str, add: f64, skeleton: Option<String>) {
    let entry = map.entry(path.to_string()).or_insert(ScoredPath {
        path: path.to_string(),
        score: 0.0,
        skeleton: None,
        tier: "lexical",
    });
    entry.score += add;
    if entry.skeleton.is_none() {
        entry.skeleton = skeleton;
    }
}

impl FSZeroSession {
    pub fn do_resolve(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        if let Err(e) = self.prepare_index_or_busy(root) {
            return e;
        }
        let (intent, opts) = match parse_resolve_arg(arg) {
            Ok(v) => v,
            Err(e) => return super::op_result::op0("resolve", e),
        };
        if intent.trim().is_empty() {
            return "resolve:0 (missing intent)".to_string();
        }
        let index = self.index.clone();
        let root_path = match &self.root {
            Some(r) => r.as_path(),
            None => return "resolve:0 (no root)".to_string(),
        };
        let use_prefilter = literal_prefilter_from_env() == LiteralPrefilter::BigramMemmem;
        let payload = build_resolve_json(
            &mut self.recovery,
            &index,
            root_path,
            &intent,
            &opts,
            use_prefilter.then_some(&mut self.lazy_bigrams),
        );
        let text = payload.to_string();
        self.recovery.put_key(RESOLVE_REF, text.as_bytes());
        let n = payload
            .get("candidates")
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0);
        format!("resolve:{n}")
    }
}

#[cfg(all(test, unix))]
mod fifo_tests {
    use super::*;
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
            .name("resolve-fifo".into())
            .spawn(move || {
                let _ = tx.send(f());
            })
            .expect("spawn timeout worker");
        match rx.recv_timeout(HANG_BUDGET) {
            Ok(value) => value,
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "timed out after {HANG_BUDGET:?}: file_content_ref hung on FIFO instead of skipping"
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("timeout worker panicked before returning a skip result")
            }
        }
    }

    #[test]
    fn file_content_ref_skips_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("hit.rs"), b"fn hit() {}\n").unwrap();
        let fifo = root.join("hang.rs");
        mkfifo(&fifo);

        let (fifo_ref, regular_ref) = within_timeout({
            let root = root.clone();
            move || {
                let mut recovery = crate::RecoveryStore::new();
                let fifo_ref = file_content_ref(&mut recovery, Some(&root), "hang.rs");
                let regular_ref = file_content_ref(&mut recovery, Some(&root), "hit.rs");
                (fifo_ref, regular_ref)
            }
        });
        assert!(
            fifo_ref.is_empty(),
            "FIFO must skip with empty content ref, got {fifo_ref:?}"
        );
        assert!(
            !regular_ref.is_empty(),
            "regular file must still mint a content ref"
        );
        let meta = std::fs::symlink_metadata(&fifo).expect("fifo metadata");
        assert!(
            meta.file_type().is_fifo(),
            "{} must remain a FIFO",
            fifo.display()
        );
    }
}
