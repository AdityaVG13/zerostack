//! Frozen same-model reasoning identity and strict no-downshift admission.
//!
//! This contract is a comparison identity. It does not infer quality from token
//! counts, cache hits, or shorter traces. Strict admission keeps semantic
//! identities and provider policy exact and permits only nondecreasing token
//! ceilings/reserves. `decoder_identity` commits resolved sampling, randomness,
//! context behavior, and provider defaults; an unresolved label is not valid authority.

use std::{collections::BTreeMap, error::Error, fmt};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{DigestV1, canonical_json, sha256};

pub const REASONING_CONTRACT_SCHEMA_VERSION_V1: &str = "racc-r-reasoning-contract/v1";
pub const REASONING_CONTRACT_VERSION_V1: u16 = 1;
pub const REASONING_CONTRACT_MAX_CANONICAL_BYTES_V1: usize = 32 * 1024;
pub const REASONING_CONTRACT_MAX_EXTENSION_BYTES_V1: usize = 8 * 1024;
pub const REASONING_CONTRACT_MAX_EXTENSION_NODES_V1: usize = 256;
pub const REASONING_CONTRACT_MAX_EXTENSION_DEPTH_V1: usize = 16;
pub const REASONING_CONTRACT_MAX_ID_BYTES_V1: usize = 128;

const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.reasoning_contract.contract.v1\0";
const INSTANCE_DOMAIN_V1: &[u8] = b"zerostack.reasoning_contract.instance.v1\0";
const ADMISSION_DOMAIN_V1: &[u8] = b"zerostack.reasoning_contract.admission.v1\0";
const SCHEMA_DOMAIN_V1: &[u8] = b"zerostack.reasoning_contract.schema.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NativeStatePolicyV1 {
    ExactRequired,
    ExactIfAvailable,
    CleanRestart,
    ScopedCertificate,
    Unavailable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReasoningContractV1 {
    schema_version: String,
    model_identity: DigestV1,
    backend_identity: DigestV1,
    tokenizer_identity: DigestV1,
    decoder_identity: DigestV1,
    tool_schema_digest: DigestV1,
    reasoning_mode: String,
    reasoning_effort: String,
    max_output_tokens: u32,
    reserved_reasoning_tokens: u32,
    reserved_visible_output_tokens: u32,
    reserved_recovery_tokens: u32,
    native_state_policy: NativeStatePolicyV1,
    allow_effort_downshift: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    provider_extension: BTreeMap<String, Value>,
}

