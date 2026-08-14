//! Trace-export pipeline tests (ZS-OPS-003 / V6-R14): durable chain-sealed
//! exports fail closed on torn tails and tampering, cache-decision records
//! require their decision-boundary annotation (typed constructor), sealed
//! benchmark manifests round-trip and refuse tampering, and the
//! decision-boundary summary artifact lists every annotated boundary.

use std::fs::OpenOptions;
use std::io::Write;

use tempfile::tempdir;

use zero_abi::{DigestV1, sha256};
use zero_cert::{
    BenchmarkReproducibilityV1, DecisionBoundaryAnnotationV1, DecisionBoundaryKindV1,
    SealedBenchmarkManifestV1, TraceEventKindV1, TraceExportErrorV1, TraceRecordV1,
    append_trace_record_v1, export_benchmark_manifest_v1, export_trace_pipeline_v1,
    open_trace_export_v1, read_exported_benchmark_manifest_v1,
    summarize_decision_boundaries_v1,
};

fn digest(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

fn annotation() -> DecisionBoundaryAnnotationV1 {
    DecisionBoundaryAnnotationV1::new(
        DecisionBoundaryKindV1::CacheAdmission,
        "dependency roots match formation-time set",
        digest(0xAD),
    )
    .unwrap()
}

/// Decision records are chained: each record's parent digest is the
/// previous record's digest.
fn chained_records(count: usize) -> Vec<TraceRecordV1> {
    let mut records = Vec::new();
    let mut parent = None;
    for seq in 0..count {
        let record = if seq == 0 {
            TraceRecordV1::cache_decision(
                seq as u64,
                annotation(),
                format!("fz://blob/dec-{seq}"),
                "cache-gate",
                parent,
            )
            .unwrap()
        } else if seq == 1 {
            TraceRecordV1::new(
                seq as u64,
                TraceEventKindV1::Invalidation,
                Some(DecisionBoundaryAnnotationV1::new(
                    DecisionBoundaryKindV1::Invalidation,
                    "dependency root changed",
                    digest(0x1D + seq as u8),
                )
                .unwrap()),
                format!("fz://blob/inv-{seq}"),
                "invalidation-sweeper",
                parent,
            )
            .unwrap()
        } else {
            TraceRecordV1::new(
                seq as u64,
                TraceEventKindV1::VerificationOutcome,
                Some(DecisionBoundaryAnnotationV1::new(
                    DecisionBoundaryKindV1::VerificationAccepted,
                    "all spans rooted",
                    digest(0x2D + seq as u8),
                )
                .unwrap()),
                format!("fz://blob/ver-{seq}"),
                "verifier",
                parent,
            )
            .unwrap()
        };
        parent = Some(record.record_digest().unwrap());
        records.push(record);
    }
    records
}

/// Full pipeline round trip: export seals a chained batch, read-back
/// replays the identical chain and verifies the sealed head.
#[test]
fn export_pipeline_seals_and_readback_verifies() {
    let directory = tempdir().unwrap();
    let records = chained_records(4);

    let receipt = export_trace_pipeline_v1(directory.path(), &records).unwrap();
    assert_eq!(receipt.records, 4);
    assert!(receipt.sealed);
    assert_ne!(receipt.head, DigestV1::ZERO);
    assert_ne!(receipt.digest(), DigestV1::ZERO);

    // The receipt head equals the last record's digest.
    let last = records.last().unwrap().record_digest().unwrap();
    assert_eq!(receipt.head, last);

    let snapshot = open_trace_export_v1(directory.path()).unwrap();
    assert_eq!(snapshot.records.len(), 4);
    assert_eq!(snapshot.head, receipt.head);
    assert_eq!(snapshot.sealed_head, Some(receipt.head));
    for (index, record) in snapshot.records.iter().enumerate() {
        assert_eq!(record, &records[index]);
    }
}

/// Cache-decision records require their decision-boundary annotation: the
/// typed constructor refuses a missing annotation.
#[test]
fn cache_decision_requires_decision_boundary_annotation() {
    // `TraceRecordV1::new` with kind CacheDecision and no annotation is the
    // only way to express "decision without annotation"; the typed
    // constructor `cache_decision` makes it impossible by signature.
    let error = TraceRecordV1::new(
        0,
        TraceEventKindV1::CacheDecision,
        None,
        "fz://blob/dec",
        "cache-gate",
        None,
    )
    .unwrap_err();
    match error {
        TraceExportErrorV1::InvalidRecord { seq, detail } => {
            assert_eq!(seq, 0);
            assert!(
                detail.contains("annotation"),
                "annotation required: {detail}"
            );
        }
        other => panic!("expected invalid record, got {other:?}"),
    }

    // Construction-time enforcement: an unannotated decision record cannot
    // be created at all, so it can never reach the export.
    assert!(matches!(
        TraceRecordV1::new(
            0,
            TraceEventKindV1::CacheDecision,
            None,
            "fz://blob/dec",
            "cache-gate",
            None,
        ),
        Err(TraceExportErrorV1::InvalidRecord { seq: 0, .. })
    ));
}

/// Torn tail fails closed on open: a partial final line is a loud refusal,
/// never a silent prefix read.
#[test]
fn torn_tail_fails_closed() {
    let directory = tempdir().unwrap();
    let records = chained_records(2);
    export_trace_pipeline_v1(directory.path(), &records).unwrap();

    // Simulate a process dying mid-append: partial JSON line at the end.
    let path = directory.path().join("trace_records.jsonl");
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    write!(file, "{{\"schema_version\":1,\"seq\":99,\"kind\":\"commit\",").unwrap();
    drop(file);

    assert!(matches!(
        open_trace_export_v1(directory.path()),
        Err(TraceExportErrorV1::TornTail { .. })
    ));
}

/// Tampering with a persisted record breaks the chain loudly.
#[test]
fn tampered_record_fails_closed() {
    let directory = tempdir().unwrap();
    let records = chained_records(3);
    export_trace_pipeline_v1(directory.path(), &records).unwrap();

    let path = directory.path().join("trace_records.jsonl");
    let content = std::fs::read_to_string(&path).unwrap();
    let tampered = content.replacen("fz://blob/dec-0", "fz://blob/dec-9", 1);
    assert_ne!(content, tampered);
    std::fs::write(&path, tampered).unwrap();

    let error = open_trace_export_v1(directory.path()).unwrap_err();
    match error {
        TraceExportErrorV1::HeadMismatch { .. } | TraceExportErrorV1::InvalidRecord { .. } => {}
        other => panic!("expected a loud chain failure, got {other:?}"),
    }
}

/// Reordering records breaks the chain loudly.
#[test]
fn reordered_records_fail_closed() {
    let directory = tempdir().unwrap();
    let records = chained_records(3);
    export_trace_pipeline_v1(directory.path(), &records).unwrap();

    let path = directory.path().join("trace_records.jsonl");
    let lines: Vec<String> = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let mut reordered = lines.clone();
    reordered.swap(1, 2);
    std::fs::write(&path, reordered.join("\n") + "\n").unwrap();

    assert!(matches!(
        open_trace_export_v1(directory.path()),
        Err(TraceExportErrorV1::InvalidRecord { .. })
    ));
}

/// Appending out of sequence is a loud refusal.
#[test]
fn out_of_sequence_append_refused() {
    let directory = tempdir().unwrap();
    let records = chained_records(2);
    export_trace_pipeline_v1(directory.path(), &records).unwrap();

    let duplicate = records[1].clone();
    assert!(matches!(
        append_trace_record_v1(directory.path(), &duplicate),
        Err(TraceExportErrorV1::InvalidRecord { seq: 1, .. })
    ));
}

/// Sealed benchmark manifest: export round trips, the seal verifies, and
/// tampering is loud. Nonreproducible benchmarks carry a reason and still
/// seal.
#[test]
fn sealed_benchmark_manifest_round_trip_and_tamper_refusal() {
    let directory = tempdir().unwrap();

    let manifest = SealedBenchmarkManifestV1::new(
        "bench-fixture",
        digest(0x10),
        digest(0x11),
        vec!["worker-a:00000000000000000000000000000000000000000000000000000000000000aa".to_owned()],
        serde_json::json!({"files": 100, "threads": 4}),
        digest(0x12),
        vec!["receipt-1".to_owned()],
        BenchmarkReproducibilityV1::Sealed,
    )
    .unwrap();
    let seal = manifest.digest().unwrap();

    let exported_seal = export_benchmark_manifest_v1(directory.path(), manifest.clone()).unwrap();
    assert_eq!(exported_seal, seal);

    let read_back = read_exported_benchmark_manifest_v1(directory.path()).unwrap();
    assert_eq!(read_back, manifest);
    assert_eq!(read_back.digest().unwrap(), seal);

    // Tampering with the manifest bytes changes the recomputed seal: loud.
    let manifest_path = directory.path().join("benchmark_manifest.json");
    let content = std::fs::read_to_string(&manifest_path).unwrap();
    let tampered = content.replace("bench-fixture", "bench-forged");
    std::fs::write(&manifest_path, tampered).unwrap();
    assert!(matches!(
        read_exported_benchmark_manifest_v1(directory.path()),
        Err(TraceExportErrorV1::InvalidManifest(_))
    ));

    // Nonreproducible is explicit with a reason, never implicit.
    let nonreproducible = SealedBenchmarkManifestV1::new(
        "bench-nr",
        digest(0x13),
        digest(0x14),
        vec![],
        serde_json::json!({}),
        digest(0x15),
        vec![],
        BenchmarkReproducibilityV1::NonReproducible {
            reason: "machine was under load; counters not sealed".to_owned(),
        },
    )
    .unwrap();
    assert_ne!(nonreproducible.digest().unwrap(), DigestV1::ZERO);
    export_benchmark_manifest_v1(directory.path(), nonreproducible).unwrap();
    let read_back = read_exported_benchmark_manifest_v1(directory.path()).unwrap();
    match read_back.reproducibility {
        BenchmarkReproducibilityV1::NonReproducible { reason } => {
            assert!(reason.contains("under load"));
        }
        other => panic!("expected nonreproducible, got {other:?}"),
    }

    // A nonreproducible manifest without a reason is refused at
    // construction.
    assert!(SealedBenchmarkManifestV1::new(
        "bench-bad",
        digest(0x16),
        digest(0x17),
        vec![],
        serde_json::json!({}),
        digest(0x18),
        vec![],
        BenchmarkReproducibilityV1::NonReproducible {
            reason: String::new(),
        },
    )
    .is_err());
}

/// The decision-boundary annotation artifact: every annotated boundary is
/// listed with its sealed decision digest and record digest; the summary
/// digest is deterministic.
#[test]
fn decision_boundary_summary_artifact() {
    let directory = tempdir().unwrap();
    let records = chained_records(4);
    export_trace_pipeline_v1(directory.path(), &records).unwrap();

    let summary = summarize_decision_boundaries_v1(directory.path()).unwrap();
    assert_eq!(summary.annotated_records, 4);
    assert_eq!(summary.boundaries.len(), 4);

    // Record 0 is the cache admission; its decision digest is the
    // annotation's digest; its record digest chains the export.
    let first = &summary.boundaries[0];
    assert_eq!(first.boundary, DecisionBoundaryKindV1::CacheAdmission);
    assert_eq!(first.decision_digest, digest(0xAD));
    assert_eq!(first.record_digest, records[0].record_digest().unwrap());
    assert_eq!(first.seq, 0);

    let digest = summary.digest();
    assert_ne!(digest, DigestV1::ZERO);
    let again = summarize_decision_boundaries_v1(directory.path()).unwrap();
    assert_eq!(again.boundaries, summary.boundaries);
    assert_eq!(again.digest(), digest, "summary digest is deterministic");
}

/// The contract manifest freezes pipeline semantics.
#[test]
fn contract_manifest_freezes_pipeline_semantics() {
    let manifest = zero_cert::trace_export_contract_v1();
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(
        manifest["pipeline"]["fail_closed"],
        serde_json::json!("torn tails, tampered or reordered records, head mismatches")
    );
    assert_eq!(
        manifest["decision_boundary"]["cache_decision_records"],
        serde_json::json!("annotation required (typed constructor)")
    );
    assert_eq!(
        manifest["benchmark_manifests"]["reproducibility"],
        serde_json::json!("sealed or explicitly nonreproducible with reason")
    );
}

/// The trace record digest is content-derived and tamper-sensitive.
#[test]
fn record_digest_is_content_derived() {
    let records = chained_records(1);
    let original = records[0].record_digest().unwrap();
    let mut tampered = records[0].clone();
    tampered.payload_root = "fz://blob/other".to_owned();
    assert_ne!(tampered.record_digest().unwrap(), original);

    // Same canonical bytes, same digest (determinism).
    let clone = records[0].clone();
    assert_eq!(clone.record_digest().unwrap(), original);
    assert_eq!(sha256(b"x").len(), 32);
}
