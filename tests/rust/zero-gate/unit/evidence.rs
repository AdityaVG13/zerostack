    use super::*;
    use crate::program::{
        AppliedGcEvidenceV1, GcProducerEpochV1, GcReport, LifecycleReport, LifecycleState,
        McpReport, PlannerReport, ProgramUsage, WorkerClosureKind, WorkerReport,
        mcp_evidence_digest,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use tempfile::TempDir;
    use zero_abi::EngineIdentity;
    use zero_store::{
        GC_RECORD_TYPE_DRY_RUN, GC_SCHEMA_VERSION, GcCandidate, GcRunReceipt, GcRunState,
        GcVerdict, gc_contract_digest_hex,
    };

    fn program_id() -> [u8; 32] {
        sha256(b"program-evidence-test")
    }

    fn applied_gc(id: [u8; 32]) -> GcReport {
        let hashes = (0..7)
            .map(|index| format!("{index:064x}"))
            .collect::<Vec<_>>();
        let receipt = GcRunReceipt {
            schema_version: GC_SCHEMA_VERSION.into(),
            record_type: GC_RECORD_TYPE_DRY_RUN.into(),
            store_contract_digest: gc_contract_digest_hex(),
            run_id: "evidence-test-applied".into(),
            store_root: "/tmp/evidence-test-applied".into(),
            evaluated_at: "2026-08-11T00:00:00.000Z".into(),
            apply: true,
            state: GcRunState::Complete,
            objects: hashes
                .iter()
                .map(|hash| GcCandidate {
                    blob_hash: hash.clone(),
                    verdict: GcVerdict::Collect,
                    reason_codes: vec!["no-live-reference".into()],
                    evidence: vec!["test applied receipt".into()],
                })
                .collect(),
            planned: hashes.clone(),
            deleted: hashes,
        };
        let epochs = [
            EngineIdentity::FsZero,
            EngineIdentity::GraphZero,
            EngineIdentity::TokenZero,
        ]
        .into_iter()
        .map(|engine| GcProducerEpochV1 { engine, epoch: 1 })
        .collect();
        let applied = AppliedGcEvidenceV1::new(receipt, epochs, 4096).unwrap();
        GcReport::new_applied(1, id, applied)
    }

    fn report_values(seed: u8) -> BTreeMap<&'static str, Value> {
        let id = program_id();
        let tools = sha256(&[seed; 8]);
        let mcp = McpReport::new(1, id, 2, 5, tools);
        let planner = PlannerReport::new(1, id, sha256(&[seed; 4]), 3);
        let worker = WorkerReport::new(
            1,
            id,
            sha256(&[seed; 5]),
            3,
            WorkerClosureKind::Commit,
            mcp_evidence_digest(2, 5, tools),
            sha256(&[seed; 6]),
            sha256(&[seed; 7]),
            ProgramUsage {
                cpu_ns: 100,
                memory_bytes: 1024,
                io_bytes: 512,
            },
        );
        let lifecycle = LifecycleReport::new(1, id, 5, 3, LifecycleState::Closed);
        let gc = applied_gc(id);
        let mut map = BTreeMap::new();
        map.insert(
            "planner",
            serde_json::to_value(&planner).expect("planner serializes"),
        );
        map.insert(
            "worker",
            serde_json::to_value(&worker).expect("worker serializes"),
        );
        map.insert("mcp", serde_json::to_value(&mcp).expect("mcp serializes"));
        map.insert(
            "lifecycle",
            serde_json::to_value(&lifecycle).expect("lifecycle serializes"),
        );
        map.insert("gc", serde_json::to_value(&gc).expect("gc serializes"));
        map
    }

    /// Builds one sealed artifact envelope: `artifact_sha256` is the digest
    /// over the canonical JSON with the sha field zeroed, and `artifact_bytes`
    /// is the exact final byte length. Mirrors `artifact_digest` exactly.
    fn sealed_artifact_bytes(mut value: Value) -> Vec<u8> {
        value["artifact_sha256"] = json!("0".repeat(64));
        // artifact_bytes is part of the digest; fix its final value first
        // (the sha field is fixed-width, so patching the digest cannot change
        // the byte length).
        let mut length = 0u64;
        for _ in 0..4 {
            value["artifact_bytes"] = json!(length);
            let canonical = canonical_json(&value);
            let next = canonical.len() as u64;
            if next == length {
                break;
            }
            length = next;
        }
        let sha = sha256_hex(canonical_json(&value).as_bytes());
        value["artifact_sha256"] = json!(sha);
        canonical_json(&value).into_bytes()
    }

    fn artifact_bytes(
        class: EvidenceClassV1,
        report: &Value,
        source_head: &str,
        hub_head: &str,
    ) -> Vec<u8> {
        sealed_artifact_bytes(json!({
            "contract": class.contract(),
            "schema_version": 1,
            "source_head": source_head,
            "hub_head": hub_head,
            "artifact_sha256": "0".repeat(64),
            "artifact_bytes": 0,
            "report": report.clone(),
        }))
    }

    fn head(byte: u8) -> String {
        format!("{:02x}", byte).repeat(20)
    }

    fn source_head() -> String {
        head(0x11)
    }
    fn hub_head() -> String {
        head(0x22)
    }
    fn engine_head() -> String {
        head(0x33)
    }

    fn files_for(
        engine: &str,
        seed: u8,
        source: &str,
        hub: &str,
        base: &Path,
    ) -> BTreeMap<String, PathBuf> {
        let dir = base.join(engine);
        std::fs::create_dir_all(&dir).expect("create artifact dir");
        let values = report_values(seed);
        let mut files = BTreeMap::new();
        for class in EvidenceClassV1::ALL {
            let path = dir.join(format!("{}.json", class.key()));
            let bytes = artifact_bytes(class, &values[class.key()], source, hub);
            std::fs::write(&path, &bytes).expect("write artifact");
            files.insert(class.key().to_owned(), path);
        }
        files
    }

    fn engine_source(head: &str, files: BTreeMap<String, PathBuf>) -> EngineEvidenceSourceV1 {
        EngineEvidenceSourceV1 {
            head: head.to_owned(),
            files,
        }
    }

    fn valid_manifest(base: &Path) -> ProgramEvidenceManifestV1 {
        let source = source_head();
        let hub = hub_head();
        let mut engines = BTreeMap::new();
        for (index, engine) in EngineIdV1::ALL.iter().enumerate() {
            engines.insert(
                engine.key().to_owned(),
                engine_source(
                    &engine_head(),
                    files_for(engine.key(), index as u8 + 1, &source, &hub, base),
                ),
            );
        }
        ProgramEvidenceManifestV1 {
            version: 1,
            source_head: source,
            hub_head: hub,
            assembly_manifest_digest: "ab".repeat(32),
            engines,
        }
    }

    fn loader_for(
        files: &BTreeMap<PathBuf, Vec<u8>>,
    ) -> impl Fn(&Path) -> Result<Vec<u8>, ProgramEvidenceErrorV1> + '_ {
        move |path: &Path| {
            files
                .get(path)
                .cloned()
                .ok_or_else(|| ProgramEvidenceErrorV1::io(format!("missing {}", path.display())))
        }
    }

    fn read_all(manifest: &ProgramEvidenceManifestV1) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        for source in manifest.engines.values() {
            for path in source.files.values() {
                let bytes = std::fs::read(path).expect("read artifact");
                files.insert(path.clone(), bytes);
            }
        }
        files
    }

    /// Re-seals an artifact file after mutation so the declared sha/bytes
    /// again bind the (mutated) exact bytes.
    fn rewrite_artifact(path: &Path, value: Value) {
        std::fs::write(path, sealed_artifact_bytes(value)).unwrap();
    }

    #[test]
    fn valid_evidence_assembles_into_a_verified_receipt() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let files = read_all(&manifest);
        let receipt = assemble_program_evidence(&manifest, loader_for(&files)).expect("assembles");
        receipt.verify().expect("receipt verifies");
        assert_eq!(receipt.engines.len(), 3);
        assert_eq!(receipt.source_repository_heads.len(), 4);
        assert_eq!(receipt.source_repository_heads[0].repository, "ZeroStack");
        assert_eq!(receipt.source_repository_heads[0].head, hub_head());
        assert_ne!(receipt.program_digest, DigestV1::ZERO);
        // The aggregate program digest is derived from real proof digests.
        let again = assemble_program_evidence(&manifest, loader_for(&files)).expect("assembles");
        assert_eq!(again.program_digest, receipt.program_digest);
    }

    #[test]
    fn derived_program_digest_changes_with_real_evidence() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let files = read_all(&manifest);
        let baseline = assemble_program_evidence(&manifest, loader_for(&files)).unwrap();
        // Different planner plan digest for TokenZero -> different proof ->
        // different derived aggregate digest (never a fixed success digest).
        let changed = manifest.clone();
        let path = changed.engines["tz"].files["planner"].clone();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["report"] = serde_json::to_value(&PlannerReport::new(
            1,
            program_id(),
            sha256(b"different-plan"),
            3,
        ))
        .unwrap();
        rewrite_artifact(&path, value);
        let files = read_all(&changed);
        let derived = assemble_program_evidence(&changed, loader_for(&files)).unwrap();
        assert_ne!(derived.program_digest, baseline.program_digest);
    }

    #[test]
    fn missing_engine_can_never_assemble() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.engines.remove("tz");
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::MissingEngine
        );
    }

    #[test]
    fn unknown_engine_can_never_assemble() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.engines.insert(
            "xx".into(),
            engine_source(
                &engine_head(),
                files_for("xx", 1, &source_head(), &hub_head(), base.path()),
            ),
        );
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::UnknownEngine
        );
    }

    #[test]
    fn missing_evidence_class_can_never_assemble() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.engines.get_mut("fz").unwrap().files.remove("gc");
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::MissingEvidenceClass
        );
    }

    #[test]
    fn unknown_evidence_class_can_never_assemble() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        let existing = manifest.engines["fz"].files["planner"].clone();
        manifest
            .engines
            .get_mut("fz")
            .unwrap()
            .files
            .insert("telemetry".into(), existing);
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::UnknownEvidenceClass
        );
    }

    #[test]
    fn partial_evidence_artifact_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        // The manifest names the artifact but the loader cannot produce it.
        let mut files = read_all(&manifest);
        files.remove(&manifest.engines["gz"].files["lifecycle"].clone());
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::ArtifactIo
        );
    }

    #[test]
    fn stale_hub_head_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        // Re-collect one artifact bound to a different hub head.
        let path = manifest.engines["fz"].files["mcp"].clone();
        let bytes = artifact_bytes(
            EvidenceClassV1::Mcp,
            &report_values(1)["mcp"],
            &manifest.source_head,
            &head(0x99),
        );
        std::fs::write(&path, &bytes).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::StaleHead
        );
    }

    #[test]
    fn stale_source_head_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["gz"].files["worker"].clone();
        let bytes = artifact_bytes(
            EvidenceClassV1::Worker,
            &report_values(2)["worker"],
            &head(0x88),
            &manifest.hub_head,
        );
        std::fs::write(&path, &bytes).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::StaleHead
        );
    }

    #[test]
    fn tampered_artifact_digest_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["tz"].files["gc"].clone();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(b'\n'); // tamper after collection: declared digest no longer binds
        std::fs::write(&path, &bytes).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::ArtifactDigestMismatch
        );
    }

    #[test]
    fn same_length_noncanonical_artifact_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["tz"].files["gc"].clone();
        let original = std::fs::read(&path).unwrap();
        let value: Value = serde_json::from_slice(&original).unwrap();
        let keys = [
            "schema_version",
            "contract",
            "source_head",
            "hub_head",
            "artifact_sha256",
            "artifact_bytes",
            "report",
        ];
        let fields: Vec<String> = keys
            .iter()
            .map(|key| {
                format!(
                    "{}:{}",
                    serde_json::to_string(key).unwrap(),
                    canonical_json(&value[*key])
                )
            })
            .collect();
        let reordered = format!("{{{}}}", fields.join(",")).into_bytes();
        assert_eq!(reordered.len(), original.len());
        assert_ne!(reordered, original);
        std::fs::write(&path, reordered).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::NonCanonicalArtifact
        );
    }

    #[test]
    fn contract_mismatch_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        // A planner slot claiming the worker contract.
        let path = manifest.engines["fz"].files["planner"].clone();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["contract"] = json!("zerostack.program.worker.v1");
        rewrite_artifact(&path, value);
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::ContractMismatch
        );
    }

    #[test]
    fn malformed_report_shape_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["fz"].files["lifecycle"].clone();
        let bytes = artifact_bytes(
            EvidenceClassV1::Lifecycle,
            &json!({}),
            &manifest.source_head,
            &manifest.hub_head,
        );
        std::fs::write(&path, &bytes).unwrap();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::MalformedReport
        );
    }

    #[test]
    fn forged_report_digest_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["fz"].files["planner"].clone();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // Flip one digest byte: the report no longer binds its own fields.
        let digest = value["report"]["digest"].as_array().unwrap().clone();
        let mut flipped = digest;
        flipped[0] = json!(flipped[0].as_u64().unwrap() ^ 0xff);
        value["report"]["digest"] = Value::Array(flipped);
        rewrite_artifact(&path, value);
        let files = read_all(&manifest);
        let error = assemble_program_evidence(&manifest, loader_for(&files)).unwrap_err();
        match error.failure_code() {
            ProgramEvidenceFailureV1::ProgramAssembly(ProgramAssemblyError::MalformedReport(_)) => {
            }
            other => panic!("expected malformed report assembly failure, got {other:?}"),
        }
    }

    #[test]
    fn artifact_schema_version_mismatch_fails_closed() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let path = manifest.engines["fz"].files["planner"].clone();
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["schema_version"] = json!(2);
        rewrite_artifact(&path, value);
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::SchemaVersionMismatch
        );
    }

    #[test]
    fn manifest_version_mismatch_fails_closed() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.version = 2;
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::ManifestVersionMismatch
        );
    }

    #[test]
    fn invalid_manifest_heads_fail_closed() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.hub_head = "not-a-head".into();
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::InvalidHead
        );
    }

    #[test]
    fn invalid_assembly_manifest_digest_fails_closed() {
        let base = TempDir::new().unwrap();
        let mut manifest = valid_manifest(base.path());
        manifest.assembly_manifest_digest = "zz".repeat(32);
        let files = read_all(&manifest);
        assert_eq!(
            assemble_program_evidence(&manifest, loader_for(&files))
                .unwrap_err()
                .failure_code(),
            &ProgramEvidenceFailureV1::InvalidAssemblyManifestDigest
        );
    }

    #[test]
    fn manifest_round_trips_canonically() {
        let base = TempDir::new().unwrap();
        let manifest = valid_manifest(base.path());
        let bytes = manifest.canonical_bytes().unwrap();
        let decoded = ProgramEvidenceManifestV1::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(decoded, manifest);
        // Noncanonical key order is rejected.
        let text = String::from_utf8(bytes)
            .unwrap()
            .replace("\"version\":1", "\"version\": 1");
        assert!(ProgramEvidenceManifestV1::from_canonical_bytes(text.as_bytes()).is_err());
    }

    #[test]
    fn valid_head_rejects_non_lowercase_hex() {
        assert!(valid_head(&"a".repeat(40)));
        assert!(valid_head(&"b".repeat(64)));
        assert!(!valid_head(&"A".repeat(40)));
        assert!(!valid_head(&"a".repeat(39)));
        assert!(!valid_head(&"g".repeat(40)));
    }
