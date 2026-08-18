//! K0 W9-E live rooted evidence and the guest wave-9 route
//! (`zerostack-fhcj`).
//!
//! The supervisor holds optional [`W9eEvidence`] — the hub-side bindings of
//! the Snap-to-File gate (route secret, tenant/epoch/index binding) plus the
//! live rooted project image, protected scope, GraphZero completeness input,
//! and native baseline. Without evidence, `z.resolve` / `z.expand` /
//! `z.snap` / `z.view` fail typed ("no live rooted evidence"); with it, the
//! guest wave-9 seam runs the real W9-E chain:
//!
//! - `z.resolve(demand)` issues a [`SafeExpandHandle`] only after the total
//!   completeness check folds `Safe` (Escaped/Refused never mint anything);
//! - `z.expand(handle)` revalidates the handle against live hub state and
//!   performs exactly one first expansion;
//! - `z.snap(task)` returns the read-only decision view + adapter-stable
//!   packet for every outcome;
//! - `z.view(handle)` live-revalidates a trusted handle and returns its
//!   Proved decision view.
//!
//! Guest JS never constructs authority: the only handle constructor is the
//! route's issuer (keyed MAC), every handle use revalidates against the
//! route's per-call session registry, and the protected scope / completeness
//! input / native baseline are hub-owned and never guest-typed.

use std::sync::Mutex;

use serde::Deserialize;
use serde_json::{Value as JsonValue, json};
use zero_abi::{
    CompletenessGrade, DecisionView, SafeExpandHandle, SafetyVerdict, Sha256Digest,
};
use zero_codemode::guest::GuestWave9;
use zero_gate::{
    DemandRequest, EVIDENCE_CLASS_CAUSAL_LENS, EVIDENCE_CLASS_COVERAGE,
    EVIDENCE_CLASS_EXACT_S0, EVIDENCE_CLASS_PROJECT, EVIDENCE_CLASS_TASK_CONTRACT,
    FirstExpansion, GraphZeroCompletenessInput, NativeBaseline, ProjectImageManifest,
    ProtectedScope, SNAP_SUPPORTED_DECISION, SnapError, SnapOutcome, SnapOutcomeKind,
    SnapToFileRoute, snap_evidence_classes,
};

/// The five evidence classes a Proved snap claim needs; the guest view path
/// certifies exactly the classes the route holds (mirrors the snap flow).
const SNAPPED_PRESENT_CLASSES: [&str; 5] = [
    EVIDENCE_CLASS_TASK_CONTRACT,
    EVIDENCE_CLASS_PROJECT,
    EVIDENCE_CLASS_CAUSAL_LENS,
    EVIDENCE_CLASS_COVERAGE,
    EVIDENCE_CLASS_EXACT_S0,
];

/// Live rooted evidence for the K0 wave-9 seam. Built only by trusted hub
/// code; the guest can never supply any of it.
#[derive(Clone, Debug)]
pub struct W9eEvidence {
    /// Route issuance secret (guests never see it).
    pub secret: [u8; 32],
    /// Tenant bound into every issued handle.
    pub tenant: String,
    /// Epoch bound into every issued handle.
    pub epoch: u64,
    /// Live GraphZero index lens root.
    pub index_root: Sha256Digest,
    /// Live GraphZero index version.
    pub index_version: String,
    /// The live rooted project image manifest (W8 shadow reporter value).
    pub manifest: ProjectImageManifest,
    /// The hub-protected scope expansion must never return.
    pub scope: ProtectedScope,
    /// The published GraphZero completeness input the checker consumes.
    pub completeness_input: GraphZeroCompletenessInput,
    /// The declared native-discovery counterfactual for the same atoms.
    pub native: NativeBaseline,
}

