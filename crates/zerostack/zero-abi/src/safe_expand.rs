//! Trusted `SafeExpandHandle` ABI for W9-E exact expansion (hub authority
//! only; zerostack-qg2a).
//!
//! A `SafeExpandHandle` is an opaque, unforgeable, read-only credential for
//! exactly one exact expansion. It binds every authority root the expansion
//! was certified against: project, request, protected scope, demand-plan
//! root, index root/version, renderer contract, tenant, epoch, projection,
//! and the completeness certificate (root, trivalent verdict, checker
//! identity/version, first-attempt law). Guest JS and models cannot construct
//! it: the only constructor is [`SafeExpandIssuer::issue`], which requires
//! the hub-owned issuance key, and every use revalidates the keyed issuance
//! MAC plus every live binding.
//!
//! Fail-closed laws (ZS-KERNEL-004 lattice: `Unsafe` dominates `Unknown`
//! dominates `Safe`):
//! - Issuance requires a total completeness check whose verdict is exactly
//!   [`SafetyVerdict::Safe`] with a certificate root and a first-attempt
//!   checker (no hidden retry). `Unsafe`, `Unknown`, a missing certificate
//!   root, or a hidden retry refuse issuance with a typed error.
//! - A handle verifies two independent seals before use: the keyed issuance
//!   MAC (forgery requires the issuer secret) and the self-rooted handle id
//!   (any tamper changes the id).
//! - Revalidation ([`SafeExpandIssuer::revalidate`]) checks every binding
//!   against the live hub state. Stale epoch/index, cross-project or
//!   cross-tenant use, altered scope/projection, renderer mismatch,
//!   missing/mismatched/Unknown evidence, or a hidden retry after issue
//!   yields a typed `Unsafe`/`Unknown` outcome; a guessed subset is never
//!   labeled complete. Only `Safe` carries the read-only [`ExpandPermit`].
//! - The permit encodes read-only authority only: it carries the bound
//!   roots and projection, never any write, edit, transaction, or commit
//!   capability.
//!
//! The completeness certificate is bound by root: this module pins the
//! certificate root, verdict, checker identity/version, and first-attempt
//! law. The certificate object itself (rooted evidence, finite witness,
//! resource ledger) is the V7 certificate ABI (zerostack-4lfp); certificate
//! composition (W7-T03) plugs into the same bound fields when it lands.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::Sha256Digest;
use crate::identity::{ObjectClass, ROOTED_ABI_VERSION, canonical_object_bytes, object_root};
use crate::schema::canonical_json;
use crate::verdict::SafetyVerdict;

/// Contract version of the handle wire form.
pub const SAFE_EXPAND_CONTRACT_VERSION: u16 = 1;

/// Domain separation for the keyed issuance MAC. A MAC from any other domain
/// can never verify.
pub const SAFE_EXPAND_MAC_DOMAIN: &[u8] = b"zerostack.safe_expand.issuance\0";

/// Upper bound for tenant ids and index versions bound into a handle.
pub const MAX_SAFE_EXPAND_STRING_BYTES: usize = 256;

