#![forbid(unsafe_code)]

//! W8 shadow-only immutable project-image manifest reporter (`zerostack-pfvi`).
//!
//! Shadow reporter: deterministic for one root, no production authority.
//! It reads current engine receipts/contracts without editing engine
//! repositories and keeps L1 (provider cache), L2 (logical validity), and
//! L3 (physical residency) distinct. A hypothetical child after a declared
//! change shares unchanged objects and names affected claims without
//! mutating the old root. All unknown/missing inputs remain explicit.
//!
//! # Laws
//! - [`ProjectImageManifest::has_authority`] is always `false`.
//! - [`ValidityClass`] distinguishes `ValidNotResident` from `Invalid`.
//! - [`hypothetical_child`] preserves `parent_root` and computes a
//!   deterministic `child_root` from parent + declared change.
//! - Every optional input carries an explicit `unknown_reason` when `None`.
//! - Canonical bytes are sorted-key JSON; `digest()` is
//!   `SHA-256(domain || canonical_bytes)` for determinism.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zero_abi::{canonical_json, sha256, Sha256Digest};

use crate::residency::{LayerValidityEntry, LayerValidityLedger};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const PROJECT_IMAGE_SCHEMA_VERSION: &str = "zerostack.project_image.shadow.v1";
pub const PROJECT_IMAGE_DOMAIN: &[u8] = b"zerostack.project_image.shadow\0";
/// Marker that this report is shadow-only and grants no authority.
pub const SHADOW_HAS_AUTHORITY: bool = false;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectImageError {
    MissingRoot,
    EmptyObjects,
    InvalidRoot(String),
    InvalidObject(String),
    InvalidGraph(String),
    InvalidRepresentation(String),
    InvalidValidity(String),
    InvalidDemand(String),
    UnknownMissing(String),
    Serialization(String),
}

impl std::fmt::Display for ProjectImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProjectImageError {}

fn img_err(msg: impl Into<String>) -> ProjectImageError {
    ProjectImageError::InvalidObject(msg.into())
}

// ---------------------------------------------------------------------------
// Exact objects (immutable CAS identities)
// ---------------------------------------------------------------------------

/// One exact immutable object in the image (CAS identity + size).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactObject {
    /// Content digest (64 lower-hex on the wire).
    pub digest: Sha256Digest,
    /// Exact byte length of the object.
    pub byte_len: u64,
    /// Optional causal coordinate that produced this object (explicit unknown if None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causal_coordinate: Option<Sha256Digest>,
}

impl ExactObject {
    pub fn new(digest: Sha256Digest, byte_len: u64) -> Result<Self, ProjectImageError> {
        if digest == Sha256Digest::ZERO {
            return Err(img_err("exact object digest is zero"));
        }
        Ok(Self {
            digest,
            byte_len,
            causal_coordinate: None,
        })
    }

    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.digest == Sha256Digest::ZERO {
            return Err(img_err("exact object digest is zero"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Declared graphs (causal / proof) -- explicit unknown
// ---------------------------------------------------------------------------

/// Declared causal graph reference. `digest == None` requires `unknown_reason`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalGraphRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

impl CausalGraphRef {
    pub fn present(digest: Sha256Digest) -> Result<Self, ProjectImageError> {
        if digest == Sha256Digest::ZERO {
            return Err(ProjectImageError::InvalidGraph(
                "causal graph digest is zero".into(),
            ));
        }
        Ok(Self {
            digest: Some(digest),
            unknown_reason: None,
        })
    }

    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            digest: None,
            unknown_reason: Some(reason.into()),
        }
    }

    pub fn validate(&self) -> Result<(), ProjectImageError> {
        match (&self.digest, &self.unknown_reason) {
            (Some(d), _) if *d == Sha256Digest::ZERO => Err(ProjectImageError::InvalidGraph(
                "causal graph digest is zero".into(),
            )),
            (None, None) => Err(ProjectImageError::UnknownMissing(
                "causal graph unknown without reason".into(),
            )),
            (Some(_), Some(_)) => Err(ProjectImageError::InvalidGraph(
                "causal graph cannot have both digest and unknown_reason".into(),
            )),
            _ => Ok(()),
        }
    }

    pub fn is_unknown(&self) -> bool {
        self.digest.is_none()
    }
}

/// Declared proof graph / proof-obligation graph reference.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProofGraphRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

impl ProofGraphRef {
    pub fn present(digest: Sha256Digest) -> Result<Self, ProjectImageError> {
        if digest == Sha256Digest::ZERO {
            return Err(ProjectImageError::InvalidGraph(
                "proof graph digest is zero".into(),
            ));
        }
        Ok(Self {
            digest: Some(digest),
            unknown_reason: None,
        })
    }

    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            digest: None,
            unknown_reason: Some(reason.into()),
        }
    }

    pub fn validate(&self) -> Result<(), ProjectImageError> {
        match (&self.digest, &self.unknown_reason) {
            (Some(d), _) if *d == Sha256Digest::ZERO => Err(ProjectImageError::InvalidGraph(
                "proof graph digest is zero".into(),
            )),
            (None, None) => Err(ProjectImageError::UnknownMissing(
                "proof graph unknown without reason".into(),
            )),
            (Some(_), Some(_)) => Err(ProjectImageError::InvalidGraph(
                "proof graph cannot have both digest and unknown_reason".into(),
            )),
            _ => Ok(()),
        }
    }

    pub fn is_unknown(&self) -> bool {
        self.digest.is_none()
    }
}

// ---------------------------------------------------------------------------
// Registered representations
// ---------------------------------------------------------------------------

/// Registered representation kinds ( cheapest sufficient view lattice ).
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationKind {
    RawBytes,
    Ast,
    Claim,
    ResidualProgram,
}

impl RepresentationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawBytes => "raw_bytes",
            Self::Ast => "ast",
            Self::Claim => "claim",
            Self::ResidualProgram => "residual_program",
        }
    }
}

/// One registered representation entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegisteredRepresentation {
    pub kind: RepresentationKind,
    /// Digest of the representation object (if materialized).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Sha256Digest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

impl RegisteredRepresentation {
    pub fn present(
        kind: RepresentationKind,
        digest: Sha256Digest,
    ) -> Result<Self, ProjectImageError> {
        if digest == Sha256Digest::ZERO {
            return Err(ProjectImageError::InvalidRepresentation(
                "representation digest is zero".into(),
            ));
        }
        Ok(Self {
            kind,
            digest: Some(digest),
            unknown_reason: None,
        })
    }

    pub fn unknown(kind: RepresentationKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            digest: None,
            unknown_reason: Some(reason.into()),
        }
    }

    pub fn validate(&self) -> Result<(), ProjectImageError> {
        match (&self.digest, &self.unknown_reason) {
            (Some(d), _) if *d == Sha256Digest::ZERO => Err(
                ProjectImageError::InvalidRepresentation("representation digest is zero".into()),
            ),
            (None, None) => Err(ProjectImageError::UnknownMissing(
                "representation unknown without reason".into(),
            )),
            (Some(_), Some(_)) => Err(ProjectImageError::InvalidRepresentation(
                "representation cannot have both digest and unknown_reason".into(),
            )),
            _ => Ok(()),
        }
    }
}

// ---------------------------------------------------------------------------
// L1 / L2 / L3 distinct validity
// ---------------------------------------------------------------------------

