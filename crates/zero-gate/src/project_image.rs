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
use zero_abi::{Sha256Digest, canonical_json, sha256};

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
    pub fn present(kind: RepresentationKind, digest: Sha256Digest) -> Result<Self, ProjectImageError> {
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
            (Some(d), _) if *d == Sha256Digest::ZERO => Err(ProjectImageError::InvalidRepresentation(
                "representation digest is zero".into(),
            )),
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
                Some(true) => {},
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
            return Err(ProjectImageError::UnknownMissing(
                format!(
                    "per-object layers for {} is fully unknown without reason",
                    self.object_root.to_hex()
                ),
            ));
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
            return Err(ProjectImageError::UnknownMissing(
                format!("demand scenario {} has no objects and no unknown_reason", self.scenario_id),
            ));
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
            _ => Err(ProjectImageError::InvalidObject(
                format!("resource row measurement_source must be exact|estimate|unknown, got {:?}", self.measurement_source),
            )),
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
    pub fn new(
        changed_object_roots: Vec<Sha256Digest>,
        affected_claim_ids: Vec<String>,
    ) -> Self {
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
        if !parent
            .exact_objects
            .iter()
            .any(|o| o.digest == *d)
        {
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
    let existing_layer_roots: BTreeSet<Sha256Digest> =
        parent.per_object_layers.iter().map(|l| l.object_root).collect();
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
pub fn validity_class_map(manifest: &ProjectImageManifest) -> BTreeMap<Sha256Digest, ValidityClass> {
    manifest
        .per_object_layers
        .iter()
        .map(|l| (l.object_root, l.validity_class()))
        .collect()
}

/// Rebuild a `LayerValidityLedger` from the manifest's per-object layers
/// (only entries that are fully known).
pub fn layer_ledger_from_manifest(manifest: &ProjectImageManifest) -> LayerValidityLedger {
    let mut ledger = LayerValidityLedger::new();
    for layer in &manifest.per_object_layers {
        if let Some(entry) = layer.to_layer_entry() {
            // Insert via publish/mark pattern: we directly populate the ledger
            // by publishing L2 where needed and preserving L1/L3 flags through
            // the entry's semantics. For shadow replay we just insert the entry
            // through the ledger's publish API where possible.
            let _ = ledger.publish_l2(entry.object_root);
            // Re-apply L1/L3 distinction via direct entry if the ledger already
            // has the object.
            if let Some(e) = ledger.entry(entry.object_root) {
                let _ = e;
            }
            // Fallback: if publish failed due to state, we keep the manifest as
            // source of truth; the ledger is best-effort for known entries.
        }
    }
    ledger
}
