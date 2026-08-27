use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use graphzero_store::store::durability_receipt::{
    CanonicalSurfaceBytes, DurabilityEvidenceInput, DurabilityMetadata, DurabilityReceipt,
    DurabilityReceiptAdapter, DurabilityReceiptExpectation, REQUIRED_FEEDER_IDS, ReceiptStatus,
};
use graphzero_store::store::indexer::{global_file_name, shard_file_name};
use graphzero_store::store::manifest::{Manifest, SnapshotEntry};
use graphzero_store::store::shard::file_hash64;
use graphzero_store::{BlobStore, ContentHash};
use serde_json::{Map, Value, json};
use tempfile::tempdir;
use zero_abi::{Sha256Digest, canonical_json};
use zero_store::{DurableProfileId, JournalBoundary, JournalError, JournalFailureCode};

const GLOBAL: &[u8] = b"global-artifact";
const SHARD: &[u8] = b"shard-artifact";
const EVIDENCE: &[u8] = b"evidence";
const RECOVERY_CHILD_ROOT: &str = "GRAPHZERO_DURABILITY_RECOVERY_CHILD_ROOT";
const RECOVERY_RECEIPT_FILE: &str = ".fresh-process-receipt.json";

fn digest(byte: u8) -> Sha256Digest {
    Sha256Digest::from_bytes([byte; 32])
}

fn evidence_input() -> DurabilityEvidenceInput {
    let hash = ContentHash::of(EVIDENCE);
    let surface = CanonicalSurfaceBytes {
        request_bytes: canonical_json(&json!({"op":"durability"})).into_bytes(),
        result_bytes: canonical_json(&json!({"ok":true})).into_bytes(),
        error_bytes: canonical_json(&json!({"error":null})).into_bytes(),
        ref_bytes: canonical_json(&json!([format!("gz://blob/{}", hash.to_hex())])).into_bytes(),
        source_is_fallback: false,
    };
    let metadata = DurabilityMetadata {
        source_revision: "graphzero-test-revision".into(),
        build_identity: "debug-test-build".into(),
        fixture_identity: "durability-receipt-fixture-v1".into(),
        verifier_identity: "graphzero-durability-verifier-v1".into(),
        assembly_manifest_digest: digest(2),
        owner_identity_digest: digest(3),
        durable_profile_id: DurableProfileId::PortableStrict,
        required_feeder_ids: REQUIRED_FEEDER_IDS
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        available_feeder_ids: REQUIRED_FEEDER_IDS
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
    };
    let candidate_manifest = Manifest {
        snapshots: vec![SnapshotEntry {
            snapshot_id: 1,
            timestamp_nanos: 7,
            global_hash: file_hash64(GLOBAL),
            shard_hashes: vec![file_hash64(SHARD)],
            segment_ids: vec![],
        }],
    };
    let expectation = DurabilityReceiptExpectation::new(&metadata, &surface);
    DurabilityEvidenceInput {
        transaction_id: digest(1),
        metadata,
        surface,
        candidate_manifest,
        expectation,
    }
}

fn fixture() -> (tempfile::TempDir, DurabilityEvidenceInput) {
    let root = tempdir().unwrap();
    fs::create_dir_all(root.path().join("shards")).unwrap();
    fs::write(root.path().join("shards").join(global_file_name(1)), GLOBAL).unwrap();
    fs::write(
        root.path().join("shards").join(shard_file_name(1, 0)),
        SHARD,
    )
    .unwrap();
    let hash = BlobStore::open(root.path()).unwrap().put(EVIDENCE).unwrap();
    assert_eq!(hash, ContentHash::of(EVIDENCE));
    (root, evidence_input())
}

#[test]
fn commit_reopen_and_replay_are_store_verified() {
    let (root, input) = fixture();
    let adapter = DurabilityReceiptAdapter::open(root.path());
    let expectation = input.expectation.clone();
    let candidate = input.candidate_manifest.clone();
    let receipt = adapter.prepare(input).unwrap().commit(&candidate).unwrap();

    assert!(receipt.store_verified);
    assert!(!receipt.native_promotable);
    let bytes = receipt.canonical_bytes().unwrap();
    assert_eq!(
        receipt,
        DurabilityReceipt::from_canonical_bytes(&bytes).unwrap()
    );
    receipt.verify(root.path(), &expectation).unwrap();
    receipt.verify(root.path(), &expectation).unwrap();
    assert_eq!(receipt.digest().unwrap(), receipt.digest().unwrap());
}