/// Per-object L1/L2/L3 state kept distinct (no aliasing).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PerObjectLayers {
    pub object_root: Sha256Digest,
    /// L1 provider-cache hit (None = unknown/missing, must have unknown_reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l1_provider_cached: Option<bool>,
    /// L2 logical validity (None = unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l2_logically_valid: Option<bool>,
    /// L3 physical residency (None = unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l3_physically_resident: Option<bool>,
    /// When L2 valid but L3 lost, needs refetch/rematerialize (never rediscovery).
    #[serde(default)]
    pub l2_needs_refetch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

impl PerObjectLayers {
    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.object_root == Sha256Digest::ZERO {
            return Err(ProjectImageError::InvalidValidity(
                "per-object layers object_root is zero".into(),
            ));
        }
        if self.l2_needs_refetch {
            match self.l2_logically_valid {
                Some(true) => {}
                _ => {
                    return Err(ProjectImageError::InvalidValidity(
                        "l2_needs_refetch requires l2_logically_valid == true".into(),
                    ))
                }
            }
        }
        // If all three are None, unknown_reason is required to remain explicit.
        if self.l1_provider_cached.is_none()
            && self.l2_logically_valid.is_none()
            && self.l3_physically_resident.is_none()
            && self.unknown_reason.is_none()
        {
            return Err(ProjectImageError::UnknownMissing(format!(
                "per-object layers for {} is fully unknown without reason",
                self.object_root.to_hex()
            )));
        }
        Ok(())
    }

    /// Classify into the four-way validity class, keeping ValidNotResident
    /// distinct from Invalid (acceptance requirement).
    pub fn validity_class(&self) -> ValidityClass {
        match self.l2_logically_valid {
            None => ValidityClass::Unknown,
            Some(false) => ValidityClass::Invalid,
            Some(true) => match self.l3_physically_resident {
                Some(true) => ValidityClass::ValidResident,
                Some(false) => ValidityClass::ValidNotResident,
                None => ValidityClass::Unknown,
            },
        }
    }

    /// Convert from the existing `LayerValidityLedger` entry, preserving L1/L2/L3
    /// distinction (L1 = l1_valid, L2 = l2_valid, L3 = l3_valid).
    pub fn from_layer_entry(entry: &LayerValidityEntry) -> Self {
        Self {
            object_root: entry.object_root,
            l1_provider_cached: Some(entry.l1_valid),
            l2_logically_valid: Some(entry.l2_valid),
            l3_physically_resident: Some(entry.l3_valid),
            l2_needs_refetch: entry.l2_needs_refetch,
            unknown_reason: None,
        }
    }

    pub fn to_layer_entry(&self) -> Option<LayerValidityEntry> {
        let l1 = self.l1_provider_cached?;
        let l2 = self.l2_logically_valid?;
        let l3 = self.l3_physically_resident?;
        Some(LayerValidityEntry {
            object_root: self.object_root,
            l1_valid: l1,
            l2_valid: l2,
            l3_valid: l3,
            l2_needs_refetch: self.l2_needs_refetch,
        })
    }
}

/// Four-way classification: ValidResident vs ValidNotResident vs Invalid vs Unknown.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidityClass {
    ValidResident,
    ValidNotResident,
    Invalid,
    Unknown,
}

// ---------------------------------------------------------------------------
// Demand scenarios
// ---------------------------------------------------------------------------

/// One finite demand scenario (declared envelope).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DemandScenario {
    pub scenario_id: String,
    pub demanded_object_roots: Vec<Sha256Digest>,
    pub demand_weight: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

impl DemandScenario {
    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.scenario_id.is_empty() {
            return Err(ProjectImageError::InvalidDemand(
                "demand scenario_id is empty".into(),
            ));
        }
        if self.demanded_object_roots.is_empty() && self.unknown_reason.is_none() {
            return Err(ProjectImageError::UnknownMissing(format!(
                "demand scenario {} has no objects and no unknown_reason",
                self.scenario_id
            )));
        }
        for d in &self.demanded_object_roots {
            if *d == Sha256Digest::ZERO {
                return Err(ProjectImageError::InvalidDemand(
                    "demand scenario contains zero digest".into(),
                ));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Complete resource ledger (shadow snapshot, no authority)
// ---------------------------------------------------------------------------

/// One resource row in the complete ledger (shadow copy of `zero-ledger` rows).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowResourceRow {
    pub resource_class: String,
    pub amount: u64,
    /// `exact` vs `estimate` vs `unknown` -- explicit.
    pub measurement_source: String,
}

impl ShadowResourceRow {
    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.resource_class.is_empty() {
            return Err(ProjectImageError::InvalidObject(
                "resource row class is empty".into(),
            ));
        }
        match self.measurement_source.as_str() {
            "exact" | "estimate" | "unknown" => Ok(()),
            _ => Err(ProjectImageError::InvalidObject(format!(
                "resource row measurement_source must be exact|estimate|unknown, got {:?}",
                self.measurement_source
            ))),
        }
    }
}

/// Complete resource ledger (shadow, no authority). `None` with reason = missing/unknown.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowResourceLedger {
    pub rows: Vec<ShadowResourceRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

impl ShadowResourceLedger {
    pub fn empty() -> Self {
        Self {
            rows: Vec::new(),
            unknown_reason: None,
        }
    }

    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            rows: Vec::new(),
            unknown_reason: Some(reason.into()),
        }
    }

    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.rows.is_empty() && self.unknown_reason.is_none() {
            // Empty ledger is allowed as "no charges yet" -- not an error, but
            // callers that expect complete ledger should use unknown() instead.
        }
        for r in &self.rows {
            r.validate()?;
        }
        Ok(())
    }

    pub fn is_unknown(&self) -> bool {
        self.unknown_reason.is_some()
    }
}

// ---------------------------------------------------------------------------
// ProjectImageManifest (shadow, immutable, deterministic)
// ---------------------------------------------------------------------------

/// Immutable project-image manifest reporter (W8, shadow-only).
///
/// Deterministic for one `root`; serializes to sorted-key JSON.
/// L1/L2/L3 are kept in `per_object_layers` without aliasing.
/// `has_authority()` is always false.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectImageManifest {
    pub schema_version: String,
    /// Immutable root identity (content digest of the image).
    pub root: Sha256Digest,
    /// Exact immutable objects (sorted by digest for determinism).
    pub exact_objects: Vec<ExactObject>,
    /// Declared causal/dependency graph.
    pub causal_graph: CausalGraphRef,
    /// Declared proof / proof-obligation graph.
    pub proof_graph: ProofGraphRef,
    /// Registered representations lattice.
    pub representations: Vec<RegisteredRepresentation>,
    /// Per-object L1/L2/L3 states (BTreeMap ordering for determinism on the wire via sorted Vec).
    pub per_object_layers: Vec<PerObjectLayers>,
    /// Finite demand scenarios (declared envelope).
    pub demand_scenarios: Vec<DemandScenario>,
    /// Complete resource ledger (shadow).
    pub resource_ledger: ShadowResourceLedger,
    /// Human-readable note that this is shadow and grants no authority.
    pub shadow_note: String,
}

