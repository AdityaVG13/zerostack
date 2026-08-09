use std::collections::BTreeSet;

use serde_json::{json, Value};
use zero_abi::{
    ApprovalGrant, CanonicalOperation, CanonicalRegistry, DispatchErrorClass, DispatchMachine,
    DispatchStage, EffectGrant, EffectPolicy, PermitGrant, RegistryEngine, SourceDiagnostic,
    SourceForm, ALL_DISPATCH_ERROR_CLASSES, CANONICAL_DISPATCH_VERSION,
};
use zerostack_shared_tests::schema::{
    validate_against_schema, SchemaName, CANONICAL_DISPATCH_SCHEMA,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/canonical_dispatch_vectors.json"))
        .expect("canonical dispatch fixture parses")
}

fn projections(fixture: &Value) -> &[Value] {
    fixture["registry_projections"]
        .as_array()
        .expect("registry_projections is an array")
}

fn registry_for(engine: RegistryEngine, operation: CanonicalOperation) -> CanonicalRegistry {
    CanonicalRegistry {
        version: CANONICAL_DISPATCH_VERSION.to_owned(),
        engine,
        operations: vec![operation],
    }
}

#[test]
fn all_engine_projections_validate_and_round_trip_losslessly() {
    let fixture = fixture();
    let projections = projections(&fixture);
    assert_eq!(projections.len(), 3);

    let mut engines = BTreeSet::new();
    for projection in projections {
        validate_against_schema(SchemaName::CanonicalDispatch, projection)
            .expect("projection matches strict canonical dispatch schema");
        let registry = CanonicalRegistry::decode(projection).expect("projection decodes");
        let encoded = serde_json::to_value(&registry).expect("registry encodes");
        assert_eq!(&encoded, projection, "registry projection must be lossless");

        let operation = &registry.operations[0];
        let encoded_operation = &encoded["operations"][0];
        assert_eq!(
            encoded_operation["canonical_id"].as_str(),
            Some(operation.canonical_id.as_str())
        );
        assert_eq!(encoded_operation["aliases"], json!(operation.aliases));
        assert_eq!(encoded_operation["args_schema"], operation.args_schema);
        assert_eq!(
            encoded_operation["effect_policy"],
            json!(operation.effect_policy)
        );
        assert_eq!(encoded_operation["errors"], json!(operation.errors));
        for alias in &operation.aliases {
            assert_eq!(
                registry.resolve(alias).unwrap().canonical_id,
                operation.canonical_id
            );
        }
        engines.insert(serde_json::to_string(&registry.engine).unwrap());
    }

    assert_eq!(
        engines,
        BTreeSet::from([
            "\"fs_zero\"".to_owned(),
            "\"graph_zero\"".to_owned(),
            "\"token_zero\"".to_owned(),
        ])
    );
    let schema: Value = serde_json::from_str(CANONICAL_DISPATCH_SCHEMA).unwrap();
    assert_eq!(
        schema["$id"],
        "https://zerostack.dev/schemas/canonical-dispatch.schema.json"
    );
    assert_eq!(
        schema["$defs"]["permitGrant"]["required"],
        json!(["permit_id", "canonical_operation_id", "effect_class"])
    );
    assert_eq!(
        schema["$defs"]["approvalGrant"]["required"],
        json!(["approval_id", "canonical_operation_id", "effect_class"])
    );
}

#[test]
fn missing_and_unknown_metadata_fail_closed_in_schema_and_decoder() {
    let fixture = fixture();
    let projection = &projections(&fixture)[0];

    let mut missing = projection.clone();
    missing["operations"][0]
        .as_object_mut()
        .unwrap()
        .remove("effect_policy");
    assert!(validate_against_schema(SchemaName::CanonicalDispatch, &missing).is_err());
    let error = CanonicalRegistry::decode(&missing).unwrap_err();
    assert_eq!(error.class, DispatchErrorClass::MissingMetadata);

    let mut unknown = projection.clone();
    unknown["operations"][0]["lexical_authorization"] = json!(true);
    assert!(validate_against_schema(SchemaName::CanonicalDispatch, &unknown).is_err());
    let error = CanonicalRegistry::decode(&unknown).unwrap_err();
    assert_eq!(error.class, DispatchErrorClass::UnknownMetadata);

    let mut unknown_effect = projection.clone();
    unknown_effect["operations"][0]["effect_policy"]["effect_class"] = json!("source_derived");
    assert!(validate_against_schema(SchemaName::CanonicalDispatch, &unknown_effect).is_err());
    let error = CanonicalRegistry::decode(&unknown_effect).unwrap_err();
    assert_eq!(error.class, DispatchErrorClass::UnknownMetadata);
}

