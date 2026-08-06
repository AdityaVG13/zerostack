//! AssemblyManifestV1 and ZBF v1 known-answer conformance vectors.

use serde::{Deserialize, Serialize};
use zero_abi::{
    ASSEMBLY_ABI_CONTRACT_VERSION, ASSEMBLY_MANIFEST_SCHEMA_VERSION, ArtifactOwnerV1,
    AssemblyManifestV1, DigestV1, EngineIdentity, LinkedArtifactV1, LinkedProfileV1,
    PlatformIdentityV1, ProfileKindV1, ReceiptSchemaIdentityV1, TargetIdentityV1,
    VerifierIdentityV1, WorkerIdentityV1, assembly_abi_contract_digest_v1, canonical_json,
    sha256_hex,
};
use zero_store::{DurableProfileV1, ZBF_HEADER_LEN_V1, ZbfArtifactKindV1, ZbfObjectV1};

pub const KAT_VECTOR_SET_V1: &str = "zerostack.assembly-zbf-kat.v1";
pub const KAT_FIXTURE_RELATIVE_DIR: &str = "conformance/assembly-zbf/v1";
pub const KAT_EXPECTED_FAILURE: &str = "fixture_digest_mismatch";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PositiveVectorV1 {
    pub id: String,
    pub file: String,
    pub byte_len: u64,
    pub sha256: String,
    pub semantic_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MutationRowV1 {
    pub id: String,
    pub field: String,
    pub mutation: String,
    pub expected_failure: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerSourceV1 {
    pub language: String,
    pub verifier_version: u16,
    pub file: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KatVectorIndexV1 {
    pub schema_version: u16,
    pub vector_set: String,
    pub evidence_scope: String,
    pub assembly_manifest: PositiveVectorV1,
    pub zbf_leaf: PositiveVectorV1,
    pub zbf_container: PositiveVectorV1,
    pub manifest_mutations: Vec<MutationRowV1>,
    pub zbf_mutations: Vec<MutationRowV1>,
    pub runners: Vec<RunnerSourceV1>,
}

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

pub fn assembly_manifest_kat_v1() -> AssemblyManifestV1 {
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

pub fn zbf_leaf_kat_v1() -> ZbfObjectV1 {
    ZbfObjectV1::new_leaf(
        ZbfArtifactKindV1::Plan,
        ArtifactOwnerV1::ZeroStack,
        digest(1),
        DurableProfileV1::portable_strict(),
        digest(2),
        digest(3),
        b"canonical payload".to_vec(),
    )
    .expect("static ZBF leaf fixture must be valid")
}

pub fn zbf_container_kat_v1() -> ZbfObjectV1 {
    ZbfObjectV1::new_container(
        ZbfArtifactKindV1::Snapshot,
        ArtifactOwnerV1::ZeroStack,
        digest(1),
        DurableProfileV1::portable_strict(),
        digest(4),
        digest(5),
        vec![zbf_leaf_kat_v1()],
    )
    .expect("static ZBF container fixture must be valid")
}

pub fn manifest_mutation_fields_v1() -> Vec<&'static str> {
    vec![
        "/schema_version",
        "/required_abi_contract_version",
        "/abi_contract_digest",
        "/linked_artifacts",
        "/linked_artifacts/0/artifact_id",
        "/linked_artifacts/0/owner",
        "/linked_artifacts/0/artifact_version",
        "/linked_artifacts/0/source_repository",
        "/linked_artifacts/0/source_revision",
        "/linked_artifacts/0/artifact_digest",
        "/linked_artifacts/0/contract_digest",
        "/linked_profiles",
        "/linked_profiles/0/profile_kind",
        "/linked_profiles/0/profile_id",
        "/linked_profiles/0/profile_version",
        "/linked_profiles/0/profile_digest",
        "/target",
        "/target/target_triple",
        "/target/architecture",
        "/target/operating_system",
        "/target/abi",
        "/platform",
        "/platform/profile_id",
        "/platform/profile_version",
        "/platform/profile_digest",
        "/verifiers",
        "/verifiers/0/verifier_id",
        "/verifiers/0/verifier_version",
        "/verifiers/0/verifier_digest",
        "/receipt_schema",
        "/receipt_schema/schema_id",
        "/receipt_schema/schema_version",
        "/receipt_schema/schema_digest",
        "/runtime_generation",
        "/assembly_epoch",
        "/workers",
        "/workers/0/engine",
        "/workers/0/artifact_digest",
        "/workers/0/worker_protocol_digest",
        "/workers/0/semantic_contract_digest",
        "/workers/0/operation_registry_digest",
        "/workers/0/capability_catalog_digest",
        "/aggregate_capability_catalog_digest",
        "@field_omission",
        "@field_reorder",
        "@bound_overflow",
    ]
}

pub fn zbf_mutation_fields_v1() -> Vec<(&'static str, Option<u64>)> {
    vec![
        ("magic", Some(0)),
        ("schema_major", Some(8)),
        ("schema_minor", Some(10)),
        ("artifact_kind", Some(12)),
        ("owner", Some(14)),
        ("flags", Some(15)),
        ("payload_len", Some(16)),
        ("assembly_manifest_digest", Some(24)),
        ("durable_profile_digest", Some(56)),
        ("source_root_digest", Some(88)),
        ("producer_contract_digest", Some(120)),
        ("payload_digest", Some(152)),
        ("reserved", Some(184)),
        ("payload", Some(ZBF_HEADER_LEN_V1 as u64)),
        ("@trailing_bytes", None),
        ("@torn_write", None),
        ("@deep_nesting", None),
    ]
}

fn mutation_id(prefix: &str, field: &str) -> String {
    let normalized = field
        .trim_start_matches(['/', '@'])
        .replace(['/', '@'], ".");
    format!("{prefix}.{normalized}")
}

pub fn vector_index_v1(runners: Vec<RunnerSourceV1>) -> KatVectorIndexV1 {
    let profile = DurableProfileV1::portable_strict();
    let manifest_bytes = assembly_manifest_kat_v1()
        .canonical_bytes()
        .expect("static manifest fixture must encode");
    let leaf_bytes = zbf_leaf_kat_v1()
        .to_bytes(profile)
        .expect("static leaf fixture must encode");
    let container_bytes = zbf_container_kat_v1()
        .to_bytes(profile)
        .expect("static container fixture must encode");
    let manifest_mutations = manifest_mutation_fields_v1()
        .into_iter()
        .map(|field| MutationRowV1 {
            id: mutation_id("manifest", field),
            field: field.into(),
            mutation: if field.starts_with('@') {
                field.trim_start_matches('@').into()
            } else {
                "value_substitution".into()
            },
            expected_failure: KAT_EXPECTED_FAILURE.into(),
            byte_offset: None,
        })
        .collect();
    let zbf_mutations = zbf_mutation_fields_v1()
        .into_iter()
        .map(|(field, byte_offset)| MutationRowV1 {
            id: mutation_id("zbf", field),
            field: field.into(),
            mutation: if field.starts_with('@') {
                field.trim_start_matches('@').into()
            } else {
                "bit_flip".into()
            },
            expected_failure: KAT_EXPECTED_FAILURE.into(),
            byte_offset,
        })
        .collect();
    KatVectorIndexV1 {
        schema_version: 1,
        vector_set: KAT_VECTOR_SET_V1.into(),
        evidence_scope: "cross_language_kat_only; rch_is_not_native_evidence".into(),
        assembly_manifest: PositiveVectorV1 {
            id: "assembly_manifest.v1".into(),
            file: "assembly-manifest-v1.json".into(),
            byte_len: manifest_bytes.len() as u64,
            sha256: sha256_hex(&manifest_bytes),
            semantic_digest: assembly_manifest_kat_v1()
                .digest()
                .expect("static manifest fixture must digest")
                .to_hex(),
        },
        zbf_leaf: PositiveVectorV1 {
            id: "zbf.leaf.v1".into(),
            file: "zbf-leaf-v1.bin".into(),
            byte_len: leaf_bytes.len() as u64,
            sha256: sha256_hex(&leaf_bytes),
            semantic_digest: sha256_hex(&leaf_bytes),
        },
        zbf_container: PositiveVectorV1 {
            id: "zbf.container.v1".into(),
            file: "zbf-container-v1.bin".into(),
            byte_len: container_bytes.len() as u64,
            sha256: sha256_hex(&container_bytes),
            semantic_digest: sha256_hex(&container_bytes),
        },
        manifest_mutations,
        zbf_mutations,
        runners,
    }
}

pub fn canonical_index_bytes_v1(index: &KatVectorIndexV1) -> Vec<u8> {
    let value = serde_json::to_value(index).expect("KAT index must serialize");
    canonical_json(&value).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::{collections::BTreeSet, fs, path::PathBuf, process::Command};
    use tempfile::tempdir;
    use zero_abi::{AssemblyManifestV1, validate_assembly_pre_dispatch_v1};
    use zero_store::{DurableProfileIdV1, ZbfFailureCodeV1};

    fn fixture_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(KAT_FIXTURE_RELATIVE_DIR)
    }

    fn load_index() -> KatVectorIndexV1 {
        let bytes = fs::read(fixture_dir().join("index.json")).unwrap();
        let index: KatVectorIndexV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(bytes, canonical_index_bytes_v1(&index));
        index
    }

    fn assert_vector(vector: &PositiveVectorV1) -> Vec<u8> {
        let bytes = fs::read(fixture_dir().join(&vector.file)).unwrap();
        assert_eq!(bytes.len() as u64, vector.byte_len);
        assert_eq!(sha256_hex(&bytes), vector.sha256);
        bytes
    }

    fn mutate_value(value: &mut Value, pointer: &str) {
        let target = value
            .pointer_mut(pointer)
            .expect("mutation pointer must exist");
        match target {
            Value::String(text) => {
                if text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    let replacement = if text.starts_with('0') { "1" } else { "0" };
                    text.replace_range(0..1, replacement);
                } else {
                    text.push_str(".mutant");
                }
            }
            Value::Number(number) => {
                *number = serde_json::Number::from(number.as_u64().unwrap() + 1);
            }
            Value::Array(items) if items.len() > 1 => items.reverse(),
            Value::Array(items) => items.clear(),
            Value::Object(map) => {
                map.insert("unexpected_mutant".into(), Value::Bool(true));
            }
            Value::Bool(boolean) => *boolean = !*boolean,
            Value::Null => *target = Value::Bool(true),
        }
    }

    fn reordered_top_level(bytes: &[u8]) -> Vec<u8> {
        let text = std::str::from_utf8(bytes).unwrap();
        let body = text.strip_prefix('{').unwrap().strip_suffix('}').unwrap();
        let first_end = body.find(',').unwrap();
        let first = &body[..first_end];
        let remaining = &body[first_end + 1..];
        let second_end = remaining.find(',').unwrap();
        let second = &remaining[..second_end];
        let rest = &remaining[second_end + 1..];
        format!("{{{second},{first},{rest}}}").into_bytes()
    }

    fn compile_c_runner() -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempdir().unwrap();
        let binary = dir.path().join("assembly-zbf-c-v1");
        let source = fixture_dir().join("runners/c/verify_v1.c");
        let output = Command::new("cc")
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-Werror"])
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "C runner compile failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let binary_hash = sha256_hex(&fs::read(&binary).unwrap());
        (dir, binary, binary_hash)
    }

    fn vector_args(index: &KatVectorIndexV1) -> Vec<String> {
        let root = fixture_dir();
        vec![
            root.join(&index.assembly_manifest.file)
                .display()
                .to_string(),
            index.assembly_manifest.sha256.clone(),
            index.assembly_manifest.semantic_digest.clone(),
            root.join(&index.zbf_leaf.file).display().to_string(),
            index.zbf_leaf.sha256.clone(),
            root.join(&index.zbf_container.file).display().to_string(),
            index.zbf_container.sha256.clone(),
        ]
    }

    fn run_c(index: &KatVectorIndexV1) -> String {
        let (_dir, binary, binary_hash) = compile_c_runner();
        assert_eq!(binary_hash.len(), 64);
        let output = Command::new(binary)
            .args(vector_args(index))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "C runner failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn run_python(script: &str) -> String {
        let source = fixture_dir().join(script);
        let source_hash = sha256_hex(&fs::read(&source).unwrap());
        assert_eq!(source_hash.len(), 64);
        let executable = Command::new("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .unwrap();
        assert!(executable.status.success());
        let executable_path = String::from_utf8(executable.stdout).unwrap();
        let executable_hash = sha256_hex(&fs::read(executable_path.trim()).unwrap());
        assert_eq!(executable_hash.len(), 64);
        let output = Command::new("python3")
            .arg(source)
            .arg(fixture_dir())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Python runner failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn assert_cross_language(index: &KatVectorIndexV1) {
        assert_eq!(run_c(index).trim(), "assembly_zbf_kat:c:v1:passed");
        assert_eq!(
            run_python("runners/python/verify_v1.py").trim(),
            "assembly_zbf_kat:python:v1:passed"
        );
        assert_eq!(
            run_python("runners/python/verify_v0.py").trim(),
            "assembly_zbf_kat:python:v0:passed"
        );
    }

    #[test]
    fn assembly_kat_cross_language_replays_n_and_n_minus_one() {
        let index = load_index();
        let manifest_bytes = assert_vector(&index.assembly_manifest);
        let manifest = AssemblyManifestV1::from_canonical_bytes(&manifest_bytes).unwrap();
        assert_eq!(manifest, assembly_manifest_kat_v1());
        assert_eq!(
            manifest.digest().unwrap().to_hex(),
            index.assembly_manifest.semantic_digest
        );
        assert_cross_language(&index);
    }

    #[test]
    fn assembly_kat_every_manifest_field_mutation_rejects() {
        let index = load_index();
        let base_bytes = assert_vector(&index.assembly_manifest);
        let base = assembly_manifest_kat_v1();
        let expectation = base.expectation().unwrap();
        let expected_fields = manifest_mutation_fields_v1()
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let indexed_fields = index
            .manifest_mutations
            .iter()
            .map(|row| row.field.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(indexed_fields, expected_fields);
        assert_eq!(index.manifest_mutations.len(), expected_fields.len());

        for row in &index.manifest_mutations {
            assert_eq!(row.expected_failure, KAT_EXPECTED_FAILURE);
            let mutant_bytes = match row.field.as_str() {
                "@field_omission" => {
                    let mut value = serde_json::to_value(&base).unwrap();
                    value.as_object_mut().unwrap().remove("target");
                    canonical_json(&value).into_bytes()
                }
                "@field_reorder" => reordered_top_level(&base_bytes),
                "@bound_overflow" => {
                    let mut value = serde_json::to_value(&base).unwrap();
                    value["linked_artifacts"][0]["artifact_id"] = Value::String("x".repeat(513));
                    canonical_json(&value).into_bytes()
                }
                pointer => {
                    let mut value = serde_json::to_value(&base).unwrap();
                    mutate_value(&mut value, pointer);
                    canonical_json(&value).into_bytes()
                }
            };
            assert_ne!(
                sha256_hex(&mutant_bytes),
                index.assembly_manifest.sha256,
                "{}",
                row.id
            );
            match AssemblyManifestV1::from_canonical_bytes(&mutant_bytes) {
                Ok(mutant) => assert!(
                    validate_assembly_pre_dispatch_v1(&mutant, &expectation).is_err(),
                    "{} was accepted",
                    row.id
                ),
                Err(error) => assert!(!error.to_string().is_empty(), "{}", row.id),
            }
        }
    }

    #[test]
    fn zbf_kat_cross_language_vectors_are_byte_exact() {
        let index = load_index();
        let profile = DurableProfileV1::portable_strict();
        let leaf_bytes = assert_vector(&index.zbf_leaf);
        let container_bytes = assert_vector(&index.zbf_container);
        assert_eq!(leaf_bytes, zbf_leaf_kat_v1().to_bytes(profile).unwrap());
        assert_eq!(
            container_bytes,
            zbf_container_kat_v1().to_bytes(profile).unwrap()
        );
        assert_eq!(
            ZbfObjectV1::from_bytes(&leaf_bytes, digest(1), profile).unwrap(),
            zbf_leaf_kat_v1()
        );
        assert_eq!(
            ZbfObjectV1::from_bytes(&container_bytes, digest(1), profile).unwrap(),
            zbf_container_kat_v1()
        );
        assert_cross_language(&index);
    }

    #[test]
    fn zbf_kat_every_header_and_bound_mutation_rejects() {
        let index = load_index();
        let base = assert_vector(&index.zbf_leaf);
        let expected_fields = zbf_mutation_fields_v1()
            .into_iter()
            .map(|(field, _)| field.to_owned())
            .collect::<BTreeSet<_>>();
        let indexed_fields = index
            .zbf_mutations
            .iter()
            .map(|row| row.field.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(indexed_fields, expected_fields);
        assert_eq!(index.zbf_mutations.len(), expected_fields.len());
        let profile = DurableProfileV1::portable_strict();

        for row in &index.zbf_mutations {
            assert_eq!(row.expected_failure, KAT_EXPECTED_FAILURE);
            match row.field.as_str() {
                "@deep_nesting" => {
                    let mut object = zbf_leaf_kat_v1();
                    let mut rejection = None;
                    for _ in 0..=profile.max_depth() {
                        match ZbfObjectV1::new_container(
                            ZbfArtifactKindV1::Snapshot,
                            ArtifactOwnerV1::ZeroStack,
                            digest(1),
                            profile,
                            digest(2),
                            digest(3),
                            vec![object.clone()],
                        ) {
                            Ok(next) => object = next,
                            Err(error) => {
                                rejection = Some(error);
                                break;
                            }
                        }
                    }
                    assert_eq!(rejection.unwrap().code(), ZbfFailureCodeV1::DepthExceeded);
                }
                "@trailing_bytes" => {
                    let mut mutant = base.clone();
                    mutant.push(0);
                    assert_ne!(sha256_hex(&mutant), index.zbf_leaf.sha256);
                    assert_eq!(
                        ZbfObjectV1::from_bytes(&mutant, digest(1), profile)
                            .unwrap_err()
                            .code(),
                        ZbfFailureCodeV1::TrailingBytes
                    );
                }
                "@torn_write" => {
                    let mut mutant = base.clone();
                    mutant.pop();
                    assert_ne!(sha256_hex(&mutant), index.zbf_leaf.sha256);
                    assert_eq!(
                        ZbfObjectV1::from_bytes(&mutant, digest(1), profile)
                            .unwrap_err()
                            .code(),
                        ZbfFailureCodeV1::UnexpectedEof
                    );
                }
                _ => {
                    let mut mutant = base.clone();
                    let offset = usize::try_from(row.byte_offset.unwrap()).unwrap();
                    mutant[offset] ^= 1;
                    assert_ne!(sha256_hex(&mutant), index.zbf_leaf.sha256, "{}", row.id);
                    if let Ok(decoded) = ZbfObjectV1::from_bytes(&mutant, digest(1), profile) {
                        assert_ne!(decoded, zbf_leaf_kat_v1(), "{}", row.id);
                    }
                }
            }
        }
    }

    #[test]
    fn assembly_kat_index_and_runner_sources_are_immutable() {
        let index = load_index();
        assert_eq!(index.schema_version, 1);
        assert_eq!(index.vector_set, KAT_VECTOR_SET_V1);
        for runner in &index.runners {
            let bytes = fs::read(fixture_dir().join(&runner.file)).unwrap();
            assert_eq!(sha256_hex(&bytes), runner.sha256, "{}", runner.file);
        }
        assert!(index.evidence_scope.contains("rch_is_not_native_evidence"));
        assert_eq!(
            DurableProfileV1::portable_strict().id(),
            DurableProfileIdV1::PortableStrict
        );
    }
}
