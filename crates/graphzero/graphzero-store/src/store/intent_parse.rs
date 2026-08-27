//! Shared intent / symbol parsing for blast and reserve footprints.
//!
//! Lives in graphzero-store so graphzero-reserve does not depend on
//! graphzero-engine (breaks the reserve→query cycle for domain dispatch).

use serde::{Deserialize, Serialize};

/// Parsed free-text intent (blast / reserve footprint).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IntentParse {
    pub intent: String,
    pub target_symbol: Option<String>,
    pub target_ref: Option<String>,
    pub error: Option<String>,
}

/// Parse a blast/reserve intent string into a symbol target.
pub fn parse_intent(intent: &str) -> IntentParse {
    let trimmed = intent.trim();
    if trimmed.is_empty() {
        return IntentParse {
            intent: intent.to_string(),
            target_symbol: None,
            target_ref: None,
            error: Some("empty intent".into()),
        };
    }
    let lower = trimmed.to_lowercase();
    let target = if lower == "change" || lower == "change signature of" {
        ""
    } else if lower.starts_with("change signature of ") {
        &trimmed["change signature of ".len()..]
    } else if lower.starts_with("change ") {
        &trimmed["change ".len()..]
    } else {
        trimmed
    };
    let symbol = extract_symbol_token(target);
    match symbol {
        Some(sym) if is_ident(&sym) => IntentParse {
            intent: intent.to_string(),
            target_symbol: Some(sym.clone()),
            target_ref: Some(format!("gz://node/{sym}")),
            error: None,
        },
        Some(sym) => IntentParse {
            intent: intent.to_string(),
            target_symbol: Some(sym),
            target_ref: None,
            error: Some("invalid target symbol".into()),
        },
        None => IntentParse {
            intent: intent.to_string(),
            target_symbol: None,
            target_ref: None,
            error: Some("missing target symbol".into()),
        },
    }
}

fn extract_symbol_token(s: &str) -> Option<String> {
    let token = s
        .split_whitespace()
        .last()
        .unwrap_or(s)
        .trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn is_ident(s: &str) -> bool {
    s.chars()
        .next()
        .map(|c| c.is_ascii_alphabetic() || c == '_')
        .unwrap_or(false)
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}
