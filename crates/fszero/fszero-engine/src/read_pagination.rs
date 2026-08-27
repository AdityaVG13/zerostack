//! Read pagination contract: range + next offset + remaining (fszero-cd0v).

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadPage {
    /// Inclusive start offset (bytes for [`page_bytes`], 1-indexed lines for [`page_lines`]).
    pub start: usize,
    /// Exclusive end for bytes; inclusive end for lines.
    pub end: usize,
    pub next_offset: Option<usize>,
    pub remaining: usize,
    pub total: usize,
    pub bytes: Vec<u8>,
}

/// Byte-window page over `full` starting at `offset` with at most `limit` bytes.
pub fn page_bytes(full: &[u8], offset: usize, limit: usize) -> ReadPage {
    let start = offset.min(full.len());
    let end = (start + limit).min(full.len());
    let remaining = full.len().saturating_sub(end);
    let next_offset = if remaining > 0 { Some(end) } else { None };
    ReadPage {
        start,
        end,
        next_offset,
        remaining,
        total: full.len(),
        bytes: full[start..end].to_vec(),
    }
}

/// 1-indexed inclusive line window. `start_line`/`end_line` are clamped to content.
/// `next_offset` is the next 1-indexed line to resume from when truncated by end.
pub fn page_lines(full: &str, start_line: usize, end_line: usize) -> ReadPage {
    let total = if full.is_empty() {
        0
    } else {
        full.lines().count()
    };
    if total == 0 || start_line == 0 {
        return ReadPage {
            start: 0,
            end: 0,
            next_offset: None,
            remaining: 0,
            total,
            bytes: Vec::new(),
        };
    }
    let start = start_line.max(1).min(total.saturating_add(1));
    let end = end_line.max(start.saturating_sub(1)).min(total);
    if start > total {
        return ReadPage {
            start,
            end: start.saturating_sub(1),
            next_offset: None,
            remaining: 0,
            total,
            bytes: Vec::new(),
        };
    }
    let mut out = String::new();
    for (i, line) in full.lines().enumerate() {
        let n = i + 1;
        if n < start {
            continue;
        }
        if n > end {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    let remaining = total.saturating_sub(end);
    let next_offset = if remaining > 0 { Some(end + 1) } else { None };
    ReadPage {
        start,
        end,
        next_offset,
        remaining,
        total,
        bytes: out.into_bytes(),
    }
}
