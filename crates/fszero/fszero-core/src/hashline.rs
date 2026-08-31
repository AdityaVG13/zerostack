//! Per-line content-hash anchors for stale edit rejection.

use sha2::{Digest, Sha256};

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

pub fn line_hash(line: &str) -> String {
    let mut h = Sha256::new();
    h.update(line.as_bytes());
    hex_encode(&h.finalize()[..8]) // short anchor
}

pub fn file_line_hashes(text: &str) -> Vec<String> {
    text.lines().map(line_hash).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleEditError {
    LineMismatch {
        line: usize,
        expected: String,
        actual: String,
    },
}

impl std::fmt::Display for StaleEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LineMismatch {
                line,
                expected,
                actual,
            } => write!(
                f,
                "stale edit at line {line}: expected hash {expected}, actual {actual}"
            ),
        }
    }
}
impl std::error::Error for StaleEditError {}

/// Reject edit if any anchored line no longer matches.
pub fn check_line_anchors(
    current_text: &str,
    anchors: &[(usize, String)],
) -> Result<(), StaleEditError> {
    let lines: Vec<&str> = current_text.lines().collect();
    for (line_1based, expected) in anchors {
        let idx = line_1based.saturating_sub(1);
        let actual = lines.get(idx).map(|l| line_hash(l)).unwrap_or_default();
        if &actual != expected {
            return Err(StaleEditError::LineMismatch {
                line: *line_1based,
                expected: expected.clone(),
                actual,
            });
        }
    }
    Ok(())
}
