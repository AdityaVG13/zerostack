//! V7 Executable Theorem Normal Form (ETNF) shadow ABI (Wave 7, audit/shadow).
//!
//! Wave 7 (`docs/internal/zerostack-handoff-2026-08-16.md`) turns a deployable
//! theorem into a fail-closed runtime decision. This module owns the
//! ZeroStack data types for that surface, in shadow only:
//!
//! ```text
//! rooted evidence + finite witness
//!   + total Safe | Unsafe | Unknown verdict
//!   + certificate (only from Safe)
//!   + narrow authority transition
//!   + explicit fallback
//!   + falsifiers
//!   + complete resource ledger
//! ```
//!
//! Authority law (W7-T01 Sound Authority, audit `zerostack-vpq2`): only a
//! live certificate may create authority, and a certificate is issued only
//! for a `Safe` verdict. `Unsafe` and `Unknown` cannot serialize a
//! certificate: [`ShadowCertificate::issue`] and [`V7ShadowReport::new`] both
//! fail closed, and [`V7ShadowReport::from_canonical_bytes`] rejects any
//! document that carries a certificate under a non-Safe verdict. Nothing in
//! this module can construct an [`crate::dispatch::ApprovalGrant`],
//! [`crate::dispatch::PermitGrant`], or any other V6 write/permit gate input:
//! shadow reports and certificates grant **no production authority** and are
//! observable, comparable evidence only.
//!
//! Trusted-code rule: only Rust/trusted code constructs reports and
//! certificates and validates canonical bytes
//! ([`V7ShadowReport::to_canonical_bytes`] /
//! [`V7ShadowReport::from_canonical_bytes`]). The model, graph builder, cache
//! planner, and candidate generator remain untrusted proposers: they submit
//! [`EvidenceItem`]s and witness facts; they never construct a verdict or
//! certificate. ProofIR stays optional/research; this module implements no
//! general program equivalence.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::digest::sha256_hex;
use crate::schema::canonical_json;
use crate::verdict::SafetyVerdict;

/// Canonical schema identity of the V7 shadow report document.
pub const ETNF_SCHEMA_ID: &str = "zerostack/v7-shadow-report/1";
/// Maximum bytes for an identifier (checker id/version, scope, contract, ...).
pub const ETNF_MAX_ID_BYTES: usize = 128;
/// Maximum bytes for a free-text field (witness fact, obligation, falsifier description).
pub const ETNF_MAX_STRING_BYTES: usize = 256;
/// Maximum evidence items per rooted evidence set.
pub const ETNF_MAX_EVIDENCE_ITEMS: usize = 128;
/// Maximum witness facts per finite witness.
pub const ETNF_MAX_WITNESS_FACTS: usize = 256;
/// Maximum named falsifiers per report.
pub const ETNF_MAX_FALSIFIERS: usize = 64;
/// Length of lowercase-hex SHA-256 digests used throughout this ABI.
pub const ETNF_HEX_DIGEST_LEN: usize = 64;

