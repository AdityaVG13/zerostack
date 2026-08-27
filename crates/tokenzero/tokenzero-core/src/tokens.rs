use std::sync::LazyLock;

/// Lookup table for hex nibble encoding.
pub(crate) const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

/// Encode one byte as two lowercase hex characters, pushed into `out`.
#[inline]
pub(crate) fn push_hex_byte(out: &mut String, b: u8) {
    out.push(HEX_CHARS[(b >> 4) as usize] as char);
    out.push(HEX_CHARS[(b & 0x0f) as usize] as char);
}

pub fn sha256_hex(text: &str) -> String {
    zero_abi::sha256_hex(text.as_bytes())
}

/// Unlabeled estimator alias. Pulse grammar is `estimator:<slug>`.
///
/// `estimate:` looks labeled but is the Phase-4 honesty hole: kernel used to
/// emit it and Pulse never accepted it. It is not an alias of `estimator:`.
pub const UNLABELED_ESTIMATE_TOKENIZER_PREFIX: &str = "estimate:";

/// Pulse-grammar estimator id for [`count_tokens`] with no family metadata.
/// Same slug the kernel measure emits when no model BPE is bound.
pub const LEXICAL_ESTIMATOR_ID: &str = "estimator:tokenzero-lexical";
/// Pulse-grammar estimator id for non-UTF-8 byte mass (1 token per byte).
pub const BYTES_ESTIMATOR_ID: &str = "estimator:tokenzero-bytes";

/// MCP hub registry labels. Forbidden as tokenizer ids (K-9 identity collision).
pub const FORBIDDEN_MCP_ENGINE_IDENTITY: &str = "EngineIdentity::TokenZero";
/// Sibling hub registry label. Forbidden as a tokenizer id for the same reason.
pub const FORBIDDEN_MCP_REGISTRY_ENGINE: &str = "RegistryEngine::TokenZero";

/// True when `id` is an MCP registry label, not a Pulse/kernel tokenizer.
pub fn is_forbidden_mcp_tokenizer_identity(id: &str) -> bool {
    id.contains(FORBIDDEN_MCP_ENGINE_IDENTITY) || id.contains(FORBIDDEN_MCP_REGISTRY_ENGINE)
}

/// Honesty preflight refusal for tokenizer ids that must never count as exact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizerIdPreflightError {
    Empty,
    UnlabeledEstimateAlias,
    Q99IsNotExact,
    ExactLabelIsNotATokenizerId,
    McpRegistryIdentity,
}

impl TokenizerIdPreflightError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "tokenizer id is empty",
            Self::UnlabeledEstimateAlias => {
                "tokenizer id estimate: is unlabeled; Pulse grammar is estimator:<slug>"
            }
            Self::Q99IsNotExact => "Q99 is not a tokenizer identity and is never exact",
            Self::ExactLabelIsNotATokenizerId => {
                "exact is not a tokenizer identity; use provider/model@digest"
            }
            Self::McpRegistryIdentity => {
                "MCP EngineIdentity::TokenZero / RegistryEngine::TokenZero is not a tokenizer id"
            }
        }
    }
}

impl std::fmt::Display for TokenizerIdPreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

fn tokenizer_id_last_component(id: &str) -> &str {
    id.rsplit([':', '/', '@']).next().unwrap_or(id)
}

/// Fail-closed honesty preflight for kernel/Pulse tokenizer ids.
///
/// Full Pulse grammar (`estimator:` / `tiktoken:` / `provider/model@hex`) stays
/// in Pulse. This gate is the retry predicate: unlabeled `estimate:` is never
/// an estimator alias, and `Q99` / `exact` never parse as exact identities.
pub fn preflight_tokenizer_id(id: &str) -> Result<(), TokenizerIdPreflightError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(TokenizerIdPreflightError::Empty);
    }
    if is_forbidden_mcp_tokenizer_identity(id) {
        return Err(TokenizerIdPreflightError::McpRegistryIdentity);
    }
    if id.starts_with(UNLABELED_ESTIMATE_TOKENIZER_PREFIX) {
        return Err(TokenizerIdPreflightError::UnlabeledEstimateAlias);
    }
    let leaf = tokenizer_id_last_component(id);
    if leaf.eq_ignore_ascii_case("q99") || leaf.eq_ignore_ascii_case("q99-input") {
        return Err(TokenizerIdPreflightError::Q99IsNotExact);
    }
    if id.eq_ignore_ascii_case("exact") {
        return Err(TokenizerIdPreflightError::ExactLabelIsNotATokenizerId);
    }
    Ok(())
}

