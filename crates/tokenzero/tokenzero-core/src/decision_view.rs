//! Deterministic Decision View rendering and stable-prefix geometry.
//!
//! ZeroStack selects the semantic components. TokenZero validates their
//! identities, renders the supplied order deterministically, and records
//! exact byte/token geometry. Rendering is order-invariant for commutative
//! runs: within a maximal run of same-class sections marked commutative,
//! order is survival-score descending (tie-break by kind then payload), so
//! any caller permutation of such a run renders byte-identical bytes.
//! Noncommutative (semantic-order) sections keep the caller's exact order.
//! Prefix byte identity is not a claim of provider eligibility, retention,
//! routing, or cache hit.

use crate::model_artifacts::{
    ExactTokenMap, ExactTokenizerAdapter, ExactTokenizerIdentity, ModelArtifactError, ModelCapsule,
    TokenPage,
};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use zero_abi::{Sha256Digest, sha256};
use zero_ref::ZeroRef;

pub const MAX_DECISION_VIEW_SECTIONS: usize = 1_024;
pub const MAX_DECISION_VIEW_BYTES: usize = 16 * 1_048_576;
pub const MAX_DECISION_VIEW_RECOVERY_REFS: usize = 4_096;
pub const MAX_DECISION_VIEW_METADATA_ITEMS: usize = 1_024;
const MAX_MARKER_FIELD_BYTES: usize = 16_384;
const RENDERER_CONTRACT: &[u8] = b"tokenzero.decision-view.renderer.v3; framing=section-kind+decimal-byte-length+lf+payload+lf; order=commutative-score-descending-run-sorted,else-caller-preserved; commutative=survival-score-bps-u32-max-10000-within-maximal-runs-of-same-stability-class; tie=kind+payload; noncommutative=verbatim-caller-order; stable=system-tool,project-capsule,task-family-capsule,typed-effect-schema; volatile=locus-evidence,working-tree-delta,user-task,uncertainty-coverage,recovery-routes; metadata=candidate-choices,supported-decisions,completeness-grade,baseline-escape";

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionViewError {
    ModelArtifact(ModelArtifactError),
    TooManySections { actual: usize, limit: usize },
    TooManyRecoveryRefs { actual: usize, limit: usize },
    ViewByteLimit { actual: usize, limit: usize },
    LengthOverflow,
    StableSectionAfterVolatile { index: usize },
    TokenizerIdentityMismatch { section: DecisionViewSectionKind },
    ToolSchemaDigestMismatch,
    CapsuleSourceRootMismatch,
    CapsuleModelProfileMismatch,
    InvalidRecoveryRef(String),
    NoncanonicalRecoveryRef(String),
    EmptyMarkerCode,
    MarkerFieldTooLong,
    EmptyChoiceId,
    ChoiceFieldTooLong,
    TooManyMetadataItems { actual: usize, limit: usize },
    SurvivalScoreOutOfRange { actual: u32, limit: u32 },
    PrefixNotTokenAligned { byte_offset: u64 },
}

impl fmt::Display for DecisionViewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid TokenZero Decision View: {self:?}")
    }
}

impl Error for DecisionViewError {}

impl From<ModelArtifactError> for DecisionViewError {
    fn from(error: ModelArtifactError) -> Self {
        Self::ModelArtifact(error)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionViewSectionKind {
    StableSystemToolContract,
    StableProjectCapsule,
    StableTaskFamilyCapsule,
    StableTypedEffectSchema,
    VolatileLocusEvidence,
    VolatileWorkingTreeDelta,
    VolatileUserTask,
    VolatileUncertaintyCoverage,
    VolatileRecoveryRoutes,
}

impl DecisionViewSectionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StableSystemToolContract => "stable_system_tool_contract",
            Self::StableProjectCapsule => "stable_project_capsule",
            Self::StableTaskFamilyCapsule => "stable_task_family_capsule",
            Self::StableTypedEffectSchema => "stable_typed_effect_schema",
            Self::VolatileLocusEvidence => "volatile_locus_evidence",
            Self::VolatileWorkingTreeDelta => "volatile_working_tree_delta",
            Self::VolatileUserTask => "volatile_user_task",
            Self::VolatileUncertaintyCoverage => "volatile_uncertainty_coverage",
            Self::VolatileRecoveryRoutes => "volatile_recovery_routes",
        }
    }

    pub const fn is_stable(self) -> bool {
        matches!(
            self,
            Self::StableSystemToolContract
                | Self::StableProjectCapsule
                | Self::StableTaskFamilyCapsule
                | Self::StableTypedEffectSchema
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionUncertaintyKind {
    Exact,
    SoundOverapproximation,
    PartialCoverage,
    Heuristic,
    Unknown,
}

impl DecisionUncertaintyKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::SoundOverapproximation => "sound_overapproximation",
            Self::PartialCoverage => "partial_coverage",
            Self::Heuristic => "heuristic",
            Self::Unknown => "unknown",
        }
    }
}

