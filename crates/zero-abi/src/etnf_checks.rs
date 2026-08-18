//! V7 shadow checkers over the ETNF certificate ABI (bead `zerostack-3cdn`,
//! program `zerostack-vcqk`).
//!
//! Three total, finite, versioned checkers consume untrusted bytes and emit a
//! [`V7ShadowReport`] over the trivalent certificate ABI from [`crate::etnf`]:
//!
//! | Checker | Theorem | Claim checked |
//! |---|---|---|
//! | [`check_certificate_chain`] | W7-T03 Certificate Composition | adjacent certificate-chain binding |
//! | [`check_causal_closure`] | W7-T11 Executable Causal Closure | demanded output is in the declared closure |
//! | [`check_savings_provenance`] | W7-T13 Savings Provenance | baseline transcript segments map 1:1 to a savings category |
//!
//! # Totality and finiteness
//!
//! Every checker is total on every byte string: no `unwrap`, no `expect`,
//! no unchecked indexing, no slicing, no recursion, and no loop over input
//! that has not first been bounded. All
//! inputs are capped by the `VCQK_MAX_*` bounds; a document that exceeds a
//! bound yields `Unknown` ("input_exceeds_checker_bounds") because the finite
//! checker version cannot complete it -- an oversized declaration is missing
//! evidence, never a falsification. Parse failures are likewise `Unknown`
//! ("unparseable_input" / "unparseable_link"): unreadable bytes are missing
//! evidence. Only *parsed* declarations that positively contradict the claim
//! yield `Unsafe`. Empty inputs are `Unknown` (fail-closed vacuity law:
//! no premises, no `Safe`).
//!
//! # Authority law
//!
//! Checkers issue a [`crate::etnf::ShadowCertificate`] only for a `Safe`
//! verdict, exactly as the ABI requires; `Unsafe`/`Unknown` cannot serialize
//! authority. The returned report is shadow evidence only: it is not accepted
//! by any write/permit gate, never alters runtime routing or permits, and
//! always names the frozen raw baseline as the explicit fallback, so the
//! baseline remains available. The resource ledger records every byte read,
//! item checked, and check run; an `Unknown` verdict cannot close the ledger
//! (`complete: false`).
//!
//! # Kill metrics
//!
//! [`KillMetrics`] accumulates the named kills: false authority (a `Safe`
//! certificate root later refuted), non-converging counterexamples (the same
//! counterexample root re-issued beyond [`VCQK_KILL_NONCONVERGENCE_MAX_ISSUES`]
//! times), and savings overhead (consumed units exceed the claimed saving).
//! Learning/refinement has **no publish authority**: there is no API to raise
//! [`KillMetrics::learning_publications`] (always zero), no constructor
//! accepts refinement output as a premise, and refinement evidence is
//! observable only.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::digest::sha256_hex;
use crate::etnf::{
    CheckerIdentity, ETNF_MAX_ID_BYTES, EtnfError, EvidenceItem, ExplicitFallback,
    FallbackKind, FiniteWitness, Falsifier, ProposedAuthorityTransition,
    ProposedTransitionKind, ResourceLedger, RootedEvidence, V7ShadowReport,
};
use crate::verdict::SafetyVerdict;

// ---------------------------------------------------------------------------
// Identity, contracts, and finiteness bounds
// ---------------------------------------------------------------------------

/// Checker identity of the certificate-chain checker (W7-T03).
pub const VCQK_CHECKER_CHAIN_ID: &str = "w7/chain_v1";
/// Checker identity of the causal-closure checker (W7-T11).
pub const VCQK_CHECKER_CAUSAL_ID: &str = "w7/causal_v1";
/// Checker identity of the savings-provenance checker (W7-T13).
pub const VCQK_CHECKER_SAVINGS_ID: &str = "w7/savings_v1";
/// Version of every checker in this module. The version is bound into each
/// certificate root, so a checker upgrade invalidates prior shadow
/// certificates.
pub const VCQK_CHECKER_VERSION: &str = "1.0.0";

/// Shadow scope of the certificate-chain check.
pub const VCQK_SCOPE_CHAIN: &str = "scope:v7/certificate-chain";
/// Shadow scope of the causal-closure check.
pub const VCQK_SCOPE_CAUSAL: &str = "scope:v7/causal-closure";
/// Shadow scope of the savings-provenance check.
pub const VCQK_SCOPE_SAVINGS: &str = "scope:v7/savings-provenance";

/// Shadow contract of the certificate-chain check.
pub const VCQK_CONTRACT_CHAIN: &str = "zero.contract/v7-chain";
/// Shadow contract of the causal-closure check.
pub const VCQK_CONTRACT_CAUSAL: &str = "zero.contract/v7-causal-closure";
/// Shadow contract of the savings-provenance check.
pub const VCQK_CONTRACT_SAVINGS: &str = "zero.contract/v7-savings-provenance";