/// Stable revalidation reason strings (wire-visible, sorted and deduplicated
/// in every outcome).
mod reasons {
    pub const WRONG_ABI_VERSION: &str = "wrong_abi_version";
    pub const FORGED_HANDLE: &str = "forged_handle";
    pub const TAMPERED_HANDLE: &str = "tampered_handle";
    pub const INVALID_HANDLE: &str = "invalid_handle";
    pub const PROJECT_MISMATCH: &str = "project_mismatch";
    pub const REQUEST_MISMATCH: &str = "request_mismatch";
    pub const SCOPE_MISMATCH: &str = "scope_mismatch";
    pub const DEMAND_PLAN_MISMATCH: &str = "demand_plan_mismatch";
    pub const INDEX_ROOT_MISMATCH: &str = "index_root_mismatch";
    pub const INDEX_VERSION_MISMATCH: &str = "index_version_mismatch";
    pub const RENDERER_MISMATCH: &str = "renderer_mismatch";
    pub const TENANT_MISMATCH: &str = "tenant_mismatch";
    pub const EPOCH_MISMATCH: &str = "epoch_mismatch";
    pub const PROJECTION_MISMATCH: &str = "projection_mismatch";
    pub const COMPLETENESS_EVIDENCE_MISSING: &str = "completeness_evidence_missing";
    pub const COMPLETENESS_CHECKER_MISSING: &str = "completeness_checker_missing";
    pub const COMPLETENESS_UNKNOWN: &str = "completeness_unknown";
    pub const COMPLETENESS_UNSAFE: &str = "completeness_unsafe";
    pub const COMPLETENESS_CERTIFICATE_MISMATCH: &str = "completeness_certificate_mismatch";
    pub const CHECKER_IDENTITY_MISMATCH: &str = "checker_identity_mismatch";
    pub const CHECKER_VERSION_MISMATCH: &str = "checker_version_mismatch";
    pub const COMPLETENESS_RETRY: &str = "completeness_retry";
    pub const HIDDEN_RETRY_AFTER_ISSUE: &str = "hidden_retry_after_issue";
}

/// Fail-closed error for handle issuance and seal verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SafeExpandError {
    /// The wire form carries an ABI version other than the rooted ABI.
    WrongAbiVersion { actual: String },
    /// The bound root is the zero digest, i.e. an absent/fabricated anchor.
    ZeroRoot(&'static str),
    /// A bound string field is empty.
    EmptyString(&'static str),
    /// A bound string field exceeds the byte bound.
    StringTooLong {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    /// A bound string field carries a control character.
    ControlCharacter { field: &'static str },
    /// The issuance nonce must be nonzero: every handle identifies one issue.
    ZeroIssueNonce,
    /// Completeness evidence whose verdict is `Unsafe` can never issue.
    UnsafeCompleteness { reasons: Vec<String> },
    /// Completeness evidence whose verdict is `Unknown` can never issue.
    UnknownCompleteness { reasons: Vec<String> },
    /// A `Safe` completeness claim must carry a certificate root.
    MissingCertificateRoot,
    /// The completeness check was retried; issuance is first-attempt only.
    HiddenRetryAtIssuance,
    /// The issuance MAC does not verify: the handle was not issued by this
    /// issuer (or the wire form was rebuilt without the issuer secret).
    ForgedHandle,
    /// The self-rooted handle id does not match the bound fields: the handle
    /// was issued but altered after issuance.
    TamperedHandle,
    /// Structural failure during serialization/rooting.
    InvalidHandle(String),
}

impl fmt::Display for SafeExpandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongAbiVersion { actual } => write!(
                formatter,
                "safe expand handle abi must be {ROOTED_ABI_VERSION}, got {actual}"
            ),
            Self::ZeroRoot(field) => {
                write!(formatter, "safe expand handle requires a nonzero {field}")
            }
            Self::EmptyString(field) => {
                write!(formatter, "safe expand handle {field} must be nonempty")
            }
            Self::StringTooLong {
                field,
                actual,
                maximum,
            } => write!(
                formatter,
                "safe expand handle {field} is {actual} bytes, maximum {maximum}"
            ),
            Self::ControlCharacter { field } => write!(
                formatter,
                "safe expand handle {field} must be free of control characters"
            ),
            Self::ZeroIssueNonce => {
                write!(formatter, "safe expand handle issue nonce must be nonzero")
            }
            Self::UnsafeCompleteness { reasons } => write!(
                formatter,
                "completeness verdict is Unsafe, handle cannot issue: {reasons:?}"
            ),
            Self::UnknownCompleteness { reasons } => write!(
                formatter,
                "completeness verdict is Unknown, handle cannot issue: {reasons:?}"
            ),
            Self::MissingCertificateRoot => write!(
                formatter,
                "a Safe completeness claim must carry a certificate root"
            ),
            Self::HiddenRetryAtIssuance => write!(
                formatter,
                "completeness check was not first-attempt; hidden retry cannot issue"
            ),
            Self::ForgedHandle => write!(
                formatter,
                "safe expand handle issuance MAC does not verify (forged or unissued)"
            ),
            Self::TamperedHandle => write!(
                formatter,
                "safe expand handle id does not match the bound fields (tampered)"
            ),
            Self::InvalidHandle(detail) => {
                write!(formatter, "invalid safe expand handle: {detail}")
            }
        }
    }
}