/// Fail-closed construction and canonical-bytes validation error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EtnfError {
    /// A required string field was empty.
    Empty { field: &'static str },
    /// A string field exceeded its byte bound.
    TooLong {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    /// A string field contained a control character.
    ControlCharacter { field: &'static str },
    /// A digest field was not 64 lowercase hex characters.
    InvalidHex { field: &'static str },
    /// A finite collection exceeded its item bound.
    TooManyItems {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    /// Certificate issuance was attempted for a non-Safe verdict.
    NotSafe,
    /// The document schema identity was not `ETNF_SCHEMA_ID`.
    InvalidSchema { actual: String },
    /// The `shadow` marker was not `true`.
    ShadowMarkerFalse,
    /// A certificate was present under an `Unsafe`/`Unknown` verdict.
    CertificateWithoutSafe,
    /// A `Safe` verdict carried no certificate.
    MissingCertificateForSafe,
    /// The certificate root did not recompute over its bound fields.
    CertificateRootMismatch,
    /// A certificate field disagreed with the report it is bound to.
    CertificateBindingMismatch { field: &'static str },
    /// Canonical bytes did not re-serialize identically.
    NonCanonicalBytes,
    /// JSON decoding failed.
    BadJson { message: String },
}

impl fmt::Display for EtnfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { field } => write!(formatter, "field `{field}` must be nonempty"),
            Self::TooLong {
                field,
                actual,
                maximum,
            } => {
                write!(
                    formatter,
                    "field `{field}` is {actual} bytes, maximum {maximum}"
                )
            }
            Self::ControlCharacter { field } => {
                write!(
                    formatter,
                    "field `{field}` must be free of control characters"
                )
            }
            Self::InvalidHex { field } => write!(
                formatter,
                "field `{field}` must be {ETNF_HEX_DIGEST_LEN} lowercase hex characters"
            ),
            Self::TooManyItems {
                field,
                actual,
                maximum,
            } => write!(
                formatter,
                "field `{field}` has {actual} items, maximum {maximum}"
            ),
            Self::NotSafe => write!(
                formatter,
                "only a Safe verdict may carry a certificate (W7-T01)"
            ),
            Self::InvalidSchema { actual } => {
                write!(formatter, "unexpected schema identity `{actual}`")
            }
            Self::ShadowMarkerFalse => write!(formatter, "shadow marker must be true"),
            Self::CertificateWithoutSafe => write!(
                formatter,
                "certificate present under a non-Safe verdict (W7-T01)"
            ),
            Self::MissingCertificateForSafe => {
                write!(formatter, "Safe verdict carries no certificate")
            }
            Self::CertificateRootMismatch => {
                write!(formatter, "certificate root does not bind its fields")
            }
            Self::CertificateBindingMismatch { field } => {
                write!(formatter, "certificate `{field}` disagrees with the report")
            }
            Self::NonCanonicalBytes => {
                write!(
                    formatter,
                    "input bytes are not canonical (key order/whitespace)"
                )
            }
            Self::BadJson { message } => write!(formatter, "invalid JSON: {message}"),
        }
    }
}

impl Error for EtnfError {}

fn check_ident(value: &str, field: &'static str) -> Result<(), EtnfError> {
    check_string(value, field, ETNF_MAX_ID_BYTES)
}

fn check_text(value: &str, field: &'static str) -> Result<(), EtnfError> {
    check_string(value, field, ETNF_MAX_STRING_BYTES)
}

fn check_string(value: &str, field: &'static str, maximum: usize) -> Result<(), EtnfError> {
    if value.is_empty() {
        return Err(EtnfError::Empty { field });
    }
    if value.len() > maximum {
        return Err(EtnfError::TooLong {
            field,
            actual: value.len(),
            maximum,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(EtnfError::ControlCharacter { field });
    }
    Ok(())
}

fn check_hex(value: &str, field: &'static str) -> Result<(), EtnfError> {
    let valid = value.len() == ETNF_HEX_DIGEST_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if valid {
        Ok(())
    } else {
        Err(EtnfError::InvalidHex { field })
    }
}

/// Identity and version of the trusted checker that produced a verdict.
///
/// `version` is part of the certificate root binding, so a checker upgrade
/// invalidates every previously issued shadow certificate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckerIdentity {
    pub id: String,
    pub version: String,
}

impl CheckerIdentity {
    pub fn new(id: impl Into<String>, version: impl Into<String>) -> Result<Self, EtnfError> {
        let checker = Self {
            id: id.into(),
            version: version.into(),
        };
        checker.validate()?;
        Ok(checker)
    }

    pub fn validate(&self) -> Result<(), EtnfError> {
        check_ident(&self.id, "id")?;
        check_ident(&self.version, "version")
    }
}

/// One named item of evidence submitted by an untrusted proposer.
///
/// `digest` is the lowercase-hex SHA-256 of the evidence bytes the item
/// claims; the trusted checker is responsible for verifying it against the
/// actual bytes before it may contribute to a verdict.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceItem {
    pub name: String,
    pub digest: String,
}

impl EvidenceItem {
    pub fn new(name: impl Into<String>, digest: impl Into<String>) -> Result<Self, EtnfError> {
        let item = Self {
            name: name.into(),
            digest: digest.into(),
        };
        item.validate()?;
        Ok(item)
    }

    pub fn validate(&self) -> Result<(), EtnfError> {
        check_ident(&self.name, "name")?;
        check_hex(&self.digest, "digest")
    }
}

/// Rooted evidence: a finite set of evidence items anchored to a root.
///
/// The root is **derived**, never declared: `root()` is a pure function of
/// `anchor` and `items`, so a proposer cannot present evidence whose claimed
/// root does not bind its own items. The certificate root then binds this
/// evidence root, which transitively binds every item.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RootedEvidence {
    /// Lowercase-hex digest of the root identity the evidence is anchored to.
    pub anchor: String,
    pub items: Vec<EvidenceItem>,
}

impl RootedEvidence {
    pub fn new(anchor: impl Into<String>, items: Vec<EvidenceItem>) -> Result<Self, EtnfError> {
        let evidence = Self {
            anchor: anchor.into(),
            items,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    pub fn validate(&self) -> Result<(), EtnfError> {
        check_hex(&self.anchor, "anchor")?;
        if self.items.len() > ETNF_MAX_EVIDENCE_ITEMS {
            return Err(EtnfError::TooManyItems {
                field: "items",
                actual: self.items.len(),
                maximum: ETNF_MAX_EVIDENCE_ITEMS,
            });
        }
        for item in &self.items {
            item.validate()?;
        }
        Ok(())
    }

    /// Derived root over the anchor and every item (canonical JSON + SHA-256).
    pub fn root(&self) -> String {
        sha256_hex(
            canonical_json(&serde_json::json!({
                "anchor": self.anchor.clone(),
                "items": self.items.iter().map(|item| serde_json::json!({
                    "digest": item.digest.clone(),
                    "name": item.name.clone(),
                })).collect::<Vec<_>>(),
            }))
            .as_bytes(),
        )
    }
}

/// Finite witness: the bounded list of facts a checker consumed.
///
/// Finiteness is enforced by construction: at most
/// [`ETNF_MAX_WITNESS_FACTS`] facts, each at most [`ETNF_MAX_STRING_BYTES`]
/// bytes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FiniteWitness {
    pub facts: Vec<String>,
}

impl FiniteWitness {
    pub fn new(facts: Vec<String>) -> Result<Self, EtnfError> {
        let witness = Self { facts };
        witness.validate()?;
        Ok(witness)
    }

    pub fn validate(&self) -> Result<(), EtnfError> {
        if self.facts.len() > ETNF_MAX_WITNESS_FACTS {
            return Err(EtnfError::TooManyItems {
                field: "facts",
                actual: self.facts.len(),
                maximum: ETNF_MAX_WITNESS_FACTS,
            });
        }
        for fact in &self.facts {
            check_text(fact, "fact")?;
        }
        Ok(())
    }
}

/// The narrow class of authority a Safe certificate *would* carry if it were
/// ever promoted. Shadow records only propose; nothing here grants.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposedTransitionKind {
    /// Reuse a cached protected result (family 3, semantic-boundary cache).
    ReuseCachedResult,
    /// Run tools privately until a preauthorized policy escapes (family 1).
    RunToolsPrivately,
    /// Skip a model turn for preauthorized observations (family 1).
    SkipModelTurn,
    /// Auto-select the unique certified maximum candidate (family 2).
    SelectUniqueMaximum,
    /// Hide only dominated candidates behind the Pareto frontier (family 2).
    HideDominatedCandidate,
    /// Keep a proof obligation live across bytes (family 3/4).
    KeepProofLive,
    /// Reverify only the descendant proof cone (family 4).
    ReverifyDescendantCone,
}

/// Narrow proposed authority transition (ETNF component).
///
/// A proposal only: there is no `granted` field, no gate input is produced,
/// and the type is not accepted by any V6 write/permit gate. `target` names
/// the protected result/object the transition would touch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposedAuthorityTransition {
    pub kind: ProposedTransitionKind,
    pub target: String,
}