/// Maximum certificate-chain links per check (one evidence item per link,
/// bounded by `ETNF_MAX_EVIDENCE_ITEMS`).
pub const VCQK_MAX_CHAIN_LINKS: usize = 64;
/// Maximum demanded outputs per causal-closure check (one evidence item per
/// demanded output, bounded by `ETNF_MAX_EVIDENCE_ITEMS`).
pub const VCQK_MAX_DEMANDED_OUTPUTS: usize = 128;
/// Maximum declared closure nodes per causal-closure check.
pub const VCQK_MAX_CLOSURE_NODES: usize = 4096;
/// Maximum declared dependency edges per causal-closure check.
pub const VCQK_MAX_CLOSURE_EDGES: usize = 16384;
/// Maximum baseline transcript segments per savings check (one evidence item
/// per segment, bounded by `ETNF_MAX_EVIDENCE_ITEMS`).
pub const VCQK_MAX_BASELINE_SEGMENTS: usize = 128;
/// Maximum savings-map entries per savings check.
pub const VCQK_MAX_SAVINGS_ENTRIES: usize = 256;
/// Maximum bytes for any checker-input identifier.
pub const VCQK_MAX_IDENTIFIER_BYTES: usize = ETNF_MAX_ID_BYTES;

/// A counterexample root re-issued more than this many times is a
/// non-converging counterexample kill (W7-T10).
pub const VCQK_KILL_NONCONVERGENCE_MAX_ISSUES: u64 = 3;
/// Maximum certificate roots the kill tracker remembers as `Safe`.
pub const VCQK_KILL_MAX_TRACKED_ROOTS: usize = 4096;
/// Maximum distinct counterexample roots the kill tracker remembers.
pub const VCQK_KILL_MAX_TRACKED_COUNTEREXAMPLES: usize = 4096;
/// Learning/refinement has no publish authority: refinement evidence is
/// observable, never a certificate source.
pub const VCQK_LEARNING_REFINEMENT_PUBLISH_AUTHORITY: bool = false;

// ---------------------------------------------------------------------------
// Input documents (bounded, deny_unknown_fields)
// ---------------------------------------------------------------------------

/// One declared causal-closure node. `kind` is observational only; it never
/// influences the verdict.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureNode {
    pub id: String,
    pub kind: String,
}

/// One declared dependency edge: `from` is causally needed by `to`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClosureEdge {
    pub from: String,
    pub to: String,
}

/// Bounded causal-closure declaration (W7-T11 checker input).
///
/// Demanded outputs are opaque identifiers; the checker derives digest
/// evidence from them, so no hex requirement is imposed on the wire.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalClosureDocument {
    pub demanded: Vec<String>,
    pub nodes: Vec<ClosureNode>,
    pub edges: Vec<ClosureEdge>,
}

/// One baseline transcript segment. `kind` is observational only.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineSegment {
    pub id: String,
    pub kind: String,
}

/// One savings-map entry: a baseline segment and its declared category.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SavingsEntry {
    pub segment: String,
    pub category: String,
}

/// Bounded savings-provenance declaration (W7-T13 checker input).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SavingsProvenanceDocument {
    pub baseline: Vec<BaselineSegment>,
    pub savings: Vec<SavingsEntry>,
}

/// The six trace-auditable savings classifications (W7-T13).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SavingsCategory {
    /// The cached baseline result was reused byte-for-byte.
    Reused,
    /// Work ran privately until a preauthorized policy escaped (family 1).
    PrivateExecution,
    /// The segment was proved irrelevant to the demanded outputs.
    ProvedIrrelevant,
    /// A verifier collapsed several segments into one decision (family 2).
    VerifierCollapsed,
    /// The observation was preauthorized by policy (family 1, W7-T04).
    PolicyPreauthorized,
    /// No saving was claimed; the baseline segment was preserved.
    BaselinePreserved,
}

impl SavingsCategory {
    /// All six categories, in stable declaration order.
    pub const ALL: [SavingsCategory; 6] = [
        SavingsCategory::Reused,
        SavingsCategory::PrivateExecution,
        SavingsCategory::ProvedIrrelevant,
        SavingsCategory::VerifierCollapsed,
        SavingsCategory::PolicyPreauthorized,
        SavingsCategory::BaselinePreserved,
    ];

    /// Total wire-name classifier: `None` for any undeclared category, which
    /// the checker treats as a positive falsification (W7-T13-f1).
    pub fn classify(value: &str) -> Option<SavingsCategory> {
        match value {
            "reused" => Some(SavingsCategory::Reused),
            "private_execution" => Some(SavingsCategory::PrivateExecution),
            "proved_irrelevant" => Some(SavingsCategory::ProvedIrrelevant),
            "verifier_collapsed" => Some(SavingsCategory::VerifierCollapsed),
            "policy_preauthorized" => Some(SavingsCategory::PolicyPreauthorized),
            "baseline_preserved" => Some(SavingsCategory::BaselinePreserved),
            _ => None,
        }
    }