fn recover_in_child(root: &Path) -> DurabilityReceipt {
    let status = Command::new(env::current_exe().unwrap())
        .args(["--exact", "fresh_process_recovery_child", "--nocapture"])
        .env(RECOVERY_CHILD_ROOT, root)
        .status()
        .unwrap();
    assert!(status.success(), "fresh recovery process failed: {status}");
    let bytes = fs::read(root.join(RECOVERY_RECEIPT_FILE)).unwrap();
    DurabilityReceipt::from_canonical_bytes(&bytes).unwrap()
}

#[test]
fn crash_after_manifest_before_journal_commit_recovers_in_fresh_process() {
    let (root, input) = fixture();
    let adapter = DurabilityReceiptAdapter::open(root.path());
    let expectation = input.expectation.clone();
    let candidate = input.candidate_manifest.clone();
    let prepared = adapter.prepare(input).unwrap();
    let mut fault = zero_store::FaultPlan::crash_at(JournalBoundary::CommitBeforeWrite);

    assert!(prepared.commit_with_fault(&candidate, &mut fault).is_err());
    assert_eq!(Manifest::load(root.path()).unwrap(), candidate);

    let receipt = recover_in_child(root.path());
    assert!(receipt.store_verified);
    assert!(!receipt.native_promotable);
    assert!(receipt.owner_death.is_some());
    receipt.verify(root.path(), &expectation).unwrap();

    let mut missing_owner_death = receipt.clone();
    missing_owner_death.owner_death = None;
    assert!(
        missing_owner_death
            .verify(root.path(), &expectation)
            .is_err()
    );
}

#[test]
fn crash_after_manifest_before_root_publish_aborts_and_rolls_back_in_fresh_process() {
    let (root, input) = fixture();
    let adapter = DurabilityReceiptAdapter::open(root.path());
    let expectation = input.expectation.clone();
    let candidate = input.candidate_manifest.clone();
    let prepared = adapter.prepare(input).unwrap();
    let mut fault = zero_store::FaultPlan::crash_at(JournalBoundary::RootPublishBeforeWrite);

    assert!(prepared.commit_with_fault(&candidate, &mut fault).is_err());
    assert_eq!(Manifest::load(root.path()).unwrap(), candidate);

    let receipt = recover_in_child(root.path());
    assert_eq!(receipt.status, ReceiptStatus::RecoveredAborted);
    assert!(receipt.owner_death.is_some());
    assert_eq!(Manifest::load(root.path()).unwrap(), Manifest::default());
    receipt.verify(root.path(), &expectation).unwrap();
}

#[test]
fn fresh_process_recovery_child() {
    let Some(root) = env::var_os(RECOVERY_CHILD_ROOT).map(PathBuf::from) else {
        return;
    };
    let input = evidence_input();
    let expectation = input.expectation.clone();
    let receipt = DurabilityReceiptAdapter::open(&root)
        .resume_and_recover(input, &expectation, 123)
        .unwrap();
    fs::write(
        root.join(RECOVERY_RECEIPT_FILE),
        receipt.canonical_bytes().unwrap(),
    )
    .unwrap();
}

#[test]
fn abort_fallback_missing_feeder_and_unknown_refs_are_rejected() {
    let (root, mut input) = fixture();
    input.surface.source_is_fallback = true;
    input.expectation = DurabilityReceiptExpectation::new(&input.metadata, &input.surface);
    assert!(
        DurabilityReceiptAdapter::open(root.path())
            .prepare(input)
            .is_err()
    );

    let (root, mut input) = fixture();
    input.metadata.available_feeder_ids.clear();
    input.expectation = DurabilityReceiptExpectation::new(&input.metadata, &input.surface);
    assert!(
        DurabilityReceiptAdapter::open(root.path())
            .prepare(input)
            .is_err()
    );

    let (root, mut input) = fixture();
    input.surface.ref_bytes = canonical_json(&json!(["gz://snap/1"])).into_bytes();
    input.expectation = DurabilityReceiptExpectation::new(&input.metadata, &input.surface);
    assert!(
        DurabilityReceiptAdapter::open(root.path())
            .prepare(input)
            .is_err()
    );

    let (root, input) = fixture();
    let receipt = DurabilityReceiptAdapter::open(root.path())
        .prepare(input)
        .unwrap()
        .abort()
        .unwrap();
    assert!(receipt.store_verified);
    assert!(!receipt.native_promotable);
}

#[derive(Clone, Debug)]
enum PathStep {
    Key(String),
    Index(usize),
}