/// The lossy declaration emitted when the visible budget drops bytes and no
/// recovery ref is available.
///
/// Single source of truth. This literal was previously duplicated in
/// `enforce_token_budget_with_ref` and in the capsule emitter, and the budget
/// test modelled a THIRD, shorter string. The test computed how many lines
/// should survive using a 14-token marker while the real marker costs 33, so
/// it demanded more lines than the budget could hold and failed as
/// P01-001. Keep every user pointed at this constant.
pub const VISIBLE_BUDGET_LOSSY_DECLARATION: &str = "[mode=lossy lossy_policy_id=tokenzero.visible-compression.v1 lossy_spans=[{description=omitted-bytes reason=visible-budget recovery_may_be_needed=true}]]";

fn visible_budget_marker(recovery_ref: Option<&str>) -> String {
    recovery_ref.map_or_else(
        || VISIBLE_BUDGET_LOSSY_DECLARATION.to_string(),
        |reference| {
            format!(
                "{VISIBLE_BUDGET_LOSSY_DECLARATION} recovery_ref={reference};                  expand {reference} for the full output"
            )
        },
    )
}

pub fn enforce_token_budget(text: &str, max_visible_tokens: usize) -> String {
    enforce_token_budget_with_ref(text, max_visible_tokens, None)
}

/// Enforce the visible budget while naming an exact recovery ref when available.
pub fn enforce_token_budget_with_ref(
    text: &str,
    max_visible_tokens: usize,
    recovery_ref: Option<&str>,
) -> String {
    if count_tokens(text) <= max_visible_tokens {
        return text.to_string();
    }

    let marker = visible_budget_marker(recovery_ref);
    let marker_tokens = count_tokens(&marker);

    // Structured elision can fit below the longer plain-text correctness floor.
    // Try it first so valid objects and arrays remain valid whenever their minimal
    // sentinel representation fits.
    if let Some(json) = elide_top_level_json(text, max_visible_tokens, recovery_ref) {
        return json;
    }
    if matches!(
        serde_json::from_str::<serde_json::Value>(text),
        Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_))
    ) {
        // A structured payload reached here only because the minimal sentinel did
        // not fit or its reserved object key collided. Never emit a JSON prefix.
        return marker;
    }

    if marker_tokens > max_visible_tokens {
        // The canonical lossy declaration is a correctness floor. An impossibly
        // small budget must not turn an omission into unclassified free text.
        return marker;
    }

    retain_plain_lines_after_marker(text, max_visible_tokens, marker)
}

const INLINE_ELISION_SENTINEL_KEY: &str = "__tokenzero_elision__";

fn elide_top_level_json(
    text: &str,
    max_visible_tokens: usize,
    recovery_ref: Option<&str>,
) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let sentinel = serde_json::json!({
        "lossy": true,
        "reason": "visible-budget",
        "recovery_ref": recovery_ref,
    });
    let sentinel = serde_json::to_string(&sentinel).expect("sentinel is serializable");
    let key =
        serde_json::to_string(INLINE_ELISION_SENTINEL_KEY).expect("sentinel key is serializable");

    match value {
        serde_json::Value::Object(entries) => {
            if entries.contains_key(INLINE_ELISION_SENTINEL_KEY) {
                return None;
            }
            let mut out = format!("{{{key}:{sentinel}");
            if count_tokens(&format!("{out}}}")) > max_visible_tokens {
                return None;
            }
            for (entry_key, value) in entries.iter().take(entries.len().saturating_sub(1)) {
                let entry_key = serde_json::to_string(entry_key).ok()?;
                let value = serde_json::to_string(value).ok()?;
                let candidate = format!("{out},{entry_key}:{value}}}");
                if count_tokens(&candidate) > max_visible_tokens {
                    break;
                }
                out.push(',');
                out.push_str(&entry_key);
                out.push(':');
                out.push_str(&value);
            }
            out.push('}');
            Some(out)
        }
        serde_json::Value::Array(items) => {
            let mut out = format!("[{{{key}:{sentinel}}}");
            if count_tokens(&format!("{out}]")) > max_visible_tokens {
                return None;
            }
            for value in items.iter().take(items.len().saturating_sub(1)) {
                let value = serde_json::to_string(value).ok()?;
                let candidate = format!("{out},{value}]");
                if count_tokens(&candidate) > max_visible_tokens {
                    break;
                }
                out.push(',');
                out.push_str(&value);
            }
            out.push(']');
            Some(out)
        }
        _ => None,
    }
}