impl Error for SafeExpandError {}

/// The completeness certificate binding of one handle.
///
/// `first_attempt` is always bound `true`: a handle may only ever be issued
/// from a first-attempt (no hidden retry) total completeness check.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletenessBinding {
    /// Root of the V7 completeness certificate (zerostack-4lfp object).
    certificate_root: Sha256Digest,
    /// Must be exactly `SafetyVerdict::Safe`; revalidation re-checks the
    /// live verdict.
    verdict: SafetyVerdict,
    /// Identity of the checker that produced the certificate.
    checker_identity: String,
    /// Version of the checker that produced the certificate.
    checker_version: String,
    /// Bound `true`; a retried check can never issue or stay live.
    first_attempt: bool,
}

impl CompletenessBinding {
    pub fn certificate_root(&self) -> Sha256Digest {
        self.certificate_root
    }

    pub fn verdict(&self) -> &SafetyVerdict {
        &self.verdict
    }

    pub fn checker_identity(&self) -> &str {
        &self.checker_identity
    }

    pub fn checker_version(&self) -> &str {
        &self.checker_version
    }

    pub fn first_attempt(&self) -> bool {
        self.first_attempt
    }
}

/// Completeness evidence submitted by the trusted hub for issuance. Only
/// `Safe` evidence with a certificate root and `first_attempt: true` may
/// issue a handle.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompletenessEvidence {
    pub certificate_root: Sha256Digest,
    pub verdict: SafetyVerdict,
    pub checker_identity: String,
    pub checker_version: String,
    /// Must be `true`; `false` means the total completeness check retried
    /// hidden and is refused at issuance.
    pub first_attempt: bool,
}

impl CompletenessEvidence {
    /// Fail-closed validation of the issuance evidence.
    pub fn validate(&self) -> Result<(), SafeExpandError> {
        match &self.verdict {
            SafetyVerdict::Safe => {}
            SafetyVerdict::Unsafe { reasons } => {
                return Err(SafeExpandError::UnsafeCompleteness {
                    reasons: reasons.clone(),
                });
            }
            SafetyVerdict::Unknown { reasons } => {
                return Err(SafeExpandError::UnknownCompleteness {
                    reasons: reasons.clone(),
                });
            }
        }
        if self.certificate_root == Sha256Digest::ZERO {
            return Err(SafeExpandError::MissingCertificateRoot);
        }
        validate_string("checker_identity", &self.checker_identity)?;
        validate_string("checker_version", &self.checker_version)?;
        if !self.first_attempt {
            return Err(SafeExpandError::HiddenRetryAtIssuance);
        }
        Ok(())
    }
}

/// One trusted issuance request. Only trusted hub code may construct and
/// submit this; the issuer secret stays out of every wire form.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SafeExpandIssueRequest {
    pub project_root: Sha256Digest,
    pub request_root: Sha256Digest,
    pub protected_scope_root: Sha256Digest,
    pub demand_plan_root: Sha256Digest,
    pub index_root: Sha256Digest,
    pub index_version: String,
    pub renderer_contract: Sha256Digest,
    pub tenant: String,
    pub epoch: u64,
    pub projection_root: Sha256Digest,
    pub completeness: CompletenessEvidence,
    /// Per-issuance nonce supplied by the hub; must be nonzero.
    pub issue_nonce: Sha256Digest,
}

