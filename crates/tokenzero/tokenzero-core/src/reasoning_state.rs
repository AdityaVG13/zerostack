//! Opaque reasoning-state transport and protected Decision View headroom.
//!
//! Opaque provider bytes are never parsed, summarized, reordered, or serialized
//! by this module. Metadata binds them to the exact provider/model/backend,
//! reasoning contract, session, position, sampler, and lineage. Exact replay is
//! refused unless every binding matches. Headroom arithmetic delegates to the
//! canonical ZeroStack [`ReasoningContract`] contract.

use crate::decision_view::{DecisionView, DecisionViewIdentity};
use serde::Serialize;
use std::{error::Error, fmt};
use zero_abi::{
    NativeStatePolicy, ReasoningContract, ReasoningContractError, Sha256Digest, sha256,
};

pub const MAX_OPAQUE_REASONING_STATE_BYTES: usize = 16 * 1_048_576;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpaqueReasoningStateKind {
    ProviderReasoningItems,
    SignedThinkingBlocks,
    EncryptedReasoningContent,
    ProviderContinuationId,
    LocalExactStateCartridge,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningContinuationStatus {
    Exact,
    ScopedCertificate,
    Approximate,
    Unavailable,
    Expired,
    Rejected,
    IdentityMismatch,
}

impl ReasoningContinuationStatus {
    const fn carries_payload(self) -> bool {
        matches!(
            self,
            Self::Exact | Self::ScopedCertificate | Self::Approximate
        )
    }
}

#[derive(Debug)]
pub enum ReasoningStateError {
    ReasoningContract(ReasoningContractError),
    ZeroIdentity(&'static str),
    MissingSamplerIdentity,
    EmptyPayload,
    PayloadTooLarge {
        actual: usize,
        limit: usize,
    },
    UnavailableKindHasPayload,
    PayloadStatusRequired,
    ScopedCertificateRequired,
    UnexpectedScopedCertificate,
    InvalidInitialOrder,
    MissingParentDigest,
    InvalidParentDigest,
    InvalidExpiry,
    InputTokenOverflow,
    TokenizerIdentityMismatch,
    ToolSchemaIdentityMismatch,
    NativeStatePolicyMismatch {
        policy: NativeStatePolicy,
        status: ReasoningContinuationStatus,
    },
    BindingMismatch,
    OrderMismatch,
    NotExact(ReasoningContinuationStatus),
    Expired,
    ContentDigestMismatch,
}

impl fmt::Display for ReasoningStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReasoningContract(error) => write!(f, "reasoning contract rejected: {error}"),
            Self::ZeroIdentity(field) => write!(f, "reasoning-state {field} identity is zero"),
            Self::MissingSamplerIdentity => {
                f.write_str("reasoning-state sampler identity is required but absent")
            }
            Self::EmptyPayload => f.write_str("opaque reasoning-state payload is empty"),
            Self::PayloadTooLarge { actual, limit } => write!(
                f,
                "opaque reasoning-state payload is {actual} bytes; limit is {limit}"
            ),
            Self::UnavailableKindHasPayload => {
                f.write_str("unavailable reasoning-state kind cannot carry opaque bytes")
            }
            Self::PayloadStatusRequired => {
                f.write_str("opaque bytes require exact, scoped-certificate, or approximate status")
            }
            Self::ScopedCertificateRequired => {
                f.write_str("scoped continuation status requires a certificate digest")
            }
            Self::UnexpectedScopedCertificate => {
                f.write_str("continuation certificate is present outside scoped status")
            }
            Self::InvalidInitialOrder => {
                f.write_str("initial reasoning state must not name a parent digest")
            }
            Self::MissingParentDigest => {
                f.write_str("noninitial reasoning state requires a parent digest")
            }
            Self::InvalidParentDigest => f.write_str("reasoning-state parent digest is zero"),
            Self::InvalidExpiry => f.write_str("reasoning-state expiry must be nonzero"),
            Self::InputTokenOverflow => {
                f.write_str("Decision View token count exceeds the hub headroom range")
            }
            Self::TokenizerIdentityMismatch => {
                f.write_str("Decision View tokenizer identity differs from reasoning contract")
            }
            Self::ToolSchemaIdentityMismatch => {
                f.write_str("Decision View tool schema differs from reasoning contract")
            }
            Self::NativeStatePolicyMismatch { policy, status } => write!(
                f,
                "reasoning-state status {status:?} is not authorized by native policy {policy:?}"
            ),
            Self::BindingMismatch => {
                f.write_str("reasoning-state replay identity binding does not match")
            }
            Self::OrderMismatch => {
                f.write_str("reasoning-state replay order or parent does not match")
            }
            Self::NotExact(status) => {
                write!(
                    f,
                    "reasoning-state status {status:?} cannot authorize exact replay"
                )
            }
            Self::Expired => f.write_str("reasoning state has expired"),
            Self::ContentDigestMismatch => {
                f.write_str("opaque reasoning-state bytes do not match their digest")
            }
        }
    }
}

