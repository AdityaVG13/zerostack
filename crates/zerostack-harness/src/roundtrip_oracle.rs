//! Round-trip oracle. Smoke pair is ZeroRef v1 parse ↔ Display.

use zero_ref::{ZeroFragment, ZeroRefV1, ZeroScheme, content_hash_hex};

use crate::oracle::ScenarioError;

pub fn zeroref_parse_display(input: &str) -> Result<(), ScenarioError> {
    let parsed = ZeroRefV1::parse(input)
        .map_err(|error| ScenarioError::new("round-trip", format!("parse {input:?}: {error}")))?;
    let rendered = parsed.to_string();
    if rendered != input {
        return Err(ScenarioError::new(
            "round-trip",
            format!("Display drifted: in={input:?} out={rendered:?}"),
        ));
    }
    let again = ZeroRefV1::parse(&rendered).map_err(|error| {
        ScenarioError::new("round-trip", format!("reparse {rendered:?}: {error}"))
    })?;
    if again != parsed {
        return Err(ScenarioError::new(
            "round-trip",
            "second parse did not equal first",
        ));
    }
    Ok(())
}

pub fn smoke_blob_ref() -> String {
    let hash = content_hash_hex(b"zerostack-harness-roundtrip");
    format!("fz://blob/{hash}")
}

pub fn smoke_roundtrip() -> Result<(), ScenarioError> {
    let whole = smoke_blob_ref();
    zeroref_parse_display(&whole)?;
    let bytes = format!("{whole}#B0-4");
    zeroref_parse_display(&bytes)?;
    let parsed = ZeroRefV1::parse(&bytes).expect("just parsed");
    assert_eq!(parsed.scheme, ZeroScheme::Fz);
    assert!(matches!(
        parsed.fragment,
        ZeroFragment::Bytes { start: 0, end: 4 }
    ));
    Ok(())
}