impl SafeExpandIssueRequest {
    /// Fail-closed validation of every binding.
    pub fn validate(&self) -> Result<(), SafeExpandError> {
        for field in [
            "project_root",
            "request_root",
            "protected_scope_root",
            "demand_plan_root",
            "index_root",
            "renderer_contract",
            "projection_root",
        ] {
            let root = match field {
                "project_root" => self.project_root,
                "request_root" => self.request_root,
                "protected_scope_root" => self.protected_scope_root,
                "demand_plan_root" => self.demand_plan_root,
                "index_root" => self.index_root,
                "renderer_contract" => self.renderer_contract,
                "projection_root" => self.projection_root,
                _ => unreachable!("validated field list"),
            };
            if root == Sha256Digest::ZERO {
                return Err(SafeExpandError::ZeroRoot(field));
            }
        }
        validate_string("index_version", &self.index_version)?;
        validate_string("tenant", &self.tenant)?;
        if self.issue_nonce == Sha256Digest::ZERO {
            return Err(SafeExpandError::ZeroIssueNonce);
        }
        self.completeness.validate()?;
        Ok(())
    }
}

/// Live hub state used to revalidate every handle binding at use time.
/// Produced only by trusted hub code from observed authority state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveExpandState {
    pub project_root: Sha256Digest,
    pub request_root: Sha256Digest,
    pub protected_scope_root: Sha256Digest,
    pub demand_plan_root: Sha256Digest,
    pub index_root: Sha256Digest,
    pub index_version: String,
    pub renderer_contract: Sha256Digest,
    pub tenant: String,
    pub epoch: u64,
    pub projection_root: Sha256Digest,
    pub completeness: LiveCompleteness,
    /// `true` when the hub observed a hidden retry after this handle was
    /// issued; any such observation revokes the handle (typed `Unsafe`).
    pub hidden_retry_after_issue: bool,
}

/// Live completeness evidence at use time. `None` fields mean the evidence
/// is missing; missing evidence is `Unknown`, never a guessed subset.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveCompleteness {
    pub certificate_root: Option<Sha256Digest>,
    pub verdict: SafetyVerdict,
    pub checker_identity: Option<String>,
    pub checker_version: Option<String>,
    /// Live checker report of whether the check was first-attempt.
    pub first_attempt: bool,
}

/// Typed outcome of one revalidation. `Safe` is the only outcome that
/// carries the read-only [`ExpandPermit`]; `Unsafe` and `Unknown` carry
/// sorted, deduplicated reasons and never a permit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpandOutcome {
    Safe(ExpandPermit),
    Unsafe { reasons: Vec<String> },
    Unknown { reasons: Vec<String> },
}

impl ExpandOutcome {
    /// The lattice view of this outcome for ledgering:
    /// `Safe -> Safe`, `Unsafe -> Unsafe`, `Unknown -> Unknown`.
    pub fn to_verdict(&self) -> SafetyVerdict {
        match self {
            ExpandOutcome::Safe(_) => SafetyVerdict::Safe,
            ExpandOutcome::Unsafe { reasons } => SafetyVerdict::Unsafe {
                reasons: reasons.clone(),
            },
            ExpandOutcome::Unknown { reasons } => SafetyVerdict::Unknown {
                reasons: reasons.clone(),
            },
        }
    }

    pub fn reasons(&self) -> &[String] {
        match self {
            ExpandOutcome::Safe(_) => &[],
            ExpandOutcome::Unsafe { reasons } | ExpandOutcome::Unknown { reasons } => reasons,
        }
    }

    pub fn is_safe(&self) -> bool {
        matches!(self, ExpandOutcome::Safe(_))
    }

    pub fn permit(&self) -> Option<&ExpandPermit> {
        match self {
            ExpandOutcome::Safe(permit) => Some(permit),
            _ => None,
        }
    }
}

/// The read-only authority granted by a `Safe` revalidation: exactly the
/// bound roots and projection of the verified handle. There is no write,
/// edit, transaction, or commit field anywhere in this type.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExpandPermit {
    handle_id: Sha256Digest,
    project_root: Sha256Digest,
    request_root: Sha256Digest,
    protected_scope_root: Sha256Digest,
    demand_plan_root: Sha256Digest,
    index_root: Sha256Digest,
    index_version: String,
    renderer_contract: Sha256Digest,
    tenant: String,
    epoch: u64,
    projection_root: Sha256Digest,
}

impl ExpandPermit {
    pub fn handle_id(&self) -> Sha256Digest {
        self.handle_id
    }

