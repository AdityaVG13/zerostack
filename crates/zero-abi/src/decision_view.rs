//! Typed `DecisionView`: the model-facing decision view (ZS-VIEW-010).
//!
//! Wire shape is closed (`additionalProperties: false`). Required roots
//! and grades: `task_contract_root`, `project_root`, `causal_lens_root`,
//! `supported_decisions`, `completeness_grade`, `baseline_escape` -- plus the
//! optional `evidence_refs`, `omitted_classes`, `expansion_handles`,
//! `unresolved_question`, and `canonical_render_root`.
//!
//! Honesty laws:
//! - The view is canonical: [`DecisionView::canonical_render_json`] is the
//!   bounded rendering (sorted-key JSON) and [`DecisionView::root`] is its
//!   SHA-256 hex. Any mutation of a rendered field changes the root
//!   ([`DecisionView::verify_root`] detects tampering). No root is ever
//!   fabricated: construction fails closed on empty roots, and the
//!   `canonical_render_root` is only carried when a harness actually bound a
//!   rendering artifact root.
//! - Completeness is certified, never asserted: [`DecisionView::certificate`]
//!   verifies the claimed grade against the evidence classes actually
//!   present. A `Proved` claim with missing classes, declared omissions, or
//!   no evidence refs fails the certificate; any other claim with a missing
//!   needed class degrades to `Unknown`.
//! - Exact expansion is bound, never guessed: [`DecisionViewBinding`] binds
//!   every listed `expansion_handles` entry to a canonical object, and
//!   [`DecisionViewBinding::expand_exact`] returns a typed miss for any
//!   unbound handle (never a silent fabrication).

use std::{collections::{BTreeMap, BTreeSet}, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::digest::sha256_hex;
use crate::schema::canonical_json;

/// Canonical schema id of the decision view contract.
pub const DECISION_VIEW_SCHEMA_ID: &str =
    "https://zerostack.dev/schemas/decision_view.schema.json";

/// Completeness grade of a decision view's evidence coverage, exactly as the
/// schema enumerates it. `Observed` records a decision surface without any
/// coverage claim; `Proved` is the only grade that must never coexist with
/// omissions or missing evidence (enforced by
/// [`DecisionView::certificate`]).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum CompletenessGrade {
    Proved,
    BoundedComplete,
    Observed,
    Unknown,
}

impl CompletenessGrade {
    /// The PascalCase wire spelling of this grade, exactly as the schema
    /// enumerates it.
    pub fn grade_name(self) -> &'static str {
        match self {
            CompletenessGrade::Proved => "Proved",
            CompletenessGrade::BoundedComplete => "BoundedComplete",
            CompletenessGrade::Observed => "Observed",
            CompletenessGrade::Unknown => "Unknown",
        }
    }
}

/// Fail-closed construction, certification, and expansion error for the
/// decision view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecisionViewError {
    /// A schema-required root field must be nonempty; a fabricated root is
    /// never acceptable.
    EmptyRoot(&'static str),
    /// A decision view that supports no decisions is not a decision view.
    EmptySupportedDecisions,
    /// A list entry (evidence ref, omitted class, expansion handle) must be
    /// nonempty.
    EmptyListEntry(&'static str),
    /// Certificate failure: the view claims `Proved` but carries no evidence
    /// refs.
    ProvedClaimWithoutEvidence,
    /// Certificate failure: the view claims `Proved` but declares omitted
    /// evidence classes -- a Proved claim must not omit anything.
    ProvedClaimWithOmissions,
    /// Certificate failure: a `Proved` claim has a needed evidence class
    /// missing from the classes actually present.
    MissingEvidenceClass { class: String },
    /// The view's canonical rendering does not hash to the given root; the
    /// view (or the root) was tampered with.
    RootMismatch,
    /// Exact expansion of a handle that was never bound: a typed miss, never
    /// a silent fabrication.
    UnknownExpansionHandle(String),
    /// A binding entry names a handle the view's `expansion_handles` does not
    /// list.
    ExpansionHandleNotListed(String),
    /// The view lists an expansion handle that the binding does not bind: a
    /// dangling claim.
    UnboundExpansionHandle(String),
}

impl fmt::Display for DecisionViewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyRoot(field) => {
                write!(formatter, "decision view requires a nonempty {field}")
            }
            Self::EmptySupportedDecisions => {
                write!(formatter, "supported_decisions must be nonempty")
            }
            Self::EmptyListEntry(field) => {
                write!(formatter, "decision view {field} entries must be nonempty")
            }
            Self::ProvedClaimWithoutEvidence => {
                write!(formatter, "a Proved claim must carry evidence refs")
            }
            Self::ProvedClaimWithOmissions => {
                write!(formatter, "a Proved claim must not declare omitted classes")
            }
            Self::MissingEvidenceClass { class } => {
                write!(formatter, "Proved claim is missing needed evidence class {class}")
            }
            Self::RootMismatch => write!(
                formatter,
                "decision view canonical render does not match the bound root"
            ),
            Self::UnknownExpansionHandle(handle) => {
                write!(formatter, "expansion handle {handle} is not bound")
            }
            Self::ExpansionHandleNotListed(handle) => {
                write!(formatter, "expansion handle {handle} is not listed by the view")
            }
            Self::UnboundExpansionHandle(handle) => {
                write!(formatter, "expansion handle {handle} is listed but not bound")
            }
        }
    }
}

