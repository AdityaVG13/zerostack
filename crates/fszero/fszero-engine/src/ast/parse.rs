use super::types::{IndexedFn, IndexedImport, SymbolNodeKind};

pub fn parse_file_fns_heuristic(txt: &str) -> Vec<IndexedFn> {
    let mut byte = 0usize;
    let mut out = Vec::new();
    for line in txt.lines() {
        if let Some(name) = parse_fn_name(line) {
            out.push(IndexedFn {
                span_start: byte,
                span_end: byte + line.len(),
                name,
                kind: SymbolNodeKind::Fn,
            });
        }
        byte += line.len() + 1;
    }
    out
}

pub fn parse_file_imports_heuristic(txt: &str) -> Vec<IndexedImport> {
    let mut byte = 0usize;
    let mut out = Vec::new();
    for line in txt.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("import ") {
            let name = rest
                .trim_end_matches(';')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                out.push(IndexedImport {
                    span_start: byte,
                    span_end: byte + line.len(),
                    name,
                });
            }
        } else if let Some(rest) = trimmed.strip_prefix("from ") {
            if let Some(mod_part) = rest.split(" import ").next() {
                out.push(IndexedImport {
                    span_start: byte,
                    span_end: byte + line.len(),
                    name: mod_part.trim().to_string(),
                });
            }
        } else if let Some(rest) = trimmed.strip_prefix("use ") {
            let name = rest.trim_end_matches(';').trim().to_string();
            out.push(IndexedImport {
                span_start: byte + line.find("use ").unwrap_or(0),
                span_end: byte + line.len(),
                name,
            });
        }
        byte += line.len() + 1;
    }

    out
}

/// Identifier immediately before the first `(` (trimmed); None if empty.
fn name_before_paren(s: &str) -> Option<String> {
    let name = s.split('(').next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

pub fn parse_fn_name(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(name) = parse_ts_export_const_arrow(t) {
        return Some(name);
    }
    if let Some(name) = parse_ts_method_definition(t) {
        return Some(name);
    }
    if t.starts_with("def ") && t.contains('(') {
        return name_before_paren(t.strip_prefix("def ")?);
    }
    if (t.starts_with("function ") || t.contains(" function ")) && t.contains('(') {
        if let Some(pos) = t.find("function ") {
            return name_before_paren(&t[pos + 9..]);
        }
    }

    if (t.starts_with("fn ") || t.starts_with("pub fn ") || t.starts_with("pub fn "))
        && t.contains('(')
    {
        return name_before_paren(t.split_once("fn ")?.1);
    }
    None
}

fn parse_ts_export_const_arrow(line: &str) -> Option<String> {
    let t = line.trim();
    if !t.contains("export") || !t.contains("const ") || !t.contains("=>") {
        return None;
    }
    let after_const = t.split("const ").nth(1)?;
    let name = after_const.split('=').next()?.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    Some(name.to_string())
}

fn parse_ts_method_definition(line: &str) -> Option<String> {
    let t = line.trim();
    if !t.contains('(') || t.contains("function ") {
        return None;
    }
    let before_paren = t.split('(').next()?.trim();
    if before_paren.is_empty() {
        return None;
    }
    let name = before_paren.split_whitespace().next_back()?;
    if name == "if" || name == "for" || name == "while" || name == "switch" || name == "catch" {
        return None;
    }
    if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        Some(name.to_string())
    } else {
        None
    }
}