    pub fn project_root(&self) -> Sha256Digest {
        self.project_root
    }

    pub fn request_root(&self) -> Sha256Digest {
        self.request_root
    }

    pub fn protected_scope_root(&self) -> Sha256Digest {
        self.protected_scope_root
    }

    pub fn demand_plan_root(&self) -> Sha256Digest {
        self.demand_plan_root
    }

    pub fn index_root(&self) -> Sha256Digest {
        self.index_root
    }

    pub fn index_version(&self) -> &str {
        &self.index_version
    }

    pub fn renderer_contract(&self) -> Sha256Digest {
        self.renderer_contract
    }

    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn projection_root(&self) -> Sha256Digest {
        self.projection_root
    }
}

/// The opaque, unforgeable, read-only safe-expand credential.
///
/// Fields are private to this module: the only constructor is
/// [`SafeExpandIssuer::issue`], and the wire form carries two independent
/// seals (issuance MAC + self-rooted id) so deserializing or altering a
/// handle never grants authority by itself.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SafeExpandHandle {
    abi_version: String,
    handle_version: u16,
    project_root: Sha256Digest,
    request_root: Sha256Digest,
    protected_scope_root: Sha256Digest,
    demand_plan_root: Sha256Digest,
    index_root: Sha256Digest,
    index_version: String,
    renderer_contract: Sha256Digest,
    tenant: String,
    epoch: u64,
    projection_root: Sha256Digest,
    completeness: CompletenessBinding,
    issue_nonce: Sha256Digest,
    /// Self-rooted id over every field including the issuance MAC. Any
    /// tamper changes the id.
    handle_id: Sha256Digest,
    /// Keyed issuance MAC over every field except the two seals. Only the
    /// trusted issuer can produce it.
    issuance_mac: Sha256Digest,
}

impl SafeExpandHandle {
    pub fn abi_version(&self) -> &str {
        &self.abi_version
    }

    pub fn handle_version(&self) -> u16 {
        self.handle_version
    }

    pub fn project_root(&self) -> Sha256Digest {
        self.project_root
    }

    pub fn request_root(&self) -> Sha256Digest {
        self.request_root
    }

    pub fn protected_scope_root(&self) -> Sha256Digest {
        self.protected_scope_root
    }

    pub fn demand_plan_root(&self) -> Sha256Digest {
        self.demand_plan_root
    }

    pub fn index_root(&self) -> Sha256Digest {
        self.index_root
    }

    pub fn index_version(&self) -> &str {
        &self.index_version
    }

    pub fn renderer_contract(&self) -> Sha256Digest {
        self.renderer_contract
    }

    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn projection_root(&self) -> Sha256Digest {
        self.projection_root
    }

    pub fn completeness(&self) -> &CompletenessBinding {
        &self.completeness
    }

    pub fn issue_nonce(&self) -> Sha256Digest {
        self.issue_nonce
    }

    pub fn handle_id(&self) -> Sha256Digest {
        self.handle_id
    }

