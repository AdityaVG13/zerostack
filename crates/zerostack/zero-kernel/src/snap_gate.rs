use std::collections::BTreeMap;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use zero_abi::{
    DecisionView, ExpandPermit, SafeExpandHandle, Sha256Digest, ZeroHandle, canonical_json,
};
use zero_gate::{
    DemandPlan, DemandRequest, ExpandLedger, ExpandedAtom, FirstExpansion,
    GraphZeroCompletenessInput, NativeBaseline, ProjectImageManifest, ProtectedScope, SnapOutcome,
    SnapPacket, SnapToFileRoute,
};
use zero_store::ZeroCas;

const MAX_LIVE_ROUTES: usize = 64;
const MAX_TRUSTED_COMPLETENESS_INPUTS: usize = 64;
const SNAP_COMPLETENESS_SCHEMA: &str = "zerostack.zero_kernel.snap_completeness";
pub const SNAP_TO_FILE_READ_SCHEMA: &str = "zerostack.zero_kernel.snap_to_file";

/// One proof-carrying Snap-to-File request routed through `z.read`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapToFileReadRequest {
    pub manifest: ProjectImageManifest,
    pub demand: DemandRequest,
    pub scope: ProtectedScope,
    /// Opaque handle registered by the trusted host from GraphZero evidence.
    pub completeness: ZeroHandle,
    pub native_baseline: NativeBaseline,
}

/// Adapter-stable result of a proof-carrying Snap-to-File read.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapToFileReadResult {
    pub schema: &'static str,
    pub packet: SnapPacket,
    pub view: DecisionView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expansion: Option<SnapFirstExpansion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<SafeExpandHandle>,
}

/// Serializable projection of the internal exactly-once continuation state.
#[derive(Clone, Debug, Serialize)]
pub struct SnapIncrementalSession {
    pub handle_id: Sha256Digest,
    pub delta_seq: u64,
    pub terminal: bool,
}

/// Serializable projection of one first expansion. The gate keeps its mutable
/// exactly-once session private; this value exposes only read-only evidence.
#[derive(Clone, Debug, Serialize)]
pub struct SnapFirstExpansion {
    pub handle_id: Sha256Digest,
    pub permit: ExpandPermit,
    pub plan: DemandPlan,
    pub atoms: Vec<ExpandedAtom>,
    pub projection_root: Sha256Digest,
    pub visible_bytes: u64,
    pub certified_atoms: usize,
    pub first_try_sufficiency: bool,
    pub ledger: ExpandLedger,
    pub native_baseline: NativeBaseline,
    pub session: SnapIncrementalSession,
}

impl From<FirstExpansion> for SnapFirstExpansion {
    fn from(expansion: FirstExpansion) -> Self {
        let session = SnapIncrementalSession {
            handle_id: expansion.session.handle_id(),
            delta_seq: expansion.session.delta_seq(),
            terminal: expansion.session.terminal(),
        };
        Self {
            handle_id: expansion.handle_id,
            permit: expansion.permit,
            plan: expansion.plan,
            atoms: expansion.atoms,
            projection_root: expansion.projection_root,
            visible_bytes: expansion.visible_bytes,
            certified_atoms: expansion.certified_atoms,
            first_try_sufficiency: expansion.first_try_sufficiency,
            ledger: expansion.ledger,
            native_baseline: expansion.native_baseline,
            session,
        }
    }
}

