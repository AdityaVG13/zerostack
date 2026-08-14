//! V6-R13: executable theorem checkers (Draft 6, Thm 5.1 / 6.1 / 7.1 / 8.1).
//!
//! Each checker takes the theorem's premises as measured/typed inputs and
//! verifies the claimed bound holds. All arithmetic is exact integer
//! arithmetic (or exact rational arithmetic via [`crate::solver::Rational`]);
//! no floats, no rounding, no extrapolation. An unmet premise is a typed
//! refusal, never a weaker certificate.
//!
//! Theorem texts (racc/v6/packs/01_CURRENT_PAPERS_TEXT.md):
//!
//! * Thm 5.1 Explanation Evidence Preservation: if every factual claim in a
//!   compact explanation is supported by an exact rooted source/runtime
//!   artifact or explicitly labeled inference, and all omitted evidence
//!   remains expandable before a protected factual decision, then the compact
//!   interface does not reduce the baseline factual strategy set.
//! * Thm 6.1 Decision-Delimited Refactor: if a refactor contains `d`
//!   unresolved adaptive semantic decisions and all other operations are
//!   privately composable and verifiable, then the prepared model-visible
//!   interaction requires exactly `d + 1` Zero Execute calls.
//! * Thm 7.1 Port Nonregression under Complete Observational Coverage: if
//!   `V = B`, the verifier is sound, the target satisfies every obligation in
//!   `V`, and the source baseline remains available for uncovered environment
//!   cases, then the published target is protected-equivalent within the
//!   declared observational contract.
//! * Thm 8.1 Greenfield Strategy Preservation: if every suggestion,
//!   capability, or plan is optional, exact project evidence remains
//!   expandable, native tools remain available, and subjective decisions
//!   remain with the model/user, then adding the backend cannot remove a
//!   baseline construction strategy.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use zero_abi::{CoverageGradeV1, ProtectedDimensionV1, ProtectedScopeObligationsV1};

// ---------------------------------------------------------------------------
// Thm 5.1 -- Explanation Evidence Preservation
// ---------------------------------------------------------------------------

/// How one factual claim inside a compact explanation view is supported.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClaimSupport {
    /// The claim is backed by an exact rooted source/runtime artifact.
    Rooted { artifact_root: String },
    /// The claim is an explicitly labeled inference, not a factual assertion.
    LabeledInference { label: String },
}

/// One factual claim in a compact explanation view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FactualClaim {
    /// Stable claim identifier.
    pub id: String,
    /// The claim's support in the compact view.
    pub support: ClaimSupport,
    /// Whether evidence for this claim was omitted from the compact view.
    pub omitted_evidence: bool,
    /// Handle that expands the omitted evidence to its bound artifact.
    pub expansion_handle: Option<String>,
}

/// A compact explanation view: the claims, the exact rooted artifacts it
/// exposes, and the expansion table for omitted evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompactExplanationView {
    /// The factual claims published in the compact view.
    pub claims: Vec<FactualClaim>,
    /// Exact rooted artifact roots present in the view.
    pub artifacts: BTreeSet<String>,
    /// Expansion table: handle -> bound artifact root.
    pub expansions: BTreeMap<String, String>,
}

/// Thm 5.1 certification: the compact view preserves the baseline factual
/// strategy set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidencePreservationCertification {
    /// Number of certified factual claims.
    pub certified_claims: usize,
    /// Number of omitted-evidence claims proven expandable to a bound
    /// artifact.
    pub expandable_omissions: usize,
}