    /// Durable round trip: a serialized handle deserializes to the identical
    /// handle id (the wire form is self-verifying). Tampered wire forms fail
    /// closed during deserialization or on `verify` against the issuer.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, SafeExpandError> {
        let value = serde_json::to_value(self)
            .map_err(|error| SafeExpandError::InvalidHandle(error.to_string()))?;
        canonical_object_bytes(ObjectClass::SafeExpandHandle, ROOTED_ABI_VERSION, &value)
            .map_err(|error| SafeExpandError::InvalidHandle(error.to_string()))
    }

    fn canonical_value(&self) -> Result<Value, SafeExpandError> {
        serde_json::to_value(self)
            .map_err(|error| SafeExpandError::InvalidHandle(error.to_string()))
    }

    /// Canonical bytes of every bound field except the two seals. This is
    /// the payload both seals cover, so neither seal can be replayed onto
    /// different bindings.
    fn sealed_payload(&self) -> Result<Vec<u8>, SafeExpandError> {
        let value = self.canonical_value()?;
        let mut object = value
            .as_object()
            .cloned()
            .ok_or_else(|| SafeExpandError::InvalidHandle("handle must be an object".into()))?;
        object.remove("handle_id");
        object.remove("issuance_mac");
        Ok(canonical_json(&Value::Object(object)).into_bytes())
    }

    /// The self-rooted id: the rooted digest of every field including the
    /// issuance MAC but excluding the id itself.
    fn compute_id(&self) -> Result<Sha256Digest, SafeExpandError> {
        let value = self.canonical_value()?;
        let mut object = value
            .as_object()
            .cloned()
            .ok_or_else(|| SafeExpandError::InvalidHandle("handle must be an object".into()))?;
        object.remove("handle_id");
        let bytes = canonical_object_bytes(
            ObjectClass::SafeExpandHandle,
            ROOTED_ABI_VERSION,
            &Value::Object(object),
        )
        .map_err(|error| SafeExpandError::InvalidHandle(error.to_string()))?;
        object_root(ObjectClass::SafeExpandHandle, ROOTED_ABI_VERSION, &bytes)
            .map_err(|error| SafeExpandError::InvalidHandle(error.to_string()))
    }
}

/// Trusted hub-owned issuer. The 32-byte secret is generated and held by
/// trusted hub code; guest JS and models never see it, so they can never
/// produce a handle whose issuance MAC verifies.
#[derive(Clone, Debug)]
pub struct SafeExpandIssuer {
    secret: [u8; 32],
}

impl SafeExpandIssuer {
    pub fn new(secret: [u8; 32]) -> Self {
        Self { secret }
    }

    /// Issue one handle after the hub's total completeness check returned
    /// `Safe`. Every binding is validated fail-closed; `Unsafe`/`Unknown`
    /// evidence, a missing certificate root, or a hidden retry refuse
    /// issuance with a typed error.
    pub fn issue(
        &self,
        request: &SafeExpandIssueRequest,
    ) -> Result<SafeExpandHandle, SafeExpandError> {
        request.validate()?;
        let mut handle = SafeExpandHandle {
            abi_version: ROOTED_ABI_VERSION.to_owned(),
            handle_version: SAFE_EXPAND_CONTRACT_VERSION,
            project_root: request.project_root,
            request_root: request.request_root,
            protected_scope_root: request.protected_scope_root,
            demand_plan_root: request.demand_plan_root,
            index_root: request.index_root,
            index_version: request.index_version.clone(),
            renderer_contract: request.renderer_contract,
            tenant: request.tenant.clone(),
            epoch: request.epoch,
            projection_root: request.projection_root,
            completeness: CompletenessBinding {
                certificate_root: request.completeness.certificate_root,
                verdict: request.completeness.verdict.clone(),
                checker_identity: request.completeness.checker_identity.clone(),
                checker_version: request.completeness.checker_version.clone(),
                first_attempt: request.completeness.first_attempt,
            },
            issue_nonce: request.issue_nonce,
            handle_id: Sha256Digest::ZERO,
            issuance_mac: Sha256Digest::ZERO,
        };
        let payload = handle.sealed_payload()?;
        handle.issuance_mac =
            Sha256Digest::from_bytes(compute_issuance_mac(&self.secret, &payload));
        handle.handle_id = handle.compute_id()?;
        Ok(handle)
    }

    /// Verify both seals of a handle: the keyed issuance MAC (issued by
    /// *this* issuer) and the self-rooted id (untampered). Any failure is a
    /// typed error; verification grants no authority by itself.
    pub fn verify(&self, handle: &SafeExpandHandle) -> Result<(), SafeExpandError> {
        if handle.abi_version != ROOTED_ABI_VERSION {
            return Err(SafeExpandError::WrongAbiVersion {
                actual: handle.abi_version.clone(),
            });
        }
        if handle.handle_version != SAFE_EXPAND_CONTRACT_VERSION {
            return Err(SafeExpandError::InvalidHandle(format!(
                "unsupported handle version {}",
                handle.handle_version
            )));
        }
        let payload = handle.sealed_payload()?;
        let expected_mac = Sha256Digest::from_bytes(compute_issuance_mac(&self.secret, &payload));
        if handle.issuance_mac != expected_mac {
            return Err(SafeExpandError::ForgedHandle);
        }
        let expected_id = handle.compute_id()?;
        if handle.handle_id != expected_id {
            return Err(SafeExpandError::TamperedHandle);
        }
        Ok(())
    }