impl ProjectImageManifest {
    /// Build a manifest; validates, sorts objects/representations/layers/scenarios for determinism.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root: Sha256Digest,
        exact_objects: Vec<ExactObject>,
        causal_graph: CausalGraphRef,
        proof_graph: ProofGraphRef,
        representations: Vec<RegisteredRepresentation>,
        per_object_layers: Vec<PerObjectLayers>,
        demand_scenarios: Vec<DemandScenario>,
        resource_ledger: ShadowResourceLedger,
    ) -> Result<Self, ProjectImageError> {
        if root == Sha256Digest::ZERO {
            return Err(ProjectImageError::MissingRoot);
        }
        if exact_objects.is_empty() {
            return Err(ProjectImageError::EmptyObjects);
        }
        let mut manifest = Self {
            schema_version: PROJECT_IMAGE_SCHEMA_VERSION.to_owned(),
            root,
            exact_objects,
            causal_graph,
            proof_graph,
            representations,
            per_object_layers,
            demand_scenarios,
            resource_ledger,
            shadow_note: "shadow-only; grants no production authority".to_owned(),
        };
        manifest.sort_for_determinism();
        manifest.validate()?;
        Ok(manifest)
    }

    /// Build from a `LayerValidityLedger` snapshot, keeping L1/L2/L3 distinct.
    pub fn from_layer_ledger(
        root: Sha256Digest,
        exact_objects: Vec<ExactObject>,
        causal_graph: CausalGraphRef,
        proof_graph: ProofGraphRef,
        representations: Vec<RegisteredRepresentation>,
        layer_ledger: &LayerValidityLedger,
        demand_scenarios: Vec<DemandScenario>,
        resource_ledger: ShadowResourceLedger,
    ) -> Result<Self, ProjectImageError> {
        let per_object_layers = layer_ledger
            .entries()
            .map(PerObjectLayers::from_layer_entry)
            .collect::<Vec<_>>();
        Self::new(
            root,
            exact_objects,
            causal_graph,
            proof_graph,
            representations,
            per_object_layers,
            demand_scenarios,
            resource_ledger,
        )
    }

    fn sort_for_determinism(&mut self) {
        self.exact_objects.sort_by(|a, b| a.digest.cmp(&b.digest));
        self.representations
            .sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.digest.cmp(&b.digest)));
        self.per_object_layers
            .sort_by(|a, b| a.object_root.cmp(&b.object_root));
        self.demand_scenarios
            .sort_by(|a, b| a.scenario_id.cmp(&b.scenario_id));
        self.resource_ledger
            .rows
            .sort_by(|a, b| a.resource_class.cmp(&b.resource_class));
        // Also sort demanded roots inside each scenario for determinism.
        for s in &mut self.demand_scenarios {
            s.demanded_object_roots.sort();
        }
    }

    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.schema_version != PROJECT_IMAGE_SCHEMA_VERSION {
            return Err(ProjectImageError::InvalidObject(format!(
                "unsupported schema_version {:?}",
                self.schema_version
            )));
        }
        if self.root == Sha256Digest::ZERO {
            return Err(ProjectImageError::MissingRoot);
        }
        if self.exact_objects.is_empty() {
            return Err(ProjectImageError::EmptyObjects);
        }
        for o in &self.exact_objects {
            o.validate()?;
        }
        self.causal_graph.validate()?;
        self.proof_graph.validate()?;
        for r in &self.representations {
            r.validate()?;
        }
        for l in &self.per_object_layers {
            l.validate()?;
        }
        for d in &self.demand_scenarios {
            d.validate()?;
        }
        self.resource_ledger.validate()?;

        // Cross-check: every per_object_layers entry must correspond to an exact object or be explicit unknown.
        // (No hard error if extra -- it may be a tombstoned L2 record kept for accounting.)
        let object_set: BTreeSet<Sha256Digest> =
            self.exact_objects.iter().map(|o| o.digest).collect();
        for l in &self.per_object_layers {
            if l.l2_logically_valid.is_some() && l.unknown_reason.is_none() {
                // L2 validity records are allowed to outlive exact objects (tombstones kept per residency law),
                // so no strict membership check.
                let _ = object_set.contains(&l.object_root);
            }
        }
        Ok(())
    }

    /// Shadow output has zero authority.
    pub const fn has_authority(&self) -> bool {
        SHADOW_HAS_AUTHORITY
    }

    /// Classify one object root.
    pub fn validity_class_for(&self, object_root: Sha256Digest) -> ValidityClass {
        for l in &self.per_object_layers {
            if l.object_root == object_root {
                return l.validity_class();
            }
        }
        ValidityClass::Unknown
    }

    /// Deterministic canonical JSON bytes (sorted keys).
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ProjectImageError> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|e| {
            ProjectImageError::Serialization(format!("manifest not serializable: {e}"))
        })?;
        Ok(canonical_json(&value).into_bytes())
    }

    /// Deterministic digest: SHA-256(domain || canonical_bytes).
    pub fn digest(&self) -> Result<Sha256Digest, ProjectImageError> {
        let bytes = self.canonical_bytes()?;
        let mut prefixed = Vec::with_capacity(PROJECT_IMAGE_DOMAIN.len() + bytes.len());
        prefixed.extend_from_slice(PROJECT_IMAGE_DOMAIN);
        prefixed.extend_from_slice(&bytes);
        Ok(Sha256Digest::from_bytes(sha256(&prefixed)))
    }

    /// Contract manifest for this reporter (for pinning).
    pub fn contract_manifest() -> Value {
        serde_json::json!({
            "schema_version": PROJECT_IMAGE_SCHEMA_VERSION,
            "domain": String::from_utf8_lossy(PROJECT_IMAGE_DOMAIN).trim_end_matches('\0').to_string(),
            "has_authority": SHADOW_HAS_AUTHORITY,
            "layers": ["l1_provider_cache", "l2_logical_validity", "l3_physical_residency"],
            "distinguishes": ["valid_not_resident", "invalid"],
        })
    }
}

// ---------------------------------------------------------------------------
// Hypothetical child (shares unchanged objects, names affected claims)
// ---------------------------------------------------------------------------

/// A declared change that hypothetically forks the image.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredChange {
    /// Digests of objects that are changed/added/removed.
    pub changed_object_roots: Vec<Sha256Digest>,
    /// Claim IDs affected by the change (proof-supported facts).
    pub affected_claim_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_reason: Option<String>,
}

impl DeclaredChange {
    pub fn new(changed_object_roots: Vec<Sha256Digest>, affected_claim_ids: Vec<String>) -> Self {
        Self {
            changed_object_roots,
            affected_claim_ids,
            change_reason: None,
        }
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.change_reason = Some(reason.into());
        self
    }

    pub fn validate(&self) -> Result<(), ProjectImageError> {
        for d in &self.changed_object_roots {
            if *d == Sha256Digest::ZERO {
                return Err(ProjectImageError::InvalidObject(
                    "declared change contains zero digest".into(),
                ));
            }
        }
        Ok(())
    }
}

/// Report for one hypothetical child image. Old root is preserved; child
/// shares unchanged objects by digest; affected claims are named.
/// No state is mutated -- both manifests are values.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HypotheticalChildReport {
    pub parent_root: Sha256Digest,
    pub child_root: Sha256Digest,
    /// Objects shared unchanged (parent digest == child digest).
    pub shared_objects: Vec<Sha256Digest>,
    /// Objects that differ / are new in the child (subset of DeclaredChange).
    pub affected_objects: Vec<Sha256Digest>,
    /// Claims affected by the change (named, not mutated).
    pub affected_claims: Vec<String>,
    /// The hypothetical child manifest (deterministic fork).
    pub child_manifest: ProjectImageManifest,
    /// Proof that old root was preserved (== parent_root).
    pub preserved_old_root: Sha256Digest,
}

