//! Public result construction and byte-stable wire contracts.

use serde_json::json;
use zero_abi::{from_step, to_wire};

const BLOB: &str = "fz://blob/1111111111111111111111111111111111111111111111111111111111111111";

#[test]
fn inline_string_stays_inline() {
    let result = from_step("R1", true, "fs.read", "", &json!("hello"), None);

    assert_eq!(result.kind(), "inline");
    assert_eq!(result.inline_value().unwrap(), &json!("hello"));
    assert_eq!(
        serde_json::to_string(&to_wire(&result)).unwrap(),
        r#"{"ack":"R1","content":{"kind":"inline","value":"hello"}}"#
    );
}

#[test]
fn canonical_blob_reference_stays_a_reference() {
    let result = from_step(
        "R2",
        true,
        "fs.read",
        BLOB,
        &json!({"preview": "first"}),
        None,
    );

    assert_eq!(result.kind(), "ref");
    assert_eq!(result.reference_value().unwrap(), BLOB);
    assert_eq!(result.preview().unwrap(), Some("first"));
    assert_eq!(
        serde_json::to_string(&to_wire(&result)).unwrap(),
        format!(r#"{{"ack":"R2","content":{{"kind":"ref","preview":"first","ref":"{BLOB}"}}}}"#)
    );
}

#[test]
fn noncanonical_alias_stays_inline() {
    let payload = json!({"value": 7});
    let result = from_step("R3", true, "fs.read", "latest-result", &payload, None);

    assert_eq!(result.kind(), "inline");
    assert_eq!(result.inline_value().unwrap(), &payload);
}

#[test]
fn failure_uses_x0_and_inline_error() {
    let result = from_step(" ", false, "fs.read", BLOB, &json!(null), Some("denied"));

    assert_eq!(result.ack(), "X0");
    assert_eq!(
        to_wire(&result),
        json!({
            "ack": "X0",
            "content": {
                "kind": "inline",
                "value": {"ok": false, "method": "fs.read", "detail": "denied"}
            }
        })
    );
}
