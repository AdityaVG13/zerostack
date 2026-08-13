    use super::*;
    use crate::{
        ALL_DISPATCH_ERROR_CLASSES, ApprovalRequirement, CasLayout, EffectClass, EffectPolicy,
        HashAlgorithm, LayoutVersion, PermitRequirement, RegistryEngine, SharedCapability,
    };
    use serde_json::json;

    fn adapter() -> DomainAdapterRegistration {
        DomainAdapterRegistration {
            engine: EngineIdentity::FsZero,
            registry: CanonicalRegistry {
                version: crate::CANONICAL_DISPATCH_VERSION.to_owned(),
                engine: RegistryEngine::FsZero,
                operations: vec![crate::CanonicalOperation {
                    canonical_id: "fs.read".into(),
                    description: String::new(),
                    aliases: vec!["read".into()],
                    args_schema: json!({"type": "object", "additionalProperties": false}),
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
            ref_ownership: crate::RefOwnership {
                engine: EngineIdentity::FsZero,
                session_id: "session-1".into(),
                refs: vec!["fz://ref-1".into()],
                snapshot: None,
            },
            telemetry_schema: TelemetrySchema::V1,
            capabilities: vec![CapabilityDescriptor::new("fs", "read")],
        }
    }

    #[test]
    fn codemode_registration_projects_to_one_valid_global_tree() {
        let registration = SurfaceRegistration::new(SurfaceKind::CodeMode, "zero", adapter());
        let global = registration.global_registration().unwrap();
        assert_eq!(global.root, "zero");
        assert_eq!(global.capabilities.len(), 1);
        assert_eq!(registration.surface.opposite(), SurfaceKind::Mcp);
        assert_eq!(
            registration.adapter.registry_digest_hex().unwrap().len(),
            64
        );
    }

    #[test]
    fn mcp_registration_cannot_be_used_as_codemode() {
        let registration = SurfaceRegistration::new(SurfaceKind::Mcp, "zero", adapter());
        assert!(matches!(
            registration.global_registration(),
            Err(SurfaceContractError::WrongSurface {
                requested: SurfaceKind::CodeMode,
                actual: SurfaceKind::Mcp
            })
        ));
    }

    #[test]
    fn adapter_engine_bindings_and_unknown_metadata_fail_closed() {
        let mut wrong = adapter();
        wrong.ref_ownership.engine = EngineIdentity::GraphZero;
        assert!(matches!(
            wrong.validate(),
            Err(SurfaceContractError::EngineMismatch {
                field: "ref_ownership.engine",
                ..
            })
        ));

        let mut wire = serde_json::to_value(SurfaceRegistration::new(
            SurfaceKind::CodeMode,
            "zero",
            adapter(),
        ))
        .unwrap();
        wire["unexpected"] = json!(true);
        assert!(serde_json::from_value::<SurfaceRegistration>(wire).is_err());
    }

    #[test]
    fn capability_catalog_must_match_canonical_operations_exactly() {
        let mut missing = adapter();
        missing.capabilities.clear();
        assert!(matches!(
            missing.validate(),
            Err(SurfaceContractError::CapabilityCatalogMismatch { missing, extra })
                if missing == vec!["fs.read"] && extra.is_empty()
        ));

        let mut extra = adapter();
        extra.capabilities = vec![
            CapabilityDescriptor::new("fs", "read"),
            CapabilityDescriptor::new("fs", "write"),
        ];
        assert!(matches!(
            extra.validate(),
            Err(SurfaceContractError::CapabilityCatalogMismatch { missing, extra })
                if missing.is_empty() && extra == vec!["fs.write"]
        ));

        let mut duplicate = adapter();
        duplicate
            .capabilities
            .push(CapabilityDescriptor::new("fs", "read"));
        assert!(matches!(
            duplicate.validate(),
            Err(SurfaceContractError::InvalidCapabilities(
                RegistrationError::DuplicateCapability(_)
            ))
        ));
    }

    #[test]
    fn surface_parser_accepts_only_canonical_faces_and_aliases() {
        assert_eq!(
            SurfaceKind::parse("codemode").unwrap(),
            SurfaceKind::CodeMode
        );
        assert_eq!(
            SurfaceKind::parse("code-mode").unwrap(),
            SurfaceKind::CodeMode
        );
        assert_eq!(SurfaceKind::parse("mcp").unwrap(), SurfaceKind::Mcp);
        assert!(SurfaceKind::parse("both").is_err());
        let capability = SharedCapability::zeroref_v1(
            HashAlgorithm::Sha256,
            CasLayout::BlobsSha256Hh,
            LayoutVersion::V1,
        );
        assert_eq!(capability.hash.algorithm, HashAlgorithm::Sha256);
    }