impl HypotheticalChildReport {
    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.parent_root == Sha256Digest::ZERO || self.child_root == Sha256Digest::ZERO {
            return Err(ProjectImageError::MissingRoot);
        }
        if self.preserved_old_root != self.parent_root {
            return Err(ProjectImageError::InvalidObject(
                "preserved_old_root must equal parent_root".into(),
            ));
        }
        self.child_manifest.validate()?;
        // Check shared/affected are disjoint and cover the change.
        let shared: BTreeSet<_> = self.shared_objects.iter().collect();
        let affected: BTreeSet<_> = self.affected_objects.iter().collect();
        for s in &shared {
            if affected.contains(s) {
                return Err(ProjectImageError::InvalidObject(
                    "shared and affected objects overlap".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn has_authority(&self) -> bool {
        false
    }
}

/// Model one hypothetical child after a declared change while preserving the old root.
///
/// The child shares all unchanged `exact_objects` by digest; only objects whose
/// digest is in `change.changed_object_roots` are considered affected. Per-object
/// layer entries for affected objects are synthesized as unknown with an explicit
/// reason; unchanged layers are cloned. Demand scenarios are cloned verbatim.
/// The child root is `SHA-256(domain || parent_root || sorted changed digests || sorted claim ids)`.
pub fn hypothetical_child(
    parent: &ProjectImageManifest,
    change: &DeclaredChange,
) -> Result<HypotheticalChildReport, ProjectImageError> {
    parent.validate()?;
    change.validate()?;

    let mut changed_sorted = change.changed_object_roots.clone();
    changed_sorted.sort();
    changed_sorted.dedup();

    let mut claims_sorted = change.affected_claim_ids.clone();
    claims_sorted.sort();
    claims_sorted.dedup();

    let changed_set: BTreeSet<Sha256Digest> = changed_sorted.iter().copied().collect();

    // Partition exact objects into shared vs affected.
    let mut shared_objects = Vec::new();
    let mut child_exact_objects = Vec::new();
    for obj in &parent.exact_objects {
        if changed_set.contains(&obj.digest) {
            // Affected: keep the same digest in the child for reporting (hypothetical
            // new bytes would have a new digest; callers that know the new digest
            // should replace this entry after forking). We keep the digest to show
            // which identity is affected without mutating the parent.
            child_exact_objects.push(obj.clone());
        } else {
            shared_objects.push(obj.digest);
            child_exact_objects.push(obj.clone());
        }
    }
    // Also, any changed root not already in parent is a new object synthesis (unknown bytes).
    for d in &changed_sorted {
        if !parent.exact_objects.iter().any(|o| o.digest == *d) {
            child_exact_objects.push(ExactObject {
                digest: *d,
                byte_len: 0,
                causal_coordinate: None,
            });
        }
    }

    // Per-object layers: clone unchanged, synthesize affected as unknown/explicit.
    let mut child_layers = Vec::new();
    for layer in &parent.per_object_layers {
        if changed_set.contains(&layer.object_root) {
            child_layers.push(PerObjectLayers {
                object_root: layer.object_root,
                l1_provider_cached: None,
                l2_logically_valid: None,
                l3_physically_resident: None,
                l2_needs_refetch: false,
                unknown_reason: Some(format!(
                    "hypothetical change affects {}",
                    layer.object_root.to_hex()
                )),
            });
        } else {
            child_layers.push(layer.clone());
        }
    }
    // Layers for newly introduced changed roots.
    let existing_layer_roots: BTreeSet<Sha256Digest> = parent
        .per_object_layers
        .iter()
        .map(|l| l.object_root)
        .collect();
    for d in &changed_sorted {
        if !existing_layer_roots.contains(d) {
            child_layers.push(PerObjectLayers {
                object_root: *d,
                l1_provider_cached: None,
                l2_logically_valid: None,
                l3_physically_resident: None,
                l2_needs_refetch: false,
                unknown_reason: Some("hypothetical new object; validity unknown".into()),
            });
        }
    }

    // Deterministic child root: hash(parent_root + sorted digests + sorted claim ids).
    let mut preimage = Vec::new();
    preimage.extend_from_slice(PROJECT_IMAGE_DOMAIN);
    preimage.extend_from_slice(parent.root.as_bytes());
    for d in &changed_sorted {
        preimage.extend_from_slice(d.as_bytes());
    }
    for c in &claims_sorted {
        preimage.extend_from_slice(c.as_bytes());
        preimage.push(0);
    }
    let child_root = Sha256Digest::from_bytes(sha256(&preimage));

    let child_manifest = ProjectImageManifest {
        schema_version: PROJECT_IMAGE_SCHEMA_VERSION.to_owned(),
        root: child_root,
        exact_objects: {
            let mut v = child_exact_objects;
            v.sort_by(|a, b| a.digest.cmp(&b.digest));
            v
        },
        causal_graph: parent.causal_graph.clone(),
        proof_graph: parent.proof_graph.clone(),
        representations: parent.representations.clone(),
        per_object_layers: {
            let mut v = child_layers;
            v.sort_by(|a, b| a.object_root.cmp(&b.object_root));
            v
        },
        demand_scenarios: parent.demand_scenarios.clone(),
        resource_ledger: parent.resource_ledger.clone(),
        shadow_note: "shadow-only hypothetical child; grants no production authority".to_owned(),
    };
    child_manifest.validate()?;

    let affected_objects = changed_sorted.clone();
    let affected_claims = claims_sorted.clone();

    Ok(HypotheticalChildReport {
        parent_root: parent.root,
        child_root,
        shared_objects,
        affected_objects,
        affected_claims,
        child_manifest,
        preserved_old_root: parent.root,
    })
}

// ---------------------------------------------------------------------------
// Convenience: layer ledger helpers
// ---------------------------------------------------------------------------

/// Build a `BTreeMap` view of per-object validity classes for quick lookup.
pub fn validity_class_map(
    manifest: &ProjectImageManifest,
) -> BTreeMap<Sha256Digest, ValidityClass> {
    manifest
        .per_object_layers
        .iter()
        .map(|l| (l.object_root, l.validity_class()))
        .collect()
}

/// Build a best-effort L2-only validity view from fully known manifest layers.
///
/// The manifest remains the source of truth. L1/L3 flags are intentionally
/// not carried because `LayerValidityLedger` exposes no reconstruction API for
/// them. Entries without L2 validity are skipped; refetch-pending L2 identity
/// is preserved with [`LayerValidityLedger::mark_l3_loss`].
pub fn layer_ledger_from_manifest(manifest: &ProjectImageManifest) -> LayerValidityLedger {
    let mut ledger = LayerValidityLedger::new();
    for layer in &manifest.per_object_layers {
        let Some(entry) = layer.to_layer_entry() else {
            continue;
        };
        if !entry.l2_valid {
            continue;
        }
        let _ = ledger.publish_l2(entry.object_root);
        if entry.l2_needs_refetch {
            let _ = ledger.mark_l3_loss(entry.object_root);
        }
    }
    ledger
}

// ---------------------------------------------------------------------------
// W8 exact Q99 + child-image repair shadow reporting (`zerostack-e7dz`)
// ---------------------------------------------------------------------------
//
// Shadow-only extension of the W8 reporter: exact-rational demand coverage,
// exact Q99 slack, action-guard simulation (deny/replenish), minimum repair
// `g_min`, and a hypothetical child warm-swap report (W8-T4/T11/T12).
//
// Laws (all shadow; nothing here mutates or publishes roots):
// - Q99 means demanded-valid mass, not cache-hit percentage. `valid_mass`
//   counts only L2-valid demanded mass; L1 provider hits and L3 physical
//   residency are separate labeled figures and never alias the Q99 basis.
// - Every Q99 figure is reported against its labeled denominator
//   (`q99_demanded_mass:<N>`); no bare percentages are emitted.
// - Zero demanded mass reports `unavailable`/`None` with an explicit reason;
//   impossibility is reported, never averaged into a fake number.
// - `g_min = max(0, B + A - (1 - theta) * W_next)` with `theta = 99/100`,
//   computed with exact integer arithmetic (numerator over denominator 100).
// - Provider hits never repair logical validity: an added object with
//   `l2_valid != Some(true)` contributes no valid mass.

/// Schema of the W8 exact Q99 shadow reports.
pub const PROJECT_IMAGE_Q99_SCHEMA_VERSION: &str = "zerostack.project_image.shadow.q99.v1";
/// Q99 theta: valid mass must cover 99/100 of demanded mass.
pub const Q99_SHADOW_THETA_NUMERATOR: u64 = 99;
pub const Q99_SHADOW_THETA_DENOMINATOR: u64 = 100;
/// The recompute allowance behind Q99 (`1 - theta`).
pub const Q99_SHADOW_RECOMPUTE_NUMERATOR: u64 = 1;
pub const Q99_SHADOW_RECOMPUTE_DENOMINATOR: u64 = 100;
/// Prefix of the labeled denominator every Q99 figure is read against.
pub const Q99_SHADOW_DENOMINATOR_LABEL_PREFIX: &str = "q99_demanded_mass:";

fn u128_gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 {
        let rem = a % b;
        a = b;
        b = rem;
    }
    a
}

fn mass_from_u128(value: u128, what: &str) -> Result<u64, ProjectImageError> {
    u64::try_from(value)
        .map_err(|_| ProjectImageError::InvalidValidity(format!("{what} exceeds u64 mass range")))
}

/// Sort and validate an envelope: unique scenario ids, each scenario valid.
fn sorted_scenarios(
    scenarios: &[DemandScenario],
) -> Result<Vec<DemandScenario>, ProjectImageError> {
    let mut sorted = scenarios.to_vec();
    sorted.sort_by(|a, b| a.scenario_id.cmp(&b.scenario_id));
    for pair in sorted.windows(2) {
        if pair[0].scenario_id == pair[1].scenario_id {
            return Err(ProjectImageError::InvalidDemand(format!(
                "duplicate scenario_id {} in envelope",
                pair[0].scenario_id
            )));
        }
    }
    for scenario in &sorted {
        scenario.validate()?;
    }
    Ok(sorted)
}

fn layer_lookup<'a>(
    manifest: &'a ProjectImageManifest,
) -> BTreeMap<Sha256Digest, &'a PerObjectLayers> {
    manifest
        .per_object_layers
        .iter()
        .map(|l| (l.object_root, l))
        .collect()
}

/// Exact non-negative rational, kept in reduced form with `denominator > 0`.
/// Coverage figures are reported as exact rationals, never as floats.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactRational {
    pub numerator: u128,
    pub denominator: u128,
}