impl Error for DecisionViewError {}

/// The typed model-facing decision view (ZS-VIEW-010).
///
/// Serialization matches `decision_view.schema.json` field for field and
/// rejects unknown fields on deserialization, exactly like the schema's
/// `additionalProperties: false`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionView {
    task_contract_root: String,
    project_root: String,
    causal_lens_root: String,
    supported_decisions: Vec<String>,
    #[serde(default)]
    evidence_refs: Vec<String>,
    #[serde(default)]
    omitted_classes: Vec<String>,
    #[serde(default)]
    expansion_handles: Vec<String>,
    completeness_grade: CompletenessGrade,
    #[serde(default)]
    unresolved_question: Option<String>,
    baseline_escape: bool,
    #[serde(default)]
    canonical_render_root: Option<String>,
}

impl DecisionView {
    /// Fail-closed construction: every schema-required root must be nonempty,
    /// `supported_decisions` must be nonempty, and every list entry must be
    /// nonempty. An empty root is a fabricated anchor and is rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        task_contract_root: impl Into<String>,
        project_root: impl Into<String>,
        causal_lens_root: impl Into<String>,
        supported_decisions: Vec<String>,
        evidence_refs: Vec<String>,
        omitted_classes: Vec<String>,
        expansion_handles: Vec<String>,
        completeness_grade: CompletenessGrade,
        unresolved_question: Option<String>,
        baseline_escape: bool,
        canonical_render_root: Option<String>,
    ) -> Result<Self, DecisionViewError> {
        let view = Self {
            task_contract_root: task_contract_root.into(),
            project_root: project_root.into(),
            causal_lens_root: causal_lens_root.into(),
            supported_decisions,
            evidence_refs,
            omitted_classes,
            expansion_handles,
            completeness_grade,
            unresolved_question,
            baseline_escape,
            canonical_render_root,
        };
        view.validate()?;
        Ok(view)
    }

    /// Fail-closed validation of the schema-required structure.
    pub fn validate(&self) -> Result<(), DecisionViewError> {
        for field in [
            "task_contract_root",
            "project_root",
            "causal_lens_root",
        ] {
            match field {
                "task_contract_root" if self.task_contract_root.is_empty() => {
                    return Err(DecisionViewError::EmptyRoot(field));
                }
                "project_root" if self.project_root.is_empty() => {
                    return Err(DecisionViewError::EmptyRoot(field));
                }
                "causal_lens_root" if self.causal_lens_root.is_empty() => {
                    return Err(DecisionViewError::EmptyRoot(field));
                }
                _ => {}
            }
        }
        if self.supported_decisions.is_empty() {
            return Err(DecisionViewError::EmptySupportedDecisions);
        }
        if self.supported_decisions.iter().any(String::is_empty) {
            return Err(DecisionViewError::EmptyListEntry("supported_decisions"));
        }
        if self.evidence_refs.iter().any(String::is_empty) {
            return Err(DecisionViewError::EmptyListEntry("evidence_refs"));
        }
        if self.omitted_classes.iter().any(String::is_empty) {
            return Err(DecisionViewError::EmptyListEntry("omitted_classes"));
        }
        if self.expansion_handles.iter().any(String::is_empty) {
            return Err(DecisionViewError::EmptyListEntry("expansion_handles"));
        }
        if self
            .canonical_render_root
            .as_deref()
            .is_some_and(str::is_empty)
        {
            return Err(DecisionViewError::EmptyRoot("canonical_render_root"));
        }
        Ok(())
    }

    /// The bounded rendering as a JSON value: the view serialized exactly as
    /// the schema spells it. Struct serialization cannot fail.
    pub fn canonical_render(&self) -> Value {
        serde_json::to_value(self)
            .expect("DecisionView canonical render serializes by construction")
    }

    /// The canonical bounded rendering: deterministic sorted-key JSON.
    pub fn canonical_render_json(&self) -> String {
        canonical_json(&self.canonical_render())
    }

    /// The digest root of the view: SHA-256 hex over the canonical bounded
    /// rendering. Two engines can never disagree on this root.
    pub fn root(&self) -> String {
        sha256_hex(self.canonical_render_json().as_bytes())
    }

    /// Fail-closed root verification: the canonical rendering must hash to
    /// the given root, or the view (or the bound root) was tampered with.
    pub fn verify_root(&self, root: &str) -> Result<(), DecisionViewError> {
        if self.root() == root {
            Ok(())
        } else {
            Err(DecisionViewError::RootMismatch)
        }
    }

    /// Certify the claimed completeness grade against the evidence classes
    /// actually present.
    ///
    /// Laws:
    /// 1. A `Proved` claim must carry evidence refs and must not declare
    ///    omissions -- either violation fails the certificate.
    /// 2. Any needed evidence class missing from the present set fails a
    ///    `Proved` claim and degrades every other claim to `Unknown`:
    ///    removing a needed evidence class can never leave a higher grade
    ///    intact.
    /// 3. With every needed class present, the claimed grade is returned
    ///    unchanged.
    pub fn certificate(
        &self,
        needed_classes: &BTreeSet<String>,
        present_classes: &BTreeSet<String>,
    ) -> Result<CompletenessGrade, DecisionViewError> {
        if self.completeness_grade == CompletenessGrade::Proved {
            if self.evidence_refs.is_empty() {
                return Err(DecisionViewError::ProvedClaimWithoutEvidence);
            }
            if !self.omitted_classes.is_empty() {
                return Err(DecisionViewError::ProvedClaimWithOmissions);
            }
        }
        if let Some(missing) = needed_classes.difference(present_classes).next() {
            if self.completeness_grade == CompletenessGrade::Proved {
                return Err(DecisionViewError::MissingEvidenceClass {
                    class: missing.clone(),
                });
            }
            return Ok(CompletenessGrade::Unknown);
        }
        Ok(self.completeness_grade)
    }

    pub fn task_contract_root(&self) -> &str {
        &self.task_contract_root
    }

    pub fn project_root(&self) -> &str {
        &self.project_root
    }

    pub fn causal_lens_root(&self) -> &str {
        &self.causal_lens_root
    }

    pub fn supported_decisions(&self) -> &[String] {
        &self.supported_decisions
    }

    pub fn evidence_refs(&self) -> &[String] {
        &self.evidence_refs
    }

    pub fn omitted_classes(&self) -> &[String] {
        &self.omitted_classes
    }

    pub fn expansion_handles(&self) -> &[String] {
        &self.expansion_handles
    }

    pub fn completeness_grade(&self) -> CompletenessGrade {
        self.completeness_grade
    }

    pub fn unresolved_question(&self) -> Option<&str> {
        self.unresolved_question.as_deref()
    }

    pub fn baseline_escape(&self) -> bool {
        self.baseline_escape
    }

    pub fn canonical_render_root(&self) -> Option<&str> {
        self.canonical_render_root.as_deref()
    }
}

