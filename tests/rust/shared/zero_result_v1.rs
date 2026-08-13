use jsonschema::Validator;
use serde_json::{Value, json};
use zero_abi::{ZeroResultAccessError, ZeroResultV1};

fn schema() -> Validator {
    let value: Value =
        serde_json::from_str(include_str!("../contracts/zero-result-v1.schema.json")).unwrap();
    jsonschema::validator_for(&value).unwrap()
}
fn assert_valid(value: &Value) {
    schema().validate(value).unwrap();
    serde_json::from_value::<ZeroResultV1>(value.clone()).unwrap();
}
fn assert_rejected(value: Value) {
    assert!(
        schema().validate(&value).is_err(),
        "schema accepted {value}"
    );
    assert!(
        serde_json::from_value::<ZeroResultV1>(value.clone()).is_err(),
        "serde accepted {value}"
    );
}

#[test]
fn valid_inline_and_ref_match_schema_and_serde() {
    assert_valid(&json!({"ack":"R2","content":{"kind":"inline","value":"file bytes"}}));
    assert_valid(
        &json!({"ack":"0","content":{"kind":"ref","ref":"tz://blob/abc","preview":"bounded"}}),
    );
}
#[test]
fn unknown_missing_and_mixed_payloads_fail_closed() {
    assert_rejected(json!({"ack":"C","content":{"kind":"inline"}}));
    assert_rejected(json!({"content":{"kind":"inline","value":1}}));
    assert_rejected(json!({"ack":"C","content":{"kind":"inline","value":1,"ref":"fz://blob/abc"}}));
    assert_rejected(json!({"ack":"C","content":{"kind":"ref","ref":"fz://blob/abc","value":1}}));
    assert_rejected(json!({"ack":"C","content":{"kind":"inline","value":1},"future":true}));
    assert_rejected(json!({"ack":"C","content":{"kind":"future","value":1}}));
    assert_rejected(json!({"ack":"C","content":{"kind":"ref","ref":"ls_manifest"}}));
}
#[test]
fn wrong_accessor_returns_a_typed_error() {
    let inline: ZeroResultV1 =
        serde_json::from_value(json!({"ack":"C","content":{"kind":"inline","value":7}})).unwrap();
    assert_eq!(
        inline.reference_value(),
        Err(ZeroResultAccessError::ExpectedRef { actual: "inline" })
    );
    let referenced: ZeroResultV1 =
        serde_json::from_value(json!({"ack":"C","content":{"kind":"ref","ref":"gz://blob/abc"}}))
            .unwrap();
    assert_eq!(
        referenced.inline_value(),
        Err(ZeroResultAccessError::ExpectedInline { actual: "ref" })
    );
}
#[test]
fn serialization_uses_only_the_canonical_tagged_shape() {
    let inline = ZeroResultV1::inline("R2", json!({"text":"evidence"})).unwrap();
    assert_eq!(
        serde_json::to_value(inline).unwrap(),
        json!({"ack":"R2","content":{"kind":"inline","value":{"text":"evidence"}}})
    );
    let referenced = ZeroResultV1::reference("0", "tz://blob/abc", Some("preview".into())).unwrap();
    assert_eq!(
        serde_json::to_value(referenced).unwrap(),
        json!({"ack":"0","content":{"kind":"ref","ref":"tz://blob/abc","preview":"preview"}})
    );
}
#[test]
fn schema_and_serde_enforce_bounds() {
    assert_rejected(json!({"ack":"","content":{"kind":"inline","value":null}}));
    assert_rejected(json!({"ack":"C","content":{"kind":"ref","ref":"http://example.test/x"}}));
    assert_rejected(
        json!({"ack":"C","content":{"kind":"ref","ref":"fz://blob/x","preview":"x".repeat(1025)}}),
    );
}
#[test]
fn cross_surface_fixture_table_uses_one_envelope() {
    let fixtures = [
        (
            "zero.fs.compound",
            json!({"ack":"R2","content":{"kind":"inline","value":"bytes"}}),
        ),
        (
            "zero.fs.plan",
            json!({"ack":"C","content":{"kind":"ref","ref":"fz://codemode/execution/e/result"}}),
        ),
        (
            "zero.fs.structural",
            json!({"ack":"C","content":{"kind":"ref","ref":"fz://blob/abc"}}),
        ),
        (
            "zero.graph.blast",
            json!({"ack":"C","content":{"kind":"ref","ref":"gz://blob/abc"}}),
        ),
        (
            "zero.graph.query",
            json!({"ack":"C","content":{"kind":"inline","value":{"hits":[]}}}),
        ),
        (
            "zero.graph.orient",
            json!({"ack":"C","content":{"kind":"ref","ref":"gz://blob/def","preview":"outline"}}),
        ),
        (
            "zero.graph.recall",
            json!({"ack":"C","content":{"kind":"inline","value":[]}}),
        ),
        (
            "zero.graph.verify",
            json!({"ack":"C","content":{"kind":"inline","value":true}}),
        ),
        (
            "zero.graph.snap",
            json!({"ack":"C","content":{"kind":"ref","ref":"gz://blob/ghi"}}),
        ),
        (
            "zero.graph.reserve",
            json!({"ack":"C","content":{"kind":"inline","value":{"reserved":true}}}),
        ),
        (
            "zero.graph.index",
            json!({"ack":"C","content":{"kind":"inline","value":{"indexed":true}}}),
        ),
        (
            "zero.graph.remember",
            json!({"ack":"C","content":{"kind":"inline","value":{"stored":true}}}),
        ),
        (
            "zero.token.shell",
            json!({"ack":"0","content":{"kind":"ref","ref":"tz://blob/abc","preview":"stdout"}}),
        ),
        (
            "zero.token.compact",
            json!({"ack":"0","content":{"kind":"ref","ref":"tz://blob/def","preview":"summary"}}),
        ),
        (
            "zero.token.expand",
            json!({"ack":"0","content":{"kind":"inline","value":"exact payload"}}),
        ),
    ];
    for (surface, fixture) in fixtures {
        schema()
            .validate(&fixture)
            .unwrap_or_else(|error| panic!("{surface}: {error}"));
        let result: ZeroResultV1 = serde_json::from_value(fixture).unwrap();
        assert!(!result.ack().is_empty(), "{surface}");
    }
}