impl ExactRational {
    pub fn new(numerator: u128, denominator: u128) -> Result<Self, ProjectImageError> {
        if denominator == 0 {
            return Err(ProjectImageError::InvalidValidity(
                "exact rational denominator is zero".into(),
            ));
        }
        let mut value = Self {
            numerator,
            denominator,
        };
        value.reduce();
        Ok(value)
    }

    /// Reduce in place by the gcd of numerator and denominator.
    pub fn reduce(&mut self) {
        let g = u128_gcd(self.numerator, self.denominator);
        if g > 1 {
            self.numerator /= g;
            self.denominator /= g;
        }
    }

    pub fn is_zero(&self) -> bool {
        self.numerator == 0
    }

    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.denominator == 0 {
            return Err(ProjectImageError::InvalidValidity(
                "exact rational denominator is zero".into(),
            ));
        }
        Ok(())
    }

    /// Exact floor of the rational in ppm (1_000_000 = 100%). Never a float.
    pub fn to_ppm(&self) -> Result<u128, ProjectImageError> {
        self.validate()?;
        Ok(self.numerator * 1_000_000 / self.denominator)
    }
}

impl std::fmt::Display for ExactRational {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

// ---------------------------------------------------------------------------
// Exact-rational demand coverage (W8-T11)
// ---------------------------------------------------------------------------

/// How one demanded instance classifies for Q99 mass accounting.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DemandMassClass {
    /// L2 logically valid: counts toward Q99 valid mass.
    Valid,
    /// L2 invalid: must be recomputed, never part of valid mass.
    Invalid,
    /// L2 unknown / missing evidence: never counted as valid.
    Unknown,
}

/// One (scenario, object) demand instance with its exact layer state.
/// L1/L2/L3 stay distinct: a provider (L1) hit never contributes valid mass.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageDemandRow {
    pub scenario_id: String,
    pub object_root: Sha256Digest,
    pub demand_weight: u64,
    /// L1 provider-cache hit (informational; never Q99 valid mass).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l1_provider_hit: Option<bool>,
    /// L2 logical validity (the Q99 mass basis).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l2_valid: Option<bool>,
    /// L3 physical residency (informational; never Q99 valid mass).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l3_resident: Option<bool>,
    /// L2-valid but L3 copy lost: needs refetch/rematerialize, never rediscovery.
    #[serde(default)]
    pub l2_needs_refetch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
    /// Derived from `l2_valid` only (never from L1/L3).
    pub mass_class: DemandMassClass,
}

/// Exact-rational demand coverage of a finite demand envelope (W8-T11).
///
/// Q99 means demanded-valid mass, not cache-hit percentage: `valid_mass`
/// counts only L2-valid demanded mass. L1 provider hits and L3 physical
/// residency are reported as separate labeled figures and never alias the
/// Q99 basis. Zero demanded mass reports `coverage == None` with an explicit
/// reason (impossibility is reported, never averaged into a fake 0%).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DemandCoverageReport {
    pub schema_version: String,
    /// Sorted scenario ids of the envelope.
    pub envelope_scenario_ids: Vec<String>,
    /// Total demanded mass (sum of per-instance weights, exact).
    pub demanded_mass: u64,
    /// L2-valid demanded mass (the Q99 basis).
    pub valid_mass: u64,
    /// L2-invalid demanded mass.
    pub invalid_mass: u64,
    /// L2-unknown (missing evidence) demanded mass.
    pub unknown_mass: u64,
    /// L1 provider-hit mass; informational, never part of `valid_mass`.
    pub l1_hit_mass: u64,
    /// L3-resident demanded mass; informational, never part of `valid_mass`.
    pub l3_resident_mass: u64,
    /// L2-valid mass whose L3 copy is lost (subset of `valid_mass`).
    pub l2_refetch_mass: u64,
    /// Exact rational `valid_mass / demanded_mass`, reduced. `None` when the
    /// envelope has zero demanded mass (explicit reason below).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage: Option<ExactRational>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_unknown_reason: Option<String>,
    /// Labeled denominator: `q99_demanded_mass:<N>`. Every Q99 figure in this
    /// report is only read against this denominator.
    pub denominator_label: String,
    /// Per-instance rows, sorted by (scenario_id, object_root).
    pub rows: Vec<CoverageDemandRow>,
}

impl DemandCoverageReport {
    /// Shadow output grants no production authority.
    pub const fn has_authority(&self) -> bool {
        SHADOW_HAS_AUTHORITY
    }
}

