use super::ast_sgrep;
use super::budget::env_usize;
use super::fuzzy_fallback::{FuzzyHit, fuzzy_fallback, is_uniformly_weak, strong_fuzzy_hits};
use super::search_cursor::SearchPage;
use super::search_prefilter_eval::LazyBigramIndex;
use super::subsystems::IndexState;
use super::target_ref::{HitRenderer, TARGET_INLINE_MAX_BYTES};
use super::{FSZeroSession, RecoveryStore};
use std::path::Path;

pub enum SearchRoute {
    AstSgrep(String),
    Structural,
    Grep,
}

/// Literal discovery's deterministic payload cap. A payload below this count
/// is complete; reaching the cap is conservatively treated as possibly truncated.
pub const GREP_HIT_LIMIT: usize = 16;

/// Collect up to this many literal hits before cursor-paging (fszero-enuj).
pub const GREP_HIT_SCAN_LIMIT: usize = 64;

/// Default page size for search cursor pagination.
pub const SEARCH_PAGE_DEFAULT: usize = GREP_HIT_LIMIT;

/// Max fuzzy path/symbol candidates returned after a zero-exact retry.
const FUZZY_HIT_LIMIT: usize = 8;
/// Max edit distance for fuzzy path/symbol retry.
const FUZZY_MAX_DIST: usize = 2;

/// Marker payload: exact search empty and all fuzzy candidates were weak noise.
pub const WEAK_FUZZY_PREFIX: &str = "weak-fuzzy:";