fn retain_plain_lines_after_marker(
    text: &str,
    max_visible_tokens: usize,
    marker: String,
) -> String {
    let mut out = marker;
    for line in text.split_inclusive('\n') {
        let mut candidate = String::with_capacity(out.len() + 1 + line.len());
        candidate.push_str(&out);
        candidate.push('\n');
        candidate.push_str(line);
        if count_tokens(&candidate) > max_visible_tokens {
            break;
        }
        out = candidate;
    }
    out
}

/// Tokenizer families whose local token-cost characteristics TokenZero knows.
///
/// No tokenizer vocabulary is linked today. The registered families therefore
/// use disclosed average character costs; unknown models retain the legacy
/// lexical counter exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenizerFamily {
    Cl100k,
    O200k,
    SentencePiece,
}

impl TokenizerFamily {
    /// Stable lowercase family name, used in count-method stamps so ledger
    /// records name the exact counting family without depending on Debug
    /// formatting.
    pub fn name(self) -> &'static str {
        match self {
            TokenizerFamily::Cl100k => "cl100k",
            TokenizerFamily::O200k => "o200k",
            TokenizerFamily::SentencePiece => "sentencepiece",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenizerMetadata {
    pub family: TokenizerFamily,
    /// Average Unicode scalar values per token, scaled by 1,000.
    pub chars_per_token_milli: usize,
    /// Whether counts and boundaries are estimates rather than vocabulary
    /// lookups. This remains true until a real tokenizer is linked.
    pub approximate: bool,
}

const fn tokenizer(family: TokenizerFamily, chars_per_token_milli: usize) -> TokenizerMetadata {
    TokenizerMetadata {
        family,
        chars_per_token_milli,
        approximate: true,
    }
}

const CL100K: TokenizerMetadata = tokenizer(TokenizerFamily::Cl100k, 4_000);
const O200K: TokenizerMetadata = tokenizer(TokenizerFamily::O200k, 4_000);
const SENTENCEPIECE: TokenizerMetadata = tokenizer(TokenizerFamily::SentencePiece, 3_500);

/// Resolve a provider model id without allocating or making network calls.
pub fn tokenizer_metadata(model_id: &str) -> Option<&'static TokenizerMetadata> {
    let model = model_id.rsplit('/').next().unwrap_or(model_id);
    const RULES: &[(&TokenizerMetadata, &[&str])] = &[
        (&O200K, &["gpt-4o", "gpt-4.1", "gpt-5", "o1", "o3", "o4"]),
        (&CL100K, &["gpt-4", "gpt-3.5"]),
        (&SENTENCEPIECE, &["llama", "mistral", "mixtral", "gemma"]),
    ];
    if contains_ignore_ascii_case(model, "codex") {
        return Some(&O200K);
    }
    RULES.iter().find_map(|(metadata, prefixes)| {
        prefixes
            .iter()
            .any(|prefix| starts_with_ignore_ascii_case(model, prefix))
            .then_some(*metadata)
    })
}

fn starts_with_ignore_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn contains_ignore_ascii_case(value: &str, needle: &str) -> bool {
    value
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[derive(Debug)]
struct ActiveTokenizer {
    model_id: Option<String>,
    metadata: Option<&'static TokenizerMetadata>,
}

static ACTIVE_TOKENIZER: LazyLock<ActiveTokenizer> = LazyLock::new(|| {
    let model_id = ["TOKENZERO_MODEL", "OMP_MODEL", "OPENAI_MODEL"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()));
    let metadata = model_id.as_deref().and_then(tokenizer_metadata);
    ActiveTokenizer { model_id, metadata }
});

/// Model selected once from `TOKENZERO_MODEL`, `OMP_MODEL`, then
/// `OPENAI_MODEL`, in precedence order.
pub fn active_model_id() -> Option<&'static str> {
    ACTIVE_TOKENIZER.model_id.as_deref()
}

