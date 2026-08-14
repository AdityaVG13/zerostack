//! ZS-ADAPTER-002 conformance: the canonical task contract round-trips
//! through every transport (CLI/RPC/native/MCP) with every semantic field
//! preserved and one identical rooted contract. A tampered projection is a
//! different root and is refused.

use serde_json::{Value, json};
use zero_abi::{
    ObjectClassV1, ROOTED_ABI_VERSION_V6, StructuredTaskContractV1, canonical_object_bytes,
    object_root, verify_object_root,
};
use zero_testkit::v6_conformance::{V6Transport, shape_signature};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../fixtures/task_contract_roundtrip_v6.json"
    ))
    .expect("fixture is valid JSON")
}

fn contract_from_fixture(fixture: &Value) -> StructuredTaskContractV1 {
    let contract: StructuredTaskContractV1 =
        serde_json::from_value(fixture["contract"].clone()).expect("fixture contract parses");
    contract.validate().expect("fixture contract validates");
    contract
}

/// Project one task submission through a transport and recover the contract
/// JSON it carries. Every transport carries the contract's canonical bytes.
fn project_contract(
    transport: V6Transport,
    contract: &StructuredTaskContractV1,
    request_id: u64,
) -> Value {
    let canonical = contract
        .canonical_bytes()
        .expect("contract canonicalizes");
    let hex = canonical.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    match transport {
        V6Transport::Cli => json!({
            "operation": "resume",
            "task_contract_canonical_hex": hex,
            "timeout_ms": 300_000,
        }),
        V6Transport::Rpc => json!({
            "transport": "rpc",
            "frame": hex,
        }),
        V6Transport::Native => json!({
            "generation": 1,
            "request_id": request_id,
            "task_contract_canonical_hex": hex,
        }),
        V6Transport::Mcp => json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "params": {"task_contract_canonical_hex": hex},
        }),
    }
}

fn recover_contract_hex(transport: V6Transport, projection: &Value) -> Result<String, String> {
    let hex = match transport {
        V6Transport::Cli => projection
            .get("task_contract_canonical_hex")
            .and_then(Value::as_str)
            .ok_or_else(|| "cli projection missing task_contract_canonical_hex".to_owned())?
            .to_owned(),
        V6Transport::Rpc => projection
            .get("frame")
            .and_then(Value::as_str)
            .ok_or_else(|| "rpc projection missing frame".to_owned())?
            .to_owned(),
        V6Transport::Native => projection
            .get("task_contract_canonical_hex")
            .and_then(Value::as_str)
            .ok_or_else(|| "native projection missing task_contract_canonical_hex".to_owned())?
            .to_owned(),
        V6Transport::Mcp => projection
            .get("params")
            .and_then(|params| params.get("task_contract_canonical_hex"))
            .and_then(Value::as_str)
            .ok_or_else(|| "mcp projection missing params.task_contract_canonical_hex".to_owned())?
            .to_owned(),
    };
    if hex.len() % 2 != 0 || hex.chars().any(|ch| !ch.is_ascii_hexdigit()) {
        return Err("recovered hex is malformed".to_owned());
    }
    Ok(hex)
}

fn contract_from_hex(hex: &str) -> Result<StructuredTaskContractV1, String> {
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|error| format!("hex decode failed: {error}"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("contract JSON decode failed: {error}"))
}