/// Caller-supplied epistemic marker. TokenZero renders it but never upgrades it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DecisionUncertaintyMarker {
    kind: DecisionUncertaintyKind,
    code: String,
    message: String,
    recovery_refs: Vec<String>,
}

impl DecisionUncertaintyMarker {
    pub fn new(
        kind: DecisionUncertaintyKind,
        code: impl Into<String>,
        message: impl Into<String>,
        recovery_refs: Vec<String>,
    ) -> Result<Self, DecisionViewError> {
        let code = code.into();
        let message = message.into();
        if code.is_empty() {
            return Err(DecisionViewError::EmptyMarkerCode);
        }
        if code.len() > MAX_MARKER_FIELD_BYTES || message.len() > MAX_MARKER_FIELD_BYTES {
            return Err(DecisionViewError::MarkerFieldTooLong);
        }
        validate_refs(&recovery_refs)?;
        Ok(Self {
            kind,
            code,
            message,
            recovery_refs,
        })
    }

    pub const fn kind(&self) -> DecisionUncertaintyKind {
        self.kind
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn recovery_refs(&self) -> &[String] {
        &self.recovery_refs
    }
}

/// One caller-selected section. Constructors preserve anchors and bindings.
///
/// A section is commutative when a survival score is set (basis points,
/// `0..=10_000`); such sections participate in score-descending ordering
/// within their maximal same-class run. Sections without a score are
/// noncommutative and keep the caller's exact position and order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DecisionViewSection {
    kind: DecisionViewSectionKind,
    payload: Vec<u8>,
    tokenizer_identity_digest: Option<Sha256Digest>,
    tool_schema_digest: Option<Sha256Digest>,
    source_root_digest: Option<Sha256Digest>,
    model_profile_digest: Option<Sha256Digest>,
    survival_score_bps: Option<u32>,
}

/// Upper bound for survival scores, expressed in basis points (10_000 = 100%).
pub const MAX_SURVIVAL_SCORE_BPS: u32 = 10_000;

impl DecisionViewSection {
    pub fn stable_system_tool_contract(map: &ExactTokenMap) -> Result<Self, DecisionViewError> {
        Self::from_map(DecisionViewSectionKind::StableSystemToolContract, map)
    }

    pub fn stable_typed_effect_schema(map: &ExactTokenMap) -> Result<Self, DecisionViewError> {
        Self::from_map(DecisionViewSectionKind::StableTypedEffectSchema, map)
    }

    pub fn stable_project_capsule(capsule: &ModelCapsule) -> Result<Self, DecisionViewError> {
        Self::from_capsule(DecisionViewSectionKind::StableProjectCapsule, capsule)
    }

    pub fn stable_task_family_capsule(capsule: &ModelCapsule) -> Result<Self, DecisionViewError> {
        Self::from_capsule(DecisionViewSectionKind::StableTaskFamilyCapsule, capsule)
    }

    pub fn volatile_locus_evidence(page: &TokenPage) -> Result<Self, DecisionViewError> {
        let mut payload = b"TOKENZERO-LOCUS-EVIDENCE-V1\n".to_vec();
        put_record(
            &mut payload,
            "source_anchor",
            page.source_anchor().as_bytes(),
        )?;
        put_record(
            &mut payload,
            "tokenizer_identity",
            page.tokenizer_identity_digest().to_hex().as_bytes(),
        )?;
        put_record(
            &mut payload,
            "token_map",
            page.map_digest().to_hex().as_bytes(),
        )?;
        let token_range = page.token_range();
        let byte_range = page.byte_range();
        payload.extend_from_slice(
            format!(
                "token_range {} {}\nbyte_range {} {}\n",
                token_range.start, token_range.end, byte_range.start, byte_range.end
            )
            .as_bytes(),
        );
        put_record(&mut payload, "exact_bytes", &page.expand())?;
        Ok(Self {
            kind: DecisionViewSectionKind::VolatileLocusEvidence,
            payload,
            tokenizer_identity_digest: Some(page.tokenizer_identity_digest()),
            tool_schema_digest: None,
            source_root_digest: None,
            model_profile_digest: None,
            survival_score_bps: None,
        })
    }

    pub fn volatile_working_tree_delta(map: &ExactTokenMap) -> Result<Self, DecisionViewError> {
        Self::from_map(DecisionViewSectionKind::VolatileWorkingTreeDelta, map)
    }

    pub fn volatile_user_task(map: &ExactTokenMap) -> Result<Self, DecisionViewError> {
        Self::from_map(DecisionViewSectionKind::VolatileUserTask, map)
    }