impl W9eEvidence {
    /// Build evidence, fail-fast: the route bindings and every rooted input
    /// must already validate, so a misconfigured supervisor fails at build
    /// time, never mid-call.
    pub fn new(
        secret: [u8; 32],
        tenant: String,
        epoch: u64,
        index_root: Sha256Digest,
        index_version: String,
        manifest: ProjectImageManifest,
        scope: ProtectedScope,
        completeness_input: GraphZeroCompletenessInput,
        native: NativeBaseline,
    ) -> Result<Self, SnapError> {
        if manifest.root == Sha256Digest::ZERO {
            return Err(SnapError::InvalidInput(
                "project root must be nonzero".into(),
            ));
        }
        scope
            .validate()
            .map_err(|error| SnapError::InvalidInput(format!("scope: {error}")))?;
        completeness_input
            .validate()
            .map_err(|error| SnapError::InvalidInput(format!("completeness input: {error}")))?;
        // Constructing the route validates the secret-bound bindings
        // (tenant/index version bounds, nonzero index root).
        SnapToFileRoute::new(secret, tenant.clone(), epoch, index_root, index_version.clone())
            .map_err(|error| {
                SnapError::InvalidInput(format!("route bindings: {error}"))
            })?;
        Ok(Self {
            secret,
            tenant,
            epoch,
            index_root,
            index_version,
            manifest,
            scope,
            completeness_input,
            native,
        })
    }
}

/// The guest demand grammar: one scenario id plus projection atom roots
/// (hex digests), exactly the W9-E target-ref grammar. `deny_unknown_fields`
/// so a guest cannot smuggle extra authority-shaped fields.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GuestDemand {
    scenario_id: String,
    projection_atoms: Vec<String>,
}

fn parse_demand(value: JsonValue) -> Result<DemandRequest, String> {
    let demand: GuestDemand = serde_json::from_value(value)
        .map_err(|error| format!("demand is not {{scenario_id, projection_atoms}}: {error}"))?;
    if demand.projection_atoms.is_empty() {
        return Err("demand projection_atoms must not be empty".into());
    }
    let mut atoms = Vec::with_capacity(demand.projection_atoms.len());
    for atom in &demand.projection_atoms {
        atoms.push(Sha256Digest::from_hex(atom).map_err(|error| {
            format!("demand atom '{atom}' is not a hex digest: {error}")
        })?);
    }
    DemandRequest::new(demand.scenario_id, atoms)
        .map_err(|error| format!("demand is invalid: {error}"))
}

fn parse_handle(value: JsonValue) -> Result<SafeExpandHandle, String> {
    serde_json::from_value(value).map_err(|error| {
        format!("handle is not a valid SafeExpandHandle wire form: {error}")
    })
}

fn expansion_json(expansion: &FirstExpansion) -> JsonValue {
    let atoms: Vec<JsonValue> = expansion
        .atoms
        .iter()
        .map(|atom| {
            json!({
                "atom_root": atom.atom_root.to_hex(),
                "byte_len": atom.byte_len,
            })
        })
        .collect();
    json!({
        "handle_id": expansion.handle_id.to_hex(),
        "atoms": atoms,
        "projection_root": expansion.projection_root.to_hex(),
        "visible_bytes": expansion.visible_bytes,
        "certified_atoms": expansion.certified_atoms,
        "first_try_sufficiency": expansion.first_try_sufficiency,
        "terminal": expansion.session.terminal(),
    })
}

fn refusal_detail(kind: SnapOutcomeKind, reasons: &[String]) -> String {
    format!("z wave-9 refusal: {kind:?} ({})", reasons.join(", "))
}

/// The guest wave-9 route: a per-call Snap-to-File gate over the supervisor's
/// live rooted evidence. The per-call route owns the exactly-once session
/// registry, so a fresh runtime can never reuse another call's expansion
/// state and every handle use revalidates live first.
pub struct SupervisorGuestWave9 {
    route: Mutex<SnapToFileRoute>,
    /// The first expansion of every handle minted by this call. The route
    /// performs the exactly-one first expansion at issuance (inside
    /// `snap`); `z.expand(handle)` live-revalidates the handle and replays
    /// that same exact projection — a handle can never expand twice, and
    /// the replayed atoms are the identical issuance-time atoms.
    expansions: Mutex<std::collections::BTreeMap<Sha256Digest, FirstExpansion>>,
    manifest: ProjectImageManifest,
    scope: ProtectedScope,
    completeness_input: GraphZeroCompletenessInput,
    native: NativeBaseline,
}

