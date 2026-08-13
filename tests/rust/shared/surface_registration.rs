use serde_json::json;
use zero_abi::{
    ALL_DISPATCH_ERROR_CLASSES, ApprovalRequirement, CanonicalOperation, CanonicalRegistry,
    CanonicalResource, EffectClass, EffectPolicy, EngineIdentity, PermitRequirement, RefOwnership,
    RegistryEngine, TelemetrySchema,
};
use zero_codemode::{
    CapabilityDescriptor, DomainAdapterRegistration, SurfaceContractError, SurfaceKind,
    SurfaceRegistration,
};

fn registration(surface: SurfaceKind) -> SurfaceRegistration {
    SurfaceRegistration::new(
        surface,
        "zero",
        DomainAdapterRegistration {
            engine: EngineIdentity::FsZero,
            registry: CanonicalRegistry {
                version: zero_abi::CANONICAL_DISPATCH_VERSION.into(),
                engine: RegistryEngine::FsZero,
                operations: vec![CanonicalOperation {
                    canonical_id: "fs.read".into(),
                    description: String::new(),
                    aliases: vec!["read".into()],
                    args_schema: json!({"type":"object", "additionalProperties":false}),
                    output_schema: None,
                    mcp_tool_name: None,
                    effect_policy: EffectPolicy {
                        effect_class: EffectClass::ReadOnly,
                        permit: PermitRequirement::NotRequired,
                        approval: ApprovalRequirement::NotRequired,
                    },
                    errors: ALL_DISPATCH_ERROR_CLASSES.to_vec(),
                }],
                resources: vec![],
            },
            ref_ownership: RefOwnership {
                engine: EngineIdentity::FsZero,
                session_id: "session".into(),
                refs: vec!["fz://ref".into()],
                snapshot: None,
            },
            telemetry_schema: TelemetrySchema::V1,
            capabilities: vec![CapabilityDescriptor::new("fs", "read")],
        },
    )
}

#[test]
fn selected_surface_is_exclusive_at_global_registration_boundary() {
    let codemode = registration(SurfaceKind::CodeMode);
    assert_eq!(codemode.global_registration().unwrap().root, "zero");

    let mcp = registration(SurfaceKind::Mcp);
    assert!(matches!(
        mcp.global_registration(),
        Err(SurfaceContractError::WrongSurface {
            requested: SurfaceKind::CodeMode,
            actual: SurfaceKind::Mcp
        })
    ));
}

#[test]
fn catalog_round_trip_is_strict_and_digestable() {
    let registration = registration(SurfaceKind::CodeMode);
    let catalog = registration.catalog().unwrap();
    assert_eq!(catalog["contract_version"], "zerostack.surface/v1");
    assert_eq!(catalog["surface"], "codemode");
    assert_eq!(
        registration.adapter.registry_digest_hex().unwrap().len(),
        64
    );

    let mut unknown = catalog;
    unknown["unexpected"] = true.into();
    assert!(serde_json::from_value::<SurfaceRegistration>(unknown).is_err());
}

#[test]
fn populated_surface_metadata_round_trips_with_stable_digest() {
    let mut registration = registration(SurfaceKind::Mcp);
    registration.instructions = Some("Use the engine catalog exactly as declared.".into());
    let operation = &mut registration.adapter.registry.operations[0];
    operation.description = "Read a UTF-8 file".into();
    operation.output_schema = Some(json!({
        "type": "object",
        "properties": {"text": {"type": "string"}}
    }));
    operation.mcp_tool_name = Some("read_file".into());
    registration.adapter.registry.resources = vec![
        CanonicalResource {
            uri: "resource://z".into(),
            name: "Z".into(),
            description: "last".into(),
            mime_type: Some("text/plain".into()),
        },
        CanonicalResource {
            uri: "resource://a".into(),
            name: "A".into(),
            description: "first".into(),
            mime_type: None,
        },
    ];
    let digest = registration.adapter.registry_digest_hex().unwrap();
    let catalog = registration.catalog().unwrap();
    let decoded: SurfaceRegistration = serde_json::from_value(catalog.clone()).unwrap();
    assert_eq!(decoded, registration);
    assert_eq!(decoded.adapter.registry_digest_hex().unwrap(), digest);
    assert_eq!(
        catalog["instructions"],
        "Use the engine catalog exactly as declared."
    );
    assert_eq!(
        catalog["adapter"]["registry"]["resources"][0]["uri"],
        "resource://z"
    );
}

#[test]
fn adapter_metadata_must_bind_to_one_engine() {
    let mut registration = registration(SurfaceKind::CodeMode);
    registration.adapter.ref_ownership.engine = EngineIdentity::GraphZero;
    assert!(matches!(
        registration.validate(),
        Err(SurfaceContractError::EngineMismatch {
            field: "ref_ownership.engine",
            ..
        })
    ));
}