impl Error for ReasoningStateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ReasoningContract(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ReasoningContractError> for ReasoningStateError {
    fn from(error: ReasoningContractError) -> Self {
        Self::ReasoningContract(error)
    }
}

/// Complete execution binding for provider-native reasoning bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReasoningStateBinding {
    provider_identity: Sha256Digest,
    model_identity: Sha256Digest,
    backend_identity: Sha256Digest,
    tokenizer_identity: Sha256Digest,
    decoder_identity: Sha256Digest,
    tool_schema_digest: Sha256Digest,
    reasoning_contract_digest: Sha256Digest,
    native_state_policy: NativeStatePolicy,
    position_identity: Sha256Digest,
    session_identity: Sha256Digest,
    sampler_identity_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sampler_identity: Option<Sha256Digest>,
}

impl ReasoningStateBinding {
    pub fn new(
        provider_identity: Sha256Digest,
        contract: &ReasoningContract,
        position_identity: Sha256Digest,
        session_identity: Sha256Digest,
        sampler_identity_required: bool,
        sampler_identity: Option<Sha256Digest>,
    ) -> Result<Self, ReasoningStateError> {
        contract.validate()?;
        for (field, digest) in [
            ("provider", provider_identity),
            ("position", position_identity),
            ("session", session_identity),
        ] {
            nonzero(field, digest)?;
        }
        if sampler_identity_required && sampler_identity.is_none() {
            return Err(ReasoningStateError::MissingSamplerIdentity);
        }
        if let Some(digest) = sampler_identity {
            nonzero("sampler", digest)?;
        }
        Ok(Self {
            provider_identity,
            model_identity: contract.model_identity(),
            backend_identity: contract.backend_identity(),
            tokenizer_identity: contract.tokenizer_identity(),
            decoder_identity: contract.decoder_identity(),
            tool_schema_digest: contract.tool_schema_digest(),
            reasoning_contract_digest: contract.identity_digest()?,
            native_state_policy: contract.native_state_policy(),
            position_identity,
            session_identity,
            sampler_identity_required,
            sampler_identity,
        })
    }

    pub const fn provider_identity(&self) -> Sha256Digest {
        self.provider_identity
    }
    pub const fn model_identity(&self) -> Sha256Digest {
        self.model_identity
    }
    pub const fn backend_identity(&self) -> Sha256Digest {
        self.backend_identity
    }
    pub const fn tokenizer_identity(&self) -> Sha256Digest {
        self.tokenizer_identity
    }
    pub const fn decoder_identity(&self) -> Sha256Digest {
        self.decoder_identity
    }
    pub const fn tool_schema_digest(&self) -> Sha256Digest {
        self.tool_schema_digest
    }
    pub const fn reasoning_contract_digest(&self) -> Sha256Digest {
        self.reasoning_contract_digest
    }
    pub const fn native_state_policy(&self) -> NativeStatePolicy {
        self.native_state_policy
    }
    pub const fn position_identity(&self) -> Sha256Digest {
        self.position_identity
    }
    pub const fn session_identity(&self) -> Sha256Digest {
        self.session_identity
    }
    pub const fn sampler_identity_required(&self) -> bool {
        self.sampler_identity_required
    }
    pub const fn sampler_identity(&self) -> Option<Sha256Digest> {
        self.sampler_identity
    }
}

/// Monotonic provider ordering and exact parent lineage.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ReasoningStateOrder {
    sequence_index: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_content_digest: Option<Sha256Digest>,
}

impl ReasoningStateOrder {
    pub fn new(
        sequence_index: u64,
        parent_content_digest: Option<Sha256Digest>,
    ) -> Result<Self, ReasoningStateError> {
        match (sequence_index, parent_content_digest) {
            (0, Some(_)) => return Err(ReasoningStateError::InvalidInitialOrder),
            (1.., None) => return Err(ReasoningStateError::MissingParentDigest),
            (_, Some(digest)) if digest == Sha256Digest::ZERO => {
                return Err(ReasoningStateError::InvalidParentDigest);
            }
            _ => {}
        }
        Ok(Self {
            sequence_index,
            parent_content_digest,
        })
    }

    pub const fn sequence_index(&self) -> u64 {
        self.sequence_index
    }
    pub const fn parent_content_digest(&self) -> Option<Sha256Digest> {
        self.parent_content_digest
    }
}

/// Serializable metadata only. It never contains provider-native reasoning bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OpaqueReasoningStateRef {
    kind: OpaqueReasoningStateKind,
    status: ReasoningContinuationStatus,
    binding: ReasoningStateBinding,
    order: ReasoningStateOrder,
    content_digest: Sha256Digest,
    byte_len: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    continuation_certificate_digest: Option<Sha256Digest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    valid_until_unix_ms: Option<u64>,
}

impl OpaqueReasoningStateRef {
    pub fn unavailable(binding: ReasoningStateBinding) -> Self {
        Self {
            kind: OpaqueReasoningStateKind::Unavailable,
            status: ReasoningContinuationStatus::Unavailable,
            binding,
            order: ReasoningStateOrder {
                sequence_index: 0,
                parent_content_digest: None,
            },
            content_digest: Sha256Digest::ZERO,
            byte_len: 0,
            continuation_certificate_digest: None,
            valid_until_unix_ms: None,
        }
    }

    pub fn rejected(
        kind: OpaqueReasoningStateKind,
        binding: ReasoningStateBinding,
        order: ReasoningStateOrder,
        content_digest: Sha256Digest,
    ) -> Result<Self, ReasoningStateError> {
        terminal_ref(
            kind,
            ReasoningContinuationStatus::Rejected,
            binding,
            order,
            content_digest,
            None,
        )
    }

