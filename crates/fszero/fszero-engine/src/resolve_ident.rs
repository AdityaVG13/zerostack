//! Identifier-aware tokenization for resolve ranking.

/// Shared English stop words for intent/token scoring (resolve + ast-sgrep).
pub const STOP: &[&str] = &[
    "a", "an", "the", "is", "are", "how", "does", "do", "what", "where", "we", "i", "to", "of",
    "in", "for", "and", "or", "it", "that", "this",
];

/// Sort by descending f64 score (NaN-safe). Ties keep relative order (stable).
#[inline]
pub fn sort_by_score_desc<T>(items: &mut [T], score: impl Fn(&T) -> f64) {
    items.sort_by(|a, b| {
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Sort by descending score, then ascending `&str` key for deterministic ties.
/// Needed when candidates come from a HashMap (iteration order is not stable).
#[inline]
pub fn sort_by_score_desc_then_key<T>(
    items: &mut [T],
    score: impl Fn(&T) -> f64,
    key: impl Fn(&T) -> &str,
) {
    items.sort_by(|a, b| {
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| key(a).cmp(key(b)))
    });
}

pub fn tokenize_intent(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    for raw in query.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        for frag in identifier_fragments(raw) {
            if frag.len() >= 2 && !STOP.contains(&frag.as_str()) {
                terms.push(frag);
            }
        }
    }
    terms.sort();
    terms.dedup();
    terms
}

pub fn identifier_fragments(ident: &str) -> Vec<String> {
    let mut frags = Vec::new();
    let mut push = |s: &str| {
        let t = s.trim().to_lowercase();
        if t.len() >= 2 {
            frags.push(t);
        }
    };
    for part in ident.split(['_', '-']) {
        if part.is_empty() {
            continue;
        }
        push(part);
        for c in split_camel_case(part) {
            push(&c);
        }
    }
    frags.sort();
    frags.dedup();
    frags
}

fn split_camel_case(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase()
            && i > 0
            && chars
                .get(i - 1)
                .map(|p| p.is_lowercase() || p.is_ascii_digit())
                .unwrap_or(false)
            && !cur.is_empty()
        {
            parts.push(cur.clone());
            cur.clear();
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

pub fn score_symbol_ident(sym: &str, terms: &[String]) -> f64 {
    let sym_l = sym.to_lowercase();
    let frags = identifier_fragments(sym);
    let mut score = 0.0;
    for t in terms {
        if sym_l == *t {
            score += 5.0;
        } else if frags.iter().any(|f| f == t) {
            score += 3.0;
        } else if sym_l.contains(t.as_str()) {
            score += 2.0;
        } else if frags
            .iter()
            .any(|f| f.contains(t.as_str()) || t.contains(f.as_str()))
        {
            score += 1.0;
        }
    }
    score
}

pub fn score_path_segments(path: &str, terms: &[String]) -> f64 {
    let p = path.to_lowercase();
    let mut score = 0.0;
    for segment in path.split(['/', '\\', '.']) {
        for frag in identifier_fragments(segment) {
            for t in terms {
                if frag == *t {
                    score += 1.5;
                } else if frag.contains(t.as_str()) {
                    score += 0.75;
                }
            }
        }
    }
    for t in terms {
        if p.contains(t.as_str()) {
            score += 1.0;
        }
    }
    score
}
