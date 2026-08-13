    use serde_json::json;

    use super::*;

    fn operation(canonical_id: &str, alias: &str) -> CanonicalOperation {
        CanonicalOperation {
            canonical_id: canonical_id.to_owned(),
            description: String::new(),
            aliases: vec![alias.to_owned()],
            output_schema: None,
            mcp_tool_name: None,
            args_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            effect_policy: EffectPolicy {
                effect_class: EffectClass::ReadOnly,
                permit: PermitRequirement::NotRequired,
                approval: ApprovalRequirement::NotRequired,
            },
            errors: vec![DispatchErrorClass::InvalidStageTransition],
        }
    }

    #[test]
    fn legacy_manifest_omits_optional_surface_metadata_and_keeps_digest() {
        let registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![operation("fs.read", "read")],
            resources: vec![],
        };
        let manifest = registry.canonical_manifest().unwrap();
        assert_eq!(
            manifest,
            json!({
                "version": CANONICAL_DISPATCH_VERSION,
                "engine": "fs_zero",
                "operations": [{
                    "canonical_id": "fs.read",
                    "aliases": ["read"],
                    "args_schema": {
                        "additionalProperties": false,
                        "properties": {},
                        "type": "object"
                    },
                    "effect_policy": {
                        "approval": "not_required",
                        "effect_class": "read_only",
                        "permit": "not_required"
                    },
                    "errors": ["invalid_stage_transition"]
                }]
            })
        );
        assert!(manifest["operations"][0].get("description").is_none());
        assert!(manifest["operations"][0].get("output_schema").is_none());
        assert!(manifest["operations"][0].get("mcp_tool_name").is_none());
        assert!(manifest.get("resources").is_none());
    }

    #[test]
    fn populated_surface_metadata_round_trips_and_resources_sort_by_uri() {
        let mut registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![operation("fs.read", "read")],
            resources: vec![
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
            ],
        };
        registry.operations[0].description = "Read bytes".into();
        registry.operations[0].output_schema = Some(json!({
            "type": "object",
            "properties": {"bytes": {"type": "integer"}}
        }));
        registry.operations[0].mcp_tool_name = Some("read_bytes".into());

        let encoded = serde_json::to_value(&registry).unwrap();
        let decoded = CanonicalRegistry::decode(&encoded).unwrap();
        assert_eq!(decoded, registry);
        let manifest = registry.canonical_manifest().unwrap();
        assert_eq!(manifest["operations"][0]["description"], "Read bytes");
        assert_eq!(manifest["operations"][0]["output_schema"]["type"], "object");
        assert_eq!(manifest["operations"][0]["mcp_tool_name"], "read_bytes");
        assert_eq!(manifest["resources"][0]["uri"], "resource://a");
        assert_eq!(manifest["resources"][1]["uri"], "resource://z");

        registry.resources[0].name = "TokenZero shell contract".into();
        assert!(registry.validate().is_ok());
        registry.resources[0].name = " TokenZero shell contract".into();
        assert_eq!(
            registry.validate().unwrap_err().class,
            DispatchErrorClass::InvalidRegistry
        );
        registry.resources[0].name.clear();
        assert_eq!(
            registry.validate().unwrap_err().class,
            DispatchErrorClass::InvalidRegistry
        );
    }

    #[test]
    fn surface_metadata_validation_rejects_invalid_schema_resource_and_name_collisions() {
        let mut invalid_schema = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![operation("fs.read", "read")],
            resources: vec![],
        };
        invalid_schema.operations[0].output_schema = Some(json!("string"));
        assert_eq!(
            invalid_schema.validate().unwrap_err().class,
            DispatchErrorClass::InvalidRegistry
        );

        let mut duplicate_resources = invalid_schema.clone();
        duplicate_resources.operations[0].output_schema = None;
        duplicate_resources.resources = vec![
            CanonicalResource {
                uri: "resource://same".into(),
                name: "one".into(),
                description: String::new(),
                mime_type: None,
            },
            CanonicalResource {
                uri: "resource://same".into(),
                name: "two".into(),
                description: String::new(),
                mime_type: None,
            },
        ];
        assert_eq!(
            duplicate_resources.validate().unwrap_err().class,
            DispatchErrorClass::InvalidRegistry
        );

        let mut primary_collision = duplicate_resources.clone();
        primary_collision.resources.clear();
        primary_collision.operations[0].mcp_tool_name = Some("read".into());
        assert_eq!(
            primary_collision.validate().unwrap_err().class,
            DispatchErrorClass::InvalidRegistry
        );

        let mut canonical_collision = primary_collision.clone();
        canonical_collision.operations[0].mcp_tool_name = Some("fs.other".into());
        canonical_collision
            .operations
            .push(operation("fs.other", "other"));
        assert_eq!(
            canonical_collision.validate().unwrap_err().class,
            DispatchErrorClass::InvalidRegistry
        );
    }

    #[test]
    fn registry_rejects_alias_collisions() {
        let registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![
                operation("fs.first", "shared"),
                operation("fs.second", "shared"),
            ],
            resources: vec![],
        };
        assert_eq!(
            registry.validate().unwrap_err().class,
            DispatchErrorClass::InvalidRegistry
        );
    }

    #[test]
    fn registry_digest_ignores_set_like_declaration_order() {
        let mut read = operation("fs.read", "read");
        read.aliases = vec!["read_bytes".to_owned(), "cat".to_owned()];
        read.errors = vec![
            DispatchErrorClass::UnknownOperation,
            DispatchErrorClass::InvalidArguments,
        ];
        read.args_schema = json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["text", "bytes"],
                    "default": "text"
                },
                "path": {"type": "string"},
                "priority": {
                    "type": "array",
                    "items": {"type": "string"},
                    "default": ["first", "second"]
                }
            },
            "required": ["path", "mode"],
            "additionalProperties": false
        });
        let write = operation("fs.write", "write");

        let declared = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![read, write],
            resources: vec![],
        };
        let mut permuted = declared.clone();
        permuted.operations.reverse();
        for operation in &mut permuted.operations {
            operation.aliases.reverse();
            operation.errors.reverse();
            if let Some(required) = operation
                .args_schema
                .get_mut("required")
                .and_then(Value::as_array_mut)
            {
                required.reverse();
            }
            if let Some(variants) = operation
                .args_schema
                .pointer_mut("/properties/mode/enum")
                .and_then(Value::as_array_mut)
            {
                variants.reverse();
            }
        }

        assert_eq!(
            declared.canonical_manifest().unwrap(),
            permuted.canonical_manifest().unwrap()
        );
        assert_eq!(
            declared.contract_digest().unwrap(),
            permuted.contract_digest().unwrap()
        );
        assert_eq!(
            declared.contract_digest_hex().unwrap(),
            permuted.contract_digest_hex().unwrap()
        );
        assert_eq!(
            declared.contract_digest_hex().unwrap(),
            "d3749160c2dc4ec5ea6c7038534a4bf8d22e68a5838fde5d4d215348c8bbf924"
        );

        let mut semantic_change = declared.clone();
        semantic_change.operations[0].args_schema["properties"]["priority"]["default"] =
            json!(["second", "first"]);
        assert_ne!(
            declared.contract_digest_hex().unwrap(),
            semantic_change.contract_digest_hex().unwrap()
        );
    }

    #[test]
    fn legacy_registry_version_fails_closed_after_digest_semantics_bump() {
        let legacy = CanonicalRegistry {
            version: "zerostack.canonical_dispatch.v1".to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![operation("fs.read", "read")],
            resources: vec![],
        };
        assert_eq!(
            legacy.contract_digest_hex().unwrap_err().class,
            DispatchErrorClass::UnknownMetadata
        );
    }

    #[test]
    fn registry_digest_rejects_duplicate_contract_entries() {
        let mut duplicate_alias = operation("fs.read", "read");
        duplicate_alias.aliases.push("read".to_owned());
        let duplicate_alias_registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![duplicate_alias],
            resources: vec![],
        };
        assert_eq!(
            duplicate_alias_registry
                .contract_digest_hex()
                .unwrap_err()
                .class,
            DispatchErrorClass::InvalidRegistry
        );

        let mut duplicate_error = operation("fs.read", "read");
        duplicate_error
            .errors
            .push(DispatchErrorClass::InvalidStageTransition);
        let duplicate_error_registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![duplicate_error],
            resources: vec![],
        };
        assert_eq!(
            duplicate_error_registry
                .contract_digest_hex()
                .unwrap_err()
                .class,
            DispatchErrorClass::InvalidRegistry
        );
    }

    #[test]
    fn out_of_order_transition_poison_is_permanent() {
        let registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![operation("fs.read", "read")],
            resources: vec![],
        };
        let mut machine = DispatchMachine::new(registry).unwrap();
        assert_eq!(
            machine.dispatched().unwrap_err().class,
            DispatchErrorClass::InvalidStageTransition
        );
        assert_eq!(machine.stage(), DispatchStage::Failed);
        assert!(machine.resolve("read").is_err());
        assert_eq!(machine.stage(), DispatchStage::Failed);
    }

    fn authority_machine_for(
        effect_policy: EffectPolicy,
        effect_grant: EffectGrant,
    ) -> DispatchMachine {
        let mut operation = operation("fixture.operation", "fixture_alias");
        operation.effect_policy = effect_policy;
        let registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::TokenZero,
            operations: vec![operation],
            resources: vec![],
        };
        let mut machine = DispatchMachine::new(registry).unwrap();
        machine.resolve("fixture_alias").unwrap();
        machine.validate_arguments(&json!({})).unwrap();
        machine.authorize_effect(effect_grant).unwrap();
        machine
    }

    fn authority_machine() -> DispatchMachine {
        authority_machine_for(
            EffectPolicy {
                effect_class: EffectClass::Irreversible,
                permit: PermitRequirement::Required,
                approval: ApprovalRequirement::Required,
            },
            EffectGrant::Irreversible,
        )
    }

    fn valid_permit() -> PermitGrant {
        PermitGrant {
            permit_id: "permit-valid".to_owned(),
            canonical_operation_id: "fixture.operation".to_owned(),
            effect_class: EffectClass::Irreversible,
        }
    }

    fn valid_approval() -> ApprovalGrant {
        ApprovalGrant {
            approval_id: "approval-valid".to_owned(),
            canonical_operation_id: "fixture.operation".to_owned(),
            effect_class: EffectClass::Irreversible,
        }
    }

    #[test]
    fn authority_grants_reject_wrong_operation_and_effect_bindings() {
        let mut wrong_operation = authority_machine();
        let mut permit = valid_permit();
        permit.canonical_operation_id = "other.operation".to_owned();
        assert_eq!(
            wrong_operation
                .acquire_authority(Some(permit), Some(valid_approval()))
                .unwrap_err()
                .class,
            DispatchErrorClass::PermitRequired
        );
        assert_eq!(wrong_operation.stage(), DispatchStage::Failed);

        let mut wrong_effect = authority_machine();
        let mut approval = valid_approval();
        approval.effect_class = EffectClass::ReadOnly;
        assert_eq!(
            wrong_effect
                .acquire_authority(Some(valid_permit()), Some(approval))
                .unwrap_err()
                .class,
            DispatchErrorClass::ApprovalRequired
        );
        assert_eq!(wrong_effect.stage(), DispatchStage::Failed);
    }

    #[test]
    fn complexity_dispatch_schema_preserves_recursive_paths_and_object_rules() {
        let schema = json!({
            "type": "object",
            "properties": {
                "entries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": {"type": "string", "enum": ["allowed"]}
                        },
                        "required": ["name"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["entries"],
            "additionalProperties": false
        });

        let missing = validate_schema_value(&schema, &json!({"entries": [{}]}), "$").unwrap_err();
        assert_eq!(missing, "missing required argument $.entries[0].name");

        let unknown = validate_schema_value(
            &schema,
            &json!({"entries": [{"name": "allowed", "extra": true}]}),
            "$",
        )
        .unwrap_err();
        assert_eq!(unknown, "unknown argument $.entries[0].extra");
    }

    #[test]
    fn complexity_dispatch_authority_requires_ids_in_fail_closed_order() {
        let mut empty_permit = valid_permit();
        empty_permit.permit_id = "  ".to_owned();
        let mut empty_approval = valid_approval();
        empty_approval.approval_id = String::new();

        let mut permit_first = authority_machine();
        let error = permit_first
            .acquire_authority(Some(empty_permit), Some(empty_approval.clone()))
            .unwrap_err();
        assert_eq!(error.class, DispatchErrorClass::PermitRequired);
        assert_eq!(error.message, "a non-empty permit is required");
        assert_eq!(permit_first.stage(), DispatchStage::Failed);

        let mut approval_after_permit = authority_machine();
        let error = approval_after_permit
            .acquire_authority(Some(valid_permit()), Some(empty_approval))
            .unwrap_err();
        assert_eq!(error.class, DispatchErrorClass::ApprovalRequired);
        assert_eq!(error.message, "a non-empty approval is required");
        assert_eq!(approval_after_permit.stage(), DispatchStage::Failed);
    }

    #[test]
    fn complexity_dispatch_rejects_authority_disallowed_by_effect_policy() {
        let mut read_only = authority_machine_for(
            EffectPolicy {
                effect_class: EffectClass::ReadOnly,
                permit: PermitRequirement::NotRequired,
                approval: ApprovalRequirement::NotRequired,
            },
            EffectGrant::ReadOnly,
        );
        let error = read_only
            .acquire_authority(Some(valid_permit()), None)
            .unwrap_err();
        assert_eq!(error.class, DispatchErrorClass::InvalidStageTransition);
        assert_eq!(
            error.message,
            "permit supplied for an operation that does not accept one"
        );

        let mut reversible = authority_machine_for(
            EffectPolicy {
                effect_class: EffectClass::ReversibleMutation,
                permit: PermitRequirement::Required,
                approval: ApprovalRequirement::NotRequired,
            },
            EffectGrant::ReversibleMutation,
        );
        let mut permit = valid_permit();
        permit.effect_class = EffectClass::ReversibleMutation;
        let error = reversible
            .acquire_authority(Some(permit), Some(valid_approval()))
            .unwrap_err();
        assert_eq!(error.class, DispatchErrorClass::InvalidStageTransition);
        assert_eq!(
            error.message,
            "approval supplied for an operation that does not accept one"
        );
    }