impl ReasoningContractV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model_identity: DigestV1,
        backend_identity: DigestV1,
        tokenizer_identity: DigestV1,
        decoder_identity: DigestV1,
        tool_schema_digest: DigestV1,
        reasoning_mode: impl Into<String>,
        reasoning_effort: impl Into<String>,
        max_output_tokens: u32,
        reserved_reasoning_tokens: u32,
        reserved_visible_output_tokens: u32,
        reserved_recovery_tokens: u32,
        native_state_policy: NativeStatePolicyV1,
        allow_effort_downshift: bool,
        provider_extension: BTreeMap<String, Value>,
    ) -> Result<Self, ReasoningContractErrorV1> {
        let contract = Self {
            schema_version: REASONING_CONTRACT_SCHEMA_VERSION_V1.into(),
            model_identity,
            backend_identity,
            tokenizer_identity,
            decoder_identity,
            tool_schema_digest,
            reasoning_mode: reasoning_mode.into(),
            reasoning_effort: reasoning_effort.into(),
            max_output_tokens,
            reserved_reasoning_tokens,
            reserved_visible_output_tokens,
            reserved_recovery_tokens,
            native_state_policy,
            allow_effort_downshift,
            provider_extension,
        };
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<(), ReasoningContractErrorV1> {
        if self.schema_version != REASONING_CONTRACT_SCHEMA_VERSION_V1 {
            return Err(ReasoningContractErrorV1::new(
                ReasoningContractFailureCodeV1::SchemaVersionMismatch,
                "reasoning contract schema version is not v1",
            ));
        }
        for (label, digest) in [
            ("model identity", self.model_identity),
            ("backend identity", self.backend_identity),
            ("tokenizer identity", self.tokenizer_identity),
            ("decoder identity", self.decoder_identity),
            ("tool schema", self.tool_schema_digest),
        ] {
            if digest == DigestV1::ZERO {
                return Err(ReasoningContractErrorV1::new(
                    ReasoningContractFailureCodeV1::MissingIdentity,
                    format!("{label} digest is zero"),
                ));
            }
        }
        validate_id("reasoning mode", &self.reasoning_mode)?;
        validate_id("reasoning effort", &self.reasoning_effort)?;
        if self.max_output_tokens == 0
            || self.reserved_visible_output_tokens > self.max_output_tokens
        {
            return Err(ReasoningContractErrorV1::new(
                ReasoningContractFailureCodeV1::InvalidTokenReservation,
                "max output must be nonzero and cover the reserved visible output",
            ));
        }
        validate_provider_extension(&self.provider_extension)?;
        if self.canonical_bytes_unchecked()?.len() > REASONING_CONTRACT_MAX_CANONICAL_BYTES_V1 {
            return Err(ReasoningContractErrorV1::new(
                ReasoningContractFailureCodeV1::CanonicalPayloadTooLarge,
                "reasoning contract exceeds its canonical byte bound",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReasoningContractErrorV1> {
        self.validate()?;
        self.canonical_bytes_unchecked()
    }

    fn canonical_bytes_unchecked(&self) -> Result<Vec<u8>, ReasoningContractErrorV1> {
        let value = serde_json::to_value(self).map_err(json_error)?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReasoningContractErrorV1> {
        if bytes.len() > REASONING_CONTRACT_MAX_CANONICAL_BYTES_V1 {
            return Err(ReasoningContractErrorV1::new(
                ReasoningContractFailureCodeV1::CanonicalPayloadTooLarge,
                "reasoning contract exceeds its canonical byte bound",
            ));
        }
        let contract: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        contract.validate()?;
        if contract.canonical_bytes_unchecked()? != bytes {
            return Err(ReasoningContractErrorV1::new(
                ReasoningContractFailureCodeV1::NonCanonicalEncoding,
                "reasoning contract is not canonical sorted-key JSON",
            ));
        }
        Ok(contract)
    }

    pub fn identity_digest(&self) -> Result<DigestV1, ReasoningContractErrorV1> {
        Ok(domain_digest(INSTANCE_DOMAIN_V1, &self.canonical_bytes()?))
    }

    pub fn admitted_input_ceiling(
        &self,
        context_capacity: u32,
        reserved_tool_tokens: u32,
    ) -> Result<u32, ReasoningContractErrorV1> {
        self.validate()?;
        context_capacity
            .checked_sub(self.reserved_reasoning_tokens)
            .and_then(|value| value.checked_sub(self.reserved_visible_output_tokens))
            .and_then(|value| value.checked_sub(self.reserved_recovery_tokens))
            .and_then(|value| value.checked_sub(reserved_tool_tokens))
            .ok_or_else(|| {
                ReasoningContractErrorV1::new(
                    ReasoningContractFailureCodeV1::ContextCapacityExceeded,
                    "reasoning, output, recovery, and tool reserves exceed context capacity",
                )
            })
    }

    pub fn admit_input(
        &self,
        context_capacity: u32,
        reserved_tool_tokens: u32,
        logical_input_tokens: u32,
    ) -> Result<u32, ReasoningContractErrorV1> {
        let ceiling = self.admitted_input_ceiling(context_capacity, reserved_tool_tokens)?;
        if logical_input_tokens > ceiling {
            return Err(ReasoningContractErrorV1::new(
                ReasoningContractFailureCodeV1::InputExceedsHeadroom,
                "logical input consumes protected reasoning, output, tool, or recovery headroom",
            ));
        }
        Ok(ceiling - logical_input_tokens)
    }

    pub const fn model_identity(&self) -> DigestV1 {
        self.model_identity
    }
    pub const fn backend_identity(&self) -> DigestV1 {
        self.backend_identity
    }
    pub const fn tokenizer_identity(&self) -> DigestV1 {
        self.tokenizer_identity
    }
    pub const fn decoder_identity(&self) -> DigestV1 {
        self.decoder_identity
    }
    pub const fn tool_schema_digest(&self) -> DigestV1 {
        self.tool_schema_digest
    }
    pub fn reasoning_mode(&self) -> &str {
        &self.reasoning_mode
    }
    pub fn reasoning_effort(&self) -> &str {
        &self.reasoning_effort
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
    pub const fn native_state_policy(&self) -> NativeStatePolicyV1 {
        self.native_state_policy
    }
    pub const fn allow_effort_downshift(&self) -> bool {
        self.allow_effort_downshift
    }
    pub fn provider_extension(&self) -> &BTreeMap<String, Value> {
        &self.provider_extension
    }
}

/// Opaque result of comparing two validated strict reasoning contracts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StrictReasoningAdmissionV1 {
    contract_version: u16,
    baseline_contract_digest: DigestV1,
    candidate_contract_digest: DigestV1,
    same_comparison_class: bool,
    max_output_tokens_added: u32,
    reasoning_tokens_added: u32,
    visible_output_tokens_added: u32,
    recovery_tokens_added: u32,
    admission_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StrictReasoningAdmissionRecordV1 {
    pub contract_version: u16,
    pub baseline_contract_digest: DigestV1,
    pub candidate_contract_digest: DigestV1,
    pub same_comparison_class: bool,
    pub max_output_tokens_added: u32,
    pub reasoning_tokens_added: u32,
    pub visible_output_tokens_added: u32,
    pub recovery_tokens_added: u32,
    pub admission_digest: DigestV1,
}

impl StrictReasoningAdmissionV1 {
    fn new(
        baseline: &ReasoningContractV1,
        candidate: &ReasoningContractV1,
    ) -> Result<Self, ReasoningContractErrorV1> {
        let mut admission = Self {
            contract_version: REASONING_CONTRACT_VERSION_V1,
            baseline_contract_digest: baseline.identity_digest()?,
            candidate_contract_digest: candidate.identity_digest()?,
            same_comparison_class: baseline == candidate,
            max_output_tokens_added: candidate.max_output_tokens - baseline.max_output_tokens,
            reasoning_tokens_added: candidate.reserved_reasoning_tokens
                - baseline.reserved_reasoning_tokens,
            visible_output_tokens_added: candidate.reserved_visible_output_tokens
                - baseline.reserved_visible_output_tokens,
            recovery_tokens_added: candidate.reserved_recovery_tokens
                - baseline.reserved_recovery_tokens,
            admission_digest: DigestV1::ZERO,
        };
        admission.admission_digest = admission.expected_digest();
        Ok(admission)
    }

    fn expected_digest(&self) -> DigestV1 {
        admission_digest(
            self.contract_version,
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.same_comparison_class,
            self.max_output_tokens_added,
            self.reasoning_tokens_added,
            self.visible_output_tokens_added,
            self.recovery_tokens_added,
        )
    }

    pub fn validate(&self) -> Result<(), ReasoningContractErrorV1> {
        validate_admission_shape(
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.same_comparison_class,
            self.max_output_tokens_added,
            self.reasoning_tokens_added,
            self.visible_output_tokens_added,
            self.recovery_tokens_added,
        )?;
        validate_admission_fields(
            self.contract_version,
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.admission_digest,
            self.expected_digest(),
        )
    }

    pub fn record(&self) -> StrictReasoningAdmissionRecordV1 {
        StrictReasoningAdmissionRecordV1 {
            contract_version: self.contract_version,
            baseline_contract_digest: self.baseline_contract_digest,
            candidate_contract_digest: self.candidate_contract_digest,
            same_comparison_class: self.same_comparison_class,
            max_output_tokens_added: self.max_output_tokens_added,
            reasoning_tokens_added: self.reasoning_tokens_added,
            visible_output_tokens_added: self.visible_output_tokens_added,
            recovery_tokens_added: self.recovery_tokens_added,
            admission_digest: self.admission_digest,
        }
    }

    pub const fn baseline_contract_digest(&self) -> DigestV1 {
        self.baseline_contract_digest
    }
    pub const fn candidate_contract_digest(&self) -> DigestV1 {
        self.candidate_contract_digest
    }
    pub const fn same_comparison_class(&self) -> bool {
        self.same_comparison_class
    }
    pub const fn digest(&self) -> DigestV1 {
        self.admission_digest
    }
}

impl StrictReasoningAdmissionRecordV1 {
    pub fn validate(&self) -> Result<(), ReasoningContractErrorV1> {
        validate_admission_shape(
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.same_comparison_class,
            self.max_output_tokens_added,
            self.reasoning_tokens_added,
            self.visible_output_tokens_added,
            self.recovery_tokens_added,
        )?;
        validate_admission_fields(
            self.contract_version,
            self.baseline_contract_digest,
            self.candidate_contract_digest,
            self.admission_digest,
            admission_digest(
                self.contract_version,
                self.baseline_contract_digest,
                self.candidate_contract_digest,
                self.same_comparison_class,
                self.max_output_tokens_added,
                self.reasoning_tokens_added,
                self.visible_output_tokens_added,
                self.recovery_tokens_added,
            ),
        )
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, ReasoningContractErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(json_error)?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ReasoningContractErrorV1> {
        if bytes.len() > REASONING_CONTRACT_MAX_CANONICAL_BYTES_V1 {
            return Err(ReasoningContractErrorV1::new(
                ReasoningContractFailureCodeV1::CanonicalPayloadTooLarge,
                "strict reasoning admission exceeds its canonical byte bound",
            ));
        }
        let record: Self = serde_json::from_slice(bytes).map_err(json_error)?;
        record.validate()?;
        if record.canonical_bytes()? != bytes {
            return Err(ReasoningContractErrorV1::new(
                ReasoningContractFailureCodeV1::NonCanonicalEncoding,
                "strict reasoning admission is not canonical sorted-key JSON",
            ));
        }
        Ok(record)
    }
}

pub fn verify_strict_no_downshift_v1(
    baseline: &ReasoningContractV1,
    candidate: &ReasoningContractV1,
) -> Result<StrictReasoningAdmissionV1, ReasoningContractErrorV1> {
    baseline.validate()?;
    candidate.validate()?;
    if baseline.allow_effort_downshift || candidate.allow_effort_downshift {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::EffortDownshiftAllowed,
            "strict and amplify contracts must set allow_effort_downshift to false",
        ));
    }
    if baseline.model_identity != candidate.model_identity
        || baseline.backend_identity != candidate.backend_identity
        || baseline.tokenizer_identity != candidate.tokenizer_identity
        || baseline.decoder_identity != candidate.decoder_identity
        || baseline.tool_schema_digest != candidate.tool_schema_digest
    {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::ComparisonIdentityMismatch,
            "model, backend, tokenizer, decoder, and tool schema must remain exact",
        ));
    }
    if baseline.reasoning_mode != candidate.reasoning_mode {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::ReasoningModeMismatch,
            "strict reasoning mode changed without a cross-class theorem",
        ));
    }
    if baseline.reasoning_effort != candidate.reasoning_effort {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::ReasoningEffortMismatch,
            "strict reasoning effort changed without a cross-class theorem",
        ));
    }
    if baseline.native_state_policy != candidate.native_state_policy {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::NativeStatePolicyMismatch,
            "native reasoning-state policy changed without a cross-class theorem",
        ));
    }
    if baseline.provider_extension != candidate.provider_extension {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::ProviderExtensionMismatch,
            "provider extension changed without a cross-class theorem",
        ));
    }
    if candidate.max_output_tokens < baseline.max_output_tokens {
        return Err(downshift(
            ReasoningContractFailureCodeV1::OutputCeilingDownshift,
            "maximum output tokens",
        ));
    }
    if candidate.reserved_reasoning_tokens < baseline.reserved_reasoning_tokens {
        return Err(downshift(
            ReasoningContractFailureCodeV1::ReasoningReserveDownshift,
            "reserved reasoning tokens",
        ));
    }
    if candidate.reserved_visible_output_tokens < baseline.reserved_visible_output_tokens {
        return Err(downshift(
            ReasoningContractFailureCodeV1::VisibleOutputReserveDownshift,
            "reserved visible output tokens",
        ));
    }
    if candidate.reserved_recovery_tokens < baseline.reserved_recovery_tokens {
        return Err(downshift(
            ReasoningContractFailureCodeV1::RecoveryReserveDownshift,
            "reserved recovery tokens",
        ));
    }
    StrictReasoningAdmissionV1::new(baseline, candidate)
}