    /// Stable wire name of this category.
    pub fn as_str(self) -> &'static str {
        match self {
            SavingsCategory::Reused => "reused",
            SavingsCategory::PrivateExecution => "private_execution",
            SavingsCategory::ProvedIrrelevant => "proved_irrelevant",
            SavingsCategory::VerifierCollapsed => "verifier_collapsed",
            SavingsCategory::PolicyPreauthorized => "policy_preauthorized",
            SavingsCategory::BaselinePreserved => "baseline_preserved",
        }
    }
}

// ---------------------------------------------------------------------------
// Kill metrics
// ---------------------------------------------------------------------------

/// Accumulated kill metrics for the V7 shadow checkers.
///
/// Kills are the sharp conditions that would end a deployable theorem:
/// false authority (a `Safe` certificate whose root is later refuted),
/// non-converging counterexamples (W7-T10 refinement that never converges),
/// and savings overhead (the check consumed more than the claimed saving).
/// All internal tracking is bounded; beyond capacity, new observations are
/// counted as untracked instead of growing without limit. Every method is
/// total and deterministic.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KillMetrics {
    safe_reports: u64,
    unsafe_reports: u64,
    unknown_reports: u64,
    false_authority: u64,
    non_converging_counterexamples: u64,
    counterexample_reissues: u64,
    savings_overhead: u64,
    refutations_total: u64,
    roots_overflow: u64,
    counterexample_untracked: u64,
    certificate_roots: BTreeSet<String>,
    counterexample_issues: BTreeMap<String, u64>,
    counted_nonconverging: BTreeSet<String>,
}

/// Savings-overhead kill predicate: consumed units exceed the claimed
/// saving. Pure and total.
pub fn savings_overhead_killed(claimed_saved_units: u64, consumed_units: u64) -> bool {
    consumed_units > claimed_saved_units
}

impl KillMetrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observe one shadow report. `Safe` reports have their certificate root
    /// tracked so a later refutation can be attributed as false authority.
    pub fn observe_report(&mut self, report: &V7ShadowReport) {
        match &report.verdict {
            SafetyVerdict::Safe => {
                self.safe_reports += 1;
                if let Some(certificate) = &report.certificate {
                    if self.certificate_roots.len() < VCQK_KILL_MAX_TRACKED_ROOTS {
                        self.certificate_roots.insert(certificate.root.clone());
                    } else if !self.certificate_roots.contains(&certificate.root) {
                        self.roots_overflow += 1;
                    }
                }
            }
            SafetyVerdict::Unsafe { .. } => self.unsafe_reports += 1,
            SafetyVerdict::Unknown { .. } => self.unknown_reports += 1,
        }
    }

    /// Record a later refutation of a certificate root. If the root was
    /// previously observed as `Safe`, this is a false-authority kill.
    pub fn observe_refutation(&mut self, certificate_root: &str) {
        self.refutations_total += 1;
        if self.certificate_roots.remove(certificate_root) {
            self.false_authority += 1;
        }
    }

    /// Record one issue of a counterexample root (W7-T10 refinement loop).
    /// The same root issued more than [`VCQK_KILL_NONCONVERGENCE_MAX_ISSUES`]
    /// times is counted once as a non-converging counterexample; further
    /// issues are reissues, and reissue events are counted separately.
    pub fn observe_counterexample(&mut self, counterexample_root: &str) {
        let tracked = !self.counted_nonconverging.contains(counterexample_root)
            && self.counterexample_issues.len() < VCQK_KILL_MAX_TRACKED_COUNTEREXAMPLES;
        if !tracked {
            self.counterexample_untracked += 1;
            return;
        }
        let issues = self
            .counterexample_issues
            .entry(counterexample_root.to_string())
            .or_insert(0);
        *issues += 1;
        if *issues > VCQK_KILL_NONCONVERGENCE_MAX_ISSUES {
            self.non_converging_counterexamples += 1;
            self.counted_nonconverging.insert(counterexample_root.to_string());
        } else if *issues > 1 {
            self.counterexample_reissues += 1;
        }
    }

    /// Observe one savings accounting: consumed units versus claimed saved
    /// units. When consumption exceeds the claim, the savings-overhead kill
    /// fires (policy costs more than it saves).
    pub fn observe_savings(&mut self, claimed_saved_units: u64, consumed_units: u64) {
        if savings_overhead_killed(claimed_saved_units, consumed_units) {
            self.savings_overhead += 1;
        }
    }

    /// Learning/refinement has no publish authority: always zero, and no API
    /// exists to raise it. Refinement evidence is observable only.
    pub fn learning_publications(&self) -> u64 {
        0
    }

    /// Constant proof that learning/refinement cannot publish authority.
    pub const fn learning_has_no_publish_authority() -> bool {
        VCQK_LEARNING_REFINEMENT_PUBLISH_AUTHORITY
    }

    pub fn safe_reports(&self) -> u64 {
        self.safe_reports
    }

    pub fn unsafe_reports(&self) -> u64 {
        self.unsafe_reports
    }

    pub fn unknown_reports(&self) -> u64 {
        self.unknown_reports
    }

    pub fn false_authority(&self) -> u64 {
        self.false_authority
    }

    pub fn non_converging_counterexamples(&self) -> u64 {
        self.non_converging_counterexamples
    }

    pub fn counterexample_reissues(&self) -> u64 {
        self.counterexample_reissues
    }

    pub fn savings_overhead(&self) -> u64 {
        self.savings_overhead
    }

    pub fn refutations_total(&self) -> u64 {
        self.refutations_total
    }

    /// Certificate roots currently remembered as `Safe`.
    pub fn tracked_certificate_roots(&self) -> usize {
        self.certificate_roots.len()
    }

    pub fn roots_overflow(&self) -> u64 {
        self.roots_overflow
    }

    pub fn counterexample_untracked(&self) -> u64 {
        self.counterexample_untracked
    }
}

