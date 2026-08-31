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
    if spec.starts_with("fz://") || spec.starts_with("gz://") || spec.starts_with("tz://") {
        return Err("retired product scheme; live content refs are z://blob/ or unprefixed keys");
    }
    let (first, repl) = if let Some(rest) = spec.strip_prefix("z://") {
        let idx = rest.find(':').ok_or("bad spec")?;
        (&spec[..5 + idx], &rest[idx + 1..])
    } else {
        let idx = spec.find(':').ok_or("bad spec")?;
        (&spec[..idx], &spec[idx + 1..])
    };
    let (old, new) = split_escaped_pipe(repl).ok_or("bad repl")?;
    let target = if first.starts_with("z://") {
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