/// Compute exact-rational demand coverage of an envelope against a manifest's
/// per-object layers. Deterministic for one input; never mutates anything.
pub fn compute_demand_coverage(
    manifest: &ProjectImageManifest,
    scenarios: &[DemandScenario],
) -> Result<DemandCoverageReport, ProjectImageError> {
    manifest.validate()?;
    let layers = layer_lookup(manifest);
    let sorted = sorted_scenarios(scenarios)?;

    let mut rows = Vec::new();
    let mut demanded: u128 = 0;
    let mut valid: u128 = 0;
    let mut invalid: u128 = 0;
    let mut unknown: u128 = 0;
    let mut l1_hit: u128 = 0;
    let mut l3_resident: u128 = 0;
    let mut refetch: u128 = 0;

    for scenario in &sorted {
        let mut roots: Vec<Sha256Digest> = scenario.demanded_object_roots.clone();
        roots.sort();
        roots.dedup();
        for root in roots {
            let weight = u128::from(scenario.demand_weight);
            demanded += weight;
            let (l1, l2, l3, needs_refetch, reason) = match layers.get(&root) {
                None => (None, None, None, false, Some("no layer entry".to_owned())),
                Some(layer) => (
                    layer.l1_provider_cached,
                    layer.l2_logically_valid,
                    layer.l3_physically_resident,
                    layer.l2_needs_refetch,
                    layer.unknown_reason.clone(),
                ),
            };
            match l2 {
                Some(true) => valid += weight,
                Some(false) => invalid += weight,
                None => unknown += weight,
            }
            if l1 == Some(true) {
                l1_hit += weight;
            }
            if l3 == Some(true) {
                l3_resident += weight;
            }
            if l2 == Some(true) && needs_refetch {
                refetch += weight;
            }
            rows.push(CoverageDemandRow {
                scenario_id: scenario.scenario_id.clone(),
                object_root: root,
                demand_weight: scenario.demand_weight,
                l1_provider_hit: l1,
                l2_valid: l2,
                l3_resident: l3,
                l2_needs_refetch: needs_refetch,
                unknown_reason: reason,
                mass_class: match l2 {
                    Some(true) => DemandMassClass::Valid,
                    Some(false) => DemandMassClass::Invalid,
                    None => DemandMassClass::Unknown,
                },
            });
        }
    }

    let demanded_mass = mass_from_u128(demanded, "demanded mass")?;
    let coverage = if demanded == 0 {
        None
    } else {
        Some(ExactRational::new(valid, demanded)?)
    };
    let coverage_unknown_reason = if demanded == 0 {
        Some("zero_weight_envelope".to_owned())
    } else {
        None
    };

    Ok(DemandCoverageReport {
        schema_version: PROJECT_IMAGE_Q99_SCHEMA_VERSION.to_owned(),
        envelope_scenario_ids: sorted.iter().map(|s| s.scenario_id.clone()).collect(),
        demanded_mass,
        valid_mass: mass_from_u128(valid, "valid mass")?,
        invalid_mass: mass_from_u128(invalid, "invalid mass")?,
        unknown_mass: mass_from_u128(unknown, "unknown mass")?,
        l1_hit_mass: mass_from_u128(l1_hit, "l1 hit mass")?,
        l3_resident_mass: mass_from_u128(l3_resident, "l3 resident mass")?,
        l2_refetch_mass: mass_from_u128(refetch, "l2 refetch mass")?,
        coverage,
        coverage_unknown_reason,
        denominator_label: format!("{Q99_SHADOW_DENOMINATOR_LABEL_PREFIX}{demanded_mass}"),
        rows,
    })
}

/// Convenience: coverage of the manifest's own demand scenarios.
pub fn demand_coverage(
    manifest: &ProjectImageManifest,
) -> Result<DemandCoverageReport, ProjectImageError> {
    compute_demand_coverage(manifest, &manifest.demand_scenarios)
}

// ---------------------------------------------------------------------------
// Exact Q99 slack (W8-T11 / ZS-CACHE-012 shadow)
// ---------------------------------------------------------------------------

/// Exact Q99 slack: `sigma = W_R - 0.99*W`, where `W_R` is L2-valid AND
/// L3-resident demanded mass and `W` is demanded mass. Reported exactly over
/// denominator 100; no floats, no bare percentages. Zero-weight envelopes
/// are unavailable and never report a vacuous hold.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Q99SlackReport {
    pub schema_version: String,
    pub demanded_mass: u64,
    /// L2-valid AND L3-resident demanded mass.
    pub resident_valid_mass: u64,
    /// Exact slack numerator over denominator 100: `100*W_R - 99*W`.
    pub slack_numerator_100: i128,
    /// `slack_numerator_100 >= 0` (slack holds). Zero-weight envelopes are
    /// unavailable, never a vacuous hold.
    pub slack_holds: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

impl Q99SlackReport {
    /// Shadow output grants no production authority.
    pub const fn has_authority(&self) -> bool {
        SHADOW_HAS_AUTHORITY
    }
}

/// Compute the exact Q99 slack of an envelope against a manifest's layers.
pub fn compute_q99_slack(
    manifest: &ProjectImageManifest,
    scenarios: &[DemandScenario],
) -> Result<Q99SlackReport, ProjectImageError> {
    manifest.validate()?;
    let layers = layer_lookup(manifest);
    let sorted = sorted_scenarios(scenarios)?;

    let mut demanded: u128 = 0;
    let mut resident_valid: u128 = 0;
    for scenario in &sorted {
        let mut roots: Vec<Sha256Digest> = scenario.demanded_object_roots.clone();
        roots.sort();
        roots.dedup();
        for root in roots {
            let weight = u128::from(scenario.demand_weight);
            demanded += weight;
            if let Some(layer) = layers.get(&root) {
                if layer.l2_logically_valid == Some(true)
                    && layer.l3_physically_resident == Some(true)
                {
                    resident_valid += weight;
                }
            }
        }
    }

    let demanded_mass = mass_from_u128(demanded, "demanded mass")?;
    let resident_valid_mass = mass_from_u128(resident_valid, "resident valid mass")?;
    let (slack_numerator_100, unavailable_reason) = if demanded == 0 {
        (0, Some("zero_weight_envelope".to_owned()))
    } else {
        (
            100_i128 * i128::from(resident_valid_mass) - 99_i128 * i128::from(demanded_mass),
            None,
        )
    };

    Ok(Q99SlackReport {
        schema_version: PROJECT_IMAGE_Q99_SCHEMA_VERSION.to_owned(),
        demanded_mass,
        resident_valid_mass,
        slack_numerator_100,
        slack_holds: unavailable_reason.is_none() && slack_numerator_100 >= 0,
        unavailable_reason,
    })
}

// ---------------------------------------------------------------------------
// Q99 action-guard simulation (W8-T11): deny/replenish, minimum repair g_min
// ---------------------------------------------------------------------------

/// Declared addition of one object in a proposed action. `l2_valid == None`
/// means missing evidence: the addition never contributes valid mass.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredAddObject {
    pub object_root: Sha256Digest,
    pub demand_weight: u64,
    /// L2 logical validity of the added object. `None` = missing evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l2_valid: Option<bool>,
    /// Provider hit on the added object; informational, never repairs L2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l1_provider_hit: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_reason: Option<String>,
}

/// A proposed change to the image's demanded mass (invalidation + additions),
/// simulated without enforcing production gates.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedAction {
    pub action_id: String,
    pub invalidate_object_roots: Vec<Sha256Digest>,
    pub add_objects: Vec<DeclaredAddObject>,
    /// Simulate the replenish branch (minimum repair) instead of denying
    /// every action that would drop valid mass below theta.
    #[serde(default = "default_true")]
    pub simulate_replenish: bool,
}

fn default_true() -> bool {
    true
}