    pub fn expired(
        kind: OpaqueReasoningStateKind,
        binding: ReasoningStateBinding,
        order: ReasoningStateOrder,
        content_digest: Sha256Digest,
        valid_until_unix_ms: u64,
    ) -> Result<Self, ReasoningStateError> {
        terminal_ref(
            kind,
            ReasoningContinuationStatus::Expired,
            binding,
            order,
            content_digest,
            Some(valid_until_unix_ms),
        )
    }

    pub fn identity_mismatch(
        kind: OpaqueReasoningStateKind,
        binding: ReasoningStateBinding,
        order: ReasoningStateOrder,
        content_digest: Sha256Digest,
    ) -> Result<Self, ReasoningStateError> {
        terminal_ref(
            kind,
            ReasoningContinuationStatus::IdentityMismatch,
            binding,
            order,
            content_digest,
            None,
        )
    }

    pub const fn kind(&self) -> OpaqueReasoningStateKind {
        self.kind
    }
    pub const fn status(&self) -> ReasoningContinuationStatus {
        self.status
    }
    pub fn binding(&self) -> &ReasoningStateBinding {
        &self.binding
    }
    pub const fn order(&self) -> ReasoningStateOrder {
        self.order
    }
    pub const fn content_digest(&self) -> Sha256Digest {
        self.content_digest
    }
    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }
    pub const fn continuation_certificate_digest(&self) -> Option<Sha256Digest> {
        self.continuation_certificate_digest
    }
    pub const fn valid_until_unix_ms(&self) -> Option<u64> {
        self.valid_until_unix_ms
    }
}

/// In-memory opaque pass-through. `Debug` is redacted and `Serialize` is absent.
pub struct OpaqueReasoningStateEnvelope {
    reference: OpaqueReasoningStateRef,
    opaque_bytes: Vec<u8>,
}

impl OpaqueReasoningStateEnvelope {
    #[allow(clippy::too_many_arguments)]
    pub fn capture(
        kind: OpaqueReasoningStateKind,
        status: ReasoningContinuationStatus,
        binding: ReasoningStateBinding,
        order: ReasoningStateOrder,
        continuation_certificate_digest: Option<Sha256Digest>,
        valid_until_unix_ms: Option<u64>,
        opaque_bytes: Vec<u8>,
    ) -> Result<Self, ReasoningStateError> {
        if kind == OpaqueReasoningStateKind::Unavailable {
            return Err(ReasoningStateError::UnavailableKindHasPayload);
        }
        if !status.carries_payload() {
            return Err(ReasoningStateError::PayloadStatusRequired);
        }
        if opaque_bytes.is_empty() {
            return Err(ReasoningStateError::EmptyPayload);
        }
        if opaque_bytes.len() > MAX_OPAQUE_REASONING_STATE_BYTES {
            return Err(ReasoningStateError::PayloadTooLarge {
                actual: opaque_bytes.len(),
                limit: MAX_OPAQUE_REASONING_STATE_BYTES,
            });
        }
        validate_certificate(status, continuation_certificate_digest)?;
        validate_native_state_policy(binding.native_state_policy(), status)?;
        validate_expiry(valid_until_unix_ms)?;
        let byte_len = u64::try_from(opaque_bytes.len()).map_err(|_| {
            ReasoningStateError::PayloadTooLarge {
                actual: opaque_bytes.len(),
                limit: MAX_OPAQUE_REASONING_STATE_BYTES,
            }
        })?;
        let reference = OpaqueReasoningStateRef {
            kind,
            status,
            binding,
            order,
            content_digest: digest(&opaque_bytes),
            byte_len,
            continuation_certificate_digest,
            valid_until_unix_ms,
        };
        Ok(Self {
            reference,
            opaque_bytes,
        })
    }

    pub fn reference(&self) -> &OpaqueReasoningStateRef {
        &self.reference
    }

    /// Exact original provider bytes, with no parse/rewrite step.
    ///
    /// This accessor does not upgrade continuation status. Strict replay must
    /// use [`Self::exact_replay_bytes`].
    pub fn opaque_bytes(&self) -> &[u8] {
        &self.opaque_bytes
    }

    pub fn exact_replay_bytes(
        &self,
        expected_binding: &ReasoningStateBinding,
        expected_order: ReasoningStateOrder,
        now_unix_ms: u64,
    ) -> Result<&[u8], ReasoningStateError> {
        if self.reference.status != ReasoningContinuationStatus::Exact {
            return Err(ReasoningStateError::NotExact(self.reference.status));
        }
        validate_native_state_policy(
            self.reference.binding.native_state_policy(),
            self.reference.status,
        )?;
        if &self.reference.binding != expected_binding {
            return Err(ReasoningStateError::BindingMismatch);
        }
        if self.reference.order != expected_order {
            return Err(ReasoningStateError::OrderMismatch);
        }
        if self
            .reference
            .valid_until_unix_ms
            .is_some_and(|expiry| now_unix_ms >= expiry)
        {
            return Err(ReasoningStateError::Expired);
        }
        if digest(&self.opaque_bytes) != self.reference.content_digest {
            return Err(ReasoningStateError::ContentDigestMismatch);
        }
        Ok(&self.opaque_bytes)
    }
}

impl fmt::Debug for OpaqueReasoningStateEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpaqueReasoningStateEnvelope")
            .field("reference", &self.reference)
            .field(
                "opaque_bytes",
                &format_args!("<redacted:{} bytes>", self.opaque_bytes.len()),
            )
            .finish()
    }
}