    pub fn volatile_uncertainty_coverage(
        marker: &DecisionUncertaintyMarker,
    ) -> Result<Self, DecisionViewError> {
        let mut payload = b"TOKENZERO-UNCERTAINTY-MARKER-V1\n".to_vec();
        payload.extend_from_slice(format!("kind {}\n", marker.kind.as_str()).as_bytes());
        put_record(&mut payload, "code", marker.code.as_bytes())?;
        put_record(&mut payload, "message", marker.message.as_bytes())?;
        put_refs(&mut payload, &marker.recovery_refs)?;
        Ok(Self {
            kind: DecisionViewSectionKind::VolatileUncertaintyCoverage,
            payload,
            tokenizer_identity_digest: None,
            tool_schema_digest: None,
            source_root_digest: None,
            model_profile_digest: None,
            survival_score_bps: None,
        })
    }

    pub fn volatile_recovery_routes(refs: Vec<String>) -> Result<Self, DecisionViewError> {
        validate_refs(&refs)?;
        let mut payload = b"TOKENZERO-RECOVERY-ROUTES-V1\n".to_vec();
        put_refs(&mut payload, &refs)?;
        Ok(Self {
            kind: DecisionViewSectionKind::VolatileRecoveryRoutes,
            payload,
            tokenizer_identity_digest: None,
            tool_schema_digest: None,
            source_root_digest: None,
            model_profile_digest: None,
            survival_score_bps: None,
        })
    }

    pub const fn kind(&self) -> DecisionViewSectionKind {
        self.kind
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Survival score in basis points, when the section is commutative.
    /// `None` means the section is noncommutative (semantic order, kept
    /// verbatim at the caller's position).
    pub const fn survival_score_bps(&self) -> Option<u32> {
        self.survival_score_bps
    }

    /// Mark this section commutative with a measured survival score in basis
    /// points (basis points, `0..=10_000`, e.g. derived from
    /// `prefix_stability_ratio` history). The score participates in the view
    /// digest even when it does not change the rendered order.
    pub fn with_survival_score_bps(mut self, score: u32) -> Result<Self, DecisionViewError> {
        if score > MAX_SURVIVAL_SCORE_BPS {
            return Err(DecisionViewError::SurvivalScoreOutOfRange {
                actual: score,
                limit: MAX_SURVIVAL_SCORE_BPS,
            });
        }
        self.survival_score_bps = Some(score);
        Ok(self)
    }

    fn from_map(
        kind: DecisionViewSectionKind,
        map: &ExactTokenMap,
    ) -> Result<Self, DecisionViewError> {
        let byte_len = map.checked_byte_len()?;
        if byte_len > MAX_DECISION_VIEW_BYTES {
            return Err(DecisionViewError::ViewByteLimit {
                actual: byte_len,
                limit: MAX_DECISION_VIEW_BYTES,
            });
        }
        Ok(Self {
            kind,
            payload: map.reconstruct(),
            tokenizer_identity_digest: Some(map.tokenizer_identity_digest()),
            tool_schema_digest: (kind == DecisionViewSectionKind::StableTypedEffectSchema)
                .then_some(map.source_digest()),
            source_root_digest: None,
            model_profile_digest: None,
            survival_score_bps: None,
        })
    }

    fn from_capsule(
        kind: DecisionViewSectionKind,
        capsule: &ModelCapsule,
    ) -> Result<Self, DecisionViewError> {
        let mut payload = b"TOKENZERO-MODEL-CAPSULE-SECTION-V1\n".to_vec();
        put_record(
            &mut payload,
            "source_root",
            capsule.source_root_digest().to_hex().as_bytes(),
        )?;
        put_record(
            &mut payload,
            "model_profile",
            capsule.model_profile_digest().to_hex().as_bytes(),
        )?;
        put_record(
            &mut payload,
            "tokenizer_identity",
            capsule.tokenizer_identity_digest().to_hex().as_bytes(),
        )?;
        put_refs(&mut payload, capsule.evidence_refs())?;
        put_digests(&mut payload, capsule.token_page_digests())?;
        put_record(&mut payload, "rendered_capsule", &capsule.render())?;
        Ok(Self {
            kind,
            payload,
            tokenizer_identity_digest: Some(capsule.tokenizer_identity_digest()),
            tool_schema_digest: None,
            source_root_digest: Some(capsule.source_root_digest()),
            model_profile_digest: Some(capsule.model_profile_digest()),
            survival_score_bps: None,
        })
    }
}

/// Complete identity tuple for canonical stable-prefix bytes P(Z,M,T,R).
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DecisionViewIdentity {
    source_root_digest: Sha256Digest,
    model_profile_digest: Sha256Digest,
    tokenizer_identity_digest: Sha256Digest,
    tool_schema_digest: Sha256Digest,
    renderer_contract_digest: Sha256Digest,
}

impl DecisionViewIdentity {
    pub fn new(
        source_root_digest: Sha256Digest,
        model_profile_digest: Sha256Digest,
        tokenizer: &ExactTokenizerIdentity,
        tool_schema_digest: Sha256Digest,
    ) -> Self {
        Self {
            source_root_digest,
            model_profile_digest,
            tokenizer_identity_digest: tokenizer.digest(),
            tool_schema_digest,
            renderer_contract_digest: decision_view_renderer_contract_digest(),
        }
    }