pub fn reasoning_contract_schema_v1() -> Value {
    json!({
        "$defs": {
            "digest": {"pattern": "^[0-9a-f]{64}$", "type": "string"},
        },
        "$id": "https://zerostack.dev/schemas/racc-r/reasoning-contract-v1.json",
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "additionalProperties": false,
        "properties": {
            "allow_effort_downshift": {"type": "boolean"},
            "backend_identity": {"$ref": "#/$defs/digest"},
            "decoder_identity": {"$ref": "#/$defs/digest"},
            "max_output_tokens": {"minimum": 1, "type": "integer"},
            "model_identity": {"$ref": "#/$defs/digest"},
            "native_state_policy": {"enum": ["exact-required", "exact-if-available", "clean-restart", "scoped-certificate", "unavailable"]},
            "provider_extension": {"additionalProperties": true, "type": "object"},
            "reasoning_effort": {"minLength": 1, "type": "string"},
            "reasoning_mode": {"minLength": 1, "type": "string"},
            "reserved_reasoning_tokens": {"minimum": 0, "type": "integer"},
            "reserved_recovery_tokens": {"minimum": 0, "type": "integer"},
            "reserved_visible_output_tokens": {"minimum": 0, "type": "integer"},
            "schema_version": {"const": REASONING_CONTRACT_SCHEMA_VERSION_V1},
            "tokenizer_identity": {"$ref": "#/$defs/digest"},
            "tool_schema_digest": {"$ref": "#/$defs/digest"},
        },
        "required": [
            "schema_version", "model_identity", "backend_identity", "tokenizer_identity",
            "decoder_identity", "tool_schema_digest", "reasoning_mode", "reasoning_effort",
            "max_output_tokens", "reserved_reasoning_tokens", "reserved_visible_output_tokens",
            "reserved_recovery_tokens", "native_state_policy", "allow_effort_downshift",
        ],
        "title": "RACC-R ReasoningContract V1",
        "type": "object",
    })
}

