//! Recall-budget formatting and skeleton attachment for memory hints.

use super::types::{MemoryFact, MemoryHint};

pub fn format_recall_budget_one(target: &str, facts: &[&MemoryFact]) -> String {
    if facts.is_empty() {
        return format!("mem: 0 facts for {target}");
    }
    let n = facts.len();
    let mut lines = vec![format!("mem: {n} facts for {target}")];
    for f in facts.iter().take(2) {
        let preview: String = f.text.chars().take(80).collect();
        lines.push(format!(
            "  {}: {} (gz://mem/{})",
            f.kind.as_str(),
            preview,
            f.id
        ));
    }
    lines.join("\n")
}

pub fn attach_memory_to_skeleton(base: &str, hints: &[MemoryHint]) -> String {
    if hints.is_empty() {
        return base.to_string();
    }
    let mut out = base.to_string();
    for h in hints {
        out.push('\n');
        out.push_str(&h.line);
    }
    out
}