/// Verifies Thm 5.1's premises over a compact explanation view.
///
/// Refuses (never certifies) when a factual claim is not supported by an
/// exact rooted artifact present in the view or an explicit inference label,
/// or when omitted evidence is not expandable to its bound artifact before a
/// protected factual decision.
pub fn check_explanation_evidence_preservation(
    view: &CompactExplanationView,
) -> Result<EvidencePreservationCertification, TheoremViolation> {
    let mut expandable_omissions = 0usize;
    for claim in &view.claims {
        if claim.id.is_empty() {
            return Err(TheoremViolation::EmptyClaimId);
        }
        match &claim.support {
            ClaimSupport::Rooted { artifact_root } => {
                if artifact_root.is_empty() {
                    return Err(TheoremViolation::EmptyArtifactRoot {
                        id: claim.id.clone(),
                    });
                }
                // Premise 1a: the exact rooted artifact must exist in the
                // view. A claim whose root is absent has no evidence
                // authority in the compact interface.
                if !view.artifacts.contains(artifact_root) {
                    return Err(TheoremViolation::UnrootedClaim {
                        id: claim.id.clone(),
                        artifact_root: artifact_root.clone(),
                    });
                }
                if claim.omitted_evidence {
                    verify_expandable_omission(view, claim, Some(artifact_root))?;
                    expandable_omissions += 1;
                }
            }
            ClaimSupport::LabeledInference { label } => {
                if label.is_empty() {
                    return Err(TheoremViolation::EmptyInferenceLabel {
                        id: claim.id.clone(),
                    });
                }
                if claim.omitted_evidence {
                    verify_expandable_omission(view, claim, None)?;
                    expandable_omissions += 1;
                }
            }
        }
    }
    Ok(EvidencePreservationCertification {
        certified_claims: view.claims.len(),
        expandable_omissions,
    })
}

/// Verifies that a claim's omitted evidence is expandable before a protected
/// factual decision. For a rooted claim the expansion must resolve exactly to
/// the claim's bound artifact (the theorem's falsifier); for a labeled
/// inference the expansion must resolve to an exact rooted artifact present
/// in the view.
fn verify_expandable_omission(
    view: &CompactExplanationView,
    claim: &FactualClaim,
    bound_artifact: Option<&String>,
) -> Result<(), TheoremViolation> {
    let handle = claim.expansion_handle.as_ref().ok_or_else(|| {
        TheoremViolation::OmittedEvidenceNotExpandable { id: claim.id.clone() }
    })?;
    let resolved = view
        .expansions
        .get(handle)
        .ok_or_else(|| TheoremViolation::UnresolvableExpansion {
            id: claim.id.clone(),
            handle: handle.clone(),
        })?;
    if let Some(bound) = bound_artifact
        && resolved != bound
    {
        return Err(TheoremViolation::ExpansionMismatch {
            id: claim.id.clone(),
            handle: handle.clone(),
            bound: resolved.clone(),
            artifact_root: bound.clone(),
        });
    }
    if !view.artifacts.contains(resolved) {
        return Err(TheoremViolation::UnrootedExpansion {
            id: claim.id.clone(),
            artifact_root: resolved.clone(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Thm 6.1 -- Decision-Delimited Refactor (d + 1 interaction call count)
// ---------------------------------------------------------------------------

/// One continuation handle of a prepared model-visible interaction.
///
/// Runtime continuation handles carry no call count today, so both the
/// declared unresolved decision count and the observed call count are
/// measured/typed inputs to the checker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContinuationHandle {
    /// Handle identifier.
    pub id: String,
    /// `d`: unresolved adaptive semantic decisions declared for the
    /// interaction.
    pub declared_unresolved_decisions: u64,
    /// Observed Zero Execute calls in the prepared model-visible interaction.
    pub observed_zero_execute_calls: u64,
}

/// Thm 6.1 checker input: the continuation handles plus the two premises on
/// the remaining operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionDelimitedRefactorInput {
    /// The continuation handles of the refactor interaction.
    pub handles: Vec<ContinuationHandle>,
    /// Premise: every other operation is privately composable.
    pub other_operations_privately_composable: bool,
    /// Premise: every other operation is verifiable.
    pub other_operations_verifiable: bool,
}

/// Thm 6.1 certification: every interaction required exactly `d + 1` calls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallCountCertification {
    /// Number of certified continuation interactions.
    pub certified_interactions: usize,
    /// Per-handle expected call count `d + 1`.
    pub expected_calls: Vec<(String, u64)>,
}

/// Verifies Thm 6.1: for every continuation handle, the observed Zero Execute
/// call count must equal `declared_unresolved_decisions + 1` exactly.
///
/// Refuses when the private-composability or verifiability premise is unmet,
/// when the interaction carries no handles, or when `d + 1` overflows.
pub fn check_decision_delimited_refactor(
    input: &DecisionDelimitedRefactorInput,
) -> Result<CallCountCertification, TheoremViolation> {
    if !input.other_operations_privately_composable {
        return Err(TheoremViolation::NotPrivatelyComposable);
    }
    if !input.other_operations_verifiable {
        return Err(TheoremViolation::NotVerifiable);
    }
    if input.handles.is_empty() {
        return Err(TheoremViolation::NoContinuationHandles);
    }
    let mut expected_calls = Vec::with_capacity(input.handles.len());
    for handle in &input.handles {
        if handle.id.is_empty() {
            return Err(TheoremViolation::EmptyHandleId);
        }
        let expected = handle
            .declared_unresolved_decisions
            .checked_add(1)
            .ok_or_else(|| TheoremViolation::DecisionCountOverflow {
                id: handle.id.clone(),
            })?;
        if handle.observed_zero_execute_calls != expected {
            return Err(TheoremViolation::CallCountMismatch {
                id: handle.id.clone(),
                expected,
                actual: handle.observed_zero_execute_calls,
            });
        }
        expected_calls.push((handle.id.clone(), expected));
    }
    Ok(CallCountCertification {
        certified_interactions: input.handles.len(),
        expected_calls,
    })
}

// ---------------------------------------------------------------------------
// Thm 7.1 -- Port Nonregression under Complete Observational Coverage
// ---------------------------------------------------------------------------

/// Thm 7.1 checker input.
///
/// `B` is the declared source-behavior obligation set; the verified subset
/// `V` is read from each obligation's coverage grade. The checker certifies
/// protected equivalence only when `V == B` and every premise holds.
pub struct PortNonregressionInput<'a> {
    /// The declared obligation set `B` with per-obligation coverage grades.
    pub obligations: &'a ProtectedScopeObligationsV1,
    /// Premise: the verifier is sound.
    pub verifier_sound: bool,
    /// Premise: the source baseline remains available for uncovered
    /// environment cases.
    pub source_baseline_available: bool,
}

/// Thm 7.1 certification: the target is protected-equivalent within the
/// declared observational contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectedEquivalenceCertification {
    /// Declared obligation count `|B|`.
    pub declared_obligations: usize,
    /// Verified obligation count `|V|` (equals `|B|` when certified).
    pub verified_obligations: usize,
    /// Every covered dimension, in declaration order.
    pub dimensions: Vec<ProtectedDimensionV1>,
}

