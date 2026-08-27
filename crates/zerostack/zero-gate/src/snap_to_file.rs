#![forbid(unsafe_code)]

//! K0 Snap-to-File gate (`zerostack-xbg3`).
//!
//! Integrates the one-family W9-E exact-scenario-closure route
//! (`zerostack-rybb` + `zerostack-qg2a`) as the read-only
//! structured `z.read` Snap-to-File gate for K0:
//!
//! - **One target-ref grammar.** The request is the W9-E [`DemandRequest`]
//!   (scenario id plus projection atom roots). There is no second grammar.
//! - **S0 exact object/file root.** Every projection atom resolves to an
//!   exact image object and is returned root-exact with its exact byte
//!   length. A single-atom projection is *primary-file orientation only*:
//!   the packet never sells it as the complete multi-file demand -- the
//!   completeness claim stays bound to the certified scenario envelope
//!   (S3).
//! - **Proved family levels.** The chosen family (exact scenario closure)
//!   proves S0 (exact object/file roots) and S3 (task -> complete
//!   file/span closure). S1/S2/S4 are declared unproved in every packet
//!   with the honest reason.
//! - **Safe one-expansion path.** A `Safe` fold issues the read-only
//!   [`SafeExpandHandle`] and performs exactly one first expansion with no
//!   model-visible discovery (every lookup is ledged `backend_work`; no
//!   `ls`/grep/probe row exists in the ledger).
//! - **Unknown native escape.** `Unknown` escapes to the frozen native
//!   baseline with the strategy preserved (`baseline_escape`, request and
//!   binding roots carried in the packet) -- never a guessed subset labeled
//!   complete.
//! - **Unsafe refusal.** `Unsafe` refuses with typed reasons; no handle, no
//!   atoms, no expansion.
//! - **Zero false-complete by construction.** A completeness claim exists
//!   only on the `Snapped` outcome, and only for the certified envelope
//!   ([`SnapPacket::validate`] enforces the outcome/claim consistency).
//! - **Read-only authority.** The packet carries no edit/transaction/commit
//!   field, and the only credential is the read-only [`SafeExpandHandle`]
//!   / [`zero_abi::ExpandPermit`] ABI.
//! - **Adapter-stable packet.** [`SnapPacket`] serializes to deterministic
//!   sorted-key JSON and round-trips byte-identically through any wire
//!   adapter; [`SnapPacket::packet_root`] binds the canonical rendering.
//!
//! The decision view is built with the existing ZS-VIEW-010
//! [`DecisionView`] ABI and certified with its honesty law
//! ([`DecisionView::certificate`]): a `Proved` claim requires every
//! evidence class the route claims to hold; any missing needed class
//! degrades to `Unknown`. The route never fabricates a root: every view
//! root is a route-proven binding (plan/request root, project root, index
//! lens root, certificate root).
//!
//! This module owns hub authority only; it edits no engine source and adds
//! no write/transaction/commit capability.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Serialize};
use zero_abi::{
    CompletenessGrade, DecisionView, DecisionViewError, ExpandOutcome, LiveExpandState,
    SafeExpandHandle, SafetyVerdict, Sha256Digest, canonical_json, sha256_hex,
};

use crate::demand_expand::{
    DemandError, DemandRequest, FirstExpansion, GraphZeroCompletenessInput, IncrementalDelta,
    IncrementalDeltaRequest, IncrementalSession, NativeBaseline, ProtectedScope, RouteOutcome,
    W9E_FAMILY_NAME, W9eRoute,
};
use crate::project_image::ProjectImageManifest;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Wire schema of the Snap-to-File gate artifacts.
pub const SNAP_TO_FILE_SCHEMA_VERSION: &str = "zerostack.w9e.snap_to_file.v1";
/// Wire packet version.
pub const SNAP_PACKET_VERSION: u16 = 1;

/// Snap level proved by the family: request already has an exact
/// object/file root (S0).
pub const SNAP_LEVEL_S0: &str = "s0";
/// Snap level proved by the family: task -> complete file/span closure for
/// evidence (S3).
pub const SNAP_LEVEL_S3: &str = "s3";
/// Snap level not proved by this family: unique symbol -> defining
/// file/span (S1).
pub const SNAP_LEVEL_S1: &str = "s1";
/// Snap level not proved by this family: symbol -> defs/refs/callers/tests
/// under a GraphZero contract (S2).
pub const SNAP_LEVEL_S2: &str = "s2";
/// Snap level not proved by this family: snap-to-edit (S4; this route has
/// no edit authority).
pub const SNAP_LEVEL_S4: &str = "s4";