impl From<SnapOutcome> for SnapToFileReadResult {
    fn from(outcome: SnapOutcome) -> Self {
        match outcome {
            SnapOutcome::Snapped {
                packet,
                view,
                expansion,
                handle,
            } => Self {
                schema: SNAP_TO_FILE_READ_SCHEMA,
                packet,
                view,
                expansion: Some(expansion.into()),
                handle: Some(handle),
            },
            SnapOutcome::Escaped { packet, view } | SnapOutcome::Refused { packet, view } => Self {
                schema: SNAP_TO_FILE_READ_SCHEMA,
                packet,
                view,
                expansion: None,
                handle: None,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RouteKey {
    index_root: Sha256Digest,
    index_version: String,
}

struct SnapToFileState {
    routes: BTreeMap<RouteKey, SnapToFileRoute>,
    trusted_completeness: BTreeMap<String, GraphZeroCompletenessInput>,
}

/// Kernel-owned route state. The issuance secret, trusted GraphZero inputs, and exactly-once
/// sessions live for the host lifetime and never enter guest-controlled state.
pub(crate) struct SnapToFileService {
    secret: [u8; 32],
    tenant: String,
    epoch: u64,
    state: Mutex<SnapToFileState>,
}

impl SnapToFileService {
    pub(crate) fn new(tenant: String) -> Result<Self, String> {
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret)
            .map_err(|error| format!("initialize Snap-to-File issuer: {error}"))?;
        let epoch_bytes = secret
            .first_chunk::<8>()
            .copied()
            .ok_or_else(|| "Snap-to-File issuer secret is shorter than eight bytes".to_owned())?;
        let epoch = u64::from_le_bytes(epoch_bytes).max(1);
        Ok(Self {
            secret,
            tenant,
            epoch,
            state: Mutex::new(SnapToFileState {
                routes: BTreeMap::new(),
                trusted_completeness: BTreeMap::new(),
            }),
        })
    }

    pub(crate) fn register_completeness(
        &self,
        cas: &ZeroCas,
        input: GraphZeroCompletenessInput,
    ) -> Result<ZeroHandle, String> {
        input.validate().map_err(|error| error.to_string())?;
        let input_value = serde_json::to_value(&input)
            .map_err(|error| format!("serialize completeness: {error}"))?;
        let envelope = serde_json::json!({
            "schema": SNAP_COMPLETENESS_SCHEMA,
            "input": input_value,
        });
        let handle = cas
            .put(canonical_json(&envelope).as_bytes())
            .map_err(|error| format!("store trusted completeness: {error}"))?;
        let mut state = self.state.lock();
        if !state.trusted_completeness.contains_key(handle.as_str())
            && state.trusted_completeness.len() >= MAX_TRUSTED_COMPLETENESS_INPUTS
        {
            return Err(format!(
                "trusted Snap-to-File completeness limit {MAX_TRUSTED_COMPLETENESS_INPUTS} reached for this host"
            ));
        }
        state
            .trusted_completeness
            .insert(handle.as_str().to_owned(), input);
        Ok(handle)
    }

    pub(crate) fn snap(
        &self,
        request: SnapToFileReadRequest,
    ) -> Result<SnapToFileReadResult, String> {
        let mut state = self.state.lock();
        let SnapToFileState {
            routes,
            trusted_completeness,
        } = &mut *state;
        let input = trusted_completeness
            .get(request.completeness.as_str())
            .ok_or_else(|| {
                "Snap-to-File completeness handle was not registered by the trusted host".to_owned()
            })?;
        let key = RouteKey {
            index_root: input.index_root,
            index_version: input.index_version.clone(),
        };
        if !routes.contains_key(&key) && routes.len() >= MAX_LIVE_ROUTES {
            return Err(format!(
                "Snap-to-File route limit {MAX_LIVE_ROUTES} reached for this host"
            ));
        }
        let route = match routes.entry(key) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let key = entry.key();
                let route = SnapToFileRoute::new(
                    self.secret,
                    self.tenant.clone(),
                    self.epoch,
                    key.index_root,
                    key.index_version.clone(),
                )
                .map_err(|error| error.to_string())?;
                entry.insert(route)
            }
        };
        route
            .snap(
                &request.manifest,
                &request.demand,
                &request.scope,
                input,
                &request.native_baseline,
            )
            .map(SnapToFileReadResult::from)
            .map_err(|error| error.to_string())
    }
}
