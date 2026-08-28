//! Independent active-waiver parser (fszero-ncib.10).
//!
//! Each waiver is a record: id, owner, expiry (ISO), scope, rationale, evidence.
//! Missing/malformed/expired fields fail closed.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiver {
    pub id: String,
    pub owner: String,
    pub expiry: String,
    pub scope: String,
    pub rationale: String,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaiverError {
    MissingFile(String),
    MissingField {
        waiver_id: String,
        field: String,
    },
    InvalidExpiry {
        waiver_id: String,
        expiry: String,
    },
    Expired {
        waiver_id: String,
        expiry: String,
        today: String,
    },
    EmptyWaivers,
}

impl std::fmt::Display for WaiverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingFile(p) => write!(f, "waiver file missing: {p}"),
            Self::MissingField { waiver_id, field } => {
                write!(f, "waiver {waiver_id} missing required field {field}")
            }
            Self::InvalidExpiry { waiver_id, expiry } => {
                write!(f, "waiver {waiver_id} has invalid ISO expiry: {expiry}")
            }
            Self::Expired {
                waiver_id,
                expiry,
                today,
            } => write!(f, "waiver {waiver_id} expired on {expiry} (today={today})"),
            Self::EmptyWaivers => {
                write!(f, "no active waivers parsed (document empty or malformed)")
            }
        }
    }
}

fn field_from_table_block(block: &str, name: &str) -> Option<String> {
    // Match rows like: | **owner** | `aditya` |  or | **owner** | aditya |
    let needle = format!("**{name}**");
    for line in block.lines() {
        let t = line.trim();
        if !t.starts_with('|') || !t.contains(&needle) {
            continue;
        }
        // Split markdown table cells
        let cells: Vec<&str> = t
            .split('|')
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .collect();
        // Expect [field, value]
        if cells.len() >= 2 && cells[0].contains(&needle) {
            let mut val = cells[1].trim().to_string();
            if val.starts_with('`') && val.ends_with('`') && val.len() >= 2 {
                val = val[1..val.len() - 1].to_string();
            }
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    None
}

fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 {
        return false;
    }
    // YYYY-MM-DD
    b[0..4].iter().all(|c| c.is_ascii_digit())
        && b[4] == b'-'
        && b[5..7].iter().all(|c| c.is_ascii_digit())
        && b[7] == b'-'
        && b[8..10].iter().all(|c| c.is_ascii_digit())
}

fn require_table_field(body: &str, waiver_id: &str, field: &str) -> Result<String, WaiverError> {
    field_from_table_block(body, field).ok_or_else(|| WaiverError::MissingField {
        waiver_id: waiver_id.to_string(),
        field: field.into(),
    })
}

/// Parse active waivers under `## Active waivers` as independent `### W…` sections.
pub fn parse_active_waivers(markdown: &str) -> Result<Vec<Waiver>, WaiverError> {
    let mut waivers = Vec::new();
    let mut in_active = false;
    let mut current: Option<String> = None;
    let mut buf = String::new();

    let flush = |id: &str, body: &str, out: &mut Vec<Waiver>| -> Result<(), WaiverError> {
        let owner = require_table_field(body, id, "owner")?;
        let expiry = require_table_field(body, id, "expiry")?;
        let scope = require_table_field(body, id, "scope")?;
        let rationale = require_table_field(body, id, "rationale")?;
        let evidence = require_table_field(body, id, "evidence")?;
        let id_field = field_from_table_block(body, "id").unwrap_or_else(|| id.to_string());
        if !is_iso_date(&expiry) {
            return Err(WaiverError::InvalidExpiry {
                waiver_id: id_field.clone(),
                expiry,
            });
        }
        out.push(Waiver {
            id: id_field,
            owner,
            expiry,
            scope,
            rationale,
            evidence,
        });
        Ok(())
    };

    for line in markdown.lines() {
        if line.starts_with("## Active waivers") {
            in_active = true;
            continue;
        }
        if in_active && line.starts_with("## ") && !line.starts_with("## Active") {
            // end active section
            if let Some(id) = current.take() {
                flush(&id, &buf, &mut waivers)?;
                buf.clear();
            }
            in_active = false;
            continue;
        }
        if !in_active {
            continue;
        }
        if line.starts_with("### ") {
            if let Some(id) = current.take() {
                flush(&id, &buf, &mut waivers)?;
                buf.clear();
            }
            current = Some(line.trim_start_matches('#').trim().to_string());
            continue;
        }
        if current.is_some() {
            buf.push_str(line);
            buf.push('\n');
        }
    }
    if let Some(id) = current.take() {
        flush(&id, &buf, &mut waivers)?;
    }
    if waivers.is_empty() {
        return Err(WaiverError::EmptyWaivers);
    }
    Ok(waivers)
}

/// Validate every waiver is non-expired relative to `today` (YYYY-MM-DD).
pub fn validate_waivers_not_expired(waivers: &[Waiver], today: &str) -> Result<(), WaiverError> {
    for w in waivers {
        if !is_iso_date(&w.expiry) {
            return Err(WaiverError::InvalidExpiry {
                waiver_id: w.id.clone(),
                expiry: w.expiry.clone(),
            });
        }
        if w.expiry.as_str() < today {
            return Err(WaiverError::Expired {
                waiver_id: w.id.clone(),
                expiry: w.expiry.clone(),
                today: today.to_string(),
            });
        }
        for (field, val) in [
            ("owner", &w.owner),
            ("scope", &w.scope),
            ("rationale", &w.rationale),
            ("evidence", &w.evidence),
        ] {
            if val.trim().is_empty() {
                return Err(WaiverError::MissingField {
                    waiver_id: w.id.clone(),
                    field: field.into(),
                });
            }
        }
    }
    Ok(())
}

/// Load + parse + validate from path.
pub fn load_and_validate_waivers(path: &Path, today: &str) -> Result<Vec<Waiver>, WaiverError> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| WaiverError::MissingFile(path.display().to_string()))?;
    let waivers = parse_active_waivers(&text)?;
    validate_waivers_not_expired(&waivers, today)?;
    Ok(waivers)
}