pub fn reasoning_contract_manifest_v1() -> Value {
    json!({
        "canonical_encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "comparison_identity_fields": [
            "model_identity", "backend_identity", "tokenizer_identity", "decoder_identity",
            "tool_schema_digest", "reasoning_mode", "reasoning_effort", "native_state_policy",
            "provider_extension",
        ],
        "contract_version": REASONING_CONTRACT_VERSION_V1,
        "headroom_order": ["reasoning", "visible_output", "tool", "recovery", "input"],
        "max_canonical_bytes": REASONING_CONTRACT_MAX_CANONICAL_BYTES_V1,
        "name": "zerostack.reasoning_contract.v1",
        "negative_space": [
            "cache_eligibility_as_hit", "effort_downshift", "opaque_state_summary_as_exact",
            "reasoning_tokens_estimated_from_visible_output", "silent_output_ceiling_reduction",
            "silent_reserve_reduction", "token_reduction_as_quality",
        ],
        "schema_digest": reasoning_contract_schema_digest_v1(),
        "strict_exact_fields": [
            "model_identity", "backend_identity", "tokenizer_identity", "decoder_identity",
            "tool_schema_digest", "reasoning_mode", "reasoning_effort", "native_state_policy",
            "provider_extension", "allow_effort_downshift_false",
        ],
        "strict_nondecreasing_fields": [
            "max_output_tokens", "reserved_reasoning_tokens", "reserved_visible_output_tokens",
            "reserved_recovery_tokens",
        ],
    })
}