pub fn active_tokenizer_metadata() -> Option<&'static TokenizerMetadata> {
    ACTIVE_TOKENIZER.metadata
}

/// Count for an explicit model. Unknown or absent ids deliberately preserve
/// the pre-registry lexical heuristic.
pub fn count_tokens_for_model(text: &str, model_id: Option<&str>) -> usize {
    match model_id.and_then(tokenizer_metadata) {
        Some(metadata) => approximate_token_count(text, metadata),
        None => count_tokens_lexical(text),
    }
}

fn approximate_token_count(text: &str, metadata: &TokenizerMetadata) -> usize {
    text.chars()
        .count()
        .saturating_mul(1_000)
        .div_ceil(metadata.chars_per_token_milli)
}

/// Return the largest prefix that fits `max_tokens` and ends at a token
/// boundary for the selected model. Registered tokenizers use their disclosed
/// average-width boundary; the fallback never cuts a lexical word.
pub fn pack_to_token_boundary(text: &str, max_tokens: usize) -> &str {
    pack_to_token_boundary_with_char_limit(text, max_tokens, usize::MAX)
}

/// Pack a preview while treating refs separately: callers can retain a full,
/// atomic ref and apply both the remaining token budget and a display-width
/// cap to only the preview.
pub fn pack_to_token_boundary_with_char_limit(
    text: &str,
    max_tokens: usize,
    max_chars: usize,
) -> &str {
    pack_to_token_boundary_for_model_with_char_limit(text, max_tokens, max_chars, active_model_id())
}

pub fn pack_to_token_boundary_for_model<'a>(
    text: &'a str,
    max_tokens: usize,
    model_id: Option<&str>,
) -> &'a str {
    pack_to_token_boundary_for_model_with_char_limit(text, max_tokens, usize::MAX, model_id)
}

pub fn pack_to_token_boundary_for_model_with_char_limit<'a>(
    text: &'a str,
    max_tokens: usize,
    max_chars: usize,
    model_id: Option<&str>,
) -> &'a str {
    if text.is_empty() || max_tokens == 0 || max_chars == 0 {
        return "";
    }
    let Some(metadata) = model_id.and_then(tokenizer_metadata) else {
        return lexical_boundary_prefix(text, max_tokens, max_chars);
    };
    let text_chars = text.chars().count();
    let budget_chars = max_tokens.saturating_mul(metadata.chars_per_token_milli) / 1_000;
    if text_chars <= budget_chars && text_chars <= max_chars {
        return text;
    }
    // Direct char boundary: min(max_chars, floor(max_tokens * chars_per_token_milli / 1_000)).
    // Converting max_chars to whole tokens and back (double floor) was lossy:
    // with chars_per_token_milli=3_500, max_tokens=1, max_chars=2 it returned
    // empty though a 2-char prefix fits both caps (tokenzero-7tse, omega-math-1).
    let boundary_chars =
        max_chars.min(max_tokens.saturating_mul(metadata.chars_per_token_milli) / 1_000);
    char_prefix(text, boundary_chars)
}

fn char_prefix(text: &str, chars: usize) -> &str {
    text.char_indices()
        .nth(chars)
        .map_or(text, |(end, _)| &text[..end])
}

fn lexical_boundary_prefix(text: &str, max_tokens: usize, max_chars: usize) -> &str {
    let mut tokens = 0usize;
    let mut in_word = false;
    let mut boundary = 0usize;
    let mut completed = true;
    for (seen, (start, ch)) in text.char_indices().enumerate() {
        if seen == max_chars {
            completed = false;
            break;
        }
        let end = start + ch.len_utf8();
        let word = ch.is_ascii_alphanumeric() || ch == '_';
        if word {
            if !in_word {
                if tokens == max_tokens {
                    completed = false;
                    break;
                }
                tokens += 1;
                in_word = true;
            }
        } else if ch.is_whitespace() {
            in_word = false;
        } else if tokens == max_tokens {
            completed = false;
            break;
        } else {
            tokens += 1;
            in_word = false;
        }
        if !in_word {
            boundary = end;
        }
    }
    if completed { text } else { &text[..boundary] }
}

