use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use serde_json::Value;
use zero_ref::{ObjectId, SpanRef, SpanRefError, ZeroFragment};

fn fixture() -> Value { serde_json::from_str(include_str!("../fixtures/zeroref_v1_vectors.json")).unwrap() }
fn decode(s: &str) -> Vec<u8> { (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect() }
fn digest_hex(digest: &[u8; 32]) -> String { digest.iter().map(|byte| format!("{byte:02x}")).collect() }

#[test]
fn fixed_span_vectors_bind_canonical_selected_bytes() {
    let f = fixture();
    for vector in f["span_vectors"].as_array().unwrap() {
        let object = decode(f["blobs"][vector["blob"].as_str().unwrap()]["bytes_hex"].as_str().unwrap());
        let spec = &vector["fragment"]; let start = spec["start"].as_u64().unwrap(); let end = spec["end"].as_u64().unwrap();
        let fragment = match spec["kind"].as_str().unwrap() { "bytes" => ZeroFragment::Bytes { start, end }, "lines" => ZeroFragment::Lines { start, end }, other => panic!("unknown span kind {other}") };
        let (span, selected) = SpanRef::from_fragment(&object, &fragment, vector["name"].as_str().unwrap()).unwrap();
        assert_eq!(span.byte_start, vector["byte_start"].as_u64().unwrap()); assert_eq!(span.byte_len, vector["byte_len"].as_u64().unwrap());
        assert_eq!(selected, decode(vector["selected_hex"].as_str().unwrap())); assert_eq!(digest_hex(&span.span_digest), vector["span_digest"].as_str().unwrap());
        assert!(span.verify_span(selected).is_ok()); assert_eq!(span.verify_and_select(&object).unwrap(), selected);
    }
}

#[test]
fn fixed_tamper_vectors_fail_typed_without_panicking() {
    let object = b"alpha\nbeta\ngamma\n"; let (span, payload) = SpanRef::from_fragment(object, &ZeroFragment::Bytes { start: 0, end: 5 }, "tamper").unwrap();
    let mut changed = payload.to_vec(); changed[0] ^= 1; assert_eq!(span.verify_span(&changed), Err(SpanRefError::SpanDigestMismatch));
    let mut range = span.clone(); range.byte_start = object.len() as u64; assert_eq!(range.verify_and_select(object), Err(SpanRefError::RangeOutOfBounds));
    let mut digest = span.clone(); digest.span_digest[0] ^= 1; assert_eq!(digest.verify_span(payload), Err(SpanRefError::SpanDigestMismatch));
    let mut overflow = span; overflow.byte_start = u64::MAX; overflow.byte_len = 1; assert_eq!(overflow.verify_and_select(object), Err(SpanRefError::RangeOverflow));
}

#[test]
fn serde_wire_shape_is_stable_and_round_trips() {
    let (span, _) = SpanRef::from_fragment(b"abc", &ZeroFragment::Bytes { start: 1, end: 2 }, "wire").unwrap(); let value = serde_json::to_value(&span).unwrap();
    assert_eq!(value.as_object().unwrap().keys().collect::<Vec<_>>(), ["byte_len", "byte_start", "object_digest", "object_id", "span_digest"]);
    assert_eq!(serde_json::from_value::<SpanRef>(value).unwrap(), span); assert_eq!(span.object_id, ObjectId(span.object_digest));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, rng_seed: RngSeed::Fixed(0x86_04_5eed), ..ProptestConfig::default() })]
    #[test]
    fn span_only_matches_full_object_verification(object in prop::collection::vec(any::<u8>(), 0..512), raw_start in 0usize..1024, raw_width in 0usize..1024) {
        let start = raw_start % (object.len() + 1); let width = raw_width % (object.len() - start + 1); let end = start + width;
        let fragment = ZeroFragment::Bytes { start: start as u64, end: end as u64 }; let (span, selected) = SpanRef::from_fragment(&object, &fragment, "property").unwrap();
        prop_assert_eq!(span.verify_span(selected), span.verify_and_select(&object).map(|_| ()));
    }
}