/// A view bound to its exact expansion objects.
///
/// Every handle the view lists in `expansion_handles` must be bound to a
/// canonical object; a listed-but-unbound handle is a dangling claim and is
/// rejected at bind time. Expansion returns the bound canonical object
/// exactly -- never a partial or guessed rendering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionViewBinding {
    view: DecisionView,
    /// Handle -> canonical JSON string of the bound object.
    expansions: BTreeMap<String, String>,
}

impl DecisionViewBinding {
    /// Fail-closed binding: every entry handle must be listed by the view,
    /// and every listed handle must have an entry. Objects are canonicalized
    /// at bind time so expansion reproduces them byte-exactly.
    pub fn new(
        view: DecisionView,
        expansions: Vec<(String, Value)>,
    ) -> Result<Self, DecisionViewError> {
        let mut bound = BTreeMap::new();
        for (handle, object) in expansions {
            if handle.is_empty() {
                return Err(DecisionViewError::EmptyListEntry("expansion handle"));
            }
            if !view.expansion_handles.contains(&handle) {
                return Err(DecisionViewError::ExpansionHandleNotListed(handle));
            }
            bound.insert(handle, canonical_json(&object));
        }
        for handle in &view.expansion_handles {
            if !bound.contains_key(handle) {
                return Err(DecisionViewError::UnboundExpansionHandle(handle.clone()));
            }
        }
        Ok(Self {
            view,
            expansions: bound,
        })
    }

    /// Exact expansion of one bound handle: the canonical object bound at
    /// construction, reproduced byte-exactly. An unbound handle is a typed
    /// miss ([`DecisionViewError::UnknownExpansionHandle`]) -- never a
    /// silent fabrication.
    pub fn expand_exact(&self, handle: &str) -> Result<Value, DecisionViewError> {
        let canonical = self
            .expansions
            .get(handle)
            .ok_or_else(|| DecisionViewError::UnknownExpansionHandle(handle.to_owned()))?;
        serde_json::from_str(canonical)
            .map_err(|error| DecisionViewError::UnknownExpansionHandle(format!(
                "{handle}: bound object failed to parse: {error}"
            )))
    }

    pub fn view(&self) -> &DecisionView {
        &self.view
    }

    /// The digest root of the bound view.
    pub fn root(&self) -> String {
        self.view.root()
    }
}