impl ProposedAuthorityTransition {
    pub fn new(kind: ProposedTransitionKind, target: impl Into<String>) -> Result<Self, EtnfError> {
        let transition = Self {
            kind,
            target: target.into(),
        };
        transition.validate()?;
        Ok(transition)
    }

    pub fn validate(&self) -> Result<(), EtnfError> {
        check_hex(&self.target, "target")
    }
}

/// The escape hatch a deployable theorem must name (ETNF component).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FallbackKind {
    /// V6 law: `Unknown` requires the frozen raw-baseline fallback.
    FrozenRawBaseline,
    /// V6-15: the native direct-tool path, `C_Z = min(C_direct, C_kernel)`.
    DirectNativePath,
    /// `Unsafe` fails closed with no recovery.
    Abort,
}

/// Explicit fallback: what runs when the verdict is not `Safe`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExplicitFallback {
    pub kind: FallbackKind,
    /// Bounded, human-readable obligation the fallback must satisfy.
    pub obligation: String,
}

impl ExplicitFallback {
    pub fn new(kind: FallbackKind, obligation: impl Into<String>) -> Result<Self, EtnfError> {
        let fallback = Self {
            kind,
            obligation: obligation.into(),
        };
        fallback.validate()?;
        Ok(fallback)
    }

