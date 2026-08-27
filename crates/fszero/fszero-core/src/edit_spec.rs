#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditTarget {
    Path(String),
    ViewId(u32),
    LastView,
    ContentRef(String),
}

#[derive(Debug, Clone)]
pub struct EditSpec {
    pub target: EditTarget,
    pub old: String,
    pub new: String,
}

/// Split `s` on the first unescaped `|` and return the unescaped `old`
/// half plus the verbatim `new` half.
fn split_escaped_pipe(s: &str) -> Option<(String, &str)> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'|' => {
                let old_raw = &s[..i];
                let new_raw = &s[i + 1..];
                let mut old = String::with_capacity(old_raw.len());
                let mut chars = old_raw.chars().peekable();
                while let Some(c) = chars.next() {
                    if c == '\\' {
                        match chars.next() {
                            Some('\\') => old.push('\\'),
                            Some('|') => old.push('|'),
                            Some(other) => {
                                old.push('\\');
                                old.push(other);
                            }
                            None => old.push('\\'),
                        }
                    } else {
                        old.push(c);
                    }
                }
                return Some((old, new_raw));
            }
            _ => i += 1,
        }
    }
    None
}

pub fn parse_edit_spec(spec: &str) -> Result<EditSpec, &'static str> {
    let parts: Vec<&str> = spec.splitn(2, ':').collect();
    if parts.len() != 2 {
        return Err("bad spec");
    }
    let (old, new) = split_escaped_pipe(parts[1]).ok_or("bad repl")?;
    let first = parts[0];
    let target = if first.starts_with("fz://") {
        EditTarget::ContentRef(first.to_string())
    } else if first == "last" || first == "lastR" {
        EditTarget::LastView
    } else if first.chars().all(|c| c.is_ascii_digit()) {
        EditTarget::ViewId(first.parse().unwrap_or(0))
    } else {
        EditTarget::Path(first.to_string())
    };
    Ok(EditSpec {
        target,
        old,
        new: new.to_string(),
    })
}

pub fn apply_unique_replace(text: &str, old: &str, new: &str) -> Result<String, &'static str> {
    if !text.contains(old) {
        return Err("no match");
    }
    if text.matches(old).count() != 1 {
        return Err("ambiguous match");
    }
    Ok(text.replacen(old, new, 1))
}

pub fn parse_path_edit_spec(spec: &str) -> Result<(String, String, String), &'static str> {
    let parsed = parse_edit_spec(spec)?;
    let EditTarget::Path(path) = parsed.target else {
        return Err("bad spec");
    };
    Ok((path, parsed.old, parsed.new))
}