/// Exactness class for one model-state continuation assessment.
///
/// Only `ExactNeutral` describes identical native continuation state. Scoped
/// and empirical evidence stay non-pointwise and never authorize exact replay.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "class")]
enum ModelStateContinuationClass {
    ExactNeutral {
        state_content_digest: Sha256Digest,
    },
    ScopedCertificate {
        state_content_digest: Sha256Digest,
        certificate_digest: Sha256Digest,
        declared_scope_digest: Sha256Digest,
    },
    Empirical {
        state_content_digest: Sha256Digest,
        frozen_distribution_digest: Sha256Digest,
        evaluation_receipt_digest: Sha256Digest,
        declared_scope_digest: Sha256Digest,
        evidence_valid_until_unix_ms: Option<u64>,
    },
    Unavailable {
        reason: ModelStateUnavailableReason,
    },
}

/// Public discriminant for the validated continuation class. It is descriptive
/// only; the unforgeable receipt is `ModelStateContinuationAssessment`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStateContinuationKind {
    ExactNeutral,
    ScopedCertificate,
    Empirical,
    Unavailable,
}

impl ModelStateContinuationClass {
    const fn kind(&self) -> ModelStateContinuationKind {
        match self {
            Self::ExactNeutral { .. } => ModelStateContinuationKind::ExactNeutral,
            Self::ScopedCertificate { .. } => ModelStateContinuationKind::ScopedCertificate,
            Self::Empirical { .. } => ModelStateContinuationKind::Empirical,
            Self::Unavailable { .. } => ModelStateContinuationKind::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStateUnavailableReason {
    ProviderUnavailable,
    StateExpired,
    StateRejected,
    IdentityMismatch,
    ScopedEvidenceAbsent,
    EmpiricalEvidenceAbsent,
    EmpiricalEvidenceExpired,
}

/// Evidence supplied with an assessment. Evidence can only preserve the
/// class already declared by the validated opaque-state reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelStateContinuationEvidence {
    None,
    Scoped {
        certificate_digest: Sha256Digest,
        declared_scope_digest: Sha256Digest,
    },
    Empirical {
        frozen_distribution_digest: Sha256Digest,
        evaluation_receipt_digest: Sha256Digest,
        declared_scope_digest: Sha256Digest,
        valid_until_unix_ms: Option<u64>,
    },
}

impl ModelStateContinuationEvidence {
    pub fn scoped(
        certificate_digest: Sha256Digest,
        declared_scope_digest: Sha256Digest,
    ) -> Result<Self, ModelStateContinuationError> {
        require_continuation_digest("certificate", certificate_digest)?;
        require_continuation_digest("declared scope", declared_scope_digest)?;
        Ok(Self::Scoped {
            certificate_digest,
            declared_scope_digest,
        })
    }

    pub fn empirical(
        frozen_distribution_digest: Sha256Digest,
        evaluation_receipt_digest: Sha256Digest,
        declared_scope_digest: Sha256Digest,
        valid_until_unix_ms: Option<u64>,
    ) -> Result<Self, ModelStateContinuationError> {
        require_continuation_digest("frozen distribution", frozen_distribution_digest)?;
        require_continuation_digest("evaluation receipt", evaluation_receipt_digest)?;
        require_continuation_digest("declared scope", declared_scope_digest)?;
        if valid_until_unix_ms == Some(0) {
            return Err(ModelStateContinuationError::InvalidEvidenceExpiry);
        }
        Ok(Self::Empirical {
            frozen_distribution_digest,
            evaluation_receipt_digest,
            declared_scope_digest,
            valid_until_unix_ms,
        })
    }
}

/// Serializable raw Decision View recovery metadata. The exact bytes live only
/// in `RawDecisionViewRecoveryEnvelope` and never enter this receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RawDecisionViewRecoveryRef {
    decision_view_identity: DecisionViewIdentity,
    decision_view_digest: Sha256Digest,
    exact_token_map_digest: Sha256Digest,
    raw_bytes_digest: Sha256Digest,
    raw_byte_len: u64,
    total_tokens: u64,
    caller_raw_baseline_identity_digest: Sha256Digest,
    caller_hub_safepoint_digest: Sha256Digest,
}

impl RawDecisionViewRecoveryRef {
    pub fn decision_view_identity(&self) -> &DecisionViewIdentity {
        &self.decision_view_identity
    }

    pub const fn decision_view_digest(&self) -> Sha256Digest {
        self.decision_view_digest
    }

    pub const fn exact_token_map_digest(&self) -> Sha256Digest {
        self.exact_token_map_digest
    }

    pub const fn raw_bytes_digest(&self) -> Sha256Digest {
        self.raw_bytes_digest
    }

    pub const fn raw_byte_len(&self) -> u64 {
        self.raw_byte_len
    }

    pub const fn total_tokens(&self) -> u64 {
        self.total_tokens
    }

    pub const fn caller_raw_baseline_identity_digest(&self) -> Sha256Digest {
        self.caller_raw_baseline_identity_digest
    }

