    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn artifact(artifact_id: &str, owner: ArtifactOwnerV1, byte: u8) -> LinkedArtifactV1 {
        LinkedArtifactV1 {
            artifact_id: artifact_id.into(),
            owner,
            artifact_version: "1.0.0".into(),
            source_repository: format!("https://example.invalid/{artifact_id}"),
            source_revision: format!("{byte:02x}").repeat(20),
            artifact_digest: digest(byte),
            contract_digest: digest(byte.wrapping_add(16)),
        }
    }

    fn worker(engine: EngineIdentity, byte: u8) -> WorkerIdentityV1 {
        WorkerIdentityV1 {
            engine,
            artifact_digest: digest(byte),
            worker_protocol_digest: digest(byte.wrapping_add(32)),
            semantic_contract_digest: digest(byte.wrapping_add(48)),
            operation_registry_digest: digest(byte.wrapping_add(64)),
            capability_catalog_digest: digest(byte.wrapping_add(80)),
        }
    }

    fn assembly_manifest_fixture() -> AssemblyManifestV1 {
        AssemblyManifestV1 {
            schema_version: ASSEMBLY_MANIFEST_SCHEMA_VERSION,
            required_abi_contract_version: ASSEMBLY_ABI_CONTRACT_VERSION,
            abi_contract_digest: assembly_abi_contract_digest_v1(),
            linked_artifacts: vec![
                artifact("fszero.worker", ArtifactOwnerV1::FsZero, 1),
                artifact("graphzero.worker", ArtifactOwnerV1::GraphZero, 2),
                artifact("tokenzero.worker", ArtifactOwnerV1::TokenZero, 3),
                artifact("zerostack.host", ArtifactOwnerV1::ZeroStack, 4),
            ],
            linked_profiles: vec![
                LinkedProfileV1 {
                    profile_kind: ProfileKindV1::Platform,
                    profile_id: "linux-x86_64-v1".into(),
                    profile_version: "1".into(),
                    profile_digest: digest(101),
                },
                LinkedProfileV1 {
                    profile_kind: ProfileKindV1::Runtime,
                    profile_id: "quickjs-v1".into(),
                    profile_version: "2025-09-13".into(),
                    profile_digest: digest(102),
                },
            ],
            target: TargetIdentityV1 {
                target_triple: "x86_64-unknown-linux-gnu".into(),
                architecture: "x86_64".into(),
                operating_system: "linux".into(),
                abi: "gnu".into(),
            },
            platform: PlatformIdentityV1 {
                profile_id: "linux-x86_64-v1".into(),
                profile_version: "1".into(),
                profile_digest: digest(101),
            },
            verifiers: vec![VerifierIdentityV1 {
                verifier_id: "zero-testkit.assembly-kat".into(),
                verifier_version: "1".into(),
                verifier_digest: digest(103),
            }],
            receipt_schema: ReceiptSchemaIdentityV1 {
                schema_id: "zerostack.proof_receipt".into(),
                schema_version: "1".into(),
                schema_digest: digest(104),
            },
            runtime_generation: 7,
            assembly_epoch: 42,
            workers: vec![
                worker(EngineIdentity::FsZero, 1),
                worker(EngineIdentity::GraphZero, 2),
                worker(EngineIdentity::TokenZero, 3),
            ],
            aggregate_capability_catalog_digest: digest(105),
        }
    }

    #[test]
    fn assembly_manifest_canonical_vector_is_stable() {
        let manifest = assembly_manifest_fixture();
        let bytes = manifest.canonical_bytes().unwrap();
        let decoded = AssemblyManifestV1::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(decoded, manifest);
        assert_eq!(
            manifest.digest().unwrap().to_hex(),
            "7a5d8c5a6bfd4e8990510d9f4129f734bd07f4cc3a2603068ce5bb3d80246b92"
        );
    }

    #[test]
    fn assembly_manifest_contract_digest_is_stable() {
        let contract = assembly_abi_contract_manifest_v1();
        assert_eq!(
            contract["linked_contracts"]["zbf_contract_digest"],
            zbf_contract_digest_v1().to_hex()
        );
        assert_eq!(
            assembly_abi_contract_digest_v1().to_hex(),
            "f9320787ce17676c1eff1b2e38f1897ca40f9a72a02d5d72ffba37d70aa70d70"
        );
    }

    #[test]
    fn assembly_manifest_rejects_noncanonical_unknown_and_unlinked_values() {
        let manifest = assembly_manifest_fixture();
        let mut bytes = manifest.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            AssemblyManifestV1::from_canonical_bytes(&bytes).unwrap_err(),
            AssemblyManifestErrorV1::NonCanonicalEncoding
        );

        let mut unknown = assembly_manifest_fixture();
        unknown.schema_version += 1;
        assert!(matches!(
            unknown.validate(),
            Err(AssemblyManifestErrorV1::UnsupportedVersion {
                field: "schema_version",
                ..
            })
        ));

        let mut unlinked = assembly_manifest_fixture();
        unlinked.workers[0].artifact_digest = digest(200);
        assert_eq!(
            unlinked.validate().unwrap_err(),
            AssemblyManifestErrorV1::UnlinkedWorkerArtifact(EngineIdentity::FsZero)
        );
    }

    #[test]
    fn assembly_manifest_predispatch_skew_is_typed() {
        let manifest = assembly_manifest_fixture();
        let mut expected = manifest.expectation().unwrap();
        expected.workers[0].artifact_digest = digest(200);
        assert_eq!(
            validate_assembly_pre_dispatch_v1(&manifest, &expected)
                .unwrap_err()
                .code(),
            AssemblyFailureCodeV1::WorkerDigestMismatch
        );

        let mut expected = manifest.expectation().unwrap();
        expected.workers[1].capability_catalog_digest = digest(201);
        assert_eq!(
            validate_assembly_pre_dispatch_v1(&manifest, &expected)
                .unwrap_err()
                .code(),
            AssemblyFailureCodeV1::CapabilityCatalogDigestMismatch
        );

        let mut expected = manifest.expectation().unwrap();
        expected.required_schema_version += 1;
        assert_eq!(
            validate_assembly_pre_dispatch_v1(&manifest, &expected)
                .unwrap_err()
                .code(),
            AssemblyFailureCodeV1::UnsupportedRequiredVersion
        );
    }

    #[test]
    fn assembly_manifest_digest_wire_form_is_strict() {
        let value = digest(0xab);
        let encoded = serde_json::to_string(&value).unwrap();
        assert_eq!(encoded, format!("\"{}\"", "ab".repeat(32)));
        assert_eq!(serde_json::from_str::<DigestV1>(&encoded).unwrap(), value);
        assert!(serde_json::from_str::<DigestV1>(&format!("\"{}\"", "AB".repeat(32))).is_err());
    }