/// Verifies Thm 7.1's premises over a [`ProtectedScopeObligationsV1`].
///
/// Refuses (obligations stay Unknown) when `V != B` -- any obligation with
/// grade `Unknown` -- when a required obligation is only `Observed` (the
/// CONTRACT-004 fail-closed rule, matching `zero_abi`'s
/// `equivalent_claim_permitted`), or when the verifier-soundness or
/// source-baseline premise is unmet. The checker never extrapolates beyond
/// the declared observational contract.
pub fn check_port_nonregression_coverage(
    input: &PortNonregressionInput,
) -> Result<ProtectedEquivalenceCertification, TheoremViolation> {
    let obligations = input.obligations;
    if obligations.obligations.is_empty() {
        return Err(TheoremViolation::NoDeclaredObligations);
    }
    // V == B: every declared obligation carries a non-Unknown grade.
    let uncovered = obligations.uncovered();
    if !uncovered.is_empty() {
        return Err(TheoremViolation::IncompleteCoverage { uncovered });
    }
    // Required obligations must be Proved or BoundedComplete: an Observed
    // grade cannot back a protected-equivalence claim.
    if let Some(weak) = obligations
        .obligations
        .iter()
        .find(|obligation| obligation.required && obligation.grade == CoverageGradeV1::Observed)
    {
        return Err(TheoremViolation::WeakRequiredObligation {
            dimension: weak.dimension,
        });
    }
    if !input.verifier_sound {
        return Err(TheoremViolation::UnsoundVerifier);
    }
    if !input.source_baseline_available {
        return Err(TheoremViolation::BaselineUnavailable);
    }
    let declared = obligations.obligations.len();
    Ok(ProtectedEquivalenceCertification {
        declared_obligations: declared,
        verified_obligations: declared,
        dimensions: obligations
            .obligations
            .iter()
            .map(|obligation| obligation.dimension)
            .collect(),
    })
}

// ---------------------------------------------------------------------------
// Thm 8.1 -- Greenfield Strategy Preservation (mandatory-gate audit)
// ---------------------------------------------------------------------------

/// What kind of backend surface is being audited.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityKind {
    /// A suggestion offered by the backend.
    Suggestion,
    /// A capability offered by the backend.
    Capability,
    /// A plan offered by the backend.
    Plan,
}

