//! Levenshtein-1 (and common typo) hints for unknown CLI subcommands.

const SUBCOMMANDS: &[&str] = &[
    "index",
    "snap",
    "expand",
    "daemon",
    "serve",
    "compact",
    "stats",
    "query-surface",
    "ingest",
    "why",
    "blast",
    "reserve",
    "pack",
    "capabilities",
    "robot-docs",
    "agent-triage",
    "doctor",
    "orient",
    "search",
    "symbol",
    "verify",
    "publish",
];

/// If `raw` looks like an unknown subcommand error, return a corrective example.
pub fn subcommand_hint_from_parse_error(raw: &str) -> Option<String> {
    let typo = extract_unrecognized_subcommand(raw)?;
    if let Some(fixed) = levenshtein1_match(typo) {
        return Some(format!("graphzero {fixed} --help"));
    }
    common_typo_map(typo).map(str::to_string)
}

fn extract_unrecognized_subcommand(raw: &str) -> Option<&str> {
    for prefix in ["unrecognized subcommand '", "unrecognized subcommand \""] {
        if let Some(rest) = raw.split(prefix).nth(1) {
            let end = rest.find('\'').or_else(|| rest.find('"'))?;
            return Some(&rest[..end]);
        }
    }
    None
}

fn levenshtein1_match(word: &str) -> Option<&'static str> {
    let w = word.to_lowercase();
    let mut best: Option<(&'static str, usize)> = None;
    for cmd in SUBCOMMANDS {
        let d = levenshtein_distance(&w, &cmd.to_lowercase());
        let dominated = best.is_some_and(|(_, prev)| d >= prev);
        if d <= 2 && !dominated {
            best = Some((cmd, d));
        }
    }
    best.map(|(cmd, _)| cmd)
}

fn common_typo_map(word: &str) -> Option<&'static str> {
    match word.to_lowercase().as_str() {
        "serach" | "serch" | "seach" => Some("search"),
        "orrient" | "orientt" => Some("orient"),
        "indx" | "indxe" => Some("index"),
        "expnd" | "expan" => Some("expand"),
        "querry" | "qury" => Some("query-surface"),
        "capabilites" => Some("capabilities"),
        "triage" => Some("agent-triage"),
        _ => None,
    }
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (m, n) = (a.len(), b.len());
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}