fn object_field_paths(value: &Value) -> Vec<Vec<PathStep>> {
    fn visit(value: &Value, prefix: &mut Vec<PathStep>, output: &mut Vec<Vec<PathStep>>) {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    prefix.push(PathStep::Key(key.clone()));
                    output.push(prefix.clone());
                    visit(child, prefix, output);
                    prefix.pop();
                }
            }
            Value::Array(array) => {
                for (index, child) in array.iter().enumerate() {
                    prefix.push(PathStep::Index(index));
                    visit(child, prefix, output);
                    prefix.pop();
                }
            }
            _ => {}
        }
    }

    let mut output = Vec::new();
    visit(value, &mut Vec::new(), &mut output);
    output
}

fn value_at_mut<'a>(mut value: &'a mut Value, path: &[PathStep]) -> &'a mut Value {
    for step in path {
        value = match step {
            PathStep::Key(key) => value.as_object_mut().unwrap().get_mut(key).unwrap(),
            PathStep::Index(index) => &mut value.as_array_mut().unwrap()[*index],
        };
    }
    value
}

fn remove_field(value: &mut Value, path: &[PathStep]) {
    let (field, parent_path) = path.split_last().unwrap();
    let PathStep::Key(key) = field else {
        unreachable!("collected paths always end in object fields")
    };
    let parent = value_at_mut(value, parent_path);
    parent.as_object_mut().unwrap().remove(key).unwrap();
}

fn alter_field(value: &mut Value, path: &[PathStep]) {
    let field = value_at_mut(value, path);
    match field {
        Value::Null => *field = Value::Bool(true),
        Value::Bool(value) => *value = !*value,
        Value::Number(number) => {
            *field = Value::from(number.as_u64().unwrap_or(0).saturating_add(1));
        }
        Value::String(value) => value.push_str("-mutant"),
        Value::Array(array) => {
            if array.is_empty() {
                array.push(Value::Null);
            } else {
                array.pop();
            }
        }
        Value::Object(object) => {
            object.insert("__mutant".into(), Value::Bool(true));
        }
    }
}

fn assert_receipt_rejected(
    value: &Value,
    root: &Path,
    expectation: &DurabilityReceiptExpectation,
    field: &[PathStep],
) {
    let bytes = canonical_json(value).into_bytes();
    if let Ok(receipt) = DurabilityReceipt::from_canonical_bytes(&bytes) {
        assert!(
            receipt.verify(root, expectation).is_err(),
            "mutant field was accepted: {field:?}"
        );
    }
}

#[test]
fn every_receipt_field_omission_and_alteration_is_rejected() {
    let (root, input) = fixture();
    let adapter = DurabilityReceiptAdapter::open(root.path());
    let expectation = input.expectation.clone();
    let candidate = input.candidate_manifest.clone();
    let receipt = adapter.prepare(input).unwrap().commit(&candidate).unwrap();
    let value: Value = serde_json::from_slice(&receipt.canonical_bytes().unwrap()).unwrap();
    let paths = object_field_paths(&value);
    assert!(
        paths.len() >= 50,
        "unexpectedly shallow receipt: {}",
        paths.len()
    );

    for path in paths {
        let mut omitted = value.clone();
        remove_field(&mut omitted, &path);
        assert_receipt_rejected(&omitted, root.path(), &expectation, &path);

        let mut altered = value.clone();
        alter_field(&mut altered, &path);
        assert_receipt_rejected(&altered, root.path(), &expectation, &path);
    }
}

#[test]
fn prepare_crash_is_incomplete_until_restart_recovery() {
    let (root, input) = fixture();
    let adapter = DurabilityReceiptAdapter::open(root.path());
    let expectation = input.expectation.clone();
    let mut fault = zero_store::FaultPlan::crash_at(JournalBoundary::PrepareBeforeWrite);
    let error = adapter
        .prepare_with_fault(input.clone(), &mut fault)
        .err()
        .unwrap();
    assert!(error.chain().any(|cause| {
        cause
            .downcast_ref::<JournalError>()
            .is_some_and(|journal| journal.code == JournalFailureCode::InjectedCrash)
    }));

    let receipt = adapter
        .resume_and_recover(input, &expectation, 124)
        .unwrap();
    assert_eq!(receipt.status, ReceiptStatus::RecoveredAborted);
    assert!(receipt.owner_death.is_none());
}

#[test]
fn receipt_top_level_unknown_field_is_rejected() {
    let (root, input) = fixture();
    let adapter = DurabilityReceiptAdapter::open(root.path());
    let candidate = input.candidate_manifest.clone();
    let receipt = adapter.prepare(input).unwrap().commit(&candidate).unwrap();
    let mut value: Value = serde_json::to_value(receipt).unwrap();
    let object: &mut Map<String, Value> = value.as_object_mut().unwrap();
    object.insert("native_receipt".into(), Value::Bool(true));
    assert!(DurabilityReceipt::from_canonical_bytes(canonical_json(&value).as_bytes()).is_err());
}