/// One backend suggestion/capability/plan under the mandatory-gate audit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendCapability {
    /// Capability identifier.
    pub id: String,
    /// Which kind of surface this is.
    pub kind: CapabilityKind,
    /// Whether the surface is optional (no mandatory backend gate).
    pub optional: bool,
    /// A native tool the surface needs, if any.
    pub requires_native_tool: Option<String>,
}

/// Thm 8.1 checker input: the optional capability set plus the three
/// remaining premises.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GreenfieldStrategyInput {
    /// The backend's suggestions/capabilities/plans.
    pub capabilities: Vec<BackendCapability>,
    /// Native tools that remain callable through the backend.
    pub native_tools_available: BTreeSet<String>,
    /// Premise: exact project evidence remains expandable.
    pub evidence_expandable: bool,
    /// Premise: subjective decisions remain with the model/user.
    pub subjective_decisions_with_model_user: bool,
}

/// Thm 8.1 certification: no mandatory gate removes a baseline construction
/// strategy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyPreservationCertification {
    /// Number of audited suggestions/capabilities/plans.
    pub audited_capabilities: usize,
}

/// Verifies Thm 8.1: every suggestion/capability/plan is optional, required
/// native tools remain callable, exact project evidence remains expandable,
/// and subjective decisions stay with the model/user.
///
/// A single mandatory gate is a loud refusal: the no-degradation envelope is
/// lost and the claim must be removed, not weakened.
pub fn check_greenfield_strategy_preservation(
    input: &GreenfieldStrategyInput,
) -> Result<StrategyPreservationCertification, TheoremViolation> {
    if !input.evidence_expandable {
        return Err(TheoremViolation::EvidenceNotExpandable);
    }
    if !input.subjective_decisions_with_model_user {
        return Err(TheoremViolation::SubjectiveDecisionAutoResolved);
    }
    for capability in &input.capabilities {
        if capability.id.is_empty() {
            return Err(TheoremViolation::EmptyCapabilityId);
        }
        if !capability.optional {
            return Err(TheoremViolation::MandatoryGate {
                id: capability.id.clone(),
            });
        }
        if let Some(tool) = &capability.requires_native_tool
            && !input.native_tools_available.contains(tool)
        {
            return Err(TheoremViolation::NativeToolUnavailable {
                id: capability.id.clone(),
                tool: tool.clone(),
            });
        }
    }
    Ok(StrategyPreservationCertification {
        audited_capabilities: input.capabilities.len(),
    })
}

// ---------------------------------------------------------------------------
// Shared typed violation
// ---------------------------------------------------------------------------

/// A theorem premise that did not hold, or a claimed bound that was violated.
/// Every variant is a refusal: the checker never returns a weaker
/// certificate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TheoremViolation {
    // Thm 5.1.
    /// A claim carried no identifier.
    EmptyClaimId,
    /// A rooted claim carried an empty artifact root.
    EmptyArtifactRoot { id: String },
    /// A labeled-inference claim carried an empty label.
    EmptyInferenceLabel { id: String },
    /// A rooted claim's exact artifact root is absent from the view.
    UnrootedClaim { id: String, artifact_root: String },
    /// Omitted evidence has no expansion handle.
    OmittedEvidenceNotExpandable { id: String },
    /// An expansion handle does not resolve in the expansion table.
    UnresolvableExpansion { id: String, handle: String },
    /// A rooted claim expands to a different artifact than its bound one.
    ExpansionMismatch {
        id: String,
        handle: String,
        bound: String,
        artifact_root: String,
    },
    /// An expansion resolves to an artifact absent from the view.
    UnrootedExpansion { id: String, artifact_root: String },
    // Thm 6.1.
    /// The interaction carried no continuation handles.
    NoContinuationHandles,
    /// A handle carried no identifier.
    EmptyHandleId,
    /// The private-composability premise is unmet.
    NotPrivatelyComposable,
    /// The verifiability premise is unmet.
    NotVerifiable,
    /// `d + 1` overflowed the integer width.
    DecisionCountOverflow { id: String },
    /// The observed call count differs from `d + 1`.
    CallCountMismatch { id: String, expected: u64, actual: u64 },
    // Thm 7.1.
    /// No obligations were declared, so nothing can be certified.
    NoDeclaredObligations,
    /// `V != B`: obligations with grade `Unknown` stay Unknown.
    IncompleteCoverage { uncovered: Vec<ProtectedDimensionV1> },
    /// A required obligation is only `Observed`, not Proved/BoundedComplete.
    WeakRequiredObligation { dimension: ProtectedDimensionV1 },
    /// The verifier-soundness premise is unmet.
    UnsoundVerifier,
    /// The source-baseline premise is unmet.
    BaselineUnavailable,
    // Thm 8.1.
    /// A suggestion/capability/plan carried no identifier.
    EmptyCapabilityId,
    /// Exact project evidence is not expandable.
    EvidenceNotExpandable,
    /// A subjective decision was auto-resolved instead of staying with the
    /// model/user.
    SubjectiveDecisionAutoResolved,
    /// A mandatory backend gate was found over the capability set.
    MandatoryGate { id: String },
    /// A capability needs a native tool that is not callable.
    NativeToolUnavailable { id: String, tool: String },
}

