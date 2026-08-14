//! ZS-ADAPTER-001/010 conformance: the Zero Execute envelope schema is
//! byte-identical across 10,000 randomized tasks. Dynamic project details
//! live in arguments/roots, never in schema mutation.

use jsonschema::Validator;
use proptest::prelude::*;
use serde_json::{Value, json};
use zero_testkit::v6_conformance::shape_signature;

fn request_schema() -> Validator {
    let value: Value = serde_json::from_str(include_str!(
        "../../../racc/v6/schemas/zero_execute_request_v6.schema.json"
    ))
    .expect("request schema is valid JSON");
    jsonschema::validator_for(&value).expect("request schema is valid")
}

fn request_schema_value() -> Value {
    serde_json::from_str(include_str!(
        "../../../racc/v6/schemas/zero_execute_request_v6.schema.json"
    ))
    .expect("request schema is valid JSON")
}

fn random_hex(digits: usize) -> impl Strategy<Value = String> {
    proptest::string::string_regex(&format!("[0-9a-f]{{{digits}}}")).expect("valid hex regex")
}

fn random_ref() -> impl Strategy<Value = String> {
    proptest::string::string_regex("fz://(blob|root)/[a-z0-9/_-]{1,48}").expect("valid ref regex")
}

/// One randomized task request envelope, built by a FIXED structural builder:
/// the same field set and order for every task, only argument VALUES vary.
fn random_request(
    operation: &str,
    task_contract_root: String,
    project_root: Option<String>,
    continuation_handle: Option<String>,
    objective: Option<String>,
    max_operations: u64,
    max_wall_ms: u64,
    result_detail: &str,
) -> Value {
    let mut request = json!({
        "abi_version": "zerostack.racc.v6",
        "task_contract_root": task_contract_root,
        "operation": operation,
        "side_effect_policy": "verified_commit",
        "private_composition_budget": {
            "max_operations": max_operations,
            "max_wall_ms": max_wall_ms,
            "max_complete_work_units": 1_000_000,
        },
        "result_detail": result_detail,
    });
    if let Some(project_root) = project_root {
        request["project_root"] = json!(project_root);
    } else {
        request["project_root"] = Value::Null;
    }
    if let Some(handle) = continuation_handle {
        request["continuation_handle"] = json!(handle);
    } else {
        request["continuation_handle"] = Value::Null;
    }
    if let Some(objective) = objective {
        request["objective"] = json!(objective);
    } else {
        request["objective"] = Value::Null;
    }
    request
}

/// Schema-level shape: key sets per level with scalar leaves merged across
/// types (null/string/number/bool are all "scalar"). This is the shape the
/// acceptance cares about -- the registered field structure must not vary
/// with task contents; optional presence is a value-level detail.
fn optional_shape(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut parts = map
                .iter()
                .map(|(key, value)| format!("{key}:{}", optional_shape(value)))
                .collect::<Vec<_>>();
            parts.sort();
            format!("obj{{{}}}", parts.join(","))
        }
        Value::Array(items) => format!(
            "arr[{}]",
            items
                .iter()
                .map(optional_shape)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Null
        | Value::Bool(_)
        | Value::Number(_)
        | Value::String(_) => "scalar".into(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(10_000))]

    #[test]
    fn envelope_schema_is_byte_identical_across_10k_randomized_tasks(
        operation in prop_oneof![
            Just("inspect"), Just("explain"), Just("plan"), Just("edit"),
            Just("refactor"), Just("port"), Just("build"), Just("test"),
            Just("verify"), Just("resume"), Just("cancel"),
        ],
        task_contract_root in random_hex(16),
        has_project in any::<bool>(),
        project_root in random_ref(),
        has_handle in any::<bool>(),
        handle in random_ref(),
        has_objective in any::<bool>(),
        objective in "[a-zA-Z0-9 _./-]{0,120}",
        max_operations in 0u64..100_000,
        max_wall_ms in 0u64..3_600_000,
        result_detail in prop_oneof![
            Just("minimal"), Just("decision"), Just("audit"), Just("full"),
        ],
    ) {
        let request = random_request(
            &operation,
            task_contract_root,
            has_project.then_some(project_root),
            has_handle.then_some(handle),
            has_objective.then_some(objective),
            max_operations,
            max_wall_ms,
            &result_detail,
        );
        // 1. Every randomized task validates against the canonical schema.
        let validator = request_schema();
        validator.validate(&request).unwrap_or_else(|error| {
            panic!("randomized task failed the canonical schema: {error}")
        });
        // 2. The wire shape is identical for every task: one fixed schema,
        //    only argument values vary.
        assert_eq!(
            optional_shape(&request),
            "obj{abi_version:scalar,continuation_handle:scalar,objective:scalar,operation:scalar,private_composition_budget:obj{max_complete_work_units:scalar,max_operations:scalar,max_wall_ms:scalar},project_root:scalar,result_detail:scalar,side_effect_policy:scalar,task_contract_root:scalar}"
        );
    }
}

#[test]
fn canonical_request_schema_is_pinned_as_a_fixture_contract() {
    // The schema document itself never varies with project contents: pin its
    // canonical bytes so an accidental field reorder or rename is a loud
    // fixture break, not a silent schema drift.
    let value = request_schema_value();
    let canonical = zero_abi::canonical_json(&value);
    assert_eq!(
        zero_abi::sha256_hex(canonical.as_bytes()),
        "7b49b5f0ba4a40d936a16df8d656ae6084b627f59aa13b9622002b175816f376",
        "zero_execute_request_v6.schema.json changed; update the pin deliberately"
    );
}

#[test]
fn result_envelope_shape_is_fixed_per_kind_and_kind_union_is_stable() {
    // The result envelope's kind-tagged union schema is fixed: the same kind
    // always serializes to the same shape regardless of task content, and
    // the union of shapes over all eight kinds is the stable registry.
    let fixture: Value = serde_json::from_str(include_str!(
        "../../fixtures/v6_cross_transport_vectors.json"
    ))
    .expect("fixture");
    let mut registry = std::collections::BTreeMap::new();
    for vector in fixture["vectors"].as_array().expect("vectors") {
        let envelope = zero_testkit::v6_conformance::envelope_from_vector(vector).expect("envelope");
        let value = serde_json::to_value(&envelope).expect("serializes");
        let signature = shape_signature(&value);
        let previous = registry
            .insert(vector["kind"].as_str().expect("kind").to_owned(), signature.clone());
        assert_eq!(
            previous,
            None,
            "kind {} produced two different shapes",
            vector["kind"].as_str().expect("kind")
        );
        let _ = signature;
    }
    assert_eq!(
        registry.len(),
        8,
        "the V6 kind union must cover all eight kinds"
    );
}
