//! Production bridge from engine-owned reachability snapshots to applied Program GC evidence.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

use zero_abi::EngineIdentity;
use zero_store::{
    DEFAULT_GC_REPORT_LIMIT, GC_MIN_GRACE_SECONDS, GcConfig, GcRunReceipt, SharedCas,
    current_reachability_snapshot, gc_project_id, run_gc,
};

use crate::{
    AppliedGcEvidence, GcProducerEpoch, GcReport, PROGRAM_ASSEMBLY_SCHEMA_VERSION,
    ProgramDigest,
};

const PRODUCERS: [(EngineIdentity, &str); 3] = [
    (EngineIdentity::FsZero, "fszero"),
    (EngineIdentity::GraphZero, "graphzero"),
    (EngineIdentity::TokenZero, "tokenzero"),
];

#[derive(Clone, Debug)]
pub struct RealGcConfig {
    pub store_root: PathBuf,
    pub run_id: String,
    pub lifecycle_closed: bool,
    pub grace_seconds: u64,
    pub min_age_seconds: u64,
    pub now: SystemTime,
}

impl RealGcConfig {
    pub fn new(store_root: impl Into<PathBuf>, run_id: impl Into<String>) -> Self {
        Self {
            store_root: store_root.into(),
            run_id: run_id.into(),
            lifecycle_closed: false,
            grace_seconds: GC_MIN_GRACE_SECONDS,
            min_age_seconds: 0,
            now: SystemTime::now(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RealGcOutcome {
    pub project_id: String,
    pub run_receipt: GcRunReceipt,
    pub producer_epochs: Vec<GcProducerEpoch>,
    pub verified_freed_bytes: u64,
    applied: AppliedGcEvidence,
}

impl RealGcOutcome {
    pub fn program_report(&self, program_id: ProgramDigest) -> GcReport {
        GcReport::new_applied(
            PROGRAM_ASSEMBLY_SCHEMA_VERSION,
            program_id,
            self.applied.clone(),
        )
    }

    pub fn applied_evidence(&self) -> &AppliedGcEvidence {
        &self.applied
    }
}

#[derive(Debug)]
pub enum RealGcError {
    LifecycleOpen,
    MissingProducer(&'static str),
    EmptyProducer(&'static str),
    Store(String),
    FreedBytesMismatch { expected: u64, observed: u64 },
    Evidence(String),
}

impl fmt::Display for RealGcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LifecycleOpen => formatter.write_str("GC apply requires a closed lifecycle"),
            Self::MissingProducer(producer) => {
                write!(
                    formatter,
                    "missing required {producer} reachability snapshot"
                )
            }
            Self::EmptyProducer(producer) => {
                write!(formatter, "{producer} published no portable reachable refs")
            }
            Self::Store(error) => formatter.write_str(error),
            Self::FreedBytesMismatch { expected, observed } => write!(
                formatter,
                "verified freed bytes mismatch: expected {expected}, observed {observed}"
            ),
            Self::Evidence(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for RealGcError {}

fn verify_producer_snapshots(
    cas: &SharedCas,
    store_root: &std::path::Path,
    project_id: &str,
) -> Result<Vec<GcProducerEpoch>, RealGcError> {
    let mut producer_epochs = Vec::with_capacity(PRODUCERS.len());
    for (engine, producer) in PRODUCERS {
        let snapshot = current_reachability_snapshot(store_root, producer, project_id)
            .map_err(|error| RealGcError::Store(format!("read {producer} snapshot: {error}")))?
            .ok_or(RealGcError::MissingProducer(producer))?;
        if snapshot.blob_hashes.is_empty() {
            return Err(RealGcError::EmptyProducer(producer));
        }
        for hash in &snapshot.blob_hashes {
            cas.get_verified(hash).map_err(|error| {
                RealGcError::Store(format!("verify {producer} root {hash}: {error}"))
            })?;
        }
        producer_epochs.push(GcProducerEpoch {
            engine,
            epoch: snapshot.epoch,
        });
    }
    Ok(producer_epochs)
}

fn verify_freed_bytes(
    cas: &SharedCas,
    deleted: &[String],
    planned_bytes: &BTreeMap<String, u64>,
) -> Result<u64, RealGcError> {
    let verified_freed_bytes = deleted.iter().try_fold(0u64, |total, hash| {
        let bytes = planned_bytes.get(hash).copied().ok_or_else(|| {
            RealGcError::FreedBytesMismatch {
                expected: 0,
                observed: total,
            }
        })?;
        if cas.contains(hash) {
            return Err(RealGcError::FreedBytesMismatch {
                expected: bytes,
                observed: 0,
            });
        }
        Ok(total.saturating_add(bytes))
    })?;
    let expected_freed_bytes = planned_bytes
        .iter()
        .filter(|(hash, _)| deleted.iter().any(|gone| gone == *hash))
        .map(|(_, bytes)| *bytes)
        .sum::<u64>();
    if verified_freed_bytes != expected_freed_bytes {
        return Err(RealGcError::FreedBytesMismatch {
            expected: expected_freed_bytes,
            observed: verified_freed_bytes,
        });
    }
    Ok(verified_freed_bytes)
}

/// Apply GC only after all three real engine producers published nonempty,
/// verifiable reachability snapshots for the same authorized store.
pub fn apply_real_reachability_gc(config: &RealGcConfig) -> Result<RealGcOutcome, RealGcError> {
    if !config.lifecycle_closed {
        return Err(RealGcError::LifecycleOpen);
    }
    let store_root = config
        .store_root
        .canonicalize()
        .map_err(|error| RealGcError::Store(format!("resolve store root: {error}")))?;
    let project_id = gc_project_id(&store_root)
        .map_err(|error| RealGcError::Store(format!("derive project identity: {error}")))?;
    let cas = SharedCas::open(&store_root);
    let producer_epochs = verify_producer_snapshots(&cas, &store_root, &project_id)?;

    let dry_config = GcConfig {
        run_id: format!("{}-measure", config.run_id),
        grace_seconds: config.grace_seconds,
        min_age_seconds: config.min_age_seconds,
        apply: false,
        now: config.now,
        fault_after_deletes: None,
        report_limit: DEFAULT_GC_REPORT_LIMIT,
        before_unlink: None,
    };
    let dry = run_gc(&store_root, &dry_config)
        .map_err(|error| RealGcError::Store(format!("measure GC candidates: {error}")))?;
    let mut planned_bytes = BTreeMap::new();
    for hash in &dry.planned {
        let bytes = cas.get_verified(hash).map_err(|error| {
            RealGcError::Store(format!("verify planned object {hash}: {error}"))
        })?;
        planned_bytes.insert(hash.clone(), bytes.len() as u64);
    }

    let apply_config = GcConfig {
        run_id: config.run_id.clone(),
        apply: true,
        ..dry_config
    };
    let run_receipt = run_gc(&store_root, &apply_config)
        .map_err(|error| RealGcError::Store(format!("apply reachability GC: {error}")))?;
    let verified_freed_bytes =
        verify_freed_bytes(&cas, &run_receipt.deleted, &planned_bytes)?;

    let applied = AppliedGcEvidence::new(
        run_receipt.clone(),
        producer_epochs.clone(),
        verified_freed_bytes,
    )
    .map_err(RealGcError::Evidence)?;
    Ok(RealGcOutcome {
        project_id,
        run_receipt,
        producer_epochs,
        verified_freed_bytes,
        applied,
    })
}