    pub const fn source_root_digest(&self) -> Sha256Digest {
        self.source_root_digest
    }

    pub const fn model_profile_digest(&self) -> Sha256Digest {
        self.model_profile_digest
    }

    pub const fn tokenizer_identity_digest(&self) -> Sha256Digest {
        self.tokenizer_identity_digest
    }

    pub const fn tool_schema_digest(&self) -> Sha256Digest {
        self.tool_schema_digest
    }

    pub const fn renderer_contract_digest(&self) -> Sha256Digest {
        self.renderer_contract_digest
    }
}

/// Exact comparison of logical prefix identity only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixComparison {
    PrefixIdentical,
    IdentityChanged,
    PrefixBytesChanged,
}

/// Provider-neutral stable-prefix geometry. It contains no hit/eligibility flag.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StablePrefixGeometry {
    identity: DecisionViewIdentity,
    bytes: Vec<u8>,
    bytes_digest: Sha256Digest,
    breakpoint_after_bytes: u64,
    breakpoint_after_tokens: u64,
    geometry_digest: Sha256Digest,
}

impl StablePrefixGeometry {
    fn new(
        identity: DecisionViewIdentity,
        bytes: Vec<u8>,
        breakpoint_after_tokens: u64,
    ) -> Result<Self, DecisionViewError> {
        let bytes_digest = digest(&bytes);
        let breakpoint_after_bytes =
            u64::try_from(bytes.len()).map_err(|_| DecisionViewError::LengthOverflow)?;
        let geometry_digest = stable_prefix_geometry_digest(
            &identity,
            bytes_digest,
            breakpoint_after_bytes,
            breakpoint_after_tokens,
            &bytes,
        )?;
        Ok(Self {
            identity,
            bytes,
            bytes_digest,
            breakpoint_after_bytes,
            breakpoint_after_tokens,
            geometry_digest,
        })
    }

    pub fn identity(&self) -> &DecisionViewIdentity {
        &self.identity
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn bytes_digest(&self) -> Sha256Digest {
        self.bytes_digest
    }

    pub const fn breakpoint_after_bytes(&self) -> u64 {
        self.breakpoint_after_bytes
    }

    pub const fn breakpoint_after_tokens(&self) -> u64 {
        self.breakpoint_after_tokens
    }

    pub const fn digest(&self) -> Sha256Digest {
        self.geometry_digest
    }

    pub fn compare(&self, other: &Self) -> PrefixComparison {
        if self.identity != other.identity {
            PrefixComparison::IdentityChanged
        } else if self.bytes == other.bytes {
            PrefixComparison::PrefixIdentical
        } else {
            PrefixComparison::PrefixBytesChanged
        }
    }
}

/// One typed, caller-offered alternative. The hub envelope carries choices as
/// untyped `Vec<Value>`; here the shape is a concrete struct so the view
/// digest covers stable canonical bytes instead of free-form JSON.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CandidateChoice {
    id: String,
    description: String,
}

impl CandidateChoice {
    pub fn new(
        id: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<Self, DecisionViewError> {
        let id = id.into();
        let description = description.into();
        if id.is_empty() {
            return Err(DecisionViewError::EmptyChoiceId);
        }
        if id.len() > MAX_MARKER_FIELD_BYTES || description.len() > MAX_MARKER_FIELD_BYTES {
            return Err(DecisionViewError::ChoiceFieldTooLong);
        }
        Ok(Self { id, description })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn description(&self) -> &str {
        &self.description
    }
}

/// Declared completeness of the decision evidence, strongest first.
/// Serde serialization matches the hub `decision_view_v6.schema.json` enum
/// (`Proved`/`BoundedComplete`/`Observed`/`Unknown`) exactly.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum CompletenessGrade {
    Proved,
    BoundedComplete,
    Observed,
    Unknown,
}

impl CompletenessGrade {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proved => "Proved",
            Self::BoundedComplete => "BoundedComplete",
            Self::Observed => "Observed",
            Self::Unknown => "Unknown",
        }
    }

    /// Join two independently declared grades, keeping the weaker (more
    /// honest) claim. `Unknown` is terminal: it is never upgraded, regardless
    /// of the other grade.
    pub const fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Observed, _) | (_, Self::Observed) => Self::Observed,
            (Self::BoundedComplete, _) | (_, Self::BoundedComplete) => Self::BoundedComplete,
            _ => Self::Proved,
        }
    }
}

