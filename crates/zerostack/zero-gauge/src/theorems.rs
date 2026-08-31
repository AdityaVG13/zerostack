//! Executable theorem-bound checkers. Each accepts measured, typed premises
//! and verifies that the claimed bound holds.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use zero_abi::{CoverageGrade, ProtectedDimension, ProtectedScopeObligations};

// Explanation Evidence Preservation

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

/// Certifies that the compact view preserves the baseline factual
/// strategy set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidencePreservationCertification {
    /// Number of certified factual claims.
    pub certified_claims: usize,
    /// Number of omitted-evidence claims proven expandable to a bound
    /// artifact.
    pub expandable_omissions: usize,
}

/// Verifies explanation-evidence premises over a compact view.
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

/// Verifies that a claim's omitted evidence is expandable before a protected factual decision.
fn verify_expandable_omission(
    view: &CompactExplanationView,
    claim: &FactualClaim,
    bound_artifact: Option<&String>,
) -> Result<(), TheoremViolation> {
    let handle = claim.expansion_handle.as_ref().ok_or_else(|| {
        TheoremViolation::OmittedEvidenceNotExpandable {
            id: claim.id.clone(),
        }
    })?;
    let resolved =
        view.expansions
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

// Decision-Delimited Refactor

/// One continuation handle of a prepared model-visible interaction. Runtime
/// continuation handles carry no call count today, so both the declared unresolved
/// decision count and the observed call count are measured/typed inputs to the checker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionDelimitedHandle {
    /// Handle identifier.
    pub id: String,
    /// `d`: unresolved adaptive semantic decisions declared for the
    /// interaction.
    pub declared_unresolved_decisions: u64,
    /// Observed ZeroKernel calls in the prepared model-visible interaction.
    pub observed_kernel_calls: u64,
}

/// Decision-delimited refactor input: continuation handles and two premises on
/// the remaining operations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecisionDelimitedRefactorInput {
    /// The continuation handles of the refactor interaction.
    pub handles: Vec<DecisionDelimitedHandle>,
    /// Premise: every other operation is privately composable.
    pub other_operations_privately_composable: bool,
    /// Premise: every other operation is verifiable.
    pub other_operations_verifiable: bool,
}

/// Certifies that every interaction required exactly `d + 1` calls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CallCountCertification {
    /// Number of certified continuation interactions.
    pub certified_interactions: usize,
    /// Per-handle expected call count `d + 1`.
    pub expected_calls: Vec<(String, u64)>,
}

/// Verifies that each handle's call count equals `declared_unresolved_decisions + 1`.
/// Refuses unmet premises, missing handles, empty identifiers, and count overflow.
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
        if handle.observed_kernel_calls != expected {
            return Err(TheoremViolation::CallCountMismatch {
                id: handle.id.clone(),
                expected,
                actual: handle.observed_kernel_calls,
            });
        }
        expected_calls.push((handle.id.clone(), expected));
    }
    Ok(CallCountCertification {
        certified_interactions: input.handles.len(),
        expected_calls,
    })
}

// Port Nonregression under Complete Observational Coverage

/// Port-nonregression input. `B` is the declared source-behavior obligation set; the
/// verified subset `V` is read from each obligation's coverage grade. The checker
/// certifies protected equivalence only when `V == B` and every premise holds.
pub struct PortNonregressionInput<'a> {
    /// The declared obligation set `B` with per-obligation coverage grades.
    pub obligations: &'a ProtectedScopeObligations,
    /// Premise: the verifier is sound.
    pub verifier_sound: bool,
    /// Premise: the source baseline remains available for uncovered
    /// environment cases.
    pub source_baseline_available: bool,
}

/// Certifies that the target is protected-equivalent within the
/// declared observational contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtectedEquivalenceCertification {
    /// Declared obligation count `|B|`.
    pub declared_obligations: usize,
    /// Verified obligation count `|V|` (equals `|B|` when certified).
    pub verified_obligations: usize,
    /// Every covered dimension, in declaration order.
    pub dimensions: Vec<ProtectedDimension>,
}

/// Verifies port-nonregression premises over `ProtectedScopeObligations`.
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
        .find(|obligation| obligation.required && obligation.grade == CoverageGrade::Observed)
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

// Greenfield Strategy Preservation

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
    /// Capability kind.
    pub kind: CapabilityKind,
    /// Whether the surface is optional (no mandatory backend gate).
    pub optional: bool,
    /// A native tool the surface needs, if any.
    pub requires_native_tool: Option<String>,
}

/// Greenfield-strategy input: the optional capability set and three
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

/// Certifies that no mandatory gate removes a baseline construction
/// strategy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyPreservationCertification {
    /// Number of audited suggestions/capabilities/plans.
    pub audited_capabilities: usize,
}

/// Verifies that suggestions, capabilities, and plans remain optional while native tools remain
/// callable, exact project evidence remains expandable, and subjective decisions stay with the
/// model/user.
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

// Shared typed violation

/// A theorem premise that did not hold, or a claimed bound that was violated.
/// Every variant is a refusal: the checker never returns a weaker
/// certificate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TheoremViolation {
    // Explanation evidence preservation.
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
    // Decision-delimited refactor.
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
    CallCountMismatch {
        id: String,
        expected: u64,
        actual: u64,
    },
    // Port nonregression.
    /// No obligations were declared, so nothing can be certified.
    NoDeclaredObligations,
    /// `V != B`: obligations with grade `Unknown` stay Unknown.
    IncompleteCoverage { uncovered: Vec<ProtectedDimension> },
    /// A required obligation is only `Observed`, not Proved/BoundedComplete.
    WeakRequiredObligation { dimension: ProtectedDimension },
    /// The verifier-soundness premise is unmet.
    UnsoundVerifier,
    /// The source-baseline premise is unmet.
    BaselineUnavailable,
    // Greenfield strategy preservation.
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
                write!(
                    formatter,
                    "claim {id} expansion handle {handle} does not resolve"
                )
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
            Self::NotVerifiable => formatter
                .write_str("other operations are not verifiable: the d+1 bound does not apply"),
            Self::DecisionCountOverflow { id } => {
                write!(formatter, "handle {id}: d + 1 overflowed the integer width")
            }
            Self::CallCountMismatch {
                id,
                expected,
                actual,
            } => write!(
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