/// Per-byte classification for the ASCII fast path of `count_tokens`.
/// 0 = non-whitespace separator (counts as a token if not currently in a token)
/// 1 = in-token (alphanumeric or `_`)
/// 2 = whitespace (breaks tokens, never itself a token)
#[rustfmt::skip]
pub(crate) const ASCII_TOKEN_CLASS: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut b: usize = 0;
    while b < 256 {
        t[b] = match b as u8 {
            b'0'..=b'9' | b'A'..=b'Z' | b'a'..=b'z' | b'_' => 1,
            b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C => 2,
            _ => 0,
        };
        b += 1;
    }
    t
};

pub fn count_tokens(text: &str) -> usize {
    active_tokenizer_metadata().map_or_else(
        || count_tokens_lexical(text),
        |metadata| approximate_token_count(text, metadata),
    )
}

/// Tokenizer id for [`count_tokens`] / MCP [`super::Accounting`] JSON.
///
/// This is the kernel measure estimator family (`estimator:tokenzero-lexical`
/// or `estimator:tokenzero-<family>`). It is never unlabeled `estimate:`,
/// never `Q99`, and never an invented `ExactTokenizerIdentity`.
pub fn count_tokens_tokenizer_id() -> String {
    let id = match active_tokenizer_metadata() {
        Some(meta) => format!("estimator:tokenzero-{}", meta.family.name()),
        None => LEXICAL_ESTIMATOR_ID.to_string(),
    };
    debug_assert_eq!(preflight_tokenizer_id(&id), Ok(()));
    id
}

fn count_ascii(bytes: &[u8], stop_at_non_ascii: bool) -> (usize, usize) {
    let (mut tokens, mut in_token) = (0, false);
    for (index, &byte) in bytes.iter().enumerate() {
        if stop_at_non_ascii && !byte.is_ascii() {
            return (tokens, index);
        }
        let class = ASCII_TOKEN_CLASS[byte as usize];
        tokens += usize::from(class == 0 || class == 1 && !in_token);
        in_token = class == 1;
    }
    (tokens, bytes.len())
}

fn count_tokens_lexical(text: &str) -> usize {
    let (tokens, ascii_end) = count_ascii(text.as_bytes(), true);
    if ascii_end == text.len() {
        tokens
    } else {
        count_tokens_tail(text, ascii_end)
    }
}

/// Finish lexical counting after the ASCII fast path reaches Unicode.
pub(crate) fn count_tokens_tail(text: &str, start_byte_offset: usize) -> usize {
    let (mut tokens, _) = count_ascii(&text.as_bytes()[..start_byte_offset], false);
    let mut in_token = false;
    for ch in text[start_byte_offset..].chars() {
        let word = ch.is_ascii_alphanumeric() || ch == '_';
        tokens += usize::from(!ch.is_whitespace() && (!word || !in_token));
        in_token = word;
    }
    tokens
}

/// Fraction of raw tokens avoided. Negative when `used_tokens` exceeds raw.
///
/// Clamping overhead to `0.0` used to report a 0% *save* for spent>raw
/// envelopes. Pulse and capsule accounting must show the signed cost instead.
pub fn savings_ratio(raw_tokens: usize, used_tokens: usize) -> f64 {
    // `usize` → `u64` is lossless on every supported target.
    savings_ratio_u64(raw_tokens as u64, used_tokens as u64)
}

/// Width-preserving signed savings ratio for class-typed `u64` token counts.
pub fn savings_ratio_u64(raw_tokens: u64, used_tokens: u64) -> f64 {
    if raw_tokens == 0 {
        return 0.0;
    }
    1.0 - (used_tokens as f64 / raw_tokens as f64)
}

#[cfg(test)]
mod savings_ratio_tests {
    use super::{savings_ratio, savings_ratio_u64};

    #[test]
    fn spent_above_raw_is_negative_not_a_clamped_save() {
        let ratio = savings_ratio(10, 15);
        assert!(
            ratio < 0.0,
            "spent>raw must not report a non-negative save, got {ratio}"
        );
        assert!((ratio - (-0.5)).abs() < 1e-12);
        assert_eq!(savings_ratio(10, 10), 0.0);
        assert!((savings_ratio(10, 5) - 0.5).abs() < 1e-12);
        assert!(savings_ratio_u64(4, 10) < 0.0);
    }
}

pub fn prefix_end_for_kept_lines(text: &str, kept_lines: usize) -> usize {
    if kept_lines == 0 {
        return 0;
    }

    text.match_indices('\n')
        .nth(kept_lines - 1)
        .map_or(text.len(), |(index, _)| index)
}