impl Default for CompletenessGrade {
    fn default() -> Self {
        Self::Unknown
    }
}

/// V6 decision metadata. Digest-covered at render time and serde-defaulted so
/// old-shaped JSON still deserializes (Unknown grade, empty vectors, no
/// baseline escape). Field names and enum values follow
/// `racc/v6/schemas/decision_view_v6.schema.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DecisionViewMetadata {
    #[serde(default)]
    candidate_choices: Vec<CandidateChoice>,
    #[serde(default)]
    supported_decisions: Vec<String>,
    #[serde(default)]
    completeness_grade: CompletenessGrade,
    #[serde(default)]
    baseline_escape: bool,
}

impl DecisionViewMetadata {
    pub fn new(
        candidate_choices: Vec<CandidateChoice>,
        supported_decisions: Vec<String>,
        completeness_grade: CompletenessGrade,
        baseline_escape: bool,
    ) -> Result<Self, DecisionViewError> {
        if candidate_choices.len() > MAX_DECISION_VIEW_METADATA_ITEMS
            || supported_decisions.len() > MAX_DECISION_VIEW_METADATA_ITEMS
        {
            let actual = candidate_choices.len().max(supported_decisions.len());
            return Err(DecisionViewError::TooManyMetadataItems {
                actual,
                limit: MAX_DECISION_VIEW_METADATA_ITEMS,
            });
        }
        Ok(Self {
            candidate_choices,
            supported_decisions,
            completeness_grade,
            baseline_escape,
        })
    }

    pub fn candidate_choices(&self) -> &[CandidateChoice] {
        &self.candidate_choices
    }

    pub fn supported_decisions(&self) -> &[String] {
        &self.supported_decisions
    }

    pub const fn completeness_grade(&self) -> CompletenessGrade {
        self.completeness_grade
    }

    pub const fn baseline_escape(&self) -> bool {
        self.baseline_escape
    }

    fn canonical_digest(&self) -> Result<Sha256Digest, DecisionViewError> {
        let mut canonical = b"TOKENZERO-DECISION-VIEW-METADATA-V1".to_vec();
        put_choices(&mut canonical, &self.candidate_choices)?;
        put_strings(
            &mut canonical,
            "supported_decisions",
            &self.supported_decisions,
        )?;
        append_bounded(&mut canonical, self.completeness_grade.as_str().as_bytes())?;
        canonical.push(u8::from(self.baseline_escape));
        Ok(digest(&canonical))
    }
}

impl Default for DecisionViewMetadata {
    fn default() -> Self {
        Self {
            candidate_choices: Vec::new(),
            supported_decisions: Vec::new(),
            completeness_grade: CompletenessGrade::Unknown,
            baseline_escape: false,
        }
    }
}

/// Deterministic rendering of one ordered, caller-selected Decision View.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DecisionView {
    identity: DecisionViewIdentity,
    section_kinds: Vec<DecisionViewSectionKind>,
    rendered: Vec<u8>,
    exact_token_map_digest: Sha256Digest,
    total_tokens: u64,
    volatile_bytes: u64,
    volatile_tokens: u64,
    stable_prefix: StablePrefixGeometry,
    metadata: DecisionViewMetadata,
    digest: Sha256Digest,
}

impl DecisionView {
    /// Render the supplied sections, preserving caller order verbatim for
    /// noncommutative sections and score-descending within commutative runs.
    pub fn render<T: ExactTokenizerAdapter + ?Sized>(
        tokenizer: &T,
        identity: DecisionViewIdentity,
        sections: Vec<DecisionViewSection>,
    ) -> Result<Self, DecisionViewError> {
        Self::render_with_metadata(
            tokenizer,
            identity,
            sections,
            DecisionViewMetadata::default(),
        )
    }

