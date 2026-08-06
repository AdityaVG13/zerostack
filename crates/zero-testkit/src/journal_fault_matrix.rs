//! Exhaustive deterministic fault matrix for the durable journal protocol.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tempfile::tempdir;
use zero_abi::DigestV1;
use zero_store::DurableProfileIdV1;
use zero_store::{
    abort_journal_with_fault_v1, commit_journal_with_fault_v1, initialize_published_root_v1,
    initialize_published_root_with_fault_v1, prepare_journal_v1, prepare_journal_with_fault_v1,
    read_journal_record_v1, read_published_root_v1, record_owner_death_v1,
    record_owner_death_with_fault_v1, recover_journal_v1, recover_journal_with_fault_v1,
    FaultPlanV1, JournalBindingV1, JournalBoundaryV1, JournalFailureCodeV1, JournalPathsV1,
    JournalStateV1, RecoveryOutcomeV1,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FaultOperationV1 {
    Initialize,
    Prepare,
    Commit,
    CommitRecovery,
    Abort,
    AbortRecovery,
    OwnerDeath,
    OwnerDeathRecovery,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalFaultCaseV1 {
    pub boundary: JournalBoundaryV1,
    pub operation: FaultOperationV1,
    pub injected: bool,
    pub final_root: DigestV1,
    pub final_state: Option<JournalStateV1>,
    pub recovery_outcome: RecoveryOutcomeV1,
    pub root_is_old_or_new: bool,
    pub journal_root_correspondence: bool,
    pub passed: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct JournalFaultMatrixReportV1 {
    pub schema_version: u16,
    pub cases: Vec<JournalFaultCaseV1>,
    pub boundaries_exercised: usize,
    pub failed: usize,
    pub all_passed: bool,
}

const ROOT_INIT: [JournalBoundaryV1; 4] = [
    JournalBoundaryV1::RootInitializeBeforeWrite,
    JournalBoundaryV1::RootInitializeAfterFileSync,
    JournalBoundaryV1::RootInitializeAfterRename,
    JournalBoundaryV1::RootInitializeAfterDirectorySync,
];
const PREPARE: [JournalBoundaryV1; 8] = [
    JournalBoundaryV1::CartridgeBeforeWrite,
    JournalBoundaryV1::CartridgeAfterFileSync,
    JournalBoundaryV1::CartridgeAfterRename,
    JournalBoundaryV1::CartridgeAfterDirectorySync,
    JournalBoundaryV1::PrepareBeforeWrite,
    JournalBoundaryV1::PrepareAfterFileSync,
    JournalBoundaryV1::PrepareAfterRename,
    JournalBoundaryV1::PrepareAfterDirectorySync,
];
const COMMIT: [JournalBoundaryV1; 8] = [
    JournalBoundaryV1::RootPublishBeforeWrite,
    JournalBoundaryV1::RootPublishAfterFileSync,
    JournalBoundaryV1::RootPublishAfterRename,
    JournalBoundaryV1::RootPublishAfterDirectorySync,
    JournalBoundaryV1::CommitBeforeWrite,
    JournalBoundaryV1::CommitAfterFileSync,
    JournalBoundaryV1::CommitAfterRename,
    JournalBoundaryV1::CommitAfterDirectorySync,
];
const RECOVERY: [JournalBoundaryV1; 4] = [
    JournalBoundaryV1::RecoveryBeforeWrite,
    JournalBoundaryV1::RecoveryAfterFileSync,
    JournalBoundaryV1::RecoveryAfterRename,
    JournalBoundaryV1::RecoveryAfterDirectorySync,
];
const ABORT: [JournalBoundaryV1; 4] = [
    JournalBoundaryV1::AbortBeforeWrite,
    JournalBoundaryV1::AbortAfterFileSync,
    JournalBoundaryV1::AbortAfterRename,
    JournalBoundaryV1::AbortAfterDirectorySync,
];
const OWNER_DEATH: [JournalBoundaryV1; 4] = [
    JournalBoundaryV1::OwnerDeathBeforeWrite,
    JournalBoundaryV1::OwnerDeathAfterFileSync,
    JournalBoundaryV1::OwnerDeathAfterRename,
    JournalBoundaryV1::OwnerDeathAfterDirectorySync,
];

fn digest(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}
fn binding() -> JournalBindingV1 {
    JournalBindingV1::new(
        digest(1),
        digest(2),
        DurableProfileIdV1::PortableStrict,
        digest(3),
        digest(4),
        digest(5),
    )
}
fn paths(directory: &Path) -> JournalPathsV1 {
    JournalPathsV1::new(
        directory.join("root.json"),
        directory.join("journal.json"),
        directory.join("cartridge.json"),
        directory.join("owner-death.json"),
        directory.join("recovery.json"),
    )
    .expect("valid isolated paths")
}
fn assert_injected(
    result: Result<impl Sized, zero_store::JournalErrorV1>,
    boundary: JournalBoundaryV1,
) {
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("selected boundary must inject a crash"),
    };
    assert_eq!(error.code, JournalFailureCodeV1::InjectedCrash);
    assert_eq!(error.boundary, Some(boundary));
}
fn case(
    paths: &JournalPathsV1,
    binding: &JournalBindingV1,
    boundary: JournalBoundaryV1,
    operation: FaultOperationV1,
    outcome: RecoveryOutcomeV1,
) -> JournalFaultCaseV1 {
    let root = read_published_root_v1(paths).expect("root after recovery");
    let journal = read_journal_record_v1(paths).ok();
    let final_state = journal.as_ref().map(|record| record.state);
    let root_is_old_or_new =
        root.root_digest == binding.old_root || root.root_digest == binding.new_root;
    let correspondence = match final_state {
        None => root.root_digest == binding.old_root,
        Some(JournalStateV1::Prepared) => false,
        Some(JournalStateV1::Committed) => {
            root.root_digest == binding.new_root && root.transaction_id == binding.transaction_id
        }
        Some(JournalStateV1::Aborted) => root.root_digest == binding.old_root,
    };
    JournalFaultCaseV1 {
        boundary,
        operation,
        injected: true,
        final_root: root.root_digest,
        final_state,
        recovery_outcome: outcome,
        root_is_old_or_new,
        journal_root_correspondence: correspondence,
        passed: root_is_old_or_new && correspondence,
    }
}

/// Runs every frozen write, file-sync, rename, and directory-sync boundary.
pub fn run_journal_fault_matrix_v1() -> JournalFaultMatrixReportV1 {
    let mut cases = Vec::new();
    for boundary in ROOT_INIT {
        let directory = tempdir().expect("temporary matrix directory");
        let paths = paths(directory.path());
        let binding = binding();
        let mut fault = FaultPlanV1::crash_at(boundary);
        assert_injected(
            initialize_published_root_with_fault_v1(&paths, binding.old_root, &mut fault),
            boundary,
        );
        if read_published_root_v1(&paths).is_err() {
            initialize_published_root_v1(&paths, binding.old_root)
                .expect("resume root initialization");
        }
        let receipt = recover_journal_v1(&paths, &binding).expect("recover initialization crash");
        cases.push(case(
            &paths,
            &binding,
            boundary,
            FaultOperationV1::Initialize,
            receipt.outcome,
        ));
    }
    for boundary in PREPARE {
        let directory = tempdir().expect("temporary matrix directory");
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).expect("initial root");
        let mut fault = FaultPlanV1::crash_at(boundary);
        assert_injected(
            prepare_journal_with_fault_v1(&paths, binding.clone(), &mut fault),
            boundary,
        );
        let receipt = recover_journal_v1(&paths, &binding).expect("recover prepare crash");
        cases.push(case(
            &paths,
            &binding,
            boundary,
            FaultOperationV1::Prepare,
            receipt.outcome,
        ));
    }
    for boundary in COMMIT {
        let directory = tempdir().expect("temporary matrix directory");
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).expect("initial root");
        let cartridge = prepare_journal_v1(&paths, binding.clone()).expect("prepared journal");
        let mut fault = FaultPlanV1::crash_at(boundary);
        assert_injected(
            commit_journal_with_fault_v1(&paths, &cartridge, &mut fault),
            boundary,
        );
        let receipt = recover_journal_v1(&paths, &binding).expect("recover commit crash");
        cases.push(case(
            &paths,
            &binding,
            boundary,
            FaultOperationV1::Commit,
            receipt.outcome,
        ));
    }
    for boundary in RECOVERY {
        let directory = tempdir().expect("temporary matrix directory");
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).expect("initial root");
        let cartridge = prepare_journal_v1(&paths, binding.clone()).expect("prepared journal");
        let mut fault = FaultPlanV1::crash_at(boundary);
        assert_injected(
            commit_journal_with_fault_v1(&paths, &cartridge, &mut fault),
            boundary,
        );
        let receipt = recover_journal_v1(&paths, &binding).expect("recover commit receipt crash");
        cases.push(case(
            &paths,
            &binding,
            boundary,
            FaultOperationV1::CommitRecovery,
            receipt.outcome,
        ));
    }
    for boundary in ABORT {
        let directory = tempdir().expect("temporary matrix directory");
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).expect("initial root");
        let cartridge = prepare_journal_v1(&paths, binding.clone()).expect("prepared journal");
        let mut fault = FaultPlanV1::crash_at(boundary);
        assert_injected(
            abort_journal_with_fault_v1(&paths, &cartridge, &mut fault),
            boundary,
        );
        let receipt = recover_journal_v1(&paths, &binding).expect("recover abort crash");
        cases.push(case(
            &paths,
            &binding,
            boundary,
            FaultOperationV1::Abort,
            receipt.outcome,
        ));
    }
    for boundary in RECOVERY {
        let directory = tempdir().expect("temporary matrix directory");
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).expect("initial root");
        let cartridge = prepare_journal_v1(&paths, binding.clone()).expect("prepared journal");
        let mut fault = FaultPlanV1::crash_at(boundary);
        assert_injected(
            abort_journal_with_fault_v1(&paths, &cartridge, &mut fault),
            boundary,
        );
        let receipt = recover_journal_v1(&paths, &binding).expect("recover abort receipt crash");
        cases.push(case(
            &paths,
            &binding,
            boundary,
            FaultOperationV1::AbortRecovery,
            receipt.outcome,
        ));
    }
    for boundary in OWNER_DEATH {
        let directory = tempdir().expect("temporary matrix directory");
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).expect("initial root");
        prepare_journal_v1(&paths, binding.clone()).expect("prepared journal");
        let mut fault = FaultPlanV1::crash_at(boundary);
        assert_injected(
            record_owner_death_with_fault_v1(&paths, binding.owner_identity_digest, 77, &mut fault),
            boundary,
        );
        let receipt = recover_journal_v1(&paths, &binding).expect("recover owner death crash");
        cases.push(case(
            &paths,
            &binding,
            boundary,
            FaultOperationV1::OwnerDeath,
            receipt.outcome,
        ));
    }
    for boundary in RECOVERY {
        let directory = tempdir().expect("temporary matrix directory");
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).expect("initial root");
        prepare_journal_v1(&paths, binding.clone()).expect("prepared journal");
        record_owner_death_v1(&paths, binding.owner_identity_digest, 77)
            .expect("owner-death receipt");
        let mut fault = FaultPlanV1::crash_at(boundary);
        assert_injected(
            recover_journal_with_fault_v1(&paths, &binding, &mut fault),
            boundary,
        );
        let receipt =
            recover_journal_v1(&paths, &binding).expect("recover owner-death receipt crash");
        cases.push(case(
            &paths,
            &binding,
            boundary,
            FaultOperationV1::OwnerDeathRecovery,
            receipt.outcome,
        ));
    }
    let failed = cases.iter().filter(|case| !case.passed).count();
    JournalFaultMatrixReportV1 {
        schema_version: 1,
        boundaries_exercised: cases.len(),
        failed,
        all_passed: failed == 0,
        cases,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zero_store::durable_journal_contract_v1;

    #[test]
    fn journal_fault_matrix_exercises_every_frozen_boundary() {
        let report = run_journal_fault_matrix_v1();
        assert_eq!(report.boundaries_exercised, 40);
        assert_eq!(report.failed, 0);
        assert!(report.all_passed);
    }

    #[test]
    fn durable_journal_model_matches_the_runtime_contract() {
        let model: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/models/durable-journal-v2.json"
        )))
        .unwrap();
        let contract = durable_journal_contract_v1();
        assert_eq!(model["model_version"], "zerostack.durable-journal.v2");
        assert_eq!(contract["schema_version"], 2);
        assert_eq!(
            model["state_machine"]["states"],
            serde_json::json!(["absent", "prepared", "committed", "aborted"])
        );
        assert_eq!(
            contract["states"],
            serde_json::json!(["prepared", "committed", "aborted"])
        );
        assert_eq!(
            model["evidence_boundary"]["rch"],
            "compilation_and_test_only"
        );
        assert_eq!(
            model["durable_profiles"]["portable_strict"]["native_claim"],
            false
        );
    }

    #[test]
    fn journal_fault_matrix_profile_substitution_mutant_fails() {
        let directory = tempdir().unwrap();
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).unwrap();
        prepare_journal_v1(&paths, binding.clone()).unwrap();
        let mut mutant = binding;
        mutant.durable_profile_id = DurableProfileIdV1::NtfsStrict;
        assert_eq!(
            recover_journal_v1(&paths, &mutant).unwrap_err().code,
            JournalFailureCodeV1::ProfileSubstitution
        );
    }

    #[test]
    fn journal_fault_matrix_owner_identity_mutant_fails() {
        let directory = tempdir().unwrap();
        let paths = paths(directory.path());
        let binding = binding();
        initialize_published_root_v1(&paths, binding.old_root).unwrap();
        prepare_journal_v1(&paths, binding.clone()).unwrap();
        assert_eq!(
            record_owner_death_with_fault_v1(&paths, digest(9), 77, &mut FaultPlanV1::none())
                .unwrap_err()
                .code,
            JournalFailureCodeV1::OwnerIdentityMismatch
        );
    }
}