pub fn reasoning_contract_schema_digest_v1() -> DigestV1 {
    domain_digest(
        SCHEMA_DOMAIN_V1,
        canonical_json(&reasoning_contract_schema_v1()).as_bytes(),
    )
}

pub fn reasoning_contract_digest_v1() -> DigestV1 {
    domain_digest(
        CONTRACT_DOMAIN_V1,
        canonical_json(&reasoning_contract_manifest_v1()).as_bytes(),
    )
}

fn admission_digest(
    contract_version: u16,
    baseline_contract_digest: DigestV1,
    candidate_contract_digest: DigestV1,
    same_comparison_class: bool,
    max_output_tokens_added: u32,
    reasoning_tokens_added: u32,
    visible_output_tokens_added: u32,
    recovery_tokens_added: u32,
) -> DigestV1 {
    domain_digest(
        ADMISSION_DOMAIN_V1,
        canonical_json(&json!({
            "baseline_contract_digest": baseline_contract_digest,
            "candidate_contract_digest": candidate_contract_digest,
            "contract_version": contract_version,
            "max_output_tokens_added": max_output_tokens_added,
            "reasoning_tokens_added": reasoning_tokens_added,
            "recovery_tokens_added": recovery_tokens_added,
            "same_comparison_class": same_comparison_class,
            "visible_output_tokens_added": visible_output_tokens_added,
        }))
        .as_bytes(),
    )
}

