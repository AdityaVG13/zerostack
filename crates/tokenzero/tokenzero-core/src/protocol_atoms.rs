//! Verified one-token protocol alphabets for the 1TP wire protocol.
//!
//! Provider tokenizers are not all distributable. Exact counts therefore live in
//! a reviewed snapshot (`tests/fixtures/one-token-atoms.json`), while this module
//! exposes the portable intersection without requiring network access.

/// Stable identifier for a tokenizer-specific verification table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProtocolTokenizer {
    Anthropic,
    O200k,
    Gemini,
    Kimi,
}

impl ProtocolTokenizer {
    pub const ALL: [Self; 4] = [Self::Anthropic, Self::O200k, Self::Gemini, Self::Kimi];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::O200k => "o200k",
            Self::Gemini => "gemini",
            Self::Kimi => "kimi",
        }
    }
}

/// Conservative protocol alphabet verified as one token in every supported
/// tokenizer table. Digits are intentionally used instead of decorative glyphs:
/// they survive Unicode normalization and transport without changing bytes.
pub const PORTABLE_ONE_TOKEN_ATOMS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Return whether `atom` is certified as exactly one token for `tokenizer`.
///
/// The current portable alphabet is the full certified set for every table.
/// Keeping the tokenizer argument explicit prevents callers from silently
/// assuming portability when provider-specific alphabets are added later.
pub fn is_verified_one_token_atom(tokenizer: ProtocolTokenizer, atom: &str) -> bool {
    let _ = tokenizer;
    PORTABLE_ONE_TOKEN_ATOMS.contains(&atom)
}

/// Return the portable intersection suitable for ACKs, ordinals, and sentinels.
pub const fn portable_one_token_atoms() -> &'static [&'static str] {
    &PORTABLE_ONE_TOKEN_ATOMS
}

/// ACK/2 classes use only atoms from the portable tokenizer intersection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckClass {
    Success,
    Validation,
    Policy,
    Substrate,
    Retryable,
    Internal,
}

impl AckClass {
    pub const fn atom(self) -> &'static str {
        match self {
            Self::Success => "0",
            Self::Validation => "1",
            Self::Policy => "2",
            Self::Substrate => "3",
            Self::Retryable => "4",
            Self::Internal => "9",
        }
    }

    /// Collapse transport-specific error kinds into the stable ACK/2 grammar.
    pub fn from_error_kind(kind: &str, retryable: bool) -> Self {
        if retryable {
            return Self::Retryable;
        }
        match kind {
            "validation" | "invalid_args" | "parse" => Self::Validation,
            "policy" | "sandbox" | "permission" | "denied" => Self::Policy,
            "substrate" | "store" | "not_found" | "io" => Self::Substrate,
            _ => Self::Internal,
        }
    }
}

/// Render a deterministic class-1 ACK. Pure successful mutations are silent;
/// detail refs travel in their dedicated envelope field and never perturb the atom.
pub fn render_ack(class: AckClass, silent_success: bool) -> &'static str {
    if silent_success && class == AckClass::Success {
        ""
    } else {
        class.atom()
    }
}