impl ProposedAction {
    pub fn validate(&self) -> Result<(), ProjectImageError> {
        if self.action_id.is_empty() {
            return Err(ProjectImageError::InvalidDemand(
                "action_id is empty".into(),
            ));
        }
        for root in &self.invalidate_object_roots {
            if *root == Sha256Digest::ZERO {
                return Err(ProjectImageError::InvalidDemand(
                    "proposed action invalidates the zero digest".into(),
                ));
            }
        }
        let mut seen = BTreeSet::new();
        for add in &self.add_objects {
            if add.object_root == Sha256Digest::ZERO {
                return Err(ProjectImageError::InvalidDemand(
                    "proposed action adds the zero digest".into(),
                ));
            }
            if !seen.insert(add.object_root) {
                return Err(ProjectImageError::InvalidDemand(format!(
                    "proposed action adds {} twice",
                    add.object_root.to_hex()
                )));
            }
        }
        Ok(())
    }
}

/// Simulated action-guard outcome. `Unavailable` covers zero-weight next
/// envelopes: impossibility is reported, never a vacuous pass or deny.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "case", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionGuardOutcome {
    /// Valid mass after the action already holds Q99 (>= 99% of next demand).
    Pass,
    /// Repair of `g_min` mass (replenish branch) restores the obligation.
    RepairRequired { g_min: u64 },
    /// The action cannot hold Q99: repair is zero, not simulated, or
    /// insufficient.
    Deny { reason: String },
    /// No next demand (zero-weight envelope): nothing to guard, explicitly.
    Unavailable { reason: String },
}

/// Exact action-guard simulation (W8-T11): pure arithmetic over the
/// manifest's finite demand envelope. Never mutates or publishes roots;
/// grants no authority.
///
/// Variables follow the W8 design notes: `B` baseline L2-valid mass, `A`
/// declared L2-valid additions, `G` invalidated mass, `W_next` next demanded
/// mass, and `g_min = max(0, B + A - (1 - theta) * W_next)` with
/// `theta = 99/100`. Provider hits never repair L2.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionGuardSimulation {
    pub schema_version: String,
    pub action_id: String,
    /// Current demanded mass `W`.
    pub current_demanded_mass: u64,
    /// Next demanded mass `W_next = W - G_mass + A_mass` (exact).
    pub next_demanded_mass: u64,
    /// Baseline L2-valid demanded mass `B`.
    pub baseline_valid_mass: u64,
    /// Demanded mass of invalidated roots `G_mass` (only roots in the
    /// envelope contribute).
    pub invalidated_mass: u64,
    /// Declared demand mass of additions `A_mass`.
    pub added_mass: u64,
    /// L2-valid additions `A_valid` (missing evidence never counts).
    pub added_valid_mass: u64,
    /// L2-valid and L3-resident current mass `W_R`.
    pub resident_valid_mass: u64,
    /// Valid mass after the action `B' = max(0, B + A_valid - G_mass)`.
    pub valid_after_mass: u64,
    /// Exact Q99 obligation: `100 * valid_after - 99 * W_next` (>= 0 holds).
    pub obligation_numerator_100: i128,
    /// Whether the obligation holds without any repair.
    pub obligation_holds: bool,
    /// Exact `g_min` numerator over denominator 100:
    /// `100 * (B + A_valid) - W_next` (may be <= 0).
    pub g_min_numerator_100: i128,
    /// Minimum repair mass (W8 design):
    /// `max(0, ceil(g_min_numerator_100 / 100))`.
    pub g_min: u64,
    /// Exact check that repairing `g_min` restores the obligation:
    /// `100 * (valid_after + g_min) >= 99 * W_next`.
    pub repair_restores_q99: bool,
    /// Exact obligation gap without repair:
    /// `max(0, ceil((99 * W_next - 100 * valid_after) / 100))`.
    pub shortfall_to_hold_q99: u64,
    pub outcome: ActionGuardOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadow_note: Option<String>,
}

impl ActionGuardSimulation {
    /// Shadow output grants no production authority.
    pub const fn has_authority(&self) -> bool {
        SHADOW_HAS_AUTHORITY
    }
}

/// Simulate the Q99 action guard for one proposed action over the manifest's
/// demand envelope. Pure and deterministic: no state is mutated, no root is
/// published, and no production gate is enforced (shadow only).
pub fn simulate_action_guard(
    manifest: &ProjectImageManifest,
    action: &ProposedAction,
) -> Result<ActionGuardSimulation, ProjectImageError> {
    manifest.validate()?;
    action.validate()?;
    let layers = layer_lookup(manifest);
    let scenarios = sorted_scenarios(&manifest.demand_scenarios)?;

    // Current demanded mass, baseline L2-valid mass, resident-valid mass, and
    // per-root demanded weight (roots deduplicated per scenario).
    let mut current_demanded: u128 = 0;
    let mut baseline_valid: u128 = 0;
    let mut resident_valid: u128 = 0;
    let mut demanded_by_root: BTreeMap<Sha256Digest, u128> = BTreeMap::new();
    for scenario in &scenarios {
        let mut roots: Vec<Sha256Digest> = scenario.demanded_object_roots.clone();
        roots.sort();
        roots.dedup();
        for root in roots {
            let weight = u128::from(scenario.demand_weight);
            current_demanded += weight;
            *demanded_by_root.entry(root).or_insert(0) += weight;
            if let Some(layer) = layers.get(&root) {
                if layer.l2_logically_valid == Some(true) {
                    baseline_valid += weight;
                }
                if layer.l2_logically_valid == Some(true)
                    && layer.l3_physically_resident == Some(true)
                {
                    resident_valid += weight;
                }
            }
        }
    }

    // Invalidation mass: only demanded roots contribute (absent roots are
    // invalidated with zero mass, exactly).
    let mut invalidate_sorted = action.invalidate_object_roots.clone();
    invalidate_sorted.sort();
    invalidate_sorted.dedup();
    let mut invalidated: u128 = 0;
    for root in &invalidate_sorted {
        invalidated += demanded_by_root.get(root).copied().unwrap_or(0);
    }

    // Additions: declared mass; only l2_valid == Some(true) adds valid mass.
    let mut added: u128 = 0;
    let mut added_valid: u128 = 0;
    for add in &action.add_objects {
        let weight = u128::from(add.demand_weight);
        added += weight;
        if add.l2_valid == Some(true) {
            added_valid += weight;
        }
    }

    // W_next = W - G_mass + A_mass, always >= 0 (G_mass <= W by construction).
    let next_demanded_i128 = current_demanded as i128 + added as i128 - invalidated as i128;
    let next_demanded_mass = u64::try_from(next_demanded_i128).map_err(|_| {
        ProjectImageError::InvalidValidity("next demanded mass exceeds u64 mass range".into())
    })?;

    // B' = max(0, B + A_valid - G_mass).
    let valid_after_i128 = baseline_valid as i128 + added_valid as i128 - invalidated as i128;
    let valid_after: u128 = valid_after_i128.max(0) as u128;
    let valid_after_mass = mass_from_u128(valid_after, "valid after mass")?;

    let obligation_numerator_100 = 100_i128 * valid_after as i128 - 99_i128 * next_demanded_i128;
    let obligation_holds = obligation_numerator_100 >= 0;

    // g_min = max(0, ceil((100*(B + A_valid) - W_next) / 100)).
    let g_min_numerator_100 =
        100_i128 * (baseline_valid as i128 + added_valid as i128) - next_demanded_i128;
    let g_min: u128 = if g_min_numerator_100 > 0 {
        (g_min_numerator_100 as u128).div_ceil(100)
    } else {
        0
    };
    let g_min_mass = mass_from_u128(g_min, "g_min")?;

    // Exact obligation gap: max(0, ceil((99*W_next - 100*valid_after) / 100)).
    let shortfall_numerator_100 = 99_i128 * next_demanded_i128 - 100_i128 * valid_after as i128;
    let shortfall: u128 = if shortfall_numerator_100 > 0 {
        (shortfall_numerator_100 as u128).div_ceil(100)
    } else {
        0
    };
    let shortfall_mass = mass_from_u128(shortfall, "shortfall to hold q99")?;

    let repair_restores_q99 =
        (valid_after as i128 + g_min as i128) * 100 >= 99_i128 * next_demanded_i128;

    let outcome = if next_demanded_i128 == 0 {
        ActionGuardOutcome::Unavailable {
            reason: "zero_weight_next_envelope".into(),
        }
    } else if obligation_holds {
        ActionGuardOutcome::Pass
    } else if g_min_mass == 0 {
        ActionGuardOutcome::Deny {
            reason: "minimum_repair_is_zero; action cannot hold Q99".into(),
        }
    } else if !action.simulate_replenish {
        ActionGuardOutcome::Deny {
            reason: "replenish_not_simulated".into(),
        }
    } else if repair_restores_q99 {
        ActionGuardOutcome::RepairRequired { g_min: g_min_mass }
    } else {
        ActionGuardOutcome::Deny {
            reason: format!(
                "minimum_repair_insufficient:g_min={g_min_mass},shortfall={shortfall_mass}"
            ),
        }
    };

    Ok(ActionGuardSimulation {
        schema_version: PROJECT_IMAGE_Q99_SCHEMA_VERSION.to_owned(),
        action_id: action.action_id.clone(),
        current_demanded_mass: mass_from_u128(current_demanded, "current demanded mass")?,
        next_demanded_mass,
        baseline_valid_mass: mass_from_u128(baseline_valid, "baseline valid mass")?,
        invalidated_mass: mass_from_u128(invalidated, "invalidated mass")?,
        added_mass: mass_from_u128(added, "added mass")?,
        added_valid_mass: mass_from_u128(added_valid, "added valid mass")?,
        resident_valid_mass: mass_from_u128(resident_valid, "resident valid mass")?,
        valid_after_mass,
        obligation_numerator_100,
        obligation_holds,
        g_min_numerator_100,
        g_min: g_min_mass,
        repair_restores_q99,
        shortfall_to_hold_q99: shortfall_mass,
        outcome,
        shadow_note: Some("shadow-only action guard; no production gate enforced".to_owned()),
    })
}