/// Split a multi-keyword query on whitespace (A-RAG length-weighted scoring).
pub fn keywords_from_query(q: &str) -> Vec<String> {
    q.split_whitespace()
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

/// Length-weighted keyword score: `sum(count(k) * len(k))` over keywords (fszero-04bn).
/// Longer keywords weigh more; pure presence of short noise tokens scores lower.
pub fn length_weighted_score(text: &str, keywords: &[String]) -> u64 {
    let mut score = 0u64;
    for k in keywords {
        if k.is_empty() {
            continue;
        }
        let count = text.matches(k.as_str()).count() as u64;
        score = score.saturating_add(count.saturating_mul(k.len() as u64));
    }
    score
}

/// Keep only lines that contain at least one keyword; elide gaps with `...`
/// (matched-sentence / progressive-disclosure snippet, fszero-04bn).
pub fn matched_line_snippet(content: &str, keywords: &[String]) -> String {
    if keywords.is_empty() {
        return content.to_string();
    }
    let mut out = String::new();
    let mut pending_ellipsis = false;
    let mut emitted_any = false;
    for line in content.lines() {
        let hit = keywords
            .iter()
            .any(|k| !k.is_empty() && line.contains(k.as_str()));
        if hit {
            if pending_ellipsis && emitted_any {
                out.push_str("...\n");
            }
            out.push_str(line);
            out.push('\n');
            emitted_any = true;
            pending_ellipsis = false;
        } else if emitted_any {
            pending_ellipsis = true;
        }
    }
    if out.is_empty() {
        // Fall back to first non-empty line so callers never get silence.
        content.lines().next().unwrap_or("").to_string()
    } else {
        out
    }
}

pub fn classify_search_query(arg: Option<&str>, q: &str) -> SearchRoute {
    if let Some(body) = ast_sgrep::parse_ast_sgrep_query(arg) {
        SearchRoute::AstSgrep(body)
    } else if q.starts_with("callers:") || q.starts_with("defs:") || q == "imports" {
        SearchRoute::Structural
    } else {
        SearchRoute::Grep
    }
}

pub fn files_budget_message(indexed_count: usize) -> Option<String> {
    let cap = env_usize("FSZERO_BUDGET_FILES")?;
    (indexed_count > cap).then(|| format!("budget:0 files cap={cap} scanned={indexed_count}"))
}

fn ast_nodes_budget_message(hits_len: usize) -> Option<String> {
    let cap = env_usize("FSZERO_BUDGET_AST_NODES")?;
    (hits_len > cap).then(|| format!("budget:0 ast_nodes cap={cap} scanned={hits_len}"))
}

pub fn parse_budget_message(msg: &str) -> Option<(&str, usize, usize)> {
    if !msg.starts_with("budget:0 ") {
        return None;
    }
    let rest = msg.strip_prefix("budget:0 ")?;
    let (dimension, tail) = rest.split_once(" cap=")?;
    let cap = tail.split_whitespace().next()?.parse().ok()?;
    let scanned = msg.split("scanned=").nth(1)?.parse().ok()?;
    Some((dimension, cap, scanned))
}

fn parse_ast_nodes_budget(msg: &str) -> Option<(usize, usize)> {
    let (dim, cap, scanned) = parse_budget_message(msg)?;
    (dim == "ast_nodes").then_some((cap, scanned))
}

pub fn build_search_payload(
    route: SearchRoute,
    root: Option<&Path>,
    index: &IndexState,
    recovery: &RecoveryStore,
    q: &str,
    bigrams: Option<&mut LazyBigramIndex>,
) -> String {
    match route {
        SearchRoute::AstSgrep(body) => root
            .map(|r| ast_sgrep::ast_sgrep_payload_parts(r, index, recovery, &body, bigrams))
            .unwrap_or_default(),
        SearchRoute::Structural => {
            let hits = structural_search_lines(root, recovery, index.ast_generation, q);
            if let Some(msg) = ast_nodes_budget_message(hits.len()) {
                return msg;
            }
            hits.join("\n")
        }
        SearchRoute::Grep => {
            if let Some(r) = root {
                let keywords = keywords_from_query(q);
                // Multi-keyword: OR scan per term; single token keeps the full query.
                let terms = if keywords.len() > 1 {
                    keywords.clone()
                } else {
                    vec![q.to_string()]
                };
                // Scan past one page so cursor pagination has remainder (enuj).
                let mut hits =
                    ast_sgrep::direct_literal_scan(r, index, &terms, GREP_HIT_SCAN_LIMIT, bigrams);
                if hits.is_empty() {
                    // fszero-svr8: zero exact → fuzzy path/symbol retry + weak gate.
                    return fuzzy_grep_fallback_payload(r, index, q);
                }
                // Rank: length-weighted TF (04bn) then definition role (nqbg).
                hits.sort_by(|a, b| {
                    let sa = length_weighted_score(&a.text, &keywords);
                    let sb = length_weighted_score(&b.text, &keywords);
                    sb.cmp(&sa)
                        .then_with(|| {
                            super::target_ref::classify_line_role(&a.text)
                                .cmp(&super::target_ref::classify_line_role(&b.text))
                        })
                        .then_with(|| a.file_key.as_ref().cmp(b.file_key.as_ref()))
                        .then_with(|| a.line_no.cmp(&b.line_no))
                });
                // Canonical hit records: target ref + role + intent + window
                // (multi-kw: matched-line snippets with ellipsis, 04bn).
                let mut renderer = HitRenderer::new(r).with_keywords(keywords);
                return hits
                    .iter()
                    .map(|h| renderer.render_hit(h.file_key.as_ref(), h.line_no, "literal"))
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            String::new()
        }
    }
}

/// Fuzzy path/symbol retry after zero literal hits (fszero-svr8).
///
/// Returns:
/// - empty string: no useful approximate candidates
/// - `weak-fuzzy:…` marker: candidates exist but quality is uniformly weak
/// - multi-line HIT payload with `kind=fuzzy` for strong approximations
pub fn fuzzy_grep_fallback_payload(root: &Path, index: &IndexState, q: &str) -> String {
    let mut candidates: Vec<String> = index.indexed_file_keys.iter().cloned().collect();
    for (sym, fk) in &index.symbols {
        candidates.push(sym.clone());
        candidates.push(fk.clone());
    }
    candidates.sort();
    candidates.dedup();
    let raw = fuzzy_fallback(q, &candidates, FUZZY_MAX_DIST);
    if raw.is_empty() {
        return String::new();
    }
    if is_uniformly_weak(&raw, q) {
        let best = raw.iter().map(|h| h.distance).min().unwrap_or(0);
        return format!(
            "{WEAK_FUZZY_PREFIX} best_dist={best} candidates={} (suppressed low-confidence fuzzy noise)",
            raw.len()
        );
    }
    let strong = strong_fuzzy_hits(raw, q, FUZZY_HIT_LIMIT);
    render_fuzzy_hits(root, index, &strong)
}

fn render_fuzzy_hits(root: &Path, index: &IndexState, hits: &[FuzzyHit]) -> String {
    let mut renderer = HitRenderer::new(root);
    let mut out = Vec::new();
    for h in hits {
        let file_key = resolve_fuzzy_file_key(index, &h.candidate);
        let Some(fk) = file_key else { continue };
        // Prefer symbol name when the candidate was a symbol.
        let sym = if index.symbols.iter().any(|(s, _)| s == &h.candidate) {
            h.candidate.as_str()
        } else {
            "(fuzzy-path)"
        };
        let mut record = renderer.render_hit_for_symbol(&fk, 1, "fuzzy", sym);
        record.push_str(&format!("\n| dist={}", h.distance));
        out.push(record);
    }
    out.join("\n")
}

fn resolve_fuzzy_file_key(index: &IndexState, candidate: &str) -> Option<String> {
    if index.indexed_file_keys.contains(candidate) {
        return Some(candidate.to_string());
    }
    if let Some((_, fk)) = index.symbols.iter().find(|(s, _)| s == candidate) {
        return Some(fk.clone());
    }
    // Basename-only match against indexed keys.
    let base = Path::new(candidate)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(candidate);
    index
        .indexed_file_keys
        .iter()
        .find(|k| Path::new(k.as_str()).file_name().and_then(|s| s.to_str()) == Some(base))
        .cloned()
}

/// Split a search payload into multi-line HIT records (for cursor pages).
pub fn split_hit_records(payload: &str) -> Vec<String> {
    if payload.is_empty()
        || payload.starts_with(WEAK_FUZZY_PREFIX)
        || payload.starts_with("budget:0")
    {
        return Vec::new();
    }
    let mut records = Vec::new();
    let mut cur = String::new();
    for line in payload.lines() {
        if line.starts_with("HIT ") {
            if !cur.is_empty() {
                records.push(cur.trim_end().to_string());
            }
            cur = line.to_string();
        } else if !cur.is_empty() {
            cur.push('\n');
            cur.push_str(line);
        } else if !line.is_empty() {
            // Legacy one-line-per-hit payloads.
            records.push(line.to_string());
        }
    }
    if !cur.is_empty() {
        records.push(cur.trim_end().to_string());
    }
    records
}

fn search_page_size() -> usize {
    env_usize("FSZERO_SEARCH_PAGE")
        .unwrap_or(SEARCH_PAGE_DEFAULT)
        .max(1)
}

fn format_search_page_ack(
    hit_count: usize,
    page: &SearchPage,
    search_ref: &str,
    fuzzy: bool,
) -> String {
    let fuzzy_note = if fuzzy { " fuzzy" } else { "" };
    let total = page.total_hint;
    let cursor_note = match &page.next_cursor {
        Some(c) => format!(" total={total} cursor={c}"),
        None if total > page.items.len() => format!(" total={total}"),
        None => String::new(),
    };
    let body = page.items.join("\n");
    if body.len() <= SEARCH_INLINE_MAX_BYTES {
        format!("search:{hit_count} hits{fuzzy_note}{cursor_note}\n{body}")
    } else {
        format!("search:{hit_count} hits{fuzzy_note}{cursor_note} ref={search_ref}")
    }
}

/// Literal discovery for fused mutation. Workspace-relative keys are selected
/// before the scanner runs, so out-of-scope files cannot consume the hit cap.
pub fn build_scoped_literal_payload(
    root: &Path,
    index: &IndexState,
    query: &str,
    scope_key: &str,
    file_scope: bool,
    bigrams: Option<&mut LazyBigramIndex>,
) -> String {
    let prefix = if scope_key.is_empty() {
        String::new()
    } else {
        format!("{scope_key}/")
    };
    let keys: std::collections::HashSet<String> = index
        .indexed_file_keys
        .iter()
        .filter(|key| {
            scope_key.is_empty()
                || if file_scope {
                    key.as_str() == scope_key
                } else {
                    key.as_str() == scope_key || key.starts_with(&prefix)
                }
        })
        .cloned()
        .collect();
    let keywords = keywords_from_query(query);
    // Fused mutation is a safety gate, not fuzzy discovery: certify the exact
    // caller-provided needle so generic keywords cannot consume the hit cap.
    let terms = vec![query.to_string()];
    let mut hits =
        ast_sgrep::direct_literal_scan_keys(root, &keys, &terms, GREP_HIT_LIMIT, bigrams);
    if hits.is_empty() {
        return String::new();
    }
    hits.sort_by(|a, b| {
        let sa = length_weighted_score(&a.text, &keywords);
        let sb = length_weighted_score(&b.text, &keywords);
        sb.cmp(&sa)
            .then_with(|| {
                super::target_ref::classify_line_role(&a.text)
                    .cmp(&super::target_ref::classify_line_role(&b.text))
            })
            .then_with(|| a.file_key.as_ref().cmp(b.file_key.as_ref()))
            .then_with(|| a.line_no.cmp(&b.line_no))
    });
    let mut renderer = HitRenderer::new(root).with_keywords(keywords);
    hits.iter()
        .map(|hit| renderer.render_hit(hit.file_key.as_ref(), hit.line_no, "literal"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Structural rows carry the same canonical HIT grammar as the grep route
/// (snap-to-file 99q7), so DEF / CALLER / IMPORT discovery is one-call
/// actionable too. The legacy prefix row is kept as the line after the
/// inlined window so existing payload consumers keep matching.
fn structural_search_lines(
    root: Option<&Path>,
    recovery: &RecoveryStore,
    ast_generation: u64,
    q: &str,
) -> Vec<String> {
    let version = ast_generation as i64;
    let mut renderer = root.map(HitRenderer::new);
    let mut hits = Vec::new();
    let mut push =
        |kind: &str, fk: &str, sym: &str, line_no: usize, legacy: String| match renderer.as_mut() {
            Some(r) => hits.push(format!(
                "{}\n{legacy}",
                r.render_hit_for_symbol(fk, line_no, kind, sym)
            )),
            None => hits.push(legacy),
        };
    if q.starts_with("callers:") {
        let sym = q.split(':').nth(1).unwrap_or(q);
        for (fk, caller) in recovery.ast.query_callers(sym, version) {
            let line_no = caller_line(root, recovery, &fk, &caller, version);
            let legacy = format!("CALLER: {fk}: {caller}");
            push("caller", &fk, &caller, line_no, legacy);
        }
    } else if q.starts_with("defs:") {
        let sym = q.split(':').nth(1).unwrap_or(q);
        for (fk, sy, start, end) in recovery.query_fns_like(sym, version) {
            let line_no = span_line(root, &fk, start);
            let legacy = format!("DEF: {fk}: {sy} span={start}..{end}");
            push("def", &fk, &sy, line_no, legacy);
        }
    } else {
        for (fk, import, start, end) in recovery.ast.query_imports(version) {
            let line_no = span_line(root, &fk, start);
            let legacy = format!("IMPORT: {fk}: {import} span={start}..{end}");
            push("import", &fk, &import, line_no, legacy);
        }
    }
    hits
}

fn span_line(root: Option<&Path>, file_key: &str, byte: i64) -> usize {
    root.map(|r| ast_sgrep::byte_span_to_line(r, file_key, byte))
        .unwrap_or(1)
}

/// A caller edge has no span of its own; anchor it on the caller's definition
/// when that definition lives in the same file.
fn caller_line(
    root: Option<&Path>,
    recovery: &RecoveryStore,
    file_key: &str,
    caller: &str,
    version: i64,
) -> usize {
    match recovery.ast.fn_span(caller, version) {
        Some((fk, start, _)) if fk == file_key => span_line(root, file_key, start),
        _ => 1,
    }
}

/// Hits are counted by canonical `HIT ` records when present; legacy
/// one-line-per-hit payloads still count lines. Weak-fuzzy markers count as 0.
pub fn search_hit_count(payload: &str) -> usize {
    if payload.is_empty()
        || payload.starts_with(WEAK_FUZZY_PREFIX)
        || payload.starts_with("budget:0")
    {
        return 0;
    }
    let records = payload.lines().filter(|l| l.starts_with("HIT ")).count();
    if records > 0 {
        records
    } else {
        payload.lines().filter(|l| !l.is_empty()).count()
    }
}

impl FSZeroSession {
    pub fn do_search(&mut self, root: Option<&Path>, arg: Option<&str>) -> String {
        let q = arg.unwrap_or("main");
        // fszero-enuj: continue a prior page via cursor:<token>
        if let Some(token) = q.strip_prefix("cursor:") {
            return self.continue_search_cursor(token.trim());
        }
        if let Err(e) = self.prepare_index_or_busy(root) {
            return e;
        }
        let index_gen = self.index.ast_generation;
        self.caches.negative_cache.invalidate_generation(index_gen);
        // fszero-ojnv: certified empty short-circuit (complete scoped scan only).
        if self.caches.negative_cache.is_certified_empty(q, index_gen) {
            return format!("search:0 hits (certified-empty gen={index_gen})");
        }
        let cache_key = (q.to_string(), index_gen);
        if root.is_some() {
            if let Some(msg) = files_budget_message(self.indexed_file_count()) {
                if let Some((_, cap, scanned)) = parse_budget_message(&msg) {
                    self.store_budget_evidence("S", "files", cap, scanned);
                }
                return msg;
            }
        }
        if let Some((payload, search_ref)) = self.caches.search.get(&cache_key) {
            let hit_count = search_hit_count(payload);
            let payload = payload.clone();
            let search_ref = std::sync::Arc::clone(search_ref);
            if self.views.last_search_payload.as_deref() != Some(payload.as_bytes()) {
                self.views.last_search_payload = Some(payload.as_bytes().to_vec());
                self.recovery.put_key("search", payload.as_bytes());
            }
            return self.format_search_ack_paged(hit_count, &payload, &search_ref, q);
        }

        let route = classify_search_query(arg, q);
        let use_prefilter =
            ast_sgrep::literal_prefilter_from_env() == ast_sgrep::LiteralPrefilter::BigramMemmem;
        let payload = build_search_payload(
            route,
            root,
            &self.index,
            &self.recovery,
            q,
            use_prefilter.then_some(&mut self.lazy_bigrams),
        );
        if let Some((cap, scanned)) = parse_ast_nodes_budget(&payload) {
            self.store_budget_evidence("S", "ast_nodes", cap, scanned);
            return payload;
        }
        if payload.starts_with("budget:0") {
            return payload;
        }
        if let Some(err) = self.store_error_suffix("search") {
            return err;
        }
        let hit_count = search_hit_count(&payload);
        // True zero hits only (not weak-fuzzy noise) → certify empty (ojnv).
        if hit_count == 0
            && matches!(classify_search_query(arg, q), SearchRoute::Grep)
            && !payload.starts_with(WEAK_FUZZY_PREFIX)
        {
            self.caches
                .negative_cache
                .put_empty(q, vec![".".to_string()], index_gen);
        }
        self.views.last_search_payload = Some(payload.as_bytes().to_vec());
        self.recovery.put_key("search", payload.as_bytes());
        self.record_search_hits_access(&payload);
        if let Some(err) = self.store_error_suffix("search") {
            return err;
        }
        let search_ref: std::sync::Arc<str> =
            std::sync::Arc::from(self.recovery.put_content_ref(payload.as_bytes()));
        let ack = self.format_search_ack_paged(hit_count, &payload, &search_ref, q);
        self.caches
            .search
            .insert(cache_key, (payload, std::sync::Arc::clone(&search_ref)));
        ack
    }

    fn continue_search_cursor(&mut self, token: &str) -> String {
        let page_size = search_page_size();
        match self.caches.search_cursors.next(token, page_size) {
            None => "search:0 hits (cursor expired or unknown)".to_string(),
            Some(page) => {
                let body = page.items.join("\n");
                let hit_count = page.items.len();
                self.views.last_search_payload = Some(body.as_bytes().to_vec());
                self.recovery.put_key("search", body.as_bytes());
                let search_ref = self.recovery.put_content_ref(body.as_bytes());
                let fuzzy = body.contains("kind=fuzzy");
                format_search_page_ack(hit_count, &page, &search_ref, fuzzy)
            }
        }
    }

    fn format_search_ack_paged(
        &mut self,
        hit_count: usize,
        payload: &str,
        search_ref: &str,
        query: &str,
    ) -> String {
        if payload.starts_with(WEAK_FUZZY_PREFIX) {
            return format!("search:0 hits ({payload})");
        }
        if hit_count == 0 {
            return zero_hit_ack(query);
        }
        let fuzzy = payload.contains("kind=fuzzy");
        let records = split_hit_records(payload);
        let page_size = search_page_size();
        if records.len() > page_size {
            let page = self.caches.search_cursors.page(records, page_size);
            return format_search_page_ack(page.items.len(), &page, search_ref, fuzzy);
        }
        format_search_ack(hit_count, payload, search_ref, query)
    }
}

/// fszero-l54f / snap-to-file 99q7: payloads under the inline threshold are
/// returned whole; only larger sets fall back to a ref.
const SEARCH_INLINE_MAX_BYTES: usize = TARGET_INLINE_MAX_BYTES;

/// Regex metacharacters that make a literal-only query a likely false negative.
/// `:` is excluded because `callers:`/`defs:` are real structural prefixes, and
/// `-`/`_`/`.` are excluded because they are ordinary identifier/path characters.
const REGEX_METACHARACTERS: &[char] = &[
    '|', '[', ']', '(', ')', '{', '}', '^', '$', '+', '?', '*', '\\',
];

/// Metacharacters present in `query`, in first-appearance order, deduplicated.
fn regex_metacharacters_in(query: &str) -> Vec<char> {
    let mut found: Vec<char> = Vec::new();
    for ch in query.chars() {
        if REGEX_METACHARACTERS.contains(&ch) && !found.contains(&ch) {
            found.push(ch);
        }
    }
    found
}

/// Zero-hit ack, annotated when the query looks like a regex.
///
/// fs.search matches literal substrings. A regex is matched literally, so an
/// alternation or character class yields a confident "0 hits" that an agent
/// reads as proof of absence (fszero-codemode-search-literal-only-silent-toxz).
fn zero_hit_ack(query: &str) -> String {
    let meta = regex_metacharacters_in(query);
    if meta.is_empty() {
        return "search:0 hits".to_string();
    }
    let rendered: String = meta
        .iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "search:0 hits (literal-only: query contains regex metacharacter(s) {rendered} and was matched literally, not as a regex; 0 hits here is NOT proof of absence — search one term at a time, or use fs.multiSearch with regex:true)"
    )
}

fn format_search_ack(hit_count: usize, payload: &str, search_ref: &str, query: &str) -> String {
    if payload.starts_with(WEAK_FUZZY_PREFIX) {
        return format!("search:0 hits ({payload})");
    }
    if hit_count == 0 {
        return zero_hit_ack(query);
    }
    let fuzzy_note = if payload.contains("kind=fuzzy") {
        " fuzzy"
    } else {
        ""
    };
    if payload.len() <= SEARCH_INLINE_MAX_BYTES {
        // Sub-threshold payloads are NEVER preview-only: the canonical hit
        // records (target ref + intent + content window) are inlined whole.
        return format!("search:{hit_count} hits{fuzzy_note}\n{payload}");
    }
    format!("search:{hit_count} hits{fuzzy_note} ref={search_ref}")
}