    pub const fn caller_hub_safepoint_digest(&self) -> Sha256Digest {
        self.caller_hub_safepoint_digest
    }
}

/// In-memory exact raw Decision View carrier for guarded fallback.
///
/// This type does not verify or create hub safepoints, persist CAS objects, or
/// trigger deoptimization. It only binds caller-supplied hub identities to the
/// exact canonical Decision View bytes and checks them before recovery.
pub struct RawDecisionViewRecoveryEnvelope {
    reference: RawDecisionViewRecoveryRef,
    raw_decision_view_bytes: Vec<u8>,
}

impl RawDecisionViewRecoveryEnvelope {
    pub fn capture(
        decision_view: &DecisionView,
        caller_raw_baseline_identity_digest: Sha256Digest,
        caller_hub_safepoint_digest: Sha256Digest,
    ) -> Result<Self, ModelStateContinuationError> {
        require_continuation_digest(
            "caller raw-baseline identity",
            caller_raw_baseline_identity_digest,
        )?;
        require_continuation_digest("caller hub safepoint", caller_hub_safepoint_digest)?;
        let raw_decision_view_bytes = decision_view.rendered().to_vec();
        let raw_byte_len = u64::try_from(raw_decision_view_bytes.len())
            .map_err(|_| ModelStateContinuationError::RawByteLengthOverflow)?;
        let reference = RawDecisionViewRecoveryRef {
            decision_view_identity: decision_view.identity().clone(),
            decision_view_digest: decision_view.digest(),
            exact_token_map_digest: decision_view.exact_token_map_digest(),
            raw_bytes_digest: digest(&raw_decision_view_bytes),
            raw_byte_len,
            total_tokens: decision_view.total_tokens(),
            caller_raw_baseline_identity_digest,
            caller_hub_safepoint_digest,
        };
        Ok(Self {
            reference,
            raw_decision_view_bytes,
        })
    }

    pub fn reference(&self) -> &RawDecisionViewRecoveryRef {
        &self.reference
    }

    #[allow(clippy::too_many_arguments)]
    pub fn exact_raw_decision_view_bytes(
        &self,
        expected_decision_view_identity: &DecisionViewIdentity,
        expected_decision_view_digest: Sha256Digest,
        expected_exact_token_map_digest: Sha256Digest,
        expected_raw_baseline_identity_digest: Sha256Digest,
        expected_hub_safepoint_digest: Sha256Digest,
    ) -> Result<&[u8], ModelStateContinuationError> {
        if &self.reference.decision_view_identity != expected_decision_view_identity {
            return Err(ModelStateContinuationError::DecisionViewIdentityMismatch);
        }
        if self.reference.decision_view_digest != expected_decision_view_digest {
            return Err(ModelStateContinuationError::DecisionViewDigestMismatch);
        }
        if self.reference.exact_token_map_digest != expected_exact_token_map_digest {
            return Err(ModelStateContinuationError::ExactTokenMapDigestMismatch);
        }
        if self.reference.caller_raw_baseline_identity_digest
            != expected_raw_baseline_identity_digest
        {
            return Err(ModelStateContinuationError::RawBaselineIdentityMismatch);
        }
        if self.reference.caller_hub_safepoint_digest != expected_hub_safepoint_digest {
            return Err(ModelStateContinuationError::HubSafepointDigestMismatch);
        }
        let actual_len = u64::try_from(self.raw_decision_view_bytes.len())
            .map_err(|_| ModelStateContinuationError::RawByteLengthOverflow)?;
        if actual_len != self.reference.raw_byte_len {
            return Err(ModelStateContinuationError::RawByteLengthMismatch);
        }
        if digest(&self.raw_decision_view_bytes) != self.reference.raw_bytes_digest {
            return Err(ModelStateContinuationError::RawBytesDigestMismatch);
        }
        Ok(&self.raw_decision_view_bytes)
    }
}

impl fmt::Debug for RawDecisionViewRecoveryEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawDecisionViewRecoveryEnvelope")
            .field("reference", &self.reference)
            .field(
                "raw_decision_view_bytes",
                &format_args!("<redacted:{} bytes>", self.raw_decision_view_bytes.len()),
            )
            .finish()
    }
}

/// Receipt-visible continuation classification plus an exact raw fallback.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModelStateContinuationAssessment {
    state_reference: OpaqueReasoningStateRef,
    class: ModelStateContinuationClass,
    raw_recovery: RawDecisionViewRecoveryRef,
}

