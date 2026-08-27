//! Deterministic wiki layer for durable memory (fszero-ockx).
//!
//! OKF frontmatter + content-hash staleness. Link graph is path-local only.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkfDocument {
    pub frontmatter: BTreeMap<String, String>,
    pub body: String,
    pub content_hash: String,
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn content_hash(body: &str) -> String {
    let mut h = Sha256::new();
    h.update(body.as_bytes());
    hex_encode(h.finalize().as_slice())
}

/// Parse simple `---` YAML-like key: value frontmatter (no nested YAML).
pub fn parse_okf(raw: &str) -> Result<OkfDocument, String> {
    let raw = raw.trim_start();
    if !raw.starts_with("---") {
        let body = raw.to_string();
        return Ok(OkfDocument {
            frontmatter: BTreeMap::new(),
            content_hash: content_hash(&body),
            body,
        });
    }
    let rest = raw.trim_start_matches("---").trim_start_matches('\n');
    let Some((fm, body)) = rest.split_once("\n---") else {
        return Err("OKF frontmatter missing closing ---".into());
    };
    let mut frontmatter = BTreeMap::new();
    for line in fm.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        frontmatter.insert(k.trim().to_string(), v.trim().to_string());
    }
    let body = body.trim_start_matches('\n').to_string();
    let hash = content_hash(&body);
    Ok(OkfDocument {
        frontmatter,
        body,
        content_hash: hash,
    })
}

pub fn is_stale(doc: &OkfDocument, expected_hash: &str) -> bool {
    doc.content_hash != expected_hash
}

/// Extract `[[wiki-links]]` from body (deterministic, order-preserving unique).
pub fn wiki_links(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        rest = &rest[start + 2..];
        if let Some(end) = rest.find("]]") {
            let link = rest[..end].trim().to_string();
            if !link.is_empty() && !out.contains(&link) {
                out.push(link);
            }
            rest = &rest[end + 2..];
        } else {
            break;
        }
    }
    out
}