    pub fn validate(&self) -> Result<(), EtnfError> {
        check_text(&self.obligation, "obligation")
    }
}

/// A named falsifier: a sharp condition that would refute the checked claim.
///
/// Falsifiers are declarations for observability; in shadow they never block
/// or authorize anything.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Falsifier {
    /// Stable falsifier identifier, e.g. `W7-T01-f1`.
    pub id: String,
    pub description: String,
}

impl Falsifier {
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Result<Self, EtnfError> {
        let falsifier = Self {
            id: id.into(),
            description: description.into(),
        };
        falsifier.validate()?;
        Ok(falsifier)
    }

    pub fn validate(&self) -> Result<(), EtnfError> {
        check_ident(&self.id, "id")?;
        check_text(&self.description, "description")
    }
}

/// Complete resource ledger (ETNF component).
///
/// `complete` is `true` only when the ledger accounts for every unit the
/// check consumed; a checker that cannot close its ledger must not report
/// `Safe` on the strength of that run.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLedger {
    pub bytes_read: u64,
    pub items_checked: u64,
    pub checks: u64,
    pub complete: bool,
}

impl ResourceLedger {
    pub fn new(bytes_read: u64, items_checked: u64, checks: u64, complete: bool) -> Self {
        Self {
            bytes_read,
            items_checked,
            checks,
            complete,
        }
    }

    /// Derived root over every ledger field (canonical JSON + SHA-256).
    pub fn root(&self) -> String {
        sha256_hex(
            canonical_json(&serde_json::json!({
                "bytes_read": self.bytes_read,
                "checks": self.checks,
                "complete": self.complete,
                "items_checked": self.items_checked,
            }))
            .as_bytes(),
        )
    }
}

/// Shadow certificate (W7-T01): issued only for a `Safe` verdict.
///
/// The `root` is derived over exactly the five bound identities: evidence
/// root, scope, contract, checker (id and version), and resource ledger root.
/// The certificate carries no authority: it is a shadow artifact that no
/// production write/permit gate accepts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowCertificate {
    /// Derived root binding every field below.
    pub root: String,
    pub evidence_root: String,
    pub scope: String,
    pub contract: String,
    pub checker: CheckerIdentity,
    pub resource_ledger_root: String,
}