    /// Render with V6 decision metadata (candidate choices, supported
    /// decisions, completeness grade, baseline escape). The metadata is
    /// digest-covered but never rendered into the view framing bytes.
    ///
    /// Ordering (renderer contract v3): the stable-first invariant is
    /// validated on the caller's order first. Then each maximal run of
    /// same-class sections marked commutative (survival score set) is
    /// sorted score-descending with an order-independent tie-break (kind,
    /// then payload), so any permutation of a commutative run renders
    /// byte-identical output. Noncommutative sections keep the caller's
    /// exact positions, and stable sections always precede volatile ones.
    pub fn render_with_metadata<T: ExactTokenizerAdapter + ?Sized>(
        tokenizer: &T,
        identity: DecisionViewIdentity,
        sections: Vec<DecisionViewSection>,
        metadata: DecisionViewMetadata,
    ) -> Result<Self, DecisionViewError> {
        if sections.len() > MAX_DECISION_VIEW_SECTIONS {
            return Err(DecisionViewError::TooManySections {
                actual: sections.len(),
                limit: MAX_DECISION_VIEW_SECTIONS,
            });
        }
        if tokenizer.identity().digest() != identity.tokenizer_identity_digest {
            return Err(DecisionViewError::TokenizerIdentityMismatch {
                section: DecisionViewSectionKind::StableSystemToolContract,
            });
        }

        // Stable-first guard runs on the caller's order, before any
        // commutative reordering, so the reported index matches the input.
        let mut saw_volatile = false;
        for (index, section) in sections.iter().enumerate() {
            if section.kind.is_stable() {
                if saw_volatile {
                    return Err(DecisionViewError::StableSectionAfterVolatile { index });
                }
            } else {
                saw_volatile = true;
            }
        }

        let mut ordered = sections;
        sort_commutative_runs(&mut ordered);

        let mut rendered = b"TOKENZERO-DECISION-VIEW-V1\n".to_vec();
        let mut stable_boundary = rendered.len();
        let mut section_kinds = Vec::with_capacity(ordered.len());
        for section in ordered.iter() {
            if section
                .tokenizer_identity_digest
                .is_some_and(|digest| digest != identity.tokenizer_identity_digest)
            {
                return Err(DecisionViewError::TokenizerIdentityMismatch {
                    section: section.kind,
                });
            }
            if section
                .tool_schema_digest
                .is_some_and(|digest| digest != identity.tool_schema_digest)
            {
                return Err(DecisionViewError::ToolSchemaDigestMismatch);
            }
            if section
                .source_root_digest
                .is_some_and(|digest| digest != identity.source_root_digest)
            {
                return Err(DecisionViewError::CapsuleSourceRootMismatch);
            }
            if section
                .model_profile_digest
                .is_some_and(|digest| digest != identity.model_profile_digest)
            {
                return Err(DecisionViewError::CapsuleModelProfileMismatch);
            }
            append_section(&mut rendered, section)?;
            if section.kind.is_stable() {
                stable_boundary = rendered.len();
            }
            if rendered.len() > MAX_DECISION_VIEW_BYTES {
                return Err(DecisionViewError::ViewByteLimit {
                    actual: rendered.len(),
                    limit: MAX_DECISION_VIEW_BYTES,
                });
            }
            section_kinds.push(section.kind);
        }

        let token_map = ExactTokenMap::tokenize(tokenizer, &rendered)?;
        let stable_boundary_u64 =
            u64::try_from(stable_boundary).map_err(|_| DecisionViewError::LengthOverflow)?;
        let stable_token_range = token_map
            .token_range_for_bytes(0..stable_boundary_u64)
            .map_err(|error| match error {
                ModelArtifactError::TokenBoundaryRequired { byte_offset } => {
                    DecisionViewError::PrefixNotTokenAligned { byte_offset }
                }
                other => DecisionViewError::ModelArtifact(other),
            })?;
        let breakpoint_after_tokens =
            u64::try_from(stable_token_range.end).map_err(|_| DecisionViewError::LengthOverflow)?;
        let total_tokens = u64::try_from(token_map.token_count())
            .map_err(|_| DecisionViewError::LengthOverflow)?;
        let volatile_tokens = total_tokens
            .checked_sub(breakpoint_after_tokens)
            .ok_or(DecisionViewError::LengthOverflow)?;
        let volatile_bytes = u64::try_from(rendered.len() - stable_boundary)
            .map_err(|_| DecisionViewError::LengthOverflow)?;
        let stable_prefix = StablePrefixGeometry::new(
            identity.clone(),
            rendered[..stable_boundary].to_vec(),
            breakpoint_after_tokens,
        )?;
        let exact_token_map_digest = token_map.digest();
        let metadata_digest = metadata.canonical_digest()?;
        let view_digest = decision_view_digest(
            stable_prefix.digest(),
            exact_token_map_digest,
            &ordered,
            total_tokens,
            volatile_tokens,
            &rendered,
            metadata_digest,
        )?;
        Ok(Self {
            identity,
            section_kinds,
            rendered,
            exact_token_map_digest,
            total_tokens,
            volatile_bytes,
            volatile_tokens,
            stable_prefix,
            metadata,
            digest: view_digest,
        })
    }

    pub fn identity(&self) -> &DecisionViewIdentity {
        &self.identity
    }

    pub fn section_kinds(&self) -> &[DecisionViewSectionKind] {
        &self.section_kinds
    }

    pub fn rendered(&self) -> &[u8] {
        &self.rendered
    }

    pub const fn exact_token_map_digest(&self) -> Sha256Digest {
        self.exact_token_map_digest
    }