impl SupervisorGuestWave9 {
    /// Construct the per-call route over the evidence.
    pub fn new(evidence: &W9eEvidence) -> Result<Self, SnapError> {
        let route = SnapToFileRoute::new(
            evidence.secret,
            evidence.tenant.clone(),
            evidence.epoch,
            evidence.index_root,
            evidence.index_version.clone(),
        )
        .map_err(|error| {
            SnapError::InvalidInput(format!("route bindings: {error}"))
        })?;
        Ok(Self {
            route: Mutex::new(route),
            expansions: Mutex::new(std::collections::BTreeMap::new()),
            manifest: evidence.manifest.clone(),
            scope: evidence.scope.clone(),
            completeness_input: evidence.completeness_input.clone(),
            native: evidence.native,
        })
    }

    fn route(&self) -> Result<std::sync::MutexGuard<'_, SnapToFileRoute>, String> {
        self.route
            .lock()
            .map_err(|_| "w9e route lock poisoned".to_owned())
    }

    fn expansions(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, std::collections::BTreeMap<Sha256Digest, FirstExpansion>>, String>
    {
        self.expansions
            .lock()
            .map_err(|_| "w9e expansion registry lock poisoned".to_owned())
    }
}

impl GuestWave9 for SupervisorGuestWave9 {
    fn resolve(&self, demand: JsonValue) -> Result<JsonValue, String> {
        let request = parse_demand(demand)?;
        // The route lock covers the snap only; the expansion registry is a
        // separate lock and is never held while the route is.
        let outcome = {
            let mut route = self.route()?;
            route
                .snap(
                    &self.manifest,
                    &request,
                    &self.scope,
                    &self.completeness_input,
                    &self.native,
                )
                .map_err(|error| format!("z.resolve: {error}"))?
        };
        match outcome {
            SnapOutcome::Snapped {
                packet,
                view,
                expansion,
                handle,
            } => {
                self.expansions()?
                    .insert(handle.handle_id(), expansion.clone());
                let handle_json = serde_json::to_value(&handle)
                    .map_err(|error| format!("z.resolve: handle serialization failed: {error}"))?;
                let handle_id = handle.handle_id().to_hex();
                let mut result = expansion_json(&expansion);
                let object = result
                    .as_object_mut()
                    .expect("expansion_json is an object");
                object.insert("handle".into(), handle_json);
                object.insert("handle_id".into(), JsonValue::String(handle_id));
                object.insert(
                    "packet".into(),
                    serde_json::to_value(&packet)
                        .map_err(|error| format!("z.resolve: packet serialization failed: {error}"))?,
                );
                object.insert(
                    "view".into(),
                    serde_json::to_value(&view)
                        .map_err(|error| format!("z.resolve: view serialization failed: {error}"))?,
                );
                Ok(result)
            }
            SnapOutcome::Escaped { packet, .. } => Err(format!(
                "z.resolve: {} (baseline_escape={})",
                refusal_detail(SnapOutcomeKind::Escaped, &packet.reasons),
                packet.baseline_escape
            )),
            SnapOutcome::Refused { packet, .. } => Err(format!(
                "z.resolve: {}",
                refusal_detail(SnapOutcomeKind::Refused, &packet.reasons)
            )),
        }
    }

