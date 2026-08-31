#![deny(unsafe_code)]

//! Daemonless in-process ZeroKernel runtime. The crate exposes only typed direct
//! operations. JavaScript binding and cell parsing layer on top of
//! [`ZeroKernel`]; engine routing never uses public command strings or a capability catalog.

mod canonical;
mod host;
mod preparation;
mod pulse;
mod runtime;
mod shell;
mod snap_gate;
mod speculation;
mod state;
mod transaction;
mod typescript;

pub use canonical::direct_contract_digest;
pub use host::{AtomicCancellation, Cell, HostError, ZeroKernel, typed_error};
pub use preparation::{CellPreparation, PreparedCell};
pub use shell::ShellCommand;
pub use snap_gate::{
    SNAP_TO_FILE_READ_SCHEMA, SnapFirstExpansion, SnapIncrementalSession, SnapToFileReadRequest,
    SnapToFileReadResult,
};
pub use speculation::{SpeculationClaimOutcome, SpeculationOutcome, SpeculationRuntime};
pub use state::{StateError, StateSnapshot, StateStore};
pub use transaction::{
    PreparedEffect, Transaction, TransactionCoordinator, TransactionError, TransactionRecord,
    TransactionState,
};
pub use zero_gate::GraphZeroCompletenessInput;
pub use zero_gauge::observation::{
    MachineFingerprint, MeasuredUsage, Observation, ObservationKind, TaskIdentity,
};
pub use zero_gauge::report::{ReportError, SavingsReport};
pub use zero_token::{
    EvidenceFreshness, LiveCandidate, LiveEntry, LiveParetoDecision, MetricOrder, ProtectedOutcome,
    VerifierIdentity, decide_live_pareto,
};

/// Build one exact savings report from an explicit comparable native/Zero pair.
/// ZeroKernel never invents a native baseline or machine identity.
pub fn paired_savings_report(
    native: Observation,
    zero: Observation,
) -> Result<SavingsReport, ReportError> {
    let pair = zero_gauge::pair::PairedObservations::new(native, zero)?;
    SavingsReport::from_pair(&pair)
}

pub use typescript::{TypeScriptError, erase_typescript};