impl fmt::Display for TheoremViolation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyClaimId => formatter.write_str("a factual claim carried no id"),
            Self::EmptyArtifactRoot { id } => {
                write!(formatter, "claim {id} carried an empty artifact root")
            }
            Self::EmptyInferenceLabel { id } => {
                write!(formatter, "claim {id} carried an empty inference label")
            }
            Self::UnrootedClaim { id, artifact_root } => write!(
                formatter,
                "claim {id} references artifact {artifact_root} absent from the view"
            ),
            Self::OmittedEvidenceNotExpandable { id } => write!(
                formatter,
                "claim {id} omits evidence but carries no expansion handle"
            ),
            Self::UnresolvableExpansion { id, handle } => {
                write!(formatter, "claim {id} expansion handle {handle} does not resolve")
            }
            Self::ExpansionMismatch {
                id,
                handle,
                bound,
                artifact_root,
            } => write!(
                formatter,
                "claim {id} handle {handle} expands to {bound}, not its bound artifact {artifact_root}"
            ),
            Self::UnrootedExpansion { id, artifact_root } => write!(
                formatter,
                "claim {id} expansion resolves to artifact {artifact_root} absent from the view"
            ),
            Self::NoContinuationHandles => {
                formatter.write_str("the interaction carried no continuation handles")
            }
            Self::EmptyHandleId => formatter.write_str("a continuation handle carried no id"),
            Self::NotPrivatelyComposable => formatter.write_str(
                "other operations are not privately composable: the d+1 bound does not apply",
            ),
            Self::NotVerifiable => formatter.write_str(
                "other operations are not verifiable: the d+1 bound does not apply",
            ),
            Self::DecisionCountOverflow { id } => {
                write!(formatter, "handle {id}: d + 1 overflowed the integer width")
            }
            Self::CallCountMismatch { id, expected, actual } => write!(
                formatter,
                "handle {id}: expected exactly {expected} Zero Execute calls, observed {actual}"
            ),
            Self::NoDeclaredObligations => {
                formatter.write_str("no obligations were declared, nothing to certify")
            }
            Self::IncompleteCoverage { uncovered } => write!(
                formatter,
                "V != B: uncovered obligations stay Unknown: {uncovered:?}"
            ),
            Self::WeakRequiredObligation { dimension } => write!(
                formatter,
                "required obligation {dimension:?} is only Observed, not Proved/BoundedComplete"
            ),
            Self::UnsoundVerifier => {
                formatter.write_str("the verifier is not sound: no equivalence claim")
            }
            Self::BaselineUnavailable => formatter.write_str(
                "the source baseline is unavailable: no equivalence claim for uncovered cases",
            ),
            Self::EmptyCapabilityId => {
                formatter.write_str("a suggestion/capability/plan carried no id")
            }
            Self::EvidenceNotExpandable => {
                formatter.write_str("exact project evidence is not expandable")
            }
            Self::SubjectiveDecisionAutoResolved => formatter.write_str(
                "a subjective decision was auto-resolved instead of staying with the model/user",
            ),
            Self::MandatoryGate { id } => {
                write!(formatter, "mandatory backend gate on capability {id}")
            }
            Self::NativeToolUnavailable { id, tool } => write!(
                formatter,
                "capability {id} requires native tool {tool} which is not callable"
            ),
        }
    }
}

impl Error for TheoremViolation {}

#[cfg(test)]
#[path = "../../../tests/rust/zero-gauge/unit/theorems.rs"]
mod tests;