    fn expand(&self, handle: JsonValue) -> Result<JsonValue, String> {
        let handle = parse_handle(handle)?;
        let handle_id = handle.handle_id();
        {
            // Live revalidation first: the route builds the live state from
            // its own session registry (an unknown or stale handle fails
            // typed) and revalidates every handle binding against it.
            let route = self.route()?;
            let live = route
                .current_live_state(&handle, SafetyVerdict::Safe, false)
                .map_err(|error| format!("z.expand: {error}"))?;
            match route.revalidate(&handle, &live) {
                zero_abi::ExpandOutcome::Safe(_) => {}
                zero_abi::ExpandOutcome::Unsafe { reasons }
                | zero_abi::ExpandOutcome::Unknown { reasons } => {
                    return Err(format!(
                        "z.expand: handle revalidation is not Safe ({})",
                        reasons.join(", ")
                    ));
                }
            }
        }
        // Replay the exactly-one first expansion this call's issuance
        // performed: the route refuses a second first expansion, so the
        // registry replays the identical issuance-time atoms.
        let expansions = self.expansions()?;
        let expansion = expansions
            .get(&handle_id)
            .ok_or_else(|| {
                format!(
                    "z.expand: handle {} was not issued by this call",
                    handle_id.to_hex()
                )
            })?;
        Ok(expansion_json(expansion))
    }

    fn snap(&self, task: JsonValue) -> Result<JsonValue, String> {
        let request = parse_demand(task)?;
        let mut route = self.route()?;
        let outcome = route
            .snap(
                &self.manifest,
                &request,
                &self.scope,
                &self.completeness_input,
                &self.native,
            )
            .map_err(|error| format!("z.snap: {error}"))?;
        let (kind, packet, view) = match &outcome {
            SnapOutcome::Snapped { packet, view, .. } => {
                (SnapOutcomeKind::Snapped, packet, view)
            }
            SnapOutcome::Escaped { packet, view } => (SnapOutcomeKind::Escaped, packet, view),
            SnapOutcome::Refused { packet, view } => (SnapOutcomeKind::Refused, packet, view),
        };
        let packet_json = serde_json::to_value(packet)
            .map_err(|error| format!("z.snap: packet serialization failed: {error}"))?;
        let view_json = serde_json::to_value(view)
            .map_err(|error| format!("z.snap: view serialization failed: {error}"))?;
        let outcome_name = match kind {
            SnapOutcomeKind::Snapped => "snapped",
            SnapOutcomeKind::Escaped => "escaped",
            SnapOutcomeKind::Refused => "refused",
        };
        Ok(json!({
            "outcome": outcome_name,
            "packet": packet_json,
            "view": view_json,
        }))
    }

    fn view(&self, handle: JsonValue) -> Result<JsonValue, String> {
        let handle = parse_handle(handle)?;
        let route = self.route()?;
        // The live state comes from the route's own session registry; the
        // revalidation then checks every handle binding against it.
        let live = route
            .current_live_state(&handle, SafetyVerdict::Safe, false)
            .map_err(|error| format!("z.view: {error}"))?;
        match route.revalidate(&handle, &live) {
            zero_abi::ExpandOutcome::Safe(_) => {}
            zero_abi::ExpandOutcome::Unsafe { reasons }
            | zero_abi::ExpandOutcome::Unknown { reasons } => {
                return Err(format!(
                    "z.view: handle revalidation is not Safe ({})",
                    reasons.join(", ")
                ));
            }
        };
        let certificate_root = handle.completeness().certificate_root().to_hex();
        let view = DecisionView::new(
            handle.demand_plan_root().to_hex(),
            handle.project_root().to_hex(),
            handle.index_root().to_hex(),
            vec![SNAP_SUPPORTED_DECISION.to_owned()],
            vec![certificate_root.clone()],
            Vec::new(),
            vec![handle.handle_id().to_hex()],
            CompletenessGrade::Proved,
            None,
            false,
            None,
        )
        .map_err(|error| format!("z.view: cannot build decision view: {error}"))?;
        let present_classes: std::collections::BTreeSet<String> = SNAPPED_PRESENT_CLASSES
            .into_iter()
            .map(str::to_owned)
            .collect();
        match view.certificate(&snap_evidence_classes(), &present_classes) {
            Ok(CompletenessGrade::Proved) => {}
            Ok(_) => {
                return Err("z.view: view failed to certify Proved".into());
            }
            Err(error) => return Err(format!("z.view: view certificate: {error}")),
        }
        serde_json::to_value(&view)
            .map_err(|error| format!("z.view: view serialization failed: {error}"))
    }
}