// ---------------------------------------------------------------------------
// Shared helpers (all total; constants cannot fail construction)
// ---------------------------------------------------------------------------

fn checker(id: &'static str) -> Result<CheckerIdentity, EtnfError> {
    CheckerIdentity::new(id, VCQK_CHECKER_VERSION)
}

fn fallback() -> Result<ExplicitFallback, EtnfError> {
    ExplicitFallback::new(FallbackKind::FrozenRawBaseline, "run the frozen raw baseline")
}

/// Equal-or-descendant path chaining used for adjacent scopes and contracts
/// (W7-T03): `parent` chains to `child` when `child == parent` or `child` is
/// `parent` followed by `/`. Total on all strings; the boundary byte is read
/// through `get`, never by indexing.
fn chains_from(parent: &str, child: &str) -> bool {
    child == parent
        || (child.len() > parent.len()
            && child.as_bytes().starts_with(parent.as_bytes())
            && child.as_bytes().get(parent.len()) == Some(&b'/'))
}

fn identifier_within_bounds(value: &str) -> bool {
    value.len() <= VCQK_MAX_IDENTIFIER_BYTES
}

fn chain_falsifiers() -> Result<Vec<Falsifier>, EtnfError> {
    Ok(vec![
        Falsifier::new(
            "W7-T03-f1",
            "adjacent root binding: successor evidence anchor must equal the predecessor certificate root",
        )?,
        Falsifier::new(
            "W7-T03-f2",
            "adjacent scope chaining: successor scope must equal the predecessor scope or extend it as a path descendant",
        )?,
        Falsifier::new(
            "W7-T03-f3",
            "adjacent contract chaining: successor contract must equal the predecessor contract or extend it as a path descendant",
        )?,
        Falsifier::new(
            "W7-T03-f4",
            "checker identity must be identical along the chain: an upgrade invalidates prior certificates",
        )?,
        Falsifier::new(
            "W7-T03-f5",
            "every chain link must be a Safe report carrying a live certificate",
        )?,
    ])
}

fn causal_falsifiers() -> Result<Vec<Falsifier>, EtnfError> {
    Ok(vec![
        Falsifier::new(
            "W7-T11-f1",
            "a demanded output is absent from the declared closure",
        )?,
        Falsifier::new(
            "W7-T11-f2",
            "a dependency edge references a node the declared closure does not contain",
        )?,
    ])
}

fn savings_falsifiers() -> Result<Vec<Falsifier>, EtnfError> {
    Ok(vec![
        Falsifier::new(
            "W7-T13-f1",
            "a savings entry declares a category outside the six trace-auditable classifications",
        )?,
        Falsifier::new(
            "W7-T13-f2",
            "a baseline segment is mapped by more than one savings entry",
        )?,
        Falsifier::new(
            "W7-T13-f3",
            "a savings entry references a segment that is not in the baseline",
        )?,
        Falsifier::new(
            "W7-T13-f4",
            "the baseline itself repeats a segment identifier",
        )?,
    ])
}

#[allow(clippy::too_many_arguments)]
fn build_report(
    verdict: SafetyVerdict,
    checker: CheckerIdentity,
    scope: &'static str,
    contract: &'static str,
    evidence: RootedEvidence,
    witness: FiniteWitness,
    proposal: Option<ProposedAuthorityTransition>,
    falsifiers: Vec<Falsifier>,
    ledger: ResourceLedger,
) -> Result<V7ShadowReport, EtnfError> {
    V7ShadowReport::new(
        verdict,
        checker,
        scope,
        contract,
        evidence,
        witness,
        proposal,
        fallback()?,
        falsifiers,
        ledger,
    )
}

/// Fail-closed `Unknown` report for missing evidence. `complete: false`:
/// an `Unknown` run cannot close its resource ledger.
#[allow(clippy::too_many_arguments)]
fn unknown_report(
    reason: &str,
    checker: CheckerIdentity,
    scope: &'static str,
    contract: &'static str,
    anchor: String,
    items: Vec<EvidenceItem>,
    facts: Vec<String>,
    falsifiers: Vec<Falsifier>,
    bytes_read: u64,
    items_checked: u64,
) -> Result<V7ShadowReport, EtnfError> {
    build_report(
        SafetyVerdict::Unknown { reasons: vec![reason.to_string()] },
        checker,
        scope,
        contract,
        RootedEvidence::new(anchor, items)?,
        FiniteWitness::new(facts)?,
        None,
        falsifiers,
        ResourceLedger::new(bytes_read, items_checked, 1, false),
    )
}