impl ModelStateContinuationAssessment {
    pub fn assess(
        state_reference: &OpaqueReasoningStateRef,
        evidence: ModelStateContinuationEvidence,
        raw_recovery: &RawDecisionViewRecoveryRef,
        now_unix_ms: u64,
    ) -> Result<Self, ModelStateContinuationError> {
        validate_continuation_evidence_for_state(state_reference, &evidence)?;
        let view_identity = raw_recovery.decision_view_identity();
        if state_reference.binding().tokenizer_identity()
            != view_identity.tokenizer_identity_digest()
        {
            return Err(ModelStateContinuationError::TokenizerIdentityMismatch);
        }
        if state_reference.binding().tool_schema_digest() != view_identity.tool_schema_digest() {
            return Err(ModelStateContinuationError::ToolSchemaIdentityMismatch);
        }
        let expired = state_reference
            .valid_until_unix_ms()
            .is_some_and(|expiry| now_unix_ms >= expiry);
        let class = if expired {
            ModelStateContinuationClass::Unavailable {
                reason: ModelStateUnavailableReason::StateExpired,
            }
        } else {
            match (state_reference.status(), evidence) {
                (ReasoningContinuationStatus::Exact, ModelStateContinuationEvidence::None) => {
                    ModelStateContinuationClass::ExactNeutral {
                        state_content_digest: state_reference.content_digest(),
                    }
                }
                (
                    ReasoningContinuationStatus::ScopedCertificate,
                    ModelStateContinuationEvidence::Scoped {
                        certificate_digest,
                        declared_scope_digest,
                    },
                ) => {
                    if state_reference.continuation_certificate_digest() != Some(certificate_digest)
                    {
                        return Err(ModelStateContinuationError::ScopedCertificateMismatch);
                    }
                    ModelStateContinuationClass::ScopedCertificate {
                        state_content_digest: state_reference.content_digest(),
                        certificate_digest,
                        declared_scope_digest,
                    }
                }
                (
                    ReasoningContinuationStatus::ScopedCertificate,
                    ModelStateContinuationEvidence::None,
                ) => ModelStateContinuationClass::Unavailable {
                    reason: ModelStateUnavailableReason::ScopedEvidenceAbsent,
                },
                (
                    ReasoningContinuationStatus::Approximate,
                    ModelStateContinuationEvidence::Empirical {
                        frozen_distribution_digest: _,
                        evaluation_receipt_digest: _,
                        declared_scope_digest: _,
                        valid_until_unix_ms,
                    },
                ) if valid_until_unix_ms.is_some_and(|expiry| now_unix_ms >= expiry) => {
                    ModelStateContinuationClass::Unavailable {
                        reason: ModelStateUnavailableReason::EmpiricalEvidenceExpired,
                    }
                }
                (
                    ReasoningContinuationStatus::Approximate,
                    ModelStateContinuationEvidence::Empirical {
                        frozen_distribution_digest,
                        evaluation_receipt_digest,
                        declared_scope_digest,
                        valid_until_unix_ms,
                    },
                ) => ModelStateContinuationClass::Empirical {
                    state_content_digest: state_reference.content_digest(),
                    frozen_distribution_digest,
                    evaluation_receipt_digest,
                    declared_scope_digest,
                    evidence_valid_until_unix_ms: valid_until_unix_ms,
                },
                (
                    ReasoningContinuationStatus::Approximate,
                    ModelStateContinuationEvidence::None,
                ) => ModelStateContinuationClass::Unavailable {
                    reason: ModelStateUnavailableReason::EmpiricalEvidenceAbsent,
                },
                (
                    ReasoningContinuationStatus::Unavailable,
                    ModelStateContinuationEvidence::None,
                ) => ModelStateContinuationClass::Unavailable {
                    reason: ModelStateUnavailableReason::ProviderUnavailable,
                },
                (ReasoningContinuationStatus::Expired, ModelStateContinuationEvidence::None) => {
                    ModelStateContinuationClass::Unavailable {
                        reason: ModelStateUnavailableReason::StateExpired,
                    }
                }
                (ReasoningContinuationStatus::Rejected, ModelStateContinuationEvidence::None) => {
                    ModelStateContinuationClass::Unavailable {
                        reason: ModelStateUnavailableReason::StateRejected,
                    }
                }
                (
                    ReasoningContinuationStatus::IdentityMismatch,
                    ModelStateContinuationEvidence::None,
                ) => ModelStateContinuationClass::Unavailable {
                    reason: ModelStateUnavailableReason::IdentityMismatch,
                },
                _ => return Err(ModelStateContinuationError::EvidenceStatusMismatch),
            }
        };
        Ok(Self {
            state_reference: state_reference.clone(),
            class,
            raw_recovery: raw_recovery.clone(),
        })
    }

    pub fn state_reference(&self) -> &OpaqueReasoningStateRef {
        &self.state_reference
    }

    pub const fn class(&self) -> ModelStateContinuationKind {
        self.class.kind()
    }

    pub const fn unavailable_reason(&self) -> Option<ModelStateUnavailableReason> {
        match self.class {
            ModelStateContinuationClass::Unavailable { reason } => Some(reason),
            _ => None,
        }
    }

    pub const fn scoped_evidence(&self) -> Option<(Sha256Digest, Sha256Digest)> {
        match self.class {
            ModelStateContinuationClass::ScopedCertificate {
                certificate_digest,
                declared_scope_digest,
                ..
            } => Some((certificate_digest, declared_scope_digest)),
            _ => None,
        }
    }

    pub const fn empirical_evidence(
        &self,
    ) -> Option<(Sha256Digest, Sha256Digest, Sha256Digest, Option<u64>)> {
        match self.class {
            ModelStateContinuationClass::Empirical {
                frozen_distribution_digest,
                evaluation_receipt_digest,
                declared_scope_digest,
                evidence_valid_until_unix_ms,
                ..
            } => Some((
                frozen_distribution_digest,
                evaluation_receipt_digest,
                declared_scope_digest,
                evidence_valid_until_unix_ms,
            )),
            _ => None,
        }
    }