fn validate_admission_shape(
    baseline_contract_digest: DigestV1,
    candidate_contract_digest: DigestV1,
    same_comparison_class: bool,
    max_output_tokens_added: u32,
    reasoning_tokens_added: u32,
    visible_output_tokens_added: u32,
    recovery_tokens_added: u32,
) -> Result<(), ReasoningContractErrorV1> {
    let all_zero = max_output_tokens_added == 0
        && reasoning_tokens_added == 0
        && visible_output_tokens_added == 0
        && recovery_tokens_added == 0;
    if same_comparison_class != (baseline_contract_digest == candidate_contract_digest)
        || same_comparison_class != all_zero
    {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::InvalidAdmission,
            "comparison-class flag, contract identities, and reserve deltas are inconsistent",
        ));
    }
    Ok(())
}

fn validate_admission_fields(
    contract_version: u16,
    baseline_contract_digest: DigestV1,
    candidate_contract_digest: DigestV1,
    admission_digest: DigestV1,
    expected_digest: DigestV1,
) -> Result<(), ReasoningContractErrorV1> {
    if contract_version != REASONING_CONTRACT_VERSION_V1 {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::SchemaVersionMismatch,
            "strict reasoning admission version is not v1",
        ));
    }
    if baseline_contract_digest == DigestV1::ZERO
        || candidate_contract_digest == DigestV1::ZERO
        || admission_digest == DigestV1::ZERO
    {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::MissingIdentity,
            "strict reasoning admission contains a zero digest",
        ));
    }
    if admission_digest != expected_digest {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::AdmissionDigestMismatch,
            "strict reasoning admission digest does not match its fields",
        ));
    }
    Ok(())
}

fn validate_id(label: &str, value: &str) -> Result<(), ReasoningContractErrorV1> {
    if value.trim().is_empty()
        || value.len() > REASONING_CONTRACT_MAX_ID_BYTES_V1
        || value.chars().any(char::is_control)
    {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::InvalidIdentifier,
            format!("{label} is empty, contains control characters, or exceeds its bound"),
        ));
    }
    Ok(())
}

fn validate_provider_extension(
    extension: &BTreeMap<String, Value>,
) -> Result<(), ReasoningContractErrorV1> {
    let value = serde_json::to_value(extension).map_err(json_error)?;
    let bytes = canonical_json(&value);
    if bytes.len() > REASONING_CONTRACT_MAX_EXTENSION_BYTES_V1 {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::ProviderExtensionTooLarge,
            "provider extension exceeds its canonical byte bound",
        ));
    }
    let mut nodes = 1usize;
    for (key, value) in extension {
        validate_id("provider extension key", key)?;
        count_value_nodes(value, 1, &mut nodes)?;
    }
    Ok(())
}