impl ShadowCertificate {
    /// Issue a certificate. Fails closed with [`EtnfError::NotSafe`] unless
    /// the verdict is `Safe`: `Unsafe` and `Unknown` cannot serialize
    /// authority.
    pub fn issue(
        verdict: &SafetyVerdict,
        evidence_root: &str,
        scope: &str,
        contract: &str,
        checker: &CheckerIdentity,
        resource_ledger_root: &str,
    ) -> Result<Self, EtnfError> {
        if !verdict.grants_authority() {
            return Err(EtnfError::NotSafe);
        }
        let mut certificate = Self {
            root: String::new(),
            evidence_root: evidence_root.into(),
            scope: scope.into(),
            contract: contract.into(),
            checker: checker.clone(),
            resource_ledger_root: resource_ledger_root.into(),
        };
        certificate.root = Self::root_of(
            &certificate.evidence_root,
            &certificate.scope,
            &certificate.contract,
            &certificate.checker,
            &certificate.resource_ledger_root,
        );
        certificate.validate()?;
        Ok(certificate)
    }

    /// Deterministic root over the five bound identities.
    pub fn root_of(
        evidence_root: &str,
        scope: &str,
        contract: &str,
        checker: &CheckerIdentity,
        resource_ledger_root: &str,
    ) -> String {
        sha256_hex(
            canonical_json(&serde_json::json!({
                "checker": { "id": checker.id.clone(), "version": checker.version.clone() },
                "contract": contract,
                "evidence_root": evidence_root,
                "resource_ledger_root": resource_ledger_root,
                "scope": scope,
            }))
            .as_bytes(),
        )
    }

    pub fn validate(&self) -> Result<(), EtnfError> {
        check_hex(&self.root, "root")?;
        check_hex(&self.evidence_root, "evidence_root")?;
        check_hex(&self.resource_ledger_root, "resource_ledger_root")?;
        check_ident(&self.scope, "scope")?;
        check_ident(&self.contract, "contract")?;
        self.checker.validate()?;
        let expected = Self::root_of(
            &self.evidence_root,
            &self.scope,
            &self.contract,
            &self.checker,
            &self.resource_ledger_root,
        );
        if self.root != expected {
            return Err(EtnfError::CertificateRootMismatch);
        }
        Ok(())
    }
}

/// Observable, comparable V7 shadow output.
///
/// `shadow` is always `true` and enforced on parse, so a shadow document can
/// never be laundered into a production artifact. `certificate` is present
/// if and only if the verdict is `Safe`, and its root binds evidence, scope,
/// contract, checker version, and resource ledger. The report grants no
/// production authority and is not accepted by any existing write/permit
/// gate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct V7ShadowReport {
    pub schema: String,
    pub shadow: bool,
    pub verdict: SafetyVerdict,
    pub checker: CheckerIdentity,
    pub scope: String,
    pub contract: String,
    pub evidence: RootedEvidence,
    pub witness: FiniteWitness,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<ProposedAuthorityTransition>,
    pub fallback: ExplicitFallback,
    pub falsifiers: Vec<Falsifier>,
    pub ledger: ResourceLedger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate: Option<ShadowCertificate>,
}