// ---------------------------------------------------------------------------
// Checker 1: W7-T03 adjacent certificate-chain binding
// ---------------------------------------------------------------------------

/// W7-T03 Certificate Composition (shadow): adjacent certificate-chain
/// binding.
///
/// Input bytes are a JSON array of canonical [`V7ShadowReport`] documents.
/// Each element that parses as a `Safe` report with a live certificate is a
/// chain link; its certificate root was already recomputed over evidence
/// root, scope, contract, checker, and resource ledger by the ABI. The
/// checker then binds every adjacent pair:
///
/// 1. **Roots**: the successor evidence anchor equals the predecessor
///    certificate root.
/// 2. **Scopes**: the successor scope chains from the predecessor scope
///    (equal or path-descendant).
/// 3. **Contracts**: the successor contract chains from the predecessor
///    contract (equal or path-descendant).
/// 4. **Checker**: identity and version are identical along the chain.
///
/// A parsed element without a certificate positively falsifies the chain
/// (W7-T03-f5). An element that fails to parse is missing evidence for that
/// link (`Unknown`, "unparseable_link"). An empty chain, an unparseable
/// document, or an oversized chain is `Unknown`; one valid link is a
/// well-formed single-link chain (`Safe`).
pub fn check_certificate_chain(bytes: &[u8]) -> Result<V7ShadowReport, EtnfError> {
    let bytes_read = bytes.len() as u64;
    let falsifiers = chain_falsifiers()?;
    let anchor = sha256_hex(bytes);
    let checker = checker(VCQK_CHECKER_CHAIN_ID)?;

    let elements: Vec<Value> = match serde_json::from_slice(bytes) {
        Ok(Value::Array(elements)) => elements,
        _ => {
            return unknown_report(
                "unparseable_input",
                checker,
                VCQK_SCOPE_CHAIN,
                VCQK_CONTRACT_CHAIN,
                anchor,
                Vec::new(),
                vec!["input is not a parseable certificate-chain document".to_string()],
                falsifiers,
                bytes_read,
                0,
            );
        }
    };

    let element_count = elements.len() as u64;
    if elements.len() > VCQK_MAX_CHAIN_LINKS {
        return unknown_report(
            "input_exceeds_checker_bounds",
            checker,
            VCQK_SCOPE_CHAIN,
            VCQK_CONTRACT_CHAIN,
            anchor,
            Vec::new(),
            vec![format!("certificate chain has {} links, maximum {}", elements.len(), VCQK_MAX_CHAIN_LINKS)],
            falsifiers,
            bytes_read,
            element_count,
        );
    }

    if elements.is_empty() {
        return unknown_report(
            "no_chain_links",
            checker,
            VCQK_SCOPE_CHAIN,
            VCQK_CONTRACT_CHAIN,
            anchor,
            Vec::new(),
            vec!["certificate chain is empty: no adjacent binding premises".to_string()],
            falsifiers,
            bytes_read,
            0,
        );
    }

    let mut verdict = SafetyVerdict::Safe;
    let mut links: Vec<V7ShadowReport> = Vec::new();
    let mut unparseable_count: u64 = 0;
    let mut previous: Option<V7ShadowReport> = None;

    for (index, element) in elements.iter().enumerate() {
        let canonical = match serde_json::to_vec(element) {
            Ok(canonical) => canonical,
            Err(_) => {
                // A Value always serializes; this arm exists only for totality.
                unparseable_count += 1;
                previous = None;
                continue;
            }
        };
        match V7ShadowReport::from_canonical_bytes(&canonical) {
            Ok(link) if link.verdict.grants_authority() => {
                if let Some(predecessor) = &previous {
                    verdict = verdict.meet(chain_pair_verdict(predecessor, &link));
                }
                links.push(link.clone());
                previous = Some(link);
            }
            Ok(_) => {
                // A valid report without a certificate is positive evidence
                // that the chain contains a non-certificate link.
                verdict = verdict.meet(SafetyVerdict::Unsafe {
                    reasons: vec![format!("non_certificate_link:{index}")],
                });
                previous = None;
            }
            Err(_) => {
                unparseable_count += 1;
                previous = None;
            }
        }
    }

    if unparseable_count > 0 {
        verdict = verdict.meet(SafetyVerdict::Unknown {
            reasons: vec!["unparseable_link".to_string()],
        });
    }

    let items = links
        .iter()
        .enumerate()
        .map(|(index, link)| {
            let root = link
                .certificate
                .as_ref()
                .map(|certificate| certificate.root.clone())
                .unwrap_or_else(|| sha256_hex(b"missing-certificate"));
            EvidenceItem::new(format!("link:{index}"), root)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut facts = vec![
        format!("certificate chain: {} links", links.len()),
        format!("unparseable links: {unparseable_count}"),
    ];
    if verdict.grants_authority() {
        facts.push("adjacent root binding holds for every adjacent pair".to_string());
        facts.push("adjacent scope/contract/checker chaining holds".to_string());
    }

    let proposal = match &verdict {
        SafetyVerdict::Safe => {
            let head = links.last().and_then(|link| {
                link.certificate.as_ref().map(|certificate| certificate.root.clone())
            });
            head.and_then(|target| {
                ProposedAuthorityTransition::new(ProposedTransitionKind::ReuseCachedResult, target)
                    .ok()
            })
        }
        _ => None,
    };

    let ledger = ResourceLedger::new(
        bytes_read,
        element_count,
        1,
        !matches!(verdict, SafetyVerdict::Unknown { .. }),
    );

    build_report(
        verdict,
        checker,
        VCQK_SCOPE_CHAIN,
        VCQK_CONTRACT_CHAIN,
        RootedEvidence::new(anchor, items)?,
        FiniteWitness::new(facts)?,
        proposal,
        falsifiers,
        ledger,
    )
}

/// Verdict of one adjacent chain pair. All checks are total; `previous` is
/// always a validated `Safe` link with a live certificate.
fn chain_pair_verdict(previous: &V7ShadowReport, next: &V7ShadowReport) -> SafetyVerdict {
    let mut verdict = SafetyVerdict::Safe;
    let previous_root = previous.certificate.as_ref().map(|cert| cert.root.as_str());
    if !previous_root.is_some_and(|root| next.evidence.anchor == root) {
        verdict = verdict.meet(SafetyVerdict::Unsafe {
            reasons: vec!["adjacent_root_not_bound".to_string()],
        });
    }
    if !chains_from(&previous.scope, &next.scope) {
        verdict = verdict.meet(SafetyVerdict::Unsafe {
            reasons: vec!["scope_does_not_chain".to_string()],
        });
    }
    if !chains_from(&previous.contract, &next.contract) {
        verdict = verdict.meet(SafetyVerdict::Unsafe {
            reasons: vec!["contract_does_not_chain".to_string()],
        });
    }
    if previous.checker != next.checker {
        verdict = verdict.meet(SafetyVerdict::Unsafe {
            reasons: vec!["checker_identity_broken".to_string()],
        });
    }
    verdict
}

// ---------------------------------------------------------------------------
// Checker 2: W7-T11 demanded-output causal closure
// ---------------------------------------------------------------------------

/// W7-T11 Executable Causal Closure (shadow): demanded output must be in the
/// declared closure.
///
/// Input bytes are a canonical [`CausalClosureDocument`]. The checker
/// requires every demanded output to be a declared node and every declared
/// dependency edge to be closed (both endpoints declared). A demanded output
/// absent from the declared closure positively falsifies the claim
/// (W7-T11-f1); an open edge positively falsifies closure well-formedness
/// (W7-T11-f2). Unparseable documents, oversized declarations, oversized
/// identifiers, and empty demand lists are `Unknown` (missing evidence or
/// vacuity; never `Safe`).
pub fn check_causal_closure(bytes: &[u8]) -> Result<V7ShadowReport, EtnfError> {
    let bytes_read = bytes.len() as u64;
    let falsifiers = causal_falsifiers()?;
    let anchor = sha256_hex(bytes);
    let checker = checker(VCQK_CHECKER_CAUSAL_ID)?;

    let document: CausalClosureDocument = match serde_json::from_slice(bytes) {
        Ok(document) => document,
        Err(_) => {
            return unknown_report(
                "unparseable_input",
                checker,
                VCQK_SCOPE_CAUSAL,
                VCQK_CONTRACT_CAUSAL,
                anchor,
                Vec::new(),
                vec!["input is not a parseable causal-closure document".to_string()],
                falsifiers,
                bytes_read,
                0,
            );
        }
    };

    if document.demanded.len() > VCQK_MAX_DEMANDED_OUTPUTS
        || document.nodes.len() > VCQK_MAX_CLOSURE_NODES
        || document.edges.len() > VCQK_MAX_CLOSURE_EDGES
    {
        return unknown_report(
            "input_exceeds_checker_bounds",
            checker,
            VCQK_SCOPE_CAUSAL,
            VCQK_CONTRACT_CAUSAL,
            anchor,
            Vec::new(),
            vec![format!(
                "declaration exceeds checker bounds: {} demanded, {} nodes, {} edges",
                document.demanded.len(),
                document.nodes.len(),
                document.edges.len()
            )],
            falsifiers,
            bytes_read,
            (document.demanded.len() + document.nodes.len() + document.edges.len()) as u64,
        );
    }

    for identifier in document
        .demanded
        .iter()
        .chain(document.nodes.iter().map(|node| &node.id))
        .chain(document.edges.iter().flat_map(|edge| [&edge.from, &edge.to]))
    {
        if !identifier_within_bounds(identifier) {
            return unknown_report(
                "identifier_too_long",
                checker,
                VCQK_SCOPE_CAUSAL,
                VCQK_CONTRACT_CAUSAL,
                anchor,
                Vec::new(),
                vec![format!(
                    "an identifier exceeds {VCQK_MAX_IDENTIFIER_BYTES} bytes; evidence unreadable"
                )],
                falsifiers,
                bytes_read,
                (document.demanded.len() + document.nodes.len() + document.edges.len()) as u64,
            );
        }
    }

    let demanded: BTreeSet<String> = document.demanded.iter().cloned().collect();
    let nodes: BTreeSet<String> = document.nodes.iter().map(|node| node.id.clone()).collect();
    let edges: BTreeSet<(String, String)> = document
        .edges
        .iter()
        .map(|edge| (edge.from.clone(), edge.to.clone()))
        .collect();

    if demanded.is_empty() {
        return unknown_report(
            "no_demanded_outputs",
            checker,
            VCQK_SCOPE_CAUSAL,
            VCQK_CONTRACT_CAUSAL,
            anchor,
            Vec::new(),
            vec!["no demanded outputs declared: vacuous, fail closed".to_string()],
            falsifiers,
            bytes_read,
            (nodes.len() + edges.len()) as u64,
        );
    }

    let mut verdict = SafetyVerdict::Safe;
    for demanded_id in &demanded {
        if !nodes.contains(demanded_id) {
            verdict = verdict.meet(SafetyVerdict::Unsafe {
                reasons: vec!["demanded_output_outside_declared_closure".to_string()],
            });
        }
    }
    for (from, to) in &edges {
        if !nodes.contains(from) || !nodes.contains(to) {
            verdict = verdict.meet(SafetyVerdict::Unsafe {
                reasons: vec!["open_dependency_edge".to_string()],
            });
        }
    }

    let items = demanded
        .iter()
        .enumerate()
        .map(|(index, id)| {
            EvidenceItem::new(format!("demand:{index}"), sha256_hex(id.as_bytes()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut facts = vec![
        format!("demanded outputs: {}", demanded.len()),
        format!("declared closure: {} nodes, {} edges", nodes.len(), edges.len()),
    ];
    if verdict.grants_authority() {
        facts.push("every demanded output is a declared node".to_string());
        facts.push("every dependency edge is closed".to_string());
    }

    let proposal = match &verdict {
        SafetyVerdict::Safe => demanded
            .iter()
            .next()
            .and_then(|first| {
                ProposedAuthorityTransition::new(
                    ProposedTransitionKind::ReuseCachedResult,
                    sha256_hex(first.as_bytes()),
                )
                .ok()
            }),
        _ => None,
    };

    let ledger = ResourceLedger::new(
        bytes_read,
        (demanded.len() + nodes.len() + edges.len()) as u64,
        1,
        !matches!(verdict, SafetyVerdict::Unknown { .. }),
    );

    build_report(
        verdict,
        checker,
        VCQK_SCOPE_CAUSAL,
        VCQK_CONTRACT_CAUSAL,
        RootedEvidence::new(anchor, items)?,
        FiniteWitness::new(facts)?,
        proposal,
        falsifiers,
        ledger,
    )
}

// ---------------------------------------------------------------------------
// Checker 3: W7-T13 savings-provenance completeness
// ---------------------------------------------------------------------------

/// W7-T13 Savings Provenance (shadow): every baseline transcript segment
/// maps 1:1 to one of the six trace-auditable categories.
///
/// Input bytes are a canonical [`SavingsProvenanceDocument`]. A segment
/// with no savings entry is missing evidence: `Unknown`
/// ("segment_unmapped:<id>") and no public saving claim. A segment mapped by
/// more than one entry (W7-T13-f2), an entry for a segment outside the
/// baseline (W7-T13-f3), a repeated baseline segment (W7-T13-f4), or an
/// entry with an undeclared category (W7-T13-f1) is `Unsafe`. Unparseable,
/// oversized, or empty-baseline documents are `Unknown`.
pub fn check_savings_provenance(bytes: &[u8]) -> Result<V7ShadowReport, EtnfError> {
    let bytes_read = bytes.len() as u64;
    let falsifiers = savings_falsifiers()?;
    let anchor = sha256_hex(bytes);
    let checker = checker(VCQK_CHECKER_SAVINGS_ID)?;

    let document: SavingsProvenanceDocument = match serde_json::from_slice(bytes) {
        Ok(document) => document,
        Err(_) => {
            return unknown_report(
                "unparseable_input",
                checker,
                VCQK_SCOPE_SAVINGS,
                VCQK_CONTRACT_SAVINGS,
                anchor,
                Vec::new(),
                vec!["input is not a parseable savings-provenance document".to_string()],
                falsifiers,
                bytes_read,
                0,
            );
        }
    };

    if document.baseline.len() > VCQK_MAX_BASELINE_SEGMENTS
        || document.savings.len() > VCQK_MAX_SAVINGS_ENTRIES
    {
        return unknown_report(
            "input_exceeds_checker_bounds",
            checker,
            VCQK_SCOPE_SAVINGS,
            VCQK_CONTRACT_SAVINGS,
            anchor,
            Vec::new(),
            vec![format!(
                "declaration exceeds checker bounds: {} segments, {} entries",
                document.baseline.len(),
                document.savings.len()
            )],
            falsifiers,
            bytes_read,
            (document.baseline.len() + document.savings.len()) as u64,
        );
    }

    for identifier in document
        .baseline
        .iter()
        .map(|segment| &segment.id)
        .chain(document.savings.iter().map(|entry| &entry.segment))
    {
        if !identifier_within_bounds(identifier) {
            return unknown_report(
                "identifier_too_long",
                checker,
                VCQK_SCOPE_SAVINGS,
                VCQK_CONTRACT_SAVINGS,
                anchor,
                Vec::new(),
                vec![format!(
                    "an identifier exceeds {VCQK_MAX_IDENTIFIER_BYTES} bytes; evidence unreadable"
                )],
                falsifiers,
                bytes_read,
                (document.baseline.len() + document.savings.len()) as u64,
            );
        }
    }

    if document.baseline.is_empty() {
        return unknown_report(
            "no_baseline_segments",
            checker,
            VCQK_SCOPE_SAVINGS,
            VCQK_CONTRACT_SAVINGS,
            anchor,
            Vec::new(),
            vec!["no baseline transcript segments declared".to_string()],
            falsifiers,
            bytes_read,
            0,
        );
    }

    let mut verdict = SafetyVerdict::Safe;
    let mut baseline_ids: BTreeSet<String> = BTreeSet::new();
    let mut entries_by_segment: BTreeMap<String, Vec<&SavingsEntry>> = BTreeMap::new();
    let mut category_counts: BTreeMap<&'static str, u64> = BTreeMap::new();

    for segment in &document.baseline {
        if !baseline_ids.insert(segment.id.clone()) {
            verdict = verdict.meet(SafetyVerdict::Unsafe {
                reasons: vec!["duplicate_baseline_segment".to_string()],
            });
        }
    }

    for entry in &document.savings {
        entries_by_segment.entry(entry.segment.clone()).or_default().push(entry);
        if !baseline_ids.contains(&entry.segment) {
            verdict = verdict.meet(SafetyVerdict::Unsafe {
                reasons: vec!["unknown_segment_in_savings_map".to_string()],
            });
        }
    }

    for segment in &document.baseline {
        match entries_by_segment.get(&segment.id) {
            None => {
                // Missing mapping is missing evidence: no public saving claim.
                verdict = verdict.meet(SafetyVerdict::Unknown {
                    reasons: vec![format!("segment_unmapped:{}", segment.id)],
                });
            }
            Some(entries) if entries.len() > 1 => {
                verdict = verdict.meet(SafetyVerdict::Unsafe {
                    reasons: vec!["duplicate_mapping".to_string()],
                });
            }
            Some(entries) => match entries.first().and_then(|entry| {
                SavingsCategory::classify(&entry.category)
            }) {
                Some(category) => {
                    *category_counts.entry(category.as_str()).or_insert(0) += 1;
                }
                None => {
                    verdict = verdict.meet(SafetyVerdict::Unsafe {
                        reasons: vec!["unsupported_category".to_string()],
                    });
                }
            },
        }
    }

    let items = baseline_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            EvidenceItem::new(format!("segment:{index}"), sha256_hex(id.as_bytes()))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut facts = vec![
        format!("baseline segments: {}", document.baseline.len()),
        format!("savings entries: {}", document.savings.len()),
    ];
    if verdict.grants_authority() {
        facts.push("every baseline segment maps 1:1 to a savings category".to_string());
        for category in SavingsCategory::ALL {
            facts.push(format!(
                "{}: {}",
                category.as_str(),
                category_counts.get(category.as_str()).copied().unwrap_or(0)
            ));
        }
    }

    let proposal = match &verdict {
        SafetyVerdict::Safe => baseline_ids
            .iter()
            .next()
            .and_then(|first| {
                ProposedAuthorityTransition::new(
                    ProposedTransitionKind::ReuseCachedResult,
                    sha256_hex(first.as_bytes()),
                )
                .ok()
            }),
        _ => None,
    };

    let ledger = ResourceLedger::new(
        bytes_read,
        (document.baseline.len() + document.savings.len()) as u64,
        1,
        !matches!(verdict, SafetyVerdict::Unknown { .. }),
    );

    build_report(
        verdict,
        checker,
        VCQK_SCOPE_SAVINGS,
        VCQK_CONTRACT_SAVINGS,
        RootedEvidence::new(anchor, items)?,
        FiniteWitness::new(facts)?,
        proposal,
        falsifiers,
        ledger,
    )
}