/// Evidence classes of the snap decision view (inputs to
/// [`DecisionView::certificate`]).
pub const EVIDENCE_CLASS_TASK_CONTRACT: &str = "task_contract";
pub const EVIDENCE_CLASS_PROJECT: &str = "project";
pub const EVIDENCE_CLASS_CAUSAL_LENS: &str = "causal_lens";
pub const EVIDENCE_CLASS_COVERAGE: &str = "coverage";
pub const EVIDENCE_CLASS_EXACT_S0: &str = "exact_s0";

/// The supported decision of every snap decision view.
pub const SNAP_SUPPORTED_DECISION: &str = "snap_to_file";

/// Verification obligations (stable wire strings). These are the model-side
/// steps the snap does not perform: exact evidence to verify and boundaries
/// the read-only snap does not cover.
pub const OBLIGATION_VERIFY_PROJECTION: &str = "verify_projection_atoms";
/// The certified closure contains atoms not yet expanded; expand them via
/// the continuation before concluding the task.
pub const OBLIGATION_EXPAND_REMAINING_CLOSURE: &str = "expand_remaining_closure";
/// The snap grants no edit/transaction/commit authority.
pub const OBLIGATION_NO_EDIT_AUTHORITY: &str = "no_edit_authority";
/// Every continuation delta revalidates the live handle first.
pub const OBLIGATION_REVALIDATE_BEFORE_CONTINUATION: &str = "revalidate_before_continuation";
/// A primary-file projection is orientation only; it is not the complete
/// multi-file demand.
pub const OBLIGATION_PRIMARY_FILE_ORIENTATION_ONLY: &str = "primary_file_orientation_only";
/// Coverage is unknown; run the frozen native baseline for this request.
pub const OBLIGATION_NATIVE_ESCAPE: &str = "native_escape";
/// Nothing was labeled complete.
pub const OBLIGATION_NO_COMPLETENESS_CLAIM: &str = "no_completeness_claim";
/// The demand was refused; do not proceed on a guessed subset.
pub const OBLIGATION_DEMAND_REFUSED: &str = "demand_refused";

/// Maximum atoms in one packet (== the family's certified bound).
pub const MAX_PACKET_ATOMS: usize = 128;
/// Maximum level entries in one packet.
pub const MAX_PACKET_LEVELS: usize = 8;
/// Maximum verification obligations in one packet.
pub const MAX_OBLIGATIONS: usize = 16;
/// Maximum reasons in one packet.
pub const MAX_PACKET_REASONS: usize = 16;
/// Maximum evidence refs in one packet.
pub const MAX_EVIDENCE_REFS: usize = 8;
/// Maximum bytes of any packet string field.
pub const MAX_PACKET_STRING_BYTES: usize = 256;

/// The levels this family proves, in stable order.
pub fn proved_levels() -> Vec<String> {
    vec![SNAP_LEVEL_S0.to_owned(), SNAP_LEVEL_S3.to_owned()]
}

/// The levels this family does not prove, each with the honest reason it is
/// absent from the snap.
pub fn unproved_levels() -> Vec<UnprovedLevel> {
    vec![
        UnprovedLevel::new(
            SNAP_LEVEL_S1,
            "unique symbol -> defining file/span is not proved by the exact_scenario_closure family",
        )
        .expect("static unproved level is valid"),
        UnprovedLevel::new(
            SNAP_LEVEL_S2,
            "symbol -> defs/refs/callers/tests is not proved by the exact_scenario_closure family",
        )
        .expect("static unproved level is valid"),
        UnprovedLevel::new(
            SNAP_LEVEL_S4,
            "snap-to-edit carries no edit authority in this read-only route",
        )
        .expect("static unproved level is valid"),
    ]
}

