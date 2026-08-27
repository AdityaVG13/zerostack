//! Compact outline skeleton string for budget-friendly agent orientation.

use super::types::OutlineItem;

const MAX_SYMBOLS: usize = 20;

/// 1-based inclusive line from byte offset in UTF-8 blob text.
pub fn line_at_byte_offset(blob: &[u8], byte_off: u32) -> u32 {
    let off = byte_off.min(blob.len() as u32) as usize;
    let mut line = 1u32;
    for &b in &blob[..off] {
        if b == b'\n' {
            line += 1;
        }
    }
    line
}

pub fn byte_span_to_lines(blob: &[u8], start: u32, end: u32) -> (u32, u32) {
    let start_line = line_at_byte_offset(blob, start);
    let end_line = line_at_byte_offset(blob, end.saturating_sub(1).max(start));
    (start_line, end_line.max(start_line))
}

fn kind_abbrev(kind: &str, name_collides_across_kinds: bool) -> String {
    if !name_collides_across_kinds {
        return String::new();
    }
    match kind {
        "function" => "fn".into(),
        "type" => "ty".into(),
        "module" => "mod".into(),
        _ => "sym".into(),
    }
}

fn names_collide_across_kinds(items: &[OutlineItem]) -> std::collections::HashSet<String> {
    use std::collections::HashMap;
    let mut kinds_by_name: HashMap<&str, std::collections::HashSet<&str>> = HashMap::new();
    for it in items {
        kinds_by_name
            .entry(it.name.as_str())
            .or_default()
            .insert(it.kind.as_str());
    }
    kinds_by_name
        .into_iter()
        .filter(|(_, kinds)| kinds.len() > 1)
        .map(|(n, _)| n.to_string())
        .collect()
}

/// Format: `{path}: name start-end; … [gz://blob/<hash>]` with kind abbrev
/// only when names collide across kinds. The trailing blob ref is the
/// ref-first recovery anchor: budget=1 capsules must always carry a gz://
/// evidence ref (release gate `assert_ref_first`), and every outline item of
/// one file shares the same blob, so one bare ref anchors them all.
pub fn format_outline_skeleton(rel: &str, items: &[OutlineItem]) -> String {
    if items.is_empty() {
        return format!("{rel}:");
    }
    let collide = names_collide_across_kinds(items);
    let mut parts: Vec<String> = Vec::new();
    let show = items.len().min(MAX_SYMBOLS);
    for it in items.iter().take(show) {
        let (sl, el) = match (it.start_line, it.end_line) {
            (Some(s), Some(e)) => (s, e),
            _ => continue,
        };
        let abbrev = kind_abbrev(&it.kind, collide.contains(&it.name));
        let seg = if abbrev.is_empty() {
            format!("{} {}-{}", it.name, sl, el)
        } else {
            format!("{} {abbrev} {}-{}", it.name, sl, el)
        };
        parts.push(seg);
    }
    let mut body = parts.join("; ");
    if items.len() > MAX_SYMBOLS {
        let more = items.len() - MAX_SYMBOLS;
        if body.is_empty() {
            body = format!("+{more} more");
        } else {
            body.push_str(&format!("; +{more} more"));
        }
    }
    if let Some(anchor) = items
        .iter()
        .find(|it| it.evidence_ref.starts_with("gz://"))
        .map(|it| {
            it.evidence_ref
                .split('#')
                .next()
                .unwrap_or(&it.evidence_ref)
        })
    {
        body.push_str(&format!(" [{anchor}]"));
    }
    format!("{rel}: {body}")
}