    /// Live revalidation of every handle binding. Total function: seal
    /// failures, binding mismatches, stale/missing/Unknown evidence, and
    /// hidden retries all fold into a typed `Unsafe`/`Unknown` outcome;
    /// only full positive revalidation returns `Safe(permit)`.
    pub fn revalidate(&self, handle: &SafeExpandHandle, live: &LiveExpandState) -> ExpandOutcome {
        let mut unsafe_reasons: Vec<String> = Vec::new();
        let mut unknown_reasons: Vec<String> = Vec::new();

        if let Err(error) = self.verify(handle) {
            unsafe_reasons.push(match error {
                SafeExpandError::WrongAbiVersion { .. } => reasons::WRONG_ABI_VERSION.to_owned(),
                SafeExpandError::ForgedHandle => reasons::FORGED_HANDLE.to_owned(),
                SafeExpandError::TamperedHandle => reasons::TAMPERED_HANDLE.to_owned(),
                SafeExpandError::InvalidHandle(_) => reasons::INVALID_HANDLE.to_owned(),
                _ => reasons::INVALID_HANDLE.to_owned(),
            });
        }

        if live.hidden_retry_after_issue {
            unsafe_reasons.push(reasons::HIDDEN_RETRY_AFTER_ISSUE.to_owned());
        }

        check_binding(
            live.project_root == handle.project_root,
            &mut unsafe_reasons,
            reasons::PROJECT_MISMATCH,
        );
        check_binding(
            live.request_root == handle.request_root,
            &mut unsafe_reasons,
            reasons::REQUEST_MISMATCH,
        );
        check_binding(
            live.protected_scope_root == handle.protected_scope_root,
            &mut unsafe_reasons,
            reasons::SCOPE_MISMATCH,
        );
        check_binding(
            live.demand_plan_root == handle.demand_plan_root,
            &mut unsafe_reasons,
            reasons::DEMAND_PLAN_MISMATCH,
        );
        check_binding(
            live.index_root == handle.index_root,
            &mut unsafe_reasons,
            reasons::INDEX_ROOT_MISMATCH,
        );
        check_binding(
            live.index_version == handle.index_version,
            &mut unsafe_reasons,
            reasons::INDEX_VERSION_MISMATCH,
        );
        check_binding(
            live.renderer_contract == handle.renderer_contract,
            &mut unsafe_reasons,
            reasons::RENDERER_MISMATCH,
        );
        check_binding(
            live.tenant == handle.tenant,
            &mut unsafe_reasons,
            reasons::TENANT_MISMATCH,
        );
        check_binding(
            live.epoch == handle.epoch,
            &mut unsafe_reasons,
            reasons::EPOCH_MISMATCH,
        );
        check_binding(
            live.projection_root == handle.projection_root,
            &mut unsafe_reasons,
            reasons::PROJECTION_MISMATCH,
        );

        // Completeness evidence: missing -> Unknown, positive mismatch ->
        // Unsafe, Unknown verdict -> Unknown. Unsafe always dominates.
        let binding = &handle.completeness;
        let certificate_root = match live.completeness.certificate_root {
            Some(root) => root,
            None => {
                unknown_reasons.push(reasons::COMPLETENESS_EVIDENCE_MISSING.to_owned());
                Sha256Digest::ZERO
            }
        };
        let checker_identity = match live.completeness.checker_identity.as_deref() {
            Some(identity) => identity,
            None => {
                unknown_reasons.push(reasons::COMPLETENESS_CHECKER_MISSING.to_owned());
                ""
            }
        };
        let checker_version = match live.completeness.checker_version.as_deref() {
            Some(version) => version,
            None => {
                unknown_reasons.push(reasons::COMPLETENESS_CHECKER_MISSING.to_owned());
                ""
            }
        };
        match &live.completeness.verdict {
            SafetyVerdict::Safe => {}
            SafetyVerdict::Unsafe { .. } => {
                unsafe_reasons.push(reasons::COMPLETENESS_UNSAFE.to_owned());
            }
            SafetyVerdict::Unknown { .. } => {
                unknown_reasons.push(reasons::COMPLETENESS_UNKNOWN.to_owned());
            }
        }
        if certificate_root != Sha256Digest::ZERO && certificate_root != binding.certificate_root {
            unsafe_reasons.push(reasons::COMPLETENESS_CERTIFICATE_MISMATCH.to_owned());
        }
        if !checker_identity.is_empty() && checker_identity != binding.checker_identity {
            unsafe_reasons.push(reasons::CHECKER_IDENTITY_MISMATCH.to_owned());
        }
        if !checker_version.is_empty() && checker_version != binding.checker_version {
            unsafe_reasons.push(reasons::CHECKER_VERSION_MISMATCH.to_owned());
        }
        if !live.completeness.first_attempt {
            unsafe_reasons.push(reasons::COMPLETENESS_RETRY.to_owned());
        }

        if !unsafe_reasons.is_empty() {
            return ExpandOutcome::Unsafe {
                reasons: sort_dedup(unsafe_reasons),
            };
        }
        if !unknown_reasons.is_empty() {
            return ExpandOutcome::Unknown {
                reasons: sort_dedup(unknown_reasons),
            };
        }
        ExpandOutcome::Safe(ExpandPermit {
            handle_id: handle.handle_id,
            project_root: handle.project_root,
            request_root: handle.request_root,
            protected_scope_root: handle.protected_scope_root,
            demand_plan_root: handle.demand_plan_root,
            index_root: handle.index_root,
            index_version: handle.index_version.clone(),
            renderer_contract: handle.renderer_contract,
            tenant: handle.tenant.clone(),
            epoch: handle.epoch,
            projection_root: handle.projection_root,
        })
    }
}