/// The full evidence-class set a `Proved` snap claim needs
/// ([`DecisionView::certificate`]).
pub fn snap_evidence_classes() -> BTreeSet<String> {
    [
        EVIDENCE_CLASS_TASK_CONTRACT,
        EVIDENCE_CLASS_PROJECT,
        EVIDENCE_CLASS_CAUSAL_LENS,
        EVIDENCE_CLASS_COVERAGE,
        EVIDENCE_CLASS_EXACT_S0,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn packet_string(field: &'static str, value: &str) -> Result<(), SnapError> {
    if value.is_empty() {
        return Err(SnapError::Packet(format!("{field} must be nonempty")));
    }
    if value.len() > MAX_PACKET_STRING_BYTES {
        return Err(SnapError::Packet(format!(
            "{field} is {} bytes, maximum {MAX_PACKET_STRING_BYTES}",
            value.len()
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(SnapError::Packet(format!(
            "{field} must be free of control characters"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Fail-closed error for the whole Snap-to-File gate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnapError {
    /// The underlying W9-E route refused the operation.
    W9E(DemandError),
    /// The decision view failed to construct or certify.
    View(DecisionViewError),
    /// The adapter-stable packet violated a consistency law.
    Packet(String),
    /// An input was invalid.
    InvalidInput(String),
    /// Internal invariant violated.
    Internal(&'static str),
}

impl fmt::Display for SnapError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::W9E(error) => write!(formatter, "snap w9e failure: {error}"),
            Self::View(error) => write!(formatter, "snap decision view failure: {error}"),
            Self::Packet(detail) => write!(formatter, "snap packet violation: {detail}"),
            Self::InvalidInput(detail) => write!(formatter, "snap invalid input: {detail}"),
            Self::Internal(detail) => {
                write!(formatter, "snap internal invariant violated: {detail}")
            }
        }
    }
}

impl Error for SnapError {}

// ---------------------------------------------------------------------------
// Adapter-stable packet
// ---------------------------------------------------------------------------

/// The outcome of one snap, as the adapter-stable packet spells it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapOutcomeKind {
    /// Safe: the certified closure was snapped with exactly one expansion.
    Snapped,
    /// Unknown coverage: escaped to the frozen native baseline.
    Escaped,
    /// Unsafe demand: refused; nothing was issued.
    Refused,
}

/// One snap level this family does not prove, with the honest reason.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UnprovedLevel {
    pub level: String,
    pub reason: String,
}

impl UnprovedLevel {
    pub fn new(level: &str, reason: &str) -> Result<Self, SnapError> {
        let entry = Self {
            level: level.to_owned(),
            reason: reason.to_owned(),
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn validate(&self) -> Result<(), SnapError> {
        packet_string("unproved_levels.level", &self.level)?;
        packet_string("unproved_levels.reason", &self.reason)?;
        Ok(())
    }
}

/// One exact atom the snap returned (S0 evidence: root + exact byte length).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PacketAtom {
    pub atom_root: String,
    pub byte_len: u64,
}

/// Ledged metrics of one snap, including the adjudicated native-comparison
/// fields. `visible_bytes`, `backend_work`, `retry_count`,
/// `first_try_sufficiency`, and `false_complete` are exact (mirroring the
/// expand ledger); `native_baseline_bytes`/`native_baseline_probes` and the
/// derived `native_savings_bytes` are the declared native-discovery
/// counterfactual -- estimates by declaration, never presented as exact.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapMetrics {
    pub visible_bytes: u64,
    pub backend_work: u64,
    pub retry_count: u64,
    pub first_try_sufficiency: bool,
    pub false_complete: bool,
    pub certified_atoms: usize,
    pub expanded_atoms: usize,
    pub native_baseline_bytes: u64,
    pub native_baseline_probes: u64,
    /// `native_baseline_bytes - visible_bytes` (saturating).
    pub native_savings_bytes: u64,
}

/// The adapter-stable snap packet: one closed wire shape
/// (`deny_unknown_fields`) that renders to deterministic sorted-key JSON
/// and round-trips byte-identically through any harness adapter.
///
/// Honesty laws (enforced by [`SnapPacket::validate`]):
/// - `Snapped` is the only outcome with a completeness claim: it carries
///   the certificate root, the certified plan/projection roots, the
///   read-only handle id, exactly the returned atoms, the proved levels
///   `["s0", "s3"]`, no `baseline_escape`, and no reasons.
/// - `Escaped` carries no claim at all: no handle, no certificate, no plan,
///   no atoms, no metrics; `baseline_escape` is `true` and the reasons
///   record why coverage is unknown. The request/binding roots stay in the
///   packet so the native strategy is not lost.
/// - `Refused` is `Escaped` without the escape: `baseline_escape` is
///   `false` and the reasons record why the demand is invalid.
/// - A single-atom projection is flagged `primary_file_orientation` and
///   always carries the `primary_file_orientation_only` obligation: a
///   primary file is never sold as the complete multi-file demand.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapPacket {
    pub schema_version: String,
    pub packet_version: u16,
    pub outcome: SnapOutcomeKind,
    pub family: String,
    /// Root of the request that produced this packet (strategy preserved on
    /// escape).
    pub request_root: String,
    pub project_root: String,
    pub scope_root: String,
    /// The coverage index lens root the evidence was evaluated against.
    pub index_root: String,
    pub index_version: String,
    /// Certified demand-plan root (Snapped only).
    pub plan_root: Option<String>,
    /// Root of the exact projection (Snapped only).
    pub projection_root: Option<String>,
    /// V7 completeness certificate root (Snapped only).
    pub certificate_root: Option<String>,
    pub checker_identity: Option<String>,
    pub checker_version: Option<String>,
    /// Read-only continuation handle id (Snapped only).
    pub handle_id: Option<String>,
    /// Levels the family proves: `["s0", "s3"]` on Snapped, empty otherwise.
    pub proved_levels: Vec<String>,
    /// Levels the family does not prove, with reasons (always present).
    pub unproved_levels: Vec<UnprovedLevel>,
    /// Exact evidence refs (the certificate root) on Snapped; empty
    /// otherwise.
    pub evidence_refs: Vec<String>,
    /// Bounded verification obligations (stable strings).
    pub obligations: Vec<String>,
    /// Exact atoms returned by the one expansion (Snapped only).
    pub atoms: Vec<PacketAtom>,
    /// Ledged metrics (Snapped only; `None` when nothing was expanded).
    pub metrics: Option<SnapMetrics>,
    /// Whether the native baseline escape hatch is open (true exactly on
    /// Escaped).
    pub baseline_escape: bool,
    /// True when the projection is exactly one atom: the request is
    /// primary-file oriented.
    pub primary_file_orientation: bool,
    /// Typed reasons (Escaped/Refused only).
    pub reasons: Vec<String>,
    /// Root of the decision view this packet carries.
    pub decision_view_root: String,
}

impl SnapPacket {
    /// Fail-closed consistency validation of the whole packet.
    pub fn validate(&self) -> Result<(), SnapError> {
        if self.schema_version != SNAP_TO_FILE_SCHEMA_VERSION {
            return Err(SnapError::Packet(format!(
                "unexpected schema_version {:?}",
                self.schema_version
            )));
        }
        if self.packet_version != SNAP_PACKET_VERSION {
            return Err(SnapError::Packet(format!(
                "unexpected packet_version {}",
                self.packet_version
            )));
        }
        if self.family != W9E_FAMILY_NAME {
            return Err(SnapError::Packet(format!(
                "unexpected family {:?}",
                self.family
            )));
        }
        for (field, value) in [
            ("request_root", &self.request_root),
            ("project_root", &self.project_root),
            ("scope_root", &self.scope_root),
            ("index_root", &self.index_root),
            ("index_version", &self.index_version),
            ("decision_view_root", &self.decision_view_root),
        ] {
            packet_string(field, value)?;
        }
        for (field, value) in [
            ("plan_root", &self.plan_root),
            ("projection_root", &self.projection_root),
            ("certificate_root", &self.certificate_root),
            ("checker_identity", &self.checker_identity),
            ("checker_version", &self.checker_version),
            ("handle_id", &self.handle_id),
        ] {
            if let Some(value) = value {
                packet_string(field, value)?;
            }
        }
        if self.atoms.len() > MAX_PACKET_ATOMS {
            return Err(SnapError::Packet(format!(
                "atoms has {} items, maximum {MAX_PACKET_ATOMS}",
                self.atoms.len()
            )));
        }
        if self.proved_levels.len() > MAX_PACKET_LEVELS
            || self.unproved_levels.len() > MAX_PACKET_LEVELS
        {
            return Err(SnapError::Packet(format!(
                "levels exceed the maximum {MAX_PACKET_LEVELS}"
            )));
        }
        if self.obligations.len() > MAX_OBLIGATIONS {
            return Err(SnapError::Packet(format!(
                "obligations has {} items, maximum {MAX_OBLIGATIONS}",
                self.obligations.len()
            )));
        }
        if self.reasons.len() > MAX_PACKET_REASONS {
            return Err(SnapError::Packet(format!(
                "reasons has {} items, maximum {MAX_PACKET_REASONS}",
                self.reasons.len()
            )));
        }
        if self.evidence_refs.len() > MAX_EVIDENCE_REFS {
            return Err(SnapError::Packet(format!(
                "evidence_refs has {} items, maximum {MAX_EVIDENCE_REFS}",
                self.evidence_refs.len()
            )));
        }
        for obligation in &self.obligations {
            packet_string("obligations entry", obligation)?;
        }
        for reason in &self.reasons {
            packet_string("reasons entry", reason)?;
        }
        for reference in &self.evidence_refs {
            packet_string("evidence_refs entry", reference)?;
        }
        for level in &self.unproved_levels {
            level.validate()?;
        }
        for atom in &self.atoms {
            packet_string("atoms.atom_root", &atom.atom_root)?;
        }

        let has_claim = self.handle_id.is_some()
            || self.plan_root.is_some()
            || self.projection_root.is_some()
            || self.certificate_root.is_some()
            || self.checker_identity.is_some()
            || self.checker_version.is_some();
        match self.outcome {
            SnapOutcomeKind::Snapped => {
                // A completeness claim exists only here, and only fully.
                if !has_claim {
                    return Err(SnapError::Packet(
                        "snapped packet must carry the full certified claim".into(),
                    ));
                }
                if self.plan_root.is_none()
                    || self.projection_root.is_none()
                    || self.certificate_root.is_none()
                    || self.checker_identity.is_none()
                    || self.checker_version.is_none()
                    || self.handle_id.is_none()
                {
                    return Err(SnapError::Packet(
                        "snapped packet must carry every certified root and the handle".into(),
                    ));
                }
                if self.atoms.is_empty() {
                    return Err(SnapError::Packet(
                        "snapped packet must carry the returned atoms".into(),
                    ));
                }
                if self.metrics.is_none() {
                    return Err(SnapError::Packet(
                        "snapped packet must carry the ledged metrics".into(),
                    ));
                }
                if self.baseline_escape {
                    return Err(SnapError::Packet(
                        "snapped packet must not open the baseline escape".into(),
                    ));
                }
                if !self.reasons.is_empty() {
                    return Err(SnapError::Packet(
                        "snapped packet must not carry refusal reasons".into(),
                    ));
                }
                if self.proved_levels != proved_levels() {
                    return Err(SnapError::Packet(
                        "snapped packet must declare exactly the proved levels".into(),
                    ));
                }
                if self.evidence_refs.is_empty() {
                    return Err(SnapError::Packet(
                        "snapped packet must carry exact evidence refs".into(),
                    ));
                }
                if self.primary_file_orientation != (self.atoms.len() == 1) {
                    return Err(SnapError::Packet(
                        "primary_file_orientation must match a single-atom projection".into(),
                    ));
                }
                if self.primary_file_orientation
                    && !self
                        .obligations
                        .contains(&OBLIGATION_PRIMARY_FILE_ORIENTATION_ONLY.to_owned())
                {
                    return Err(SnapError::Packet(
                        "a primary-file snap must carry the orientation-only obligation".into(),
                    ));
                }
            }
            SnapOutcomeKind::Escaped | SnapOutcomeKind::Refused => {
                // No claim at all: no handle, no roots, no atoms, no metrics.
                if has_claim {
                    return Err(SnapError::Packet(
                        "escaped/refused packet must not carry a certified claim".into(),
                    ));
                }
                if !self.atoms.is_empty() {
                    return Err(SnapError::Packet(
                        "escaped/refused packet must not carry atoms".into(),
                    ));
                }
                if self.metrics.is_some() {
                    return Err(SnapError::Packet(
                        "escaped/refused packet must not carry metrics".into(),
                    ));
                }
                if !self.proved_levels.is_empty() || !self.evidence_refs.is_empty() {
                    return Err(SnapError::Packet(
                        "escaped/refused packet must not claim proved levels or evidence".into(),
                    ));
                }
                if self.reasons.is_empty() {
                    return Err(SnapError::Packet(
                        "escaped/refused packet must carry typed reasons".into(),
                    ));
                }
                if self.primary_file_orientation {
                    return Err(SnapError::Packet(
                        "escaped/refused packet must not flag primary-file orientation".into(),
                    ));
                }
                if self.outcome == SnapOutcomeKind::Escaped && !self.baseline_escape {
                    return Err(SnapError::Packet(
                        "escaped packet must open the baseline escape".into(),
                    ));
                }
                if self.outcome == SnapOutcomeKind::Refused && self.baseline_escape {
                    return Err(SnapError::Packet(
                        "refused packet must not open the baseline escape".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// The canonical rendering: deterministic sorted-key JSON. Two harness
    /// adapters can never disagree on these bytes.
    pub fn canonical_render_json(&self) -> String {
        let value = serde_json::to_value(self)
            .expect("SnapPacket canonical render serializes by construction");
        canonical_json(&value)
    }

    /// The digest root of the packet: SHA-256 over the canonical rendering.
    pub fn packet_root(&self) -> String {
        sha256_hex(self.canonical_render_json().as_bytes())
    }

    /// Fail-closed root verification: the canonical rendering must hash to
    /// the given root, or the packet (or the root) was tampered with.
    pub fn verify_root(&self, root: &str) -> Result<(), SnapError> {
        if self.packet_root() == root {
            Ok(())
        } else {
            Err(SnapError::Packet("packet root mismatch".into()))
        }
    }
}

// ---------------------------------------------------------------------------
// Snap outcome
// ---------------------------------------------------------------------------

/// Outcome of one read-only snap.
#[derive(Clone, Debug, PartialEq)]
pub enum SnapOutcome {
    /// Safe: the certified closure was snapped. `expansion` is the exactly
    /// one first expansion (root/projection exact), `handle` is the
    /// read-only continuation credential, `view` is the decision view
    /// (Proved), and `packet` is the adapter-stable wire form.
    Snapped {
        packet: SnapPacket,
        view: DecisionView,
        expansion: FirstExpansion,
        handle: SafeExpandHandle,
    },
    /// Unknown coverage: escaped to the frozen native baseline with the
    /// strategy preserved. Nothing was issued and nothing was labeled
    /// complete.
    Escaped {
        packet: SnapPacket,
        view: DecisionView,
    },
    /// Unsafe demand: refused with typed reasons. Nothing was issued and
    /// nothing was labeled complete.
    Refused {
        packet: SnapPacket,
        view: DecisionView,
    },
}

impl SnapOutcome {
    pub fn packet(&self) -> &SnapPacket {
        match self {
            SnapOutcome::Snapped { packet, .. }
            | SnapOutcome::Escaped { packet, .. }
            | SnapOutcome::Refused { packet, .. } => packet,
        }
    }

    pub fn view(&self) -> &DecisionView {
        match self {
            SnapOutcome::Snapped { view, .. }
            | SnapOutcome::Escaped { view, .. }
            | SnapOutcome::Refused { view, .. } => view,
        }
    }

    pub fn outcome_kind(&self) -> SnapOutcomeKind {
        match self {
            SnapOutcome::Snapped { .. } => SnapOutcomeKind::Snapped,
            SnapOutcome::Escaped { .. } => SnapOutcomeKind::Escaped,
            SnapOutcome::Refused { .. } => SnapOutcomeKind::Refused,
        }
    }
}

// ---------------------------------------------------------------------------
// The trusted Snap-to-File route
// ---------------------------------------------------------------------------

/// The trusted hub-owned Snap-to-File gate. Owns the W9-E route (issuance
/// secret, live bindings, exactly-once sessions) and the whole
/// request -> decision-view + packet mapping. Read-only: the only
/// credential ever produced is the read-only [`SafeExpandHandle`]; there is
/// no edit, transaction, or commit surface anywhere.
#[derive(Clone, Debug)]
pub struct SnapToFileRoute {
    w9e: W9eRoute,
}

impl SnapToFileRoute {
    /// Construct the gate over the W9-E route's trusted bindings.
    pub fn new(
        secret: [u8; 32],
        tenant: String,
        epoch: u64,
        index_root: Sha256Digest,
        index_version: String,
    ) -> Result<Self, SnapError> {
        let w9e = W9eRoute::new(secret, tenant, epoch, index_root, index_version)
            .map_err(SnapError::W9E)?;
        Ok(Self { w9e })
    }

    fn build_snapped_view(
        plan_root: Sha256Digest,
        project_root: &str,
        index_root: &str,
        handle_id: Sha256Digest,
        certificate_root: Sha256Digest,
    ) -> Result<DecisionView, SnapError> {
        DecisionView::new(
            plan_root.to_hex(),
            project_root.to_owned(),
            index_root.to_owned(),
            vec![SNAP_SUPPORTED_DECISION.to_owned()],
            vec![certificate_root.to_hex()],
            Vec::new(),
            vec![handle_id.to_hex()],
            CompletenessGrade::Proved,
            None,
            false,
            None,
        )
        .map_err(SnapError::View)
    }

    fn build_non_safe_view(
        request_root: &str,
        project_root: &str,
        index_root: &str,
        baseline_escape: bool,
    ) -> Result<DecisionView, SnapError> {
        DecisionView::new(
            request_root.to_owned(),
            project_root.to_owned(),
            index_root.to_owned(),
            vec![SNAP_SUPPORTED_DECISION.to_owned()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            CompletenessGrade::Unknown,
            None,
            baseline_escape,
            None,
        )
        .map_err(SnapError::View)
    }

    fn snapped_obligations(expansion: &FirstExpansion, primary: bool) -> Vec<String> {
        let mut obligations = vec![
            OBLIGATION_VERIFY_PROJECTION.to_owned(),
            OBLIGATION_NO_EDIT_AUTHORITY.to_owned(),
            OBLIGATION_REVALIDATE_BEFORE_CONTINUATION.to_owned(),
        ];
        if !expansion.session.terminal() {
            obligations.push(OBLIGATION_EXPAND_REMAINING_CLOSURE.to_owned());
        }
        if primary {
            obligations.push(OBLIGATION_PRIMARY_FILE_ORIENTATION_ONLY.to_owned());
        }
        obligations
    }

    fn snapped_metrics(expansion: &FirstExpansion, native: &NativeBaseline) -> SnapMetrics {
        SnapMetrics {
            visible_bytes: expansion.visible_bytes,
            backend_work: expansion.ledger.total("backend_work"),
            retry_count: expansion.ledger.total("retry_count"),
            first_try_sufficiency: expansion.first_try_sufficiency,
            // By construction: a completeness claim only ever covers the
            // certified envelope, and the corpus adjudicates that claim
            // against ground truth separately.
            false_complete: false,
            certified_atoms: expansion.certified_atoms,
            expanded_atoms: expansion.atoms.len(),
            native_baseline_bytes: native.discovery_bytes,
            native_baseline_probes: native.probe_count,
            native_savings_bytes: native
                .discovery_bytes
                .saturating_sub(expansion.visible_bytes),
        }
    }

    /// Run one read-only snap: compile the demand, run the total
    /// completeness check through the published GraphZero inputs, and on a
    /// `Safe` fold issue the read-only handle and perform exactly one first
    /// expansion -- no model-visible discovery, no hidden retry, no second
    /// target-ref grammar. `Unknown` escapes to the native baseline with
    /// the strategy preserved; `Unsafe` refuses with typed reasons.
    pub fn snap(
        &mut self,
        manifest: &ProjectImageManifest,
        request: &DemandRequest,
        scope: &ProtectedScope,
        input: &GraphZeroCompletenessInput,
        native: &NativeBaseline,
    ) -> Result<SnapOutcome, SnapError> {
        if manifest.root == Sha256Digest::ZERO {
            return Err(SnapError::InvalidInput(
                "project root must be nonzero".into(),
            ));
        }
        request
            .validate()
            .map_err(|error| SnapError::InvalidInput(format!("request: {error}")))?;
        scope
            .validate()
            .map_err(|error| SnapError::InvalidInput(format!("scope: {error}")))?;
        input
            .validate()
            .map_err(|error| SnapError::InvalidInput(format!("completeness input: {error}")))?;

        let project_root = manifest.root.to_hex();
        let scope_root = scope.scope_root.to_hex();
        let request_root = request.request_root.to_hex();
        let index_root = input.index_root.to_hex();
        let index_version = input.index_version.clone();

        match self
            .w9e
            .compile_and_check(manifest, request, scope, input)
            .map_err(SnapError::W9E)?
        {
            RouteOutcome::Issued {
                handle,
                plan,
                certificate_root,
                checker_identity,
                checker_version,
            } => {
                // Exactly one first expansion, live-revalidated.
                let live = self
                    .w9e
                    .current_live_state(&handle, SafetyVerdict::Safe, false)
                    .map_err(SnapError::W9E)?;
                let expansion = self
                    .w9e
                    .expand_first(&handle, &live, native)
                    .map_err(SnapError::W9E)?;

                // The decision view: Proved, bound to the certificate and
                // the read-only handle. Every root is route-proven.
                let view = Self::build_snapped_view(
                    plan.plan_root,
                    &project_root,
                    &index_root,
                    handle.handle_id(),
                    certificate_root,
                )?;
                let present_classes: BTreeSet<String> = [
                    EVIDENCE_CLASS_TASK_CONTRACT,
                    EVIDENCE_CLASS_PROJECT,
                    EVIDENCE_CLASS_CAUSAL_LENS,
                    EVIDENCE_CLASS_COVERAGE,
                    EVIDENCE_CLASS_EXACT_S0,
                ]
                .into_iter()
                .map(str::to_owned)
                .collect();
                match view.certificate(&snap_evidence_classes(), &present_classes) {
                    Ok(CompletenessGrade::Proved) => {}
                    Ok(_) => {
                        return Err(SnapError::Internal("snapped view failed to certify Proved"));
                    }
                    Err(error) => return Err(SnapError::View(error)),
                }

                let primary = request.projection_atoms.len() == 1;
                let obligations = Self::snapped_obligations(&expansion, primary);
                let metrics = Self::snapped_metrics(&expansion, native);
                let atoms: Vec<PacketAtom> = expansion
                    .atoms
                    .iter()
                    .map(|atom| PacketAtom {
                        atom_root: atom.atom_root.to_hex(),
                        byte_len: atom.byte_len,
                    })
                    .collect();

                let packet = SnapPacket {
                    schema_version: SNAP_TO_FILE_SCHEMA_VERSION.to_owned(),
                    packet_version: SNAP_PACKET_VERSION,
                    outcome: SnapOutcomeKind::Snapped,
                    family: W9E_FAMILY_NAME.to_owned(),
                    request_root,
                    project_root,
                    scope_root,
                    index_root,
                    index_version,
                    plan_root: Some(plan.plan_root.to_hex()),
                    projection_root: Some(plan.projection_root.to_hex()),
                    certificate_root: Some(certificate_root.to_hex()),
                    checker_identity: Some(checker_identity),
                    checker_version: Some(checker_version),
                    handle_id: Some(handle.handle_id().to_hex()),
                    proved_levels: proved_levels(),
                    unproved_levels: unproved_levels(),
                    evidence_refs: vec![certificate_root.to_hex()],
                    obligations,
                    atoms,
                    metrics: Some(metrics),
                    baseline_escape: false,
                    primary_file_orientation: primary,
                    reasons: Vec::new(),
                    decision_view_root: view.root(),
                };
                packet.validate()?;

                Ok(SnapOutcome::Snapped {
                    packet,
                    view,
                    expansion,
                    handle,
                })
            }
            RouteOutcome::Refused { verdict } => {
                let (kind, baseline_escape, obligations) = match &verdict {
                    SafetyVerdict::Unknown { .. } => (
                        SnapOutcomeKind::Escaped,
                        true,
                        vec![
                            OBLIGATION_NATIVE_ESCAPE.to_owned(),
                            OBLIGATION_NO_COMPLETENESS_CLAIM.to_owned(),
                        ],
                    ),
                    SafetyVerdict::Unsafe { .. } => (
                        SnapOutcomeKind::Refused,
                        false,
                        vec![
                            OBLIGATION_DEMAND_REFUSED.to_owned(),
                            OBLIGATION_NO_COMPLETENESS_CLAIM.to_owned(),
                        ],
                    ),
                    SafetyVerdict::Safe => {
                        return Err(SnapError::Internal("route refused with a Safe verdict"));
                    }
                };

                // The view records the refusal surface honestly: claimed
                // grade Unknown (no completeness claim), no evidence refs,
                // no expansion handles.
                let view = Self::build_non_safe_view(
                    &request_root,
                    &project_root,
                    &index_root,
                    baseline_escape,
                )?;

                let packet = SnapPacket {
                    schema_version: SNAP_TO_FILE_SCHEMA_VERSION.to_owned(),
                    packet_version: SNAP_PACKET_VERSION,
                    outcome: kind,
                    family: W9E_FAMILY_NAME.to_owned(),
                    request_root,
                    project_root,
                    scope_root,
                    index_root,
                    index_version,
                    plan_root: None,
                    projection_root: None,
                    certificate_root: None,
                    checker_identity: None,
                    checker_version: None,
                    handle_id: None,
                    proved_levels: Vec::new(),
                    unproved_levels: unproved_levels(),
                    evidence_refs: Vec::new(),
                    obligations,
                    atoms: Vec::new(),
                    metrics: None,
                    baseline_escape,
                    primary_file_orientation: false,
                    reasons: verdict.reasons().to_vec(),
                    decision_view_root: view.root(),
                };
                packet.validate()?;

                if baseline_escape {
                    Ok(SnapOutcome::Escaped { packet, view })
                } else {
                    Ok(SnapOutcome::Refused { packet, view })
                }
            }
        }
    }

    /// Live revalidation of one read-only handle (passthrough to the W9-E
    /// route; typed outcome).
    pub fn revalidate(&self, handle: &SafeExpandHandle, live: &LiveExpandState) -> ExpandOutcome {
        self.w9e.revalidate(handle, live)
    }

    /// Exactly one first expansion of a trusted, live-revalidated handle
    /// (passthrough to the W9-E route behind canonical `z.read`).
    pub fn expand_first(
        &mut self,
        handle: &SafeExpandHandle,
        live: &LiveExpandState,
        native: &NativeBaseline,
    ) -> Result<FirstExpansion, SnapError> {
        self.w9e
            .expand_first(handle, live, native)
            .map_err(SnapError::W9E)
    }

    /// Build the live hub state that mirrors this gate's bindings for one
    /// issued handle (passthrough; tests mutate fields to exercise
    /// stale/mismatch paths).
    pub fn current_live_state(
        &self,
        handle: &SafeExpandHandle,
        verdict: SafetyVerdict,
        hidden_retry_after_issue: bool,
    ) -> Result<LiveExpandState, SnapError> {
        self.w9e
            .current_live_state(handle, verdict, hidden_retry_after_issue)
            .map_err(SnapError::W9E)
    }

    /// Append one continuation-bound incremental delta (passthrough to the
    /// W9-E route: new atoms only, live revalidation first).
    pub fn expand_delta(
        &mut self,
        session: &IncrementalSession,
        request: &IncrementalDeltaRequest,
        live: &LiveExpandState,
    ) -> Result<IncrementalDelta, SnapError> {
        self.w9e
            .expand_delta(session, request, live)
            .map_err(SnapError::W9E)
    }
}