#[test]
fn golden_vectors_cover_every_effect_and_error_class() {
    let fixture = fixture();
    let vectors = fixture["effect_class_vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 4);

    let mut covered_effects = BTreeSet::new();
    for vector in vectors {
        let policy: EffectPolicy = serde_json::from_value(vector["effect_policy"].clone()).unwrap();
        let grant: EffectGrant = serde_json::from_value(vector["grant"].clone()).unwrap();
        let permit: Option<PermitGrant> = serde_json::from_value(vector["permit"].clone()).unwrap();
        let approval: Option<ApprovalGrant> =
            serde_json::from_value(vector["approval"].clone()).unwrap();
        covered_effects.insert(serde_json::to_string(&policy.effect_class).unwrap());

        let operation = CanonicalOperation {
            canonical_id: "fixture.operation".to_owned(),
            aliases: vec!["fixture_alias".to_owned()],
            args_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            effect_policy: policy,
            errors: ALL_DISPATCH_ERROR_CLASSES.to_vec(),
        };
        let mut machine =
            DispatchMachine::new(registry_for(RegistryEngine::TokenZero, operation)).unwrap();
        machine.resolve("fixture_alias").unwrap();
        machine.validate_arguments(&json!({})).unwrap();
        machine.authorize_effect(grant).unwrap();
        machine.acquire_authority(permit, approval).unwrap();
        machine.dispatched().unwrap();
        machine.journaled().unwrap();
        machine.finish().unwrap();
        assert_eq!(machine.stage(), DispatchStage::Complete);
    }

    assert_eq!(
        covered_effects,
        BTreeSet::from([
            "\"approval_required_mutation\"".to_owned(),
            "\"irreversible\"".to_owned(),
            "\"read_only\"".to_owned(),
            "\"reversible_mutation\"".to_owned(),
        ])
    );

    let encoded_errors: BTreeSet<_> = fixture["error_class_vectors"]
        .as_array()
        .unwrap()
        .iter()
        .cloned()
        .map(|value| serde_json::from_value::<DispatchErrorClass>(value).unwrap())
        .collect();
    assert_eq!(
        encoded_errors,
        ALL_DISPATCH_ERROR_CLASSES.into_iter().collect()
    );
    for error_class in ALL_DISPATCH_ERROR_CLASSES {
        let encoded = serde_json::to_value(error_class).unwrap();
        assert!(fixture["error_class_vectors"]
            .as_array()
            .unwrap()
            .contains(&encoded));
    }
}

#[test]
fn authority_grants_are_bound_to_operation_and_effect() {
    let fixture = fixture();
    let operation = CanonicalOperation {
        canonical_id: "fixture.operation".to_owned(),
        aliases: vec!["fixture_alias".to_owned()],
        args_schema: json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
        effect_policy: EffectPolicy {
            effect_class: zero_abi::EffectClass::Irreversible,
            permit: zero_abi::PermitRequirement::Required,
            approval: zero_abi::ApprovalRequirement::Required,
        },
        errors: ALL_DISPATCH_ERROR_CLASSES.to_vec(),
    };

    let vectors = fixture["authority_binding_failure_vectors"]
        .as_array()
        .unwrap();
    assert_eq!(vectors.len(), 4);
    for vector in vectors {
        let permit: Option<PermitGrant> = serde_json::from_value(vector["permit"].clone()).unwrap();
        let approval: Option<ApprovalGrant> =
            serde_json::from_value(vector["approval"].clone()).unwrap();
        let expected: DispatchErrorClass =
            serde_json::from_value(vector["expected_error"].clone()).unwrap();

        let mut machine =
            DispatchMachine::new(registry_for(RegistryEngine::TokenZero, operation.clone()))
                .unwrap();
        machine.resolve("fixture_alias").unwrap();
        machine.validate_arguments(&json!({})).unwrap();
        machine.authorize_effect(EffectGrant::Irreversible).unwrap();
        let error = machine.acquire_authority(permit, approval).unwrap_err();
        assert_eq!(error.class, expected, "{}", vector["kind"]);
        assert_eq!(machine.stage(), DispatchStage::Failed);
    }
}

#[test]
fn lexical_source_forms_are_diagnostic_only() {
    let fixture = fixture();
    let token_projection = projections(&fixture)
        .iter()
        .find(|projection| projection["engine"] == "token_zero")
        .unwrap();
    let registry = CanonicalRegistry::decode(token_projection).unwrap();

    let mut authorization_results = Vec::new();
    let mut source_texts = BTreeSet::new();
    for vector in fixture["source_diagnostics"].as_array().unwrap() {
        let diagnostic = SourceDiagnostic {
            form: serde_json::from_value::<SourceForm>(vector["form"].clone()).unwrap(),
            source_text: vector["source_text"].as_str().unwrap().to_owned(),
            invoked_name: vector["invoked_name"].as_str().unwrap().to_owned(),
        };
        let grant = serde_json::from_value::<EffectGrant>(vector["grant"].clone()).unwrap();
        source_texts.insert(diagnostic.source_text.clone());

        let mut machine = DispatchMachine::new(registry.clone()).unwrap();
        let operation = machine.resolve_with_diagnostic(diagnostic.clone()).unwrap();
        assert_eq!(
            operation.canonical_id.as_str(),
            vector["canonical_id"].as_str().unwrap()
        );
        machine
            .validate_arguments(&json!({ "command": "printf ok" }))
            .unwrap();
        machine.authorize_effect(grant).unwrap();
        authorization_results.push((
            machine.operation().unwrap().canonical_id.clone(),
            machine.operation().unwrap().effect_policy,
            machine.stage(),
        ));
        assert_eq!(machine.diagnostic(), Some(&diagnostic));
    }

    assert_eq!(
        source_texts.len(),
        3,
        "the lexical forms must actually differ"
    );
    assert!(authorization_results
        .windows(2)
        .all(|window| window[0] == window[1]));
    assert_eq!(authorization_results[0].2, DispatchStage::AcquireAuthority);
}

#[test]
fn invalid_arguments_grants_and_stage_transitions_poison_the_machine() {
    let fixture = fixture();
    let registry = CanonicalRegistry::decode(&projections(&fixture)[0]).unwrap();

    let mut invalid_args = DispatchMachine::new(registry.clone()).unwrap();
    invalid_args.resolve("shell").unwrap();
    let error = invalid_args
        .validate_arguments(&json!({ "command": 3 }))
        .unwrap_err();
    assert_eq!(error.class, DispatchErrorClass::InvalidArguments);
    assert_eq!(invalid_args.stage(), DispatchStage::Failed);

    let mut wrong_grant = DispatchMachine::new(registry.clone()).unwrap();
    wrong_grant.resolve("shell").unwrap();
    wrong_grant
        .validate_arguments(&json!({ "command": "true" }))
        .unwrap();
    let error = wrong_grant
        .authorize_effect(EffectGrant::ReadOnly)
        .unwrap_err();
    assert_eq!(error.class, DispatchErrorClass::UnauthorizedEffect);
    assert_eq!(wrong_grant.stage(), DispatchStage::Failed);

    let mut skipped = DispatchMachine::new(registry).unwrap();
    let error = skipped.dispatched().unwrap_err();
    assert_eq!(error.class, DispatchErrorClass::InvalidStageTransition);
    assert_eq!(skipped.stage(), DispatchStage::Failed);
    assert!(skipped.resolve("shell").is_err());
}