fn check_binding(equal: bool, reasons: &mut Vec<String>, reason: &'static str) {
    if !equal {
        reasons.push(reason.to_owned());
    }
}

fn sort_dedup(mut reasons: Vec<String>) -> Vec<String> {
    reasons.sort();
    reasons.dedup();
    reasons
}

fn validate_string(field: &'static str, value: &str) -> Result<(), SafeExpandError> {
    if value.is_empty() {
        return Err(SafeExpandError::EmptyString(field));
    }
    if value.len() > MAX_SAFE_EXPAND_STRING_BYTES {
        return Err(SafeExpandError::StringTooLong {
            field,
            actual: value.len(),
            maximum: MAX_SAFE_EXPAND_STRING_BYTES,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(SafeExpandError::ControlCharacter { field });
    }
    Ok(())
}

/// Keyed issuance MAC: HMAC-SHA256 over `SAFE_EXPAND_MAC_DOMAIN ||
/// canonical sealed payload`. The issuer secret is the only way to produce a
/// verifying MAC.
fn compute_issuance_mac(secret: &[u8; 32], payload: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(SAFE_EXPAND_MAC_DOMAIN.len() + payload.len());
    preimage.extend_from_slice(SAFE_EXPAND_MAC_DOMAIN);
    preimage.extend_from_slice(payload);
    hmac_sha256(secret, &preimage)
}

/// HMAC-SHA256 (RFC 2104) without allocations. Keys are exactly 32 bytes,
/// shorter than the 64-byte block, so the RFC's key-stretching branch
/// (keys longer than one block) is unreachable by construction.
fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut key_block = [0u8; BLOCK];
    key_block[..key.len()].copy_from_slice(key);
    let mut ipad = [0u8; BLOCK];
    let mut opad = [0u8; BLOCK];
    for (index, byte) in key_block.iter().enumerate() {
        ipad[index] = byte ^ 0x36;
        opad[index] = byte ^ 0x5c;
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_hash = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_hash);
    outer.finalize().into()
}