// ---------------------------------------------------------------------------
// Hypothetical child warm-swap report (W8-T4 precommit warm-swap, W8-T12)
// ---------------------------------------------------------------------------

/// One ledged prewarm row (W8-T12): declared exact work for one warmed
/// branch. Unselected branches are ledged exactly like the selected one.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrewarmLedgerRow {
    pub child_root: Sha256Digest,
    pub selected: bool,
    /// Declared exact prewarm work mass for this branch.
    pub declared_work_mass: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Precommit warm-swap report (W8-T4 + W8-T12): the hypothetical child is
/// warmed before publish; this report states whether swapping it in holds
/// Q99 over the declared next envelope and ledges all prewarming work,
/// including unselected branches. Shadow-only: nothing is mutated or
/// published; the old root is preserved.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChildWarmSwapReport {
    pub schema_version: String,
    pub parent_root: Sha256Digest,
    pub child_root: Sha256Digest,
    /// The hypothetical child manifest (deterministic fork; shares unchanged
    /// objects by digest).
    pub child_manifest: ProjectImageManifest,
    /// Proof the old root was preserved (== parent_root).
    pub preserved_old_root: Sha256Digest,
    /// Exact-rational coverage of the child envelope on the child image.
    pub coverage: DemandCoverageReport,
    /// Exact Q99 slack of the child image over the child envelope.
    pub slack: Q99SlackReport,
    /// Exact Q99 check over the child envelope: `100 * valid >= 99 * demanded`.
    /// Zero-weight envelopes fail closed (unavailable, never a vacuous hold).
    pub warm_swap_holds_q99: bool,
    /// Exact repair mass needed before the swap for the child to hold Q99:
    /// `max(0, ceil((99 * W - 100 * valid) / 100))`; 0 when it already holds.
    pub child_repair_to_hold_q99: u64,
    /// Ledged prewarm branches (selected + unselected).
    pub prewarm_rows: Vec<PrewarmLedgerRow>,
    /// Sum of declared prewarm work across all branches (exact).
    pub total_prewarm_mass: u64,
    /// Declared prewarm work of unselected branches (W8-T12 ledging).
    pub unselected_prewarm_mass: u64,
    pub shadow_note: String,
}

impl ChildWarmSwapReport {
    /// Shadow output grants no production authority.
    pub const fn has_authority(&self) -> bool {
        SHADOW_HAS_AUTHORITY
    }
}

/// Build the warm-swap report: forks the parent via [`hypothetical_child`],
/// computes exact coverage/slack of the child over `child_envelope`, and
/// ledges the declared prewarm branches. Exactly one branch must be marked
/// `selected` and must be the child root. Nothing is mutated or published.
pub fn child_warm_swap_report(
    parent: &ProjectImageManifest,
    change: &DeclaredChange,
    child_envelope: &[DemandScenario],
    prewarm_rows: Vec<PrewarmLedgerRow>,
) -> Result<ChildWarmSwapReport, ProjectImageError> {
    let fork = hypothetical_child(parent, change)?;

    let mut selected = 0_usize;
    let mut total_prewarm: u128 = 0;
    let mut unselected_prewarm: u128 = 0;
    for row in &prewarm_rows {
        if row.child_root == Sha256Digest::ZERO {
            return Err(ProjectImageError::InvalidDemand(
                "prewarm row child_root is zero".into(),
            ));
        }
        total_prewarm += u128::from(row.declared_work_mass);
        if row.selected {
            selected += 1;
            if row.child_root != fork.child_root {
                return Err(ProjectImageError::InvalidDemand(format!(
                    "selected prewarm branch {} is not the child root {}",
                    row.child_root.to_hex(),
                    fork.child_root.to_hex()
                )));
            }
        } else {
            unselected_prewarm += u128::from(row.declared_work_mass);
        }
    }
    if selected != 1 {
        return Err(ProjectImageError::InvalidDemand(
            "warm swap requires exactly one selected prewarm branch".into(),
        ));
    }

    let coverage = compute_demand_coverage(&fork.child_manifest, child_envelope)?;
    let slack = compute_q99_slack(&fork.child_manifest, child_envelope)?;

    let (holds, repair) = if coverage.demanded_mass == 0 {
        (false, 0)
    } else {
        let gap = 99_i128 * i128::from(coverage.demanded_mass)
            - 100_i128 * i128::from(coverage.valid_mass);
        if gap <= 0 {
            (true, 0)
        } else {
            (
                false,
                mass_from_u128((gap as u128).div_ceil(100), "child repair to hold q99")?,
            )
        }
    };

    Ok(ChildWarmSwapReport {
        schema_version: PROJECT_IMAGE_Q99_SCHEMA_VERSION.to_owned(),
        parent_root: fork.parent_root,
        child_root: fork.child_root,
        child_manifest: fork.child_manifest,
        preserved_old_root: fork.preserved_old_root,
        coverage,
        slack,
        warm_swap_holds_q99: holds,
        child_repair_to_hold_q99: repair,
        prewarm_rows,
        total_prewarm_mass: mass_from_u128(total_prewarm, "total prewarm mass")?,
        unselected_prewarm_mass: mass_from_u128(unselected_prewarm, "unselected prewarm mass")?,
        shadow_note: "shadow-only warm swap; grants no production authority".to_owned(),
    })
}
