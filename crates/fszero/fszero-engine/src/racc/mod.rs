//! RACC-R exact-bytes authority surfaces owned by FSZero.
//!
//! These modules implement pure identity / digest / path logic for snapshots,
//! evidence pages, overlays/journals, safepoints, deopt rehydration, and
//! successor maps. Composition and quality admission remain ZeroStack hub work.

mod bridge;
mod deopt_restore;
mod durability;
mod evidence_page;
mod exact_snapshot;
mod overlay_publish;
mod safepoint;
mod successor_map;

pub use bridge::{load_tree, record_path_move, safepoint_for_snapshot, snapshot_from_files};
pub use deopt_restore::{DeoptRestoreError, DeoptRestoreReceipt, rehydrate_from_safepoint};
pub use durability::{
    DurabilityCase, DurabilityCaseResult, DurabilityMatrixReport, run_durability_matrix,
};
pub use evidence_page::{
    EvidencePage, EvidencePageError, ExactRange, LineEndingPolicy, canonicalize_line_endings,
    line_digest_hex, range_digest_hex,
};
pub use exact_snapshot::{
    DEFAULT_FILE_MODE, ExactSnapshot, FileRecord, NonsemanticExclusion, SnapshotEntry,
    SnapshotError, ToolchainContract, snapshot_root_digest, toolchain_contract_digest,
};
pub use overlay_publish::{
    AtomicPublication, CrashPoint, EffectMutation, JournalRecord, Overlay, OverlayError,
    PublicationStage, realize_effects,
};
pub use safepoint::{RawBaselineSafepoint, SafepointError};
pub use successor_map::{RefFate, SuccessorMap, SuccessorMapError};