    pub const fn total_tokens(&self) -> u64 {
        self.total_tokens
    }

    pub const fn volatile_bytes(&self) -> u64 {
        self.volatile_bytes
    }

    pub const fn volatile_tokens(&self) -> u64 {
        self.volatile_tokens
    }

    pub fn stable_prefix(&self) -> &StablePrefixGeometry {
        &self.stable_prefix
    }

    pub fn metadata(&self) -> &DecisionViewMetadata {
        &self.metadata
    }

    pub const fn digest(&self) -> Sha256Digest {
        self.digest
    }
}

pub fn decision_view_renderer_contract_digest() -> Sha256Digest {
    digest(RENDERER_CONTRACT)
}

fn validate_refs(refs: &[String]) -> Result<(), DecisionViewError> {
    if refs.len() > MAX_DECISION_VIEW_RECOVERY_REFS {
        return Err(DecisionViewError::TooManyRecoveryRefs {
            actual: refs.len(),
            limit: MAX_DECISION_VIEW_RECOVERY_REFS,
        });
    }
    for reference in refs {
        let parsed = ZeroRef::parse(reference)
            .map_err(|error| DecisionViewError::InvalidRecoveryRef(error.to_string()))?;
        if parsed.to_string() != *reference {
            return Err(DecisionViewError::NoncanonicalRecoveryRef(
                reference.clone(),
            ));
        }
    }
    Ok(())
}

fn put_record(out: &mut Vec<u8>, label: &str, value: &[u8]) -> Result<(), DecisionViewError> {
    let len = u64::try_from(value.len()).map_err(|_| DecisionViewError::LengthOverflow)?;
    let header = format!(
        "{label} {len}
"
    );
    let projected = out
        .len()
        .checked_add(header.len())
        .and_then(|size| size.checked_add(value.len()))
        .and_then(|size| size.checked_add(1))
        .ok_or(DecisionViewError::LengthOverflow)?;
    ensure_view_bound(projected)?;
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(value);
    out.push(b'\n');
    Ok(())
}

fn put_refs(out: &mut Vec<u8>, refs: &[String]) -> Result<(), DecisionViewError> {
    let count = u64::try_from(refs.len()).map_err(|_| DecisionViewError::LengthOverflow)?;
    append_bounded(
        out,
        format!(
            "recovery_refs {count}
"
        )
        .as_bytes(),
    )?;
    for reference in refs {
        put_record(out, "recovery_ref", reference.as_bytes())?;
    }
    Ok(())
}

fn put_digests(out: &mut Vec<u8>, digests: &[Sha256Digest]) -> Result<(), DecisionViewError> {
    let count = u64::try_from(digests.len()).map_err(|_| DecisionViewError::LengthOverflow)?;
    append_bounded(
        out,
        format!(
            "token_page_digests {count}
"
        )
        .as_bytes(),
    )?;
    for value in digests {
        put_record(out, "token_page_digest", value.to_hex().as_bytes())?;
    }
    Ok(())
}

fn append_section(
    rendered: &mut Vec<u8>,
    section: &DecisionViewSection,
) -> Result<(), DecisionViewError> {
    let len =
        u64::try_from(section.payload.len()).map_err(|_| DecisionViewError::LengthOverflow)?;
    let header = format!(
        "section {} {len}
",
        section.kind.as_str()
    );
    let projected = rendered
        .len()
        .checked_add(header.len())
        .and_then(|size| size.checked_add(section.payload.len()))
        .and_then(|size| size.checked_add(1))
        .ok_or(DecisionViewError::LengthOverflow)?;
    ensure_view_bound(projected)?;
    rendered.extend_from_slice(header.as_bytes());
    rendered.extend_from_slice(&section.payload);
    rendered.push(b'\n');
    Ok(())
}

fn append_bounded(out: &mut Vec<u8>, value: &[u8]) -> Result<(), DecisionViewError> {
    let projected = out
        .len()
        .checked_add(value.len())
        .ok_or(DecisionViewError::LengthOverflow)?;
    ensure_view_bound(projected)?;
    out.extend_from_slice(value);
    Ok(())
}

fn ensure_view_bound(actual: usize) -> Result<(), DecisionViewError> {
    if actual > MAX_DECISION_VIEW_BYTES {
        return Err(DecisionViewError::ViewByteLimit {
            actual,
            limit: MAX_DECISION_VIEW_BYTES,
        });
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(bytes))
}

fn put_identity(out: &mut Vec<u8>, identity: &DecisionViewIdentity) {
    out.extend_from_slice(identity.source_root_digest.as_bytes());
    out.extend_from_slice(identity.model_profile_digest.as_bytes());
    out.extend_from_slice(identity.tokenizer_identity_digest.as_bytes());
    out.extend_from_slice(identity.tool_schema_digest.as_bytes());
    out.extend_from_slice(identity.renderer_contract_digest.as_bytes());
}