#[test]
fn canonical_contract_root_matches_the_cross_release_pin() {
    let fixture = fixture();
    let contract = contract_from_fixture(&fixture);
    let root = contract.contract_root().expect("contract roots");
    let expected = fixture["expected_contract_root"]
        .as_str()
        .expect("expected root");
    assert_eq!(
        root.to_hex(),
        expected,
        "the canonical task contract root changed; update the fixture pin deliberately"
    );
    // The pinned canonical bytes must be the exact byte path the root binds.
    let canonical = contract.canonical_bytes().expect("canonical bytes");
    assert_eq!(
        canonical
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
        fixture["expected_canonical_bytes_hex"].as_str().expect("bytes pin"),
        "the canonical byte path changed; update the fixture pin deliberately"
    );
    // Root class binding: the same payload rooted under another class is a
    // different root (no cross-class root reuse).
    let value = serde_json::to_value(&contract).expect("contract serializes");
    let other_class = canonical_object_bytes(
        ObjectClassV1::ExecuteResult,
        ROOTED_ABI_VERSION_V6,
        &value,
    )
    .expect("canonical under other class");
    let other_root =
        object_root(ObjectClassV1::ExecuteResult, ROOTED_ABI_VERSION_V6, &other_class)
            .expect("other class root");
    assert_ne!(other_root, root, "cross-class roots must never collide");
}

#[test]
fn contract_round_trips_through_every_transport_with_identical_root() {
    let fixture = fixture();
    let contract = contract_from_fixture(&fixture);
    let expected_root = contract.contract_root().expect("root");
    let mut projections = Vec::new();
    for transport in V6Transport::ALL {
        let projection = project_contract(transport, &contract, 3);
        let hex = recover_contract_hex(transport, &projection)
            .unwrap_or_else(|error| panic!("{} recovery failed: {error}", transport.name()));
        let recovered = contract_from_hex(&hex)
            .unwrap_or_else(|error| panic!("{} contract parse failed: {error}", transport.name()));
        // Every semantic field survives byte-identically.
        assert_eq!(recovered, contract, "{} lost semantic fields", transport.name());
        assert_eq!(
            recovered.contract_root().expect("root"),
            expected_root,
            "{} changed the rooted contract",
            transport.name()
        );
        projections.push((transport, projection));
    }
    // The transport wire shapes are fixed (one schema per transport), and
    // every transport carries the same contract bytes.
    let shapes = projections
        .iter()
        .map(|(_, projection)| shape_signature(projection))
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(shapes.len(), 4, "every transport has its own fixed wire shape");
    let hexes = projections
        .iter()
        .map(|(transport, projection)| recover_contract_hex(*transport, projection).unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(hexes.len(), 1, "all transports must carry identical contract bytes");
}

#[test]
fn tampered_transport_projection_is_a_different_root_and_refused() {
    let fixture = fixture();
    let contract = contract_from_fixture(&fixture);
    let expected_root = contract.contract_root().expect("root");
    // Tamper the CLI projection: swap the carried bytes for a contract whose
    // acceptance criterion differs.
    let mut tampered = contract.clone();
    tampered.acceptance_criteria = vec!["tampered criterion".to_owned()];
    let tampered_root = tampered.contract_root().expect("tampered root");
    assert_ne!(tampered_root, expected_root, "tampering must change the root");
    let projection = project_contract(V6Transport::Cli, &tampered, 3);
    let hex = recover_contract_hex(V6Transport::Cli, &projection).expect("hex");
    let recovered = contract_from_hex(&hex).expect("parses");
    assert_eq!(recovered, tampered);
    assert_ne!(
        recovered.contract_root().expect("root"),
        expected_root,
        "a tampered projection must never verify against the pinned root"
    );
    // The pinned root fails closed against the tampered payload: verify with
    // the pinned canonical bytes (the fixture's contract bytes) under the
    // expected root must reject the tampered bytes.
    let canonical = contract.canonical_bytes().expect("canonical");
    assert!(verify_object_root(
        ObjectClassV1::TaskContract,
        ROOTED_ABI_VERSION_V6,
        &canonical,
        expected_root,
    ));
    let tampered_canonical = recovered.canonical_bytes().expect("tampered canonical");
    assert!(!verify_object_root(
        ObjectClassV1::TaskContract,
        ROOTED_ABI_VERSION_V6,
        &tampered_canonical,
        expected_root,
    ));
    // The fixture declares the refusal expectation; assert the fixture
    // documents it (the fixture is the contract).
    let violations = fixture["violations"].as_array().expect("violations");
    assert!(violations.iter().any(|violation| violation["id"]
        == json!("mutated-acceptance-criterion")));
}
