//! Session-scoped short ref aliases for visible capsules.
//!
//! Full-hash `tz://blob/<64hex>` / `fz://blob/<64hex>` refs cost ~18-25 BPE tokens
//! each. Visible text emits `tz://s/<16hex>` (GraphZero-aligned prefix length)
//! while the recovery store keeps `short → full` in the alias table so expand
//! accepts either form.
//!
//! Opacity (W4-OPAQUE-CAS-ALIAS): visible alias bytes MUST NOT be derived
//! from the raw content hash. Emission routes through the keyed derivation
//! `session_alias_hex_keyed` (HMAC-SHA-256 under a per-store key held inside
//! the recovery store state), so concurrent engines sharing a store agree on
//! the short form while the visible handle reveals nothing about payload
//! identity. The legacy content-derived helpers below remain only as shape
//! checkers (`*_is_some` gates) and for reading pre-opacity alias tables; new
//! emission must use the `*_keyed` variants.

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Byte length of the per-store alias derivation key.
pub const ALIAS_KEY_BYTES: usize = 32;

/// HMAC-SHA-256 (RFC 2104) over `msg` with a 32-byte key. Implemented on the
/// sha2 crate already in the tree; the block size for SHA-256 is 64 bytes.
fn hmac_sha256(key: &[u8; ALIAS_KEY_BYTES], msg: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..ALIAS_KEY_BYTES {
        ipad[i] ^= key[i];
        opad[i] ^= key[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner);
    outer.finalize().into()
}

/// Opaque 16-hex alias body for a 64-hex content hash under `key`.
///
/// Keyed derivation (HMAC-SHA-256 truncated to 8 bytes): the visible bytes
/// are computationally independent of the content hash without the key, and
/// deterministic for every engine sharing the store key.
pub fn session_alias_hex_keyed(key: &[u8; ALIAS_KEY_BYTES], hash: &str) -> String {
    let mac = hmac_sha256(key, hash.as_bytes());
    let mut out = String::with_capacity(SESSION_ALIAS_HEX_LEN);
    for byte in &mac[..SESSION_ALIAS_HEX_LEN / 2] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Session-visible short form under the store's alias key, preserving
/// fragments. Keyed twin of [`session_visible_blob_alias`].
pub fn session_visible_blob_alias_keyed(
    key: &[u8; ALIAS_KEY_BYTES],
    ref_id: &str,
) -> Option<String> {
    let (bare, frag) = split_ref_fragment(ref_id);
    let hash = full_hash_blob_parts(bare)?;
    let short = format!(
        "{SESSION_ALIAS_PREFIX}{}",
        session_alias_hex_keyed(key, hash)
    );
    Some(match frag {
        Some(f) => format!("{short}#{f}"),
        None => short,
    })
}

/// Prefix length for session-visible short aliases (matches GraphZero's 16-hex habit).
pub const SESSION_ALIAS_HEX_LEN: usize = 16;

const SESSION_ALIAS_PREFIX: &str = "tz://s/";
const SESSION_ORDINAL_PREFIX: &str = "tz://o/";

/// Format a generation-qualified dense ordinal alias.
pub fn session_ordinal_ref(generation: u64, ordinal: u64) -> String {
    format!("{SESSION_ORDINAL_PREFIX}{generation}/{ordinal}")
}

/// Parse a generation-qualified ordinal alias, excluding fragments.
pub fn parse_session_ordinal_bare(bare: &str) -> Option<(u64, u64)> {
    let (generation, ordinal) = bare.strip_prefix(SESSION_ORDINAL_PREFIX)?.split_once('/')?;
    let generation = generation.parse().ok()?;
    let ordinal = ordinal.parse().ok()?;
    (generation > 0 && ordinal > 0).then_some((generation, ordinal))
}

/// True when `bare` is a valid generation-qualified ordinal alias.
pub fn is_session_ordinal_bare(bare: &str) -> bool {
    parse_session_ordinal_bare(bare).is_some()
}

/// Split a ref into bare identity + optional `#B`/`#L` fragment.
pub fn split_ref_fragment(ref_id: &str) -> (&str, Option<&str>) {
    ref_id
        .split_once('#')
        .map_or((ref_id, None), |(bare, frag)| (bare, Some(frag)))
}

fn is_lower_hex(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// True when `bare` is a portable full-hash blob ref (`tz|fz|gz://blob/<64hex>`).
pub fn is_full_hash_blob_bare(bare: &str) -> bool {
    full_hash_blob_parts(bare).is_some()
}

fn full_hash_blob_parts(bare: &str) -> Option<&str> {
    for prefix in ["tz://blob/", "fz://blob/", "gz://blob/"] {
        if let Some(hash) = bare.strip_prefix(prefix)
            && hash.len() == 64
            && is_lower_hex(hash)
        {
            return Some(hash);
        }
    }
    None
}

/// Canonical full-hash target stored behind a session alias (`tz://blob/<64hex>`).
pub fn canonical_full_blob_ref(bare: &str) -> Option<String> {
    full_hash_blob_parts(bare).map(|hash| format!("tz://blob/{hash}"))
}

/// LEGACY content-derived short form (first 16 hex of the content hash).
/// Retained for shape checks and pre-opacity alias tables only; emission
/// must use [`session_visible_blob_alias_keyed`] (W4-OPAQUE-CAS-ALIAS).
///
/// Session-visible short form for a full-hash blob ref, preserving fragments.
///
/// Returns `None` when `ref_id` is not a portable full-hash blob ref (already
/// short, logical, file/unit, etc.).
pub fn session_visible_blob_alias(ref_id: &str) -> Option<String> {
    let (bare, frag) = split_ref_fragment(ref_id);
    let hash = full_hash_blob_parts(bare)?;
    let short = format!("{SESSION_ALIAS_PREFIX}{}", &hash[..SESSION_ALIAS_HEX_LEN]);
    Some(match frag {
        Some(f) => format!("{short}#{f}"),
        None => short,
    })
}

/// True when `bare` is a session short alias (`tz://s/<1-64 hex>`).
pub fn is_session_alias_bare(bare: &str) -> bool {
    bare.strip_prefix(SESSION_ALIAS_PREFIX)
        .is_some_and(|id| !id.is_empty() && id.len() <= 64 && is_lower_hex(id))
}

/// Replace every full-hash blob ref in `text` with its session-visible alias.
pub fn rewrite_full_hash_blob_refs_in_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if let Some((consumed, replacement)) = match_full_hash_blob_at(text, i) {
            out.push_str(&replacement);
            i += consumed;
            continue;
        }
        // Advance one full character: refs are pure ASCII, so multibyte
        // characters are copied verbatim (byte-wise `as char` casts would
        // mojibake them, and mid-char slicing panics).
        let next = (i + 1..=text.len())
            .find(|&index| text.is_char_boundary(index))
            .unwrap_or(text.len());
        out.push_str(&text[i..next]);
        i = next;
    }
    out
}

/// If `text[start..]` begins with a full-hash blob ref (optional fragment),
/// return `(end_byte_index, full_ref_string)`.
pub fn take_full_hash_blob_at(text: &str, start: usize) -> Option<(usize, String)> {
    let (consumed, _replacement) = match_full_hash_blob_at(text, start)?;
    let full = text[start..start + consumed].to_string();
    Some((start + consumed, full))
}

fn match_full_hash_blob_at(text: &str, start: usize) -> Option<(usize, String)> {
    // Callers scan byte offsets; a mid-character offset can never start an
    // ASCII ref and must not panic the slice below.
    if !text.is_char_boundary(start) {
        return None;
    }
    let rest = &text[start..];
    for prefix in ["tz://blob/", "fz://blob/", "gz://blob/"] {
        if !rest.starts_with(prefix) {
            continue;
        }
        let after = &rest[prefix.len()..];
        if after.len() < 64 {
            return None;
        }
        let hash = &after[..64];
        if !is_lower_hex(hash) {
            return None;
        }
        let mut consumed = prefix.len() + 64;
        let mut frag: Option<&str> = None;
        if let Some(tail) = after.get(64..)
            && let Some(stripped) = tail.strip_prefix('#')
            && let Some(frag_len) = fragment_len(stripped)
        {
            frag = Some(&tail[..=frag_len]);
            consumed += 1 + frag_len;
        }
        let short = format!("{SESSION_ALIAS_PREFIX}{}", &hash[..SESSION_ALIAS_HEX_LEN]);
        let replacement = match frag {
            Some(f) => format!("{short}{f}"),
            None => short,
        };
        return Some((consumed, replacement));
    }
    None
}

fn fragment_len(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    let kind = bytes[0] as char;
    if kind != 'B' && kind != 'L' {
        return None;
    }
    let mut i = 1;
    let mut saw_digit = false;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        saw_digit = true;
        i += 1;
    }
    if !saw_digit {
        return None;
    }
    if i < bytes.len() && bytes[i] == b'-' {
        i += 1;
        let mut saw_end = false;
        if kind == 'L' && i < bytes.len() && bytes[i] == b'L' {
            i += 1;
        }
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            saw_end = true;
            i += 1;
        }
        if !saw_end {
            return None;
        }
    }
    Some(i)
}

/// Walk a JSON value and rewrite full-hash blob ref strings in place.
pub fn rewrite_full_hash_blob_refs_in_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            if let Some(short) = session_visible_blob_alias(text) {
                *text = short;
            } else if text.contains("://blob/") {
                let rewritten = rewrite_full_hash_blob_refs_in_text(text);
                if rewritten != *text {
                    *text = rewritten;
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_full_hash_blob_refs_in_value(item);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                rewrite_full_hash_blob_refs_in_value(item);
            }
        }
        _ => {}
    }
}