    pub fn raw_recovery(&self) -> &RawDecisionViewRecoveryRef {
        &self.raw_recovery
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelStateContinuationError {
    ZeroIdentity(&'static str),
    InvalidEvidenceExpiry,
    EvidenceStatusMismatch,
    ScopedCertificateMismatch,
    TokenizerIdentityMismatch,
    ToolSchemaIdentityMismatch,
    RawByteLengthOverflow,
    DecisionViewIdentityMismatch,
    DecisionViewDigestMismatch,
    ExactTokenMapDigestMismatch,
    RawBaselineIdentityMismatch,
    HubSafepointDigestMismatch,
    RawByteLengthMismatch,
    RawBytesDigestMismatch,
}

impl fmt::Display for ModelStateContinuationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroIdentity(field) => {
                write!(f, "model-state continuation {field} digest is zero")
            }
            Self::InvalidEvidenceExpiry => {
                f.write_str("model-state empirical evidence expiry must be nonzero")
            }
            Self::EvidenceStatusMismatch => {
                f.write_str("model-state evidence does not match the declared continuation status")
            }
            Self::ScopedCertificateMismatch => f.write_str(
                "model-state scoped evidence certificate differs from the state reference",
            ),
            Self::TokenizerIdentityMismatch => f.write_str(
                "raw Decision View tokenizer identity differs from the reasoning-state binding",
            ),
            Self::ToolSchemaIdentityMismatch => f.write_str(
                "raw Decision View tool schema differs from the reasoning-state binding",
            ),
            Self::RawByteLengthOverflow => {
                f.write_str("raw Decision View byte length exceeds the receipt range")
            }
            Self::DecisionViewIdentityMismatch => {
                f.write_str("raw Decision View identity differs from the expected identity")
            }
            Self::DecisionViewDigestMismatch => {
                f.write_str("raw Decision View digest differs from the expected digest")
            }
            Self::ExactTokenMapDigestMismatch => {
                f.write_str("raw Decision View token map differs from the expected token map")
            }
            Self::RawBaselineIdentityMismatch => {
                f.write_str("caller raw-baseline identity differs from the recovery binding")
            }
            Self::HubSafepointDigestMismatch => {
                f.write_str("caller hub safepoint digest differs from the recovery binding")
            }
            Self::RawByteLengthMismatch => {
                f.write_str("raw Decision View byte length differs from recovery metadata")
            }
            Self::RawBytesDigestMismatch => {
                f.write_str("raw Decision View bytes differ from recovery metadata")
            }
        }
    }
}

impl Error for ModelStateContinuationError {}

fn validate_continuation_evidence_for_state(
    state_reference: &OpaqueReasoningStateRef,
    evidence: &ModelStateContinuationEvidence,
) -> Result<(), ModelStateContinuationError> {
    match evidence {
        ModelStateContinuationEvidence::None => {}
        ModelStateContinuationEvidence::Scoped {
            certificate_digest,
            declared_scope_digest,
        } => {
            require_continuation_digest("certificate", *certificate_digest)?;
            require_continuation_digest("declared scope", *declared_scope_digest)?;
        }
        ModelStateContinuationEvidence::Empirical {
            frozen_distribution_digest,
            evaluation_receipt_digest,
            declared_scope_digest,
            valid_until_unix_ms,
        } => {
            require_continuation_digest("frozen distribution", *frozen_distribution_digest)?;
            require_continuation_digest("evaluation receipt", *evaluation_receipt_digest)?;
            require_continuation_digest("declared scope", *declared_scope_digest)?;
            if *valid_until_unix_ms == Some(0) {
                return Err(ModelStateContinuationError::InvalidEvidenceExpiry);
            }
        }
    }
    match (state_reference.status(), evidence) {
        (ReasoningContinuationStatus::Exact, ModelStateContinuationEvidence::None)
        | (ReasoningContinuationStatus::ScopedCertificate, ModelStateContinuationEvidence::None)
        | (ReasoningContinuationStatus::Approximate, ModelStateContinuationEvidence::None)
        | (
            ReasoningContinuationStatus::Approximate,
            ModelStateContinuationEvidence::Empirical { .. },
        )
        | (
            ReasoningContinuationStatus::Unavailable
            | ReasoningContinuationStatus::Expired
            | ReasoningContinuationStatus::Rejected
            | ReasoningContinuationStatus::IdentityMismatch,
            ModelStateContinuationEvidence::None,
        ) => Ok(()),
        (
            ReasoningContinuationStatus::ScopedCertificate,
            ModelStateContinuationEvidence::Scoped {
                certificate_digest, ..
            },
        ) if state_reference.continuation_certificate_digest() == Some(*certificate_digest) => {
            Ok(())
        }
        (
            ReasoningContinuationStatus::ScopedCertificate,
            ModelStateContinuationEvidence::Scoped { .. },
        ) => Err(ModelStateContinuationError::ScopedCertificateMismatch),
        _ => Err(ModelStateContinuationError::EvidenceStatusMismatch),
    }
}

fn require_continuation_digest(
    field: &'static str,
    digest: Sha256Digest,
) -> Result<(), ModelStateContinuationError> {
    if digest == Sha256Digest::ZERO {
        Err(ModelStateContinuationError::ZeroIdentity(field))
    } else {
        Ok(())
    }
}

/// Receipt-visible proof that input rendering preserved all protected reserves.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DecisionViewHeadroomPlan {
    reasoning_contract_digest: Sha256Digest,
    decision_view_digest: Sha256Digest,
    exact_token_map_digest: Sha256Digest,
    context_capacity: u32,
    logical_input_tokens: u32,
    max_output_tokens: u32,
    reserved_reasoning_tokens: u32,
    reserved_visible_output_tokens: u32,
    reserved_recovery_tokens: u32,
    reserved_tool_tokens: u32,
    admitted_input_ceiling: u32,
    remaining_input_headroom: u32,
}