fn stable_prefix_geometry_digest(
    identity: &DecisionViewIdentity,
    bytes_digest: Sha256Digest,
    byte_count: u64,
    token_count: u64,
    bytes: &[u8],
) -> Result<Sha256Digest, DecisionViewError> {
    let mut canonical = b"TOKENZERO-STABLE-PREFIX-GEOMETRY-V1".to_vec();
    put_identity(&mut canonical, identity);
    canonical.extend_from_slice(bytes_digest.as_bytes());
    canonical.extend_from_slice(&byte_count.to_be_bytes());
    canonical.extend_from_slice(&token_count.to_be_bytes());
    put_binary(&mut canonical, bytes)?;
    Ok(digest(&canonical))
}

fn decision_view_digest(
    prefix_geometry: Sha256Digest,
    token_map: Sha256Digest,
    sections: &[DecisionViewSection],
    total_tokens: u64,
    volatile_tokens: u64,
    rendered: &[u8],
    metadata: Sha256Digest,
) -> Result<Sha256Digest, DecisionViewError> {
    let mut canonical = b"TOKENZERO-DECISION-VIEW-IDENTITY-V1".to_vec();
    canonical.extend_from_slice(prefix_geometry.as_bytes());
    canonical.extend_from_slice(token_map.as_bytes());
    canonical.extend_from_slice(
        &u64::try_from(sections.len())
            .map_err(|_| DecisionViewError::LengthOverflow)?
            .to_be_bytes(),
    );
    for section in sections {
        put_binary(&mut canonical, section.kind.as_str().as_bytes())?;
        // Survival score participates in the digest even when it does not
        // change the rendered order (flag + u32 basis points).
        canonical.push(u8::from(section.survival_score_bps.is_some()));
        canonical.extend_from_slice(&section.survival_score_bps.unwrap_or(0).to_be_bytes());
    }
    canonical.extend_from_slice(&total_tokens.to_be_bytes());
    canonical.extend_from_slice(&volatile_tokens.to_be_bytes());
    put_binary(&mut canonical, rendered)?;
    canonical.extend_from_slice(metadata.as_bytes());
    Ok(digest(&canonical))
}

/// Score-descending sort of each maximal run of same-class commutative
/// sections, in place. Noncommutative sections keep the caller's exact
/// position; stable and volatile sections never share a run, so the
/// stable-first invariant survives reordering. Tie-break by kind then
/// payload -- both order-independent -- so the result is a canonical
/// function of the section multiset: any caller permutation of a
/// commutative run renders byte-identical output.
fn sort_commutative_runs(sections: &mut [DecisionViewSection]) {
    let mut index = 0;
    while index < sections.len() {
        if sections[index].survival_score_bps.is_none() {
            index += 1;
            continue;
        }
        let is_stable = sections[index].kind.is_stable();
        let mut end = index + 1;
        while end < sections.len()
            && sections[end].survival_score_bps.is_some()
            && sections[end].kind.is_stable() == is_stable
        {
            end += 1;
        }
        sections[index..end].sort_by(|left, right| {
            right
                .survival_score_bps
                .unwrap_or(0)
                .cmp(&left.survival_score_bps.unwrap_or(0))
                .then_with(|| left.kind.as_str().cmp(right.kind.as_str()))
                .then_with(|| left.payload.cmp(&right.payload))
        });
        index = end;
    }
}

fn put_binary(out: &mut Vec<u8>, value: &[u8]) -> Result<(), DecisionViewError> {
    out.extend_from_slice(
        &u64::try_from(value.len())
            .map_err(|_| DecisionViewError::LengthOverflow)?
            .to_be_bytes(),
    );
    out.extend_from_slice(value);
    Ok(())
}

fn put_choices(out: &mut Vec<u8>, choices: &[CandidateChoice]) -> Result<(), DecisionViewError> {
    let count = u64::try_from(choices.len()).map_err(|_| DecisionViewError::LengthOverflow)?;
    append_bounded(
        out,
        format!(
            "candidate_choices {count}
"
        )
        .as_bytes(),
    )?;
    for choice in choices {
        put_record(out, "choice_id", choice.id.as_bytes())?;
        put_record(out, "choice_description", choice.description.as_bytes())?;
    }
    Ok(())
}

fn put_strings(out: &mut Vec<u8>, label: &str, values: &[String]) -> Result<(), DecisionViewError> {
    let count = u64::try_from(values.len()).map_err(|_| DecisionViewError::LengthOverflow)?;
    append_bounded(out, format!("{label} {count}\n").as_bytes())?;
    for value in values {
        put_record(out, "item", value.as_bytes())?;
    }
    Ok(())
}

