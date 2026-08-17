//! Four transform families over live ZeroStack types. Never `rand::random()`.

use serde_json::Value;
use zero_abi::raw_worker::{
    decode_request_frame, encode_frame, ShutdownRequest, WorkerRequestFrame,
    DEFAULT_MAX_FRAME_BYTES,
};
use zero_abi::schema::{canonical_schema_json, normalize_schema};
use zero_ref::{select_fragment_with_policy, LineEndPolicy, ZeroFragment, ZeroRefV1};

use crate::repo::sha256_hex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformFamily {
    Predicate,
    Projection,
    Structural,
    Literal,
}

impl TransformFamily {
    pub const ALL: [Self; 4] = [
        Self::Predicate,
        Self::Projection,
        Self::Structural,
        Self::Literal,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EquivalenceExpectation {
    ExactRowMatch,
    MultisetEquivalence,
    SetEquivalence,
    TypeCoercionEquivalent,
}

pub fn derive_entry_seed(corpus_entry_id: &str) -> u64 {
    let digest = sha256_hex(corpus_entry_id.as_bytes());
    let mut bytes = [0u8; 8];
    for (i, chunk) in digest.as_bytes().chunks(2).take(8).enumerate() {
        let text = std::str::from_utf8(chunk).unwrap_or("00");
        bytes[i] = u8::from_str_radix(text, 16).unwrap_or(0);
    }
    u64::from_le_bytes(bytes)
}

pub struct MetamorphicCase {
    pub family: TransformFamily,
    pub expectation: EquivalenceExpectation,
    pub transform_id: &'static str,
    pub soundness_proof_sketch: &'static str,
}

/// Predicate: a looser ClampEnd line end is a no-op once end ≥ last line.
pub const PREDICATE_CLAMP: MetamorphicCase = MetamorphicCase {
    family: TransformFamily::Predicate,
    expectation: EquivalenceExpectation::ExactRowMatch,
    transform_id: "line-end-clamp-tautology",
    soundness_proof_sketch: "\
        select_fragment_with_policy under ClampEnd resolves the requested line \
        end to min(end, line_count) after validating start. If start is a legal \
        first line and the original end already covers the last real line, \
        raising end cannot add or drop bytes: the clamped interval is identical. \
        Strict policy is a different function and is not claimed equivalent.",
};

/// Projection: Display is a pure function of scheme, hash, and fragment.
pub const PROJECTION_DISPLAY: MetamorphicCase = MetamorphicCase {
    family: TransformFamily::Projection,
    expectation: EquivalenceExpectation::ExactRowMatch,
    transform_id: "zeroref-field-projection",
    soundness_proof_sketch: "\
        ZeroRefV1::Display writes only scheme, hash, and the fragment \
        discriminant. Reconstructing the same three fields and formatting \
        again is the same function applied to the same arguments, so the \
        strings are identical. No other state participates.",
};

/// Structural: parse ∘ Display ∘ parse is identity on a parsed value.
pub const STRUCTURAL_PARSE: MetamorphicCase = MetamorphicCase {
    family: TransformFamily::Structural,
    expectation: EquivalenceExpectation::ExactRowMatch,
    transform_id: "zeroref-parse-display-parse",
    soundness_proof_sketch: "\
        Display emits the unique canonical grammar (never the deprecated \
        #Bstart+len alias). parse of that grammar reconstructs the same \
        ZeroRefV1 fields. Therefore parse(Display(parse(s))) equals parse(s) \
        for every string that parses. Legacy aliases are normalized on the \
        first parse, so the second parse sees only the canonical form.",
};

/// Literal: #Bstart+len is the same Bytes span as #Bstart-end.
pub const LITERAL_LEGACY_SPAN: MetamorphicCase = MetamorphicCase {
    family: TransformFamily::Literal,
    expectation: EquivalenceExpectation::TypeCoercionEquivalent,
    transform_id: "legacy-byte-span-alias",
    soundness_proof_sketch: "\
        parse_fragment maps #B<start>+<len> to Bytes { start, end: start+len } \
        and #B<start>-<end> to the same struct when end = start+len. Display \
        always emits the hyphen form. The two literals are therefore the same \
        value; only the input spelling differs.",
};

/// Structural (schema): normalize_schema is idempotent.
pub const STRUCTURAL_SCHEMA: MetamorphicCase = MetamorphicCase {
    family: TransformFamily::Structural,
    expectation: EquivalenceExpectation::ExactRowMatch,
    transform_id: "schema-normalize-idempotent",
    soundness_proof_sketch: "\
        normalize_schema sorts keys that are order-insensitive and recurses. \
        Applying it twice cannot change an already-normalized tree: each \
        branch is a deterministic function of its children. canonical_schema_json \
        of N(x) therefore equals canonical_schema_json of N(N(x)).",
};

/// Literal (frames): encode_frame then decode_request_frame is identity.
pub const LITERAL_FRAME: MetamorphicCase = MetamorphicCase {
    family: TransformFamily::Literal,
    expectation: EquivalenceExpectation::ExactRowMatch,
    transform_id: "raw-worker-shutdown-roundtrip",
    soundness_proof_sketch: "\
        encode_frame writes canonical JSON plus one trailing newline. \
        decode_request_frame trims at most one trailing CR/LF and deserializes \
        the same tagged enum. For a well-formed Shutdown frame the pair is \
        an inverse, so decode(encode(frame)) equals frame.",
};

pub fn predicate_clamp_equivalent(body: &str, start: u64, end: u64, looser_end: u64) -> bool {
    let fragment = ZeroFragment::Lines { start, end };
    let looser = ZeroFragment::Lines {
        start,
        end: looser_end,
    };
    let left =
        select_fragment_with_policy(body.as_bytes(), &fragment, "orig", LineEndPolicy::ClampEnd);
    let right =
        select_fragment_with_policy(body.as_bytes(), &looser, "looser", LineEndPolicy::ClampEnd);
    match (left, right) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn projection_display_holds(input: &str) -> Result<bool, String> {
    let parsed = ZeroRefV1::parse(input).map_err(|e| e.to_string())?;
    let projected = ZeroRefV1 {
        scheme: parsed.scheme,
        hash: parsed.hash.clone(),
        fragment: parsed.fragment,
    };
    Ok(projected.to_string() == parsed.to_string())
}

pub fn structural_parse_holds(input: &str) -> Result<bool, String> {
    let parsed = ZeroRefV1::parse(input).map_err(|e| e.to_string())?;
    let again = ZeroRefV1::parse(&parsed.to_string()).map_err(|e| e.to_string())?;
    Ok(again == parsed)
}

pub fn literal_legacy_span_holds(hash: &str, start: u64, len: u64) -> Result<bool, String> {
    let end = start.checked_add(len).ok_or("overflow")?;
    let alias = format!("fz://blob/{hash}#B{start}+{len}");
    let canon = format!("fz://blob/{hash}#B{start}-{end}");
    let a = ZeroRefV1::parse(&alias).map_err(|e| e.to_string())?;
    let b = ZeroRefV1::parse(&canon).map_err(|e| e.to_string())?;
    Ok(a.hash == b.hash && a.fragment == b.fragment && a.to_string() == canon)
}

pub fn structural_schema_holds(schema: &Value) -> bool {
    let once = normalize_schema(schema);
    let twice = normalize_schema(&once);
    canonical_schema_json(&once) == canonical_schema_json(&twice)
}

pub fn literal_frame_holds(reason: &str) -> Result<bool, String> {
    let frame = WorkerRequestFrame::Shutdown {
        request: ShutdownRequest {
            reason: reason.to_owned(),
        },
    };
    let bytes = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).map_err(|e| e.to_string())?;
    let decoded =
        decode_request_frame(&bytes, DEFAULT_MAX_FRAME_BYTES).map_err(|e| e.to_string())?;
    Ok(decoded == frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zero_ref::content_hash_hex;

    #[test]
    fn seed_is_deterministic() {
        let a = derive_entry_seed("corpus/entry-1");
        let b = derive_entry_seed("corpus/entry-1");
        let c = derive_entry_seed("corpus/entry-2");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn each_family_has_a_sound_transform() {
        let hash = content_hash_hex(b"metamorphic-body");
        let input = format!("fz://blob/{hash}#B0-4");
        assert!(predicate_clamp_equivalent("one\ntwo\nthree\n", 1, 3, 99));
        assert!(projection_display_holds(&input).unwrap());
        assert!(structural_parse_holds(&input).unwrap());
        assert!(literal_legacy_span_holds(&hash, 0, 4).unwrap());
        assert!(structural_schema_holds(&json!({
            "type": "object",
            "required": ["b", "a"],
            "properties": { "b": {"type": "string"}, "a": {"type": "number"} }
        })));
        assert!(literal_frame_holds("phase6").unwrap());
        assert_eq!(PREDICATE_CLAMP.family, TransformFamily::Predicate);
        assert_eq!(PROJECTION_DISPLAY.family, TransformFamily::Projection);
        assert_eq!(STRUCTURAL_PARSE.family, TransformFamily::Structural);
        assert_eq!(LITERAL_LEGACY_SPAN.family, TransformFamily::Literal);
        for case in [
            &PREDICATE_CLAMP,
            &PROJECTION_DISPLAY,
            &STRUCTURAL_PARSE,
            &LITERAL_LEGACY_SPAN,
            &STRUCTURAL_SCHEMA,
            &LITERAL_FRAME,
        ] {
            assert!(case.soundness_proof_sketch.len() > 80);
        }
    }

    #[test]
    fn clamp_is_not_strict() {
        let body = b"only\n";
        let fragment = ZeroFragment::Lines { start: 1, end: 9 };
        let clamp = select_fragment_with_policy(body, &fragment, "c", LineEndPolicy::ClampEnd);
        let strict = select_fragment_with_policy(body, &fragment, "s", LineEndPolicy::Strict);
        assert!(clamp.is_ok());
        assert!(strict.is_err());
    }
}