impl DecisionViewHeadroomPlan {
    pub fn plan(
        contract: &ReasoningContract,
        context_capacity: u32,
        reserved_tool_tokens: u32,
        view: &DecisionView,
    ) -> Result<Self, ReasoningStateError> {
        contract.validate()?;
        if contract.tokenizer_identity() != view.identity().tokenizer_identity_digest() {
            return Err(ReasoningStateError::TokenizerIdentityMismatch);
        }
        if contract.tool_schema_digest() != view.identity().tool_schema_digest() {
            return Err(ReasoningStateError::ToolSchemaIdentityMismatch);
        }
        let logical_input_tokens = u32::try_from(view.total_tokens())
            .map_err(|_| ReasoningStateError::InputTokenOverflow)?;
        let admitted_input_ceiling =
            contract.admitted_input_ceiling(context_capacity, reserved_tool_tokens)?;
        let remaining_input_headroom =
            contract.admit_input(context_capacity, reserved_tool_tokens, logical_input_tokens)?;
        Ok(Self {
            reasoning_contract_digest: contract.identity_digest()?,
            decision_view_digest: view.digest(),
            exact_token_map_digest: view.exact_token_map_digest(),
            context_capacity,
            logical_input_tokens,
            max_output_tokens: contract.max_output_tokens(),
            reserved_reasoning_tokens: contract.reserved_reasoning_tokens(),
            reserved_visible_output_tokens: contract.reserved_visible_output_tokens(),
            reserved_recovery_tokens: contract.reserved_recovery_tokens(),
            reserved_tool_tokens,
            admitted_input_ceiling,
            remaining_input_headroom,
        })
    }

    pub const fn reasoning_contract_digest(&self) -> Sha256Digest {
        self.reasoning_contract_digest
    }
    pub const fn decision_view_digest(&self) -> Sha256Digest {
        self.decision_view_digest
    }
    pub const fn exact_token_map_digest(&self) -> Sha256Digest {
        self.exact_token_map_digest
    }
    pub const fn context_capacity(&self) -> u32 {
        self.context_capacity
    }
    pub const fn logical_input_tokens(&self) -> u32 {
        self.logical_input_tokens
    }
    pub const fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }
    pub const fn reserved_reasoning_tokens(&self) -> u32 {
        self.reserved_reasoning_tokens
    }
    pub const fn reserved_visible_output_tokens(&self) -> u32 {
        self.reserved_visible_output_tokens
    }
    pub const fn reserved_recovery_tokens(&self) -> u32 {
        self.reserved_recovery_tokens
    }
    pub const fn reserved_tool_tokens(&self) -> u32 {
        self.reserved_tool_tokens
    }
    pub const fn admitted_input_ceiling(&self) -> u32 {
        self.admitted_input_ceiling
    }
    pub const fn remaining_input_headroom(&self) -> u32 {
        self.remaining_input_headroom
    }
}

fn terminal_ref(
    kind: OpaqueReasoningStateKind,
    status: ReasoningContinuationStatus,
    binding: ReasoningStateBinding,
    order: ReasoningStateOrder,
    content_digest: Sha256Digest,
    valid_until_unix_ms: Option<u64>,
) -> Result<OpaqueReasoningStateRef, ReasoningStateError> {
    if kind == OpaqueReasoningStateKind::Unavailable {
        return Err(ReasoningStateError::UnavailableKindHasPayload);
    }
    nonzero("content", content_digest)?;
    validate_expiry(valid_until_unix_ms)?;
    Ok(OpaqueReasoningStateRef {
        kind,
        status,
        binding,
        order,
        content_digest,
        byte_len: 0,
        continuation_certificate_digest: None,
        valid_until_unix_ms,
    })
}

fn validate_native_state_policy(
    policy: NativeStatePolicy,
    status: ReasoningContinuationStatus,
) -> Result<(), ReasoningStateError> {
    let authorized = matches!(
        (policy, status),
        (
            NativeStatePolicy::ExactRequired | NativeStatePolicy::ExactIfAvailable,
            ReasoningContinuationStatus::Exact
        ) | (
            NativeStatePolicy::ExactIfAvailable,
            ReasoningContinuationStatus::Approximate
        ) | (
            NativeStatePolicy::ScopedCertificate,
            ReasoningContinuationStatus::ScopedCertificate
        )
    );
    if authorized {
        Ok(())
    } else {
        Err(ReasoningStateError::NativeStatePolicyMismatch { policy, status })
    }
}

fn validate_certificate(
    status: ReasoningContinuationStatus,
    certificate: Option<Sha256Digest>,
) -> Result<(), ReasoningStateError> {
    match (status, certificate) {
        (ReasoningContinuationStatus::ScopedCertificate, None) => {
            Err(ReasoningStateError::ScopedCertificateRequired)
        }
        (ReasoningContinuationStatus::ScopedCertificate, Some(digest)) => {
            nonzero("continuation certificate", digest)
        }
        (_, Some(_)) => Err(ReasoningStateError::UnexpectedScopedCertificate),
        _ => Ok(()),
    }
}

fn validate_expiry(expiry: Option<u64>) -> Result<(), ReasoningStateError> {
    if expiry == Some(0) {
        Err(ReasoningStateError::InvalidExpiry)
    } else {
        Ok(())
    }
}

fn nonzero(field: &'static str, digest: Sha256Digest) -> Result<(), ReasoningStateError> {
    if digest == Sha256Digest::ZERO {
        Err(ReasoningStateError::ZeroIdentity(field))
    } else {
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(bytes))
}