fn count_value_nodes(
    value: &Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), ReasoningContractErrorV1> {
    if depth > REASONING_CONTRACT_MAX_EXTENSION_DEPTH_V1 {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::ProviderExtensionTooLarge,
            "provider extension exceeds its depth bound",
        ));
    }
    *nodes = nodes.checked_add(1).ok_or_else(|| {
        ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::ProviderExtensionTooLarge,
            "provider extension node count overflowed",
        )
    })?;
    if *nodes > REASONING_CONTRACT_MAX_EXTENSION_NODES_V1 {
        return Err(ReasoningContractErrorV1::new(
            ReasoningContractFailureCodeV1::ProviderExtensionTooLarge,
            "provider extension exceeds its node bound",
        ));
    }
    match value {
        Value::Array(values) => {
            for value in values {
                count_value_nodes(value, depth + 1, nodes)?;
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                validate_id("provider extension key", key)?;
                count_value_nodes(value, depth + 1, nodes)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn domain_digest(domain: &[u8], payload: &[u8]) -> DigestV1 {
    let mut bytes = Vec::with_capacity(domain.len() + payload.len());
    bytes.extend_from_slice(domain);
    bytes.extend_from_slice(payload);
    DigestV1::from_bytes(sha256(&bytes))
}

fn json_error(error: serde_json::Error) -> ReasoningContractErrorV1 {
    ReasoningContractErrorV1::new(
        ReasoningContractFailureCodeV1::SerializationFailure,
        format!("reasoning contract JSON error: {error}"),
    )
}

fn downshift(code: ReasoningContractFailureCodeV1, field: &str) -> ReasoningContractErrorV1 {
    ReasoningContractErrorV1::new(code, format!("strict candidate lowered {field}"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReasoningContractFailureCodeV1 {
    SchemaVersionMismatch,
    MissingIdentity,
    InvalidIdentifier,
    InvalidTokenReservation,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    ProviderExtensionTooLarge,
    EffortDownshiftAllowed,
    ComparisonIdentityMismatch,
    ReasoningModeMismatch,
    ReasoningEffortMismatch,
    NativeStatePolicyMismatch,
    ProviderExtensionMismatch,
    OutputCeilingDownshift,
    ReasoningReserveDownshift,
    VisibleOutputReserveDownshift,
    RecoveryReserveDownshift,
    ContextCapacityExceeded,
    InputExceedsHeadroom,
    AdmissionDigestMismatch,
    InvalidAdmission,
    SerializationFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReasoningContractErrorV1 {
    code: ReasoningContractFailureCodeV1,
    detail: String,
}

impl ReasoningContractErrorV1 {
    fn new(code: ReasoningContractFailureCodeV1, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> ReasoningContractFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for ReasoningContractErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.code, self.detail)
    }
}

impl Error for ReasoningContractErrorV1 {}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn contract() -> ReasoningContractV1 {
        ReasoningContractV1::new(
            digest(1),
            digest(2),
            digest(3),
            digest(4),
            digest(5),
            "native-reasoning",
            "high",
            4_096,
            8_192,
            2_048,
            1_024,
            NativeStatePolicyV1::ExactRequired,
            false,
            BTreeMap::from([("sampler".into(), json!({"temperature_ppm": 0}))]),
        )
        .unwrap()
    }

    #[test]
    fn canonical_schema_and_contract_round_trip() {
        let contract = contract();
        let bytes = contract.canonical_bytes().unwrap();
        assert_eq!(
            ReasoningContractV1::from_canonical_bytes(&bytes).unwrap(),
            contract
        );
        let mut spaced = bytes.clone();
        spaced.push(b' ');
        assert_eq!(
            ReasoningContractV1::from_canonical_bytes(&spaced)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::NonCanonicalEncoding
        );
        assert_eq!(
            reasoning_contract_schema_v1()["required"]
                .as_array()
                .unwrap()
                .len(),
            14
        );
        let published: Value = serde_json::from_str(include_str!(
            "../../../conformance/schemas/reasoning-contract-v1.schema.json"
        ))
        .unwrap();
        assert_eq!(published, reasoning_contract_schema_v1());
    }

    #[test]
    fn strict_equal_contract_mints_opaque_admission() {
        let baseline = contract();
        let admission = verify_strict_no_downshift_v1(&baseline, &baseline).unwrap();
        assert!(admission.same_comparison_class());
        admission.validate().unwrap();
        let record = admission.record();
        record.validate().unwrap();
        let bytes = record.canonical_bytes().unwrap();
        assert_eq!(
            StrictReasoningAdmissionRecordV1::from_canonical_bytes(&bytes).unwrap(),
            record
        );
        assert_eq!(
            admission.baseline_contract_digest(),
            baseline.identity_digest().unwrap()
        );
    }

    #[test]
    fn strict_identity_mode_effort_state_and_provider_changes_reclassify() {
        let baseline = contract();
        let mut candidate = baseline.clone();
        candidate.tool_schema_digest = digest(99);
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ComparisonIdentityMismatch
        );
        let mut candidate = baseline.clone();
        candidate.reasoning_mode = "other".into();
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ReasoningModeMismatch
        );
        let mut candidate = baseline.clone();
        candidate.reasoning_effort = "low".into();
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ReasoningEffortMismatch
        );
        let mut candidate = baseline.clone();
        candidate.native_state_policy = NativeStatePolicyV1::CleanRestart;
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::NativeStatePolicyMismatch
        );
        let mut candidate = baseline.clone();
        candidate
            .provider_extension
            .insert("phase".into(), json!(2));
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ProviderExtensionMismatch
        );
    }

    #[test]
    fn strict_rejects_every_numeric_downshift_and_effort_flag() {
        let baseline = contract();
        for (code, candidate) in [
            (
                ReasoningContractFailureCodeV1::OutputCeilingDownshift,
                ReasoningContractV1 {
                    max_output_tokens: baseline.max_output_tokens - 1,
                    ..baseline.clone()
                },
            ),
            (
                ReasoningContractFailureCodeV1::ReasoningReserveDownshift,
                ReasoningContractV1 {
                    reserved_reasoning_tokens: baseline.reserved_reasoning_tokens - 1,
                    ..baseline.clone()
                },
            ),
            (
                ReasoningContractFailureCodeV1::VisibleOutputReserveDownshift,
                ReasoningContractV1 {
                    reserved_visible_output_tokens: baseline.reserved_visible_output_tokens - 1,
                    ..baseline.clone()
                },
            ),
            (
                ReasoningContractFailureCodeV1::RecoveryReserveDownshift,
                ReasoningContractV1 {
                    reserved_recovery_tokens: baseline.reserved_recovery_tokens - 1,
                    ..baseline.clone()
                },
            ),
        ] {
            assert_eq!(
                verify_strict_no_downshift_v1(&baseline, &candidate)
                    .unwrap_err()
                    .failure_code(),
                code
            );
        }
        let candidate = ReasoningContractV1 {
            allow_effort_downshift: true,
            ..baseline.clone()
        };
        assert_eq!(
            verify_strict_no_downshift_v1(&baseline, &candidate)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::EffortDownshiftAllowed
        );
    }

    #[test]
    fn strict_permits_only_visible_numeric_reinvestment() {
        let baseline = contract();
        let candidate = ReasoningContractV1 {
            max_output_tokens: baseline.max_output_tokens + 100,
            reserved_reasoning_tokens: baseline.reserved_reasoning_tokens + 200,
            reserved_visible_output_tokens: baseline.reserved_visible_output_tokens + 50,
            reserved_recovery_tokens: baseline.reserved_recovery_tokens + 25,
            ..baseline.clone()
        };
        let admission = verify_strict_no_downshift_v1(&baseline, &candidate).unwrap();
        assert!(!admission.same_comparison_class());
        let record = admission.record();
        assert_eq!(record.max_output_tokens_added, 100);
        assert_eq!(record.reasoning_tokens_added, 200);
        assert_eq!(record.visible_output_tokens_added, 50);
        assert_eq!(record.recovery_tokens_added, 25);
    }

    #[test]
    fn headroom_is_reserved_before_input() {
        let contract = contract();
        assert_eq!(
            contract.admitted_input_ceiling(32_768, 1_024).unwrap(),
            20_480
        );
        assert_eq!(contract.admit_input(32_768, 1_024, 20_000).unwrap(), 480);
        assert_eq!(
            contract
                .admit_input(32_768, 1_024, 20_481)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::InputExceedsHeadroom
        );
        assert_eq!(
            contract
                .admitted_input_ceiling(1_000, 1_024)
                .unwrap_err()
                .failure_code(),
            ReasoningContractFailureCodeV1::ContextCapacityExceeded
        );
    }

    #[test]
    fn admission_and_extension_tampering_fail_closed() {
        let baseline = contract();
        let admission = verify_strict_no_downshift_v1(&baseline, &baseline).unwrap();
        let mut record = admission.record();
        record.admission_digest = digest(99);
        assert_eq!(
            record.validate().unwrap_err().failure_code(),
            ReasoningContractFailureCodeV1::AdmissionDigestMismatch
        );
        let mut record = admission.record();
        record.reasoning_tokens_added = 1;
        assert_eq!(
            record.validate().unwrap_err().failure_code(),
            ReasoningContractFailureCodeV1::InvalidAdmission
        );
        let mut candidate = baseline.clone();
        candidate.provider_extension = BTreeMap::from([(
            "oversized".into(),
            Value::String("x".repeat(REASONING_CONTRACT_MAX_EXTENSION_BYTES_V1)),
        )]);
        assert_eq!(
            candidate.validate().unwrap_err().failure_code(),
            ReasoningContractFailureCodeV1::ProviderExtensionTooLarge
        );
    }

    #[test]
    fn reasoning_contract_digest_is_stable() {
        assert_eq!(
            reasoning_contract_digest_v1(),
            DigestV1::from_hex("4906ff9514b220cbb8193f845d9f86eb5ea2423914a1974ec3eb309007230339")
                .unwrap()
        );
        assert_eq!(
            reasoning_contract_schema_digest_v1(),
            DigestV1::from_hex("80258e0d9c5b24ccdabd94bd5806a3e1407c99343def267b8ad99ca39f230db9")
                .unwrap()
        );
    }
}