impl V7ShadowReport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        verdict: SafetyVerdict,
        checker: CheckerIdentity,
        scope: impl Into<String>,
        contract: impl Into<String>,
        evidence: RootedEvidence,
        witness: FiniteWitness,
        transition: Option<ProposedAuthorityTransition>,
        fallback: ExplicitFallback,
        falsifiers: Vec<Falsifier>,
        ledger: ResourceLedger,
    ) -> Result<Self, EtnfError> {
        let scope = scope.into();
        let contract = contract.into();
        let certificate = if verdict.grants_authority() {
            Some(ShadowCertificate::issue(
                &verdict,
                &evidence.root(),
                &scope,
                &contract,
                &checker,
                &ledger.root(),
            )?)
        } else {
            None
        };
        let report = Self {
            schema: ETNF_SCHEMA_ID.into(),
            shadow: true,
            verdict,
            checker,
            scope,
            contract,
            evidence,
            witness,
            transition,
            fallback,
            falsifiers,
            ledger,
            certificate,
        };
        report.validate()?;
        Ok(report)
    }

    /// Validate every invariant, including certificate binding and root
    /// recomputation over the bound fields.
    pub fn validate(&self) -> Result<(), EtnfError> {
        if self.schema != ETNF_SCHEMA_ID {
            return Err(EtnfError::InvalidSchema {
                actual: self.schema.clone(),
            });
        }
        if !self.shadow {
            return Err(EtnfError::ShadowMarkerFalse);
        }
        check_ident(&self.scope, "scope")?;
        check_ident(&self.contract, "contract")?;
        self.checker.validate()?;
        self.evidence.validate()?;
        self.witness.validate()?;
        self.fallback.validate()?;
        if let Some(transition) = &self.transition {
            transition.validate()?;
        }
        for falsifier in &self.falsifiers {
            falsifier.validate()?;
        }
        if self.falsifiers.len() > ETNF_MAX_FALSIFIERS {
            return Err(EtnfError::TooManyItems {
                field: "falsifiers",
                actual: self.falsifiers.len(),
                maximum: ETNF_MAX_FALSIFIERS,
            });
        }
        let safe = self.verdict.grants_authority();
        match (&self.certificate, safe) {
            (None, true) => return Err(EtnfError::MissingCertificateForSafe),
            (Some(_), false) => return Err(EtnfError::CertificateWithoutSafe),
            (None, false) => {}
            (Some(certificate), true) => {
                certificate.validate()?;
                if certificate.evidence_root != self.evidence.root() {
                    return Err(EtnfError::CertificateBindingMismatch {
                        field: "evidence_root",
                    });
                }
                if certificate.resource_ledger_root != self.ledger.root() {
                    return Err(EtnfError::CertificateBindingMismatch {
                        field: "resource_ledger_root",
                    });
                }
                if certificate.checker != self.checker {
                    return Err(EtnfError::CertificateBindingMismatch { field: "checker" });
                }
                let expected = ShadowCertificate::root_of(
                    &certificate.evidence_root,
                    &certificate.scope,
                    &certificate.contract,
                    &certificate.checker,
                    &certificate.resource_ledger_root,
                );
                if certificate.root != expected {
                    return Err(EtnfError::CertificateRootMismatch);
                }
            }
        }
        Ok(())
    }

    /// Canonical bytes (sorted keys, no whitespace, derived roots) for
    /// storage, comparison, and cross-checker auditing.
    pub fn to_canonical_bytes(&self) -> Result<Vec<u8>, EtnfError> {
        let value = serde_json::to_value(self).map_err(|error| EtnfError::BadJson {
            message: error.to_string(),
        })?;
        Ok(canonical_json(&value).into_bytes())
    }

    /// Parse and validate canonical bytes. Rejects non-canonical encodings,
    /// wrong schema/shadow markers, certificates under non-Safe verdicts, and
    /// any root that does not bind its fields.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, EtnfError> {
        let report: V7ShadowReport =
            serde_json::from_slice(bytes).map_err(|error| EtnfError::BadJson {
                message: error.to_string(),
            })?;
        report.validate()?;
        if report.to_canonical_bytes()? != bytes {
            return Err(EtnfError::NonCanonicalBytes);
        }
        Ok(report)
    }

    /// Shadow-semantic authority shape: `Safe` verdict with a live
    /// certificate. Grants no production authority.
    pub fn grants_authority(&self) -> bool {
        self.verdict.grants_authority() && self.certificate.is_some()
    }
}
