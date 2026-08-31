//! Exclusive causal-work accounting. Legacy token classes remain readable but do not carry full
//! causal authority. Receipts bind one parent-measured counter window, one class per unique work
//! unit, and an explicit residue policy.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};
use zero_abi::{Sha256Digest, canonical_json, sha256};

pub const CAUSAL_WORK_TAXONOMY_VERSION: u16 = 3;
pub const CAUSAL_WORK_RECEIPT_SCHEMA: u16 = 1;
pub const CAUSAL_WORK_MAX_CHARGES: usize = 65_536;
pub const CAUSAL_WORK_MAX_ID_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalWorkClass {
    Candidate,
    Verification,
    Comparison,
    Baseline,
    Fallback,
    Restoration,
    Prewarm,
    Residue,
}

impl CausalWorkClass {
    pub const ALL: [Self; 8] = [
        Self::Candidate,
        Self::Verification,
        Self::Comparison,
        Self::Baseline,
        Self::Fallback,
        Self::Restoration,
        Self::Prewarm,
        Self::Residue,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Verification => "verification",
            Self::Comparison => "comparison",
            Self::Baseline => "baseline",
            Self::Fallback => "fallback",
            Self::Restoration => "restoration",
            Self::Prewarm => "prewarm",
            Self::Residue => "residue",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalCounterUnit {
    Tokens,
    Bytes,
    Calls,
    CpuNanoseconds,
    WallNanoseconds,
    AllocatedBytes,
    IoBytes,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParentCounterIdentity {
    pub counter_id: String,
    pub unit: CausalCounterUnit,
    pub boundary_digest: Sha256Digest,
    pub adapter_digest: Sha256Digest,
    pub platform_profile_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParentCounterWindow {
    pub identity: ParentCounterIdentity,
    pub start: u64,
    pub end: u64,
}

impl ParentCounterWindow {
    pub fn delta(&self) -> Result<u64, CausalWorkError> {
        self.end
            .checked_sub(self.start)
            .ok_or(CausalWorkError::CounterRegressed {
                start: self.start,
                end: self.end,
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParentCounterObservation {
    Measured {
        window: ParentCounterWindow,
    },
    Unmeasured {
        identity: ParentCounterIdentity,
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredEstimate {
    pub estimator_id: String,
    pub identity: ParentCounterIdentity,
    pub declared_value: u64,
    pub assumptions_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalWorkCharge {
    pub work_unit_id: Sha256Digest,
    pub class: CausalWorkClass,
    pub amount: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResiduePolicy {
    RejectUnclassified,
    AssignToResidue {
        policy_id: String,
        policy_digest: Sha256Digest,
        residue_work_unit_id: Sha256Digest,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalClassTotals {
    pub candidate: u64,
    pub verification: u64,
    pub comparison: u64,
    pub baseline: u64,
    pub fallback: u64,
    pub restoration: u64,
    pub prewarm: u64,
    pub residue: u64,
}

impl CausalClassTotals {
    pub fn class_total(&self, class: CausalWorkClass) -> u64 {
        match class {
            CausalWorkClass::Candidate => self.candidate,
            CausalWorkClass::Verification => self.verification,
            CausalWorkClass::Comparison => self.comparison,
            CausalWorkClass::Baseline => self.baseline,
            CausalWorkClass::Fallback => self.fallback,
            CausalWorkClass::Restoration => self.restoration,
            CausalWorkClass::Prewarm => self.prewarm,
            CausalWorkClass::Residue => self.residue,
        }
    }

    fn add(&mut self, class: CausalWorkClass, amount: u64) -> Result<(), CausalWorkError> {
        let target = match class {
            CausalWorkClass::Candidate => &mut self.candidate,
            CausalWorkClass::Verification => &mut self.verification,
            CausalWorkClass::Comparison => &mut self.comparison,
            CausalWorkClass::Baseline => &mut self.baseline,
            CausalWorkClass::Fallback => &mut self.fallback,
            CausalWorkClass::Restoration => &mut self.restoration,
            CausalWorkClass::Prewarm => &mut self.prewarm,
            CausalWorkClass::Residue => &mut self.residue,
        };
        *target = target
            .checked_add(amount)
            .ok_or(CausalWorkError::CounterOverflow)?;
        Ok(())
    }

    pub fn checked_total(&self) -> Result<u64, CausalWorkError> {
        CausalWorkClass::ALL
            .into_iter()
            .try_fold(0_u64, |total, class| {
                total
                    .checked_add(self.class_total(class))
                    .ok_or(CausalWorkError::CounterOverflow)
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalWorkReceipt {
    pub schema_version: u16,
    pub taxonomy_version: u16,
    pub assembly_manifest_digest: Sha256Digest,
    pub measurement: ParentCounterWindow,
    pub residue_policy: ResiduePolicy,
    pub charges: Vec<CausalWorkCharge>,
    pub class_totals: CausalClassTotals,
    pub classified_total: u64,
    pub observed_total: u64,
    pub receipt_digest: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CausalWorkReceiptWire {
    schema_version: u16,
    taxonomy_version: u16,
    assembly_manifest_digest: Sha256Digest,
    measurement: ParentCounterWindow,
    residue_policy: ResiduePolicy,
    charges: Vec<CausalWorkCharge>,
    class_totals: CausalClassTotals,
    classified_total: u64,
    observed_total: u64,
    receipt_digest: Sha256Digest,
}

impl<'de> Deserialize<'de> for CausalWorkReceipt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CausalWorkReceiptWire::deserialize(deserializer)?;
        let receipt = Self {
            schema_version: wire.schema_version,
            taxonomy_version: wire.taxonomy_version,
            assembly_manifest_digest: wire.assembly_manifest_digest,
            measurement: wire.measurement,
            residue_policy: wire.residue_policy,
            charges: wire.charges,
            class_totals: wire.class_totals,
            classified_total: wire.classified_total,
            observed_total: wire.observed_total,
            receipt_digest: wire.receipt_digest,
        };
        receipt.validate().map_err(de::Error::custom)?;
        Ok(receipt)
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum CausalWorkOutcome {
    Measured {
        receipt: CausalWorkReceipt,
    },
    Unmeasured {
        identity: ParentCounterIdentity,
        reason: String,
    },
}

impl CausalWorkReceipt {
    pub fn build(
        assembly_manifest_digest: Sha256Digest,
        observation: ParentCounterObservation,
        mut charges: Vec<CausalWorkCharge>,
        residue_policy: ResiduePolicy,
    ) -> Result<CausalWorkOutcome, CausalWorkError> {
        let window = match observation {
            ParentCounterObservation::Unmeasured { identity, reason } => {
                validate_identity(&identity)?;
                validate_text("unmeasured.reason", &reason)?;
                if !charges.is_empty() {
                    return Err(CausalWorkError::ChargesForUnmeasuredCounter);
                }
                return Ok(CausalWorkOutcome::Unmeasured { identity, reason });
            }
            ParentCounterObservation::Measured { window } => window,
        };
        validate_identity(&window.identity)?;
        if charges.len() > CAUSAL_WORK_MAX_CHARGES {
            return Err(CausalWorkError::TooManyCharges);
        }
        charges.sort_by_key(|charge| charge.work_unit_id);
        for pair in charges.windows(2) {
            if pair[0].work_unit_id == pair[1].work_unit_id {
                return Err(CausalWorkError::DoubleClassifiedWorkUnit(
                    pair[0].work_unit_id,
                ));
            }
        }
        let mut totals = CausalClassTotals::default();
        for charge in &charges {
            if charge.amount == 0 {
                return Err(CausalWorkError::ZeroAmountCharge(charge.work_unit_id));
            }
            if charge.class == CausalWorkClass::Residue
                && !matches!(residue_policy, ResiduePolicy::AssignToResidue { .. })
            {
                return Err(CausalWorkError::ResidueWithoutPolicy);
            }
            totals.add(charge.class, charge.amount)?;
        }
        let observed_total = window.delta()?;
        let classified_before_residue = totals.checked_total()?;
        if classified_before_residue > observed_total {
            return Err(CausalWorkError::NonConservation {
                observed: observed_total,
                classified: classified_before_residue,
            });
        }
        let missing = observed_total - classified_before_residue;
        if missing != 0 {
            match &residue_policy {
                ResiduePolicy::RejectUnclassified => {
                    return Err(CausalWorkError::UnclassifiedWork { amount: missing });
                }
                ResiduePolicy::AssignToResidue {
                    policy_id,
                    residue_work_unit_id,
                    ..
                } => {
                    validate_text("residue_policy.policy_id", policy_id)?;
                    if charges
                        .binary_search_by_key(residue_work_unit_id, |charge| charge.work_unit_id)
                        .is_ok()
                    {
                        return Err(CausalWorkError::DoubleClassifiedWorkUnit(
                            *residue_work_unit_id,
                        ));
                    }
                    charges.push(CausalWorkCharge {
                        work_unit_id: *residue_work_unit_id,
                        class: CausalWorkClass::Residue,
                        amount: missing,
                    });
                    charges.sort_by_key(|charge| charge.work_unit_id);
                    totals.add(CausalWorkClass::Residue, missing)?;
                }
            }
        } else if let ResiduePolicy::AssignToResidue { policy_id, .. } = &residue_policy {
            validate_text("residue_policy.policy_id", policy_id)?;
        }
        let classified_total = totals.checked_total()?;
        if classified_total != observed_total {
            return Err(CausalWorkError::NonConservation {
                observed: observed_total,
                classified: classified_total,
            });
        }
        let mut receipt = Self {
            schema_version: CAUSAL_WORK_RECEIPT_SCHEMA,
            taxonomy_version: CAUSAL_WORK_TAXONOMY_VERSION,
            assembly_manifest_digest,
            measurement: window,
            residue_policy,
            charges,
            class_totals: totals,
            classified_total,
            observed_total,
            receipt_digest: Sha256Digest::ZERO,
        };
        receipt.receipt_digest = receipt.compute_digest()?;
        receipt.validate()?;
        Ok(CausalWorkOutcome::Measured { receipt })
    }

    pub fn validate(&self) -> Result<(), CausalWorkError> {
        if self.schema_version != CAUSAL_WORK_RECEIPT_SCHEMA
            || self.taxonomy_version != CAUSAL_WORK_TAXONOMY_VERSION
        {
            return Err(CausalWorkError::UnsupportedVersion);
        }
        validate_identity(&self.measurement.identity)?;
        if self.charges.len() > CAUSAL_WORK_MAX_CHARGES {
            return Err(CausalWorkError::TooManyCharges);
        }
        if let ResiduePolicy::AssignToResidue { policy_id, .. } = &self.residue_policy {
            validate_text("residue_policy.policy_id", policy_id)?;
        }
        let mut seen = BTreeSet::new();
        let mut totals = CausalClassTotals::default();
        for charge in &self.charges {
            if charge.amount == 0 {
                return Err(CausalWorkError::ZeroAmountCharge(charge.work_unit_id));
            }
            if !seen.insert(charge.work_unit_id) {
                return Err(CausalWorkError::DoubleClassifiedWorkUnit(
                    charge.work_unit_id,
                ));
            }
            if charge.class == CausalWorkClass::Residue {
                match &self.residue_policy {
                    ResiduePolicy::AssignToResidue {
                        residue_work_unit_id,
                        ..
                    } if charge.work_unit_id == *residue_work_unit_id => {}
                    _ => return Err(CausalWorkError::ResidueWithoutPolicy),
                }
            }
            totals.add(charge.class, charge.amount)?;
        }
        if self.charges.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(CausalWorkError::NonCanonicalCharges);
        }
        if totals != self.class_totals {
            return Err(CausalWorkError::ClassTotalsMismatch);
        }
        let observed = self.measurement.delta()?;
        let classified = totals.checked_total()?;
        if observed != self.observed_total
            || classified != self.classified_total
            || observed != classified
        {
            return Err(CausalWorkError::NonConservation {
                observed,
                classified,
            });
        }
        if self.compute_digest()? != self.receipt_digest {
            return Err(CausalWorkError::ReceiptDigestMismatch);
        }
        Ok(())
    }

    pub fn compute_digest(&self) -> Result<Sha256Digest, CausalWorkError> {
        let mut value =
            serde_json::to_value(self).map_err(|error| CausalWorkError::Json(error.to_string()))?;
        value
            .as_object_mut()
            .ok_or_else(|| CausalWorkError::Json("receipt must be object".into()))?
            .remove("receipt_digest");
        let mut bytes = b"zerostack.causal_work_receipt\0".to_vec();
        bytes.extend_from_slice(canonical_json(&value).as_bytes());
        Ok(Sha256Digest::from_bytes(sha256(&bytes)))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterEvidenceMode {
    Synthetic,
    RchCompilation,
    Native,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CounterCorrespondenceReceipt {
    platform_profile: String,
    evidence_mode: CounterEvidenceMode,
    identity: ParentCounterIdentity,
    parent_window: ParentCounterWindow,
    adapter_observed_delta: u64,
    adapter_binary_digest: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CounterCorrespondenceReceiptWire {
    platform_profile: String,
    evidence_mode: CounterEvidenceMode,
    identity: ParentCounterIdentity,
    parent_window: ParentCounterWindow,
    adapter_observed_delta: u64,
    adapter_binary_digest: Sha256Digest,
}

impl<'de> Deserialize<'de> for CounterCorrespondenceReceipt {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CounterCorrespondenceReceiptWire::deserialize(deserializer)?;
        Self::new(
            wire.platform_profile,
            wire.evidence_mode,
            wire.identity,
            wire.parent_window,
            wire.adapter_observed_delta,
            wire.adapter_binary_digest,
        )
        .map_err(de::Error::custom)
    }
}

impl CounterCorrespondenceReceipt {
    pub fn new(
        platform_profile: String,
        evidence_mode: CounterEvidenceMode,
        identity: ParentCounterIdentity,
        parent_window: ParentCounterWindow,
        adapter_observed_delta: u64,
        adapter_binary_digest: Sha256Digest,
    ) -> Result<Self, CausalWorkError> {
        let receipt = Self {
            platform_profile,
            evidence_mode,
            identity,
            parent_window,
            adapter_observed_delta,
            adapter_binary_digest,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> Result<(), CausalWorkError> {
        validate_text("platform_profile", &self.platform_profile)?;
        validate_identity(&self.identity)?;
        if self.parent_window.identity != self.identity
            || self.parent_window.delta()? != self.adapter_observed_delta
            || self.identity.adapter_digest != self.adapter_binary_digest
        {
            return Err(CausalWorkError::CounterCorrespondenceMismatch);
        }
        Ok(())
    }

    pub const fn is_native_evidence(&self) -> bool {
        matches!(self.evidence_mode, CounterEvidenceMode::Native)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyChargeClass {
    Billed,
    FailedTrial,
    Retry,
    Recovery,
    Reexpansion,
    Fallback,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyClassMapping {
    pub legacy: LegacyChargeClass,
    pub suggested_causal_class: CausalWorkClass,
    pub requires_remeasurement: bool,
    pub measured_fact: bool,
}

pub const fn map_legacy_class(legacy: LegacyChargeClass) -> LegacyClassMapping {
    let suggested_causal_class = match legacy {
        LegacyChargeClass::Billed | LegacyChargeClass::FailedTrial => CausalWorkClass::Candidate,
        LegacyChargeClass::Retry | LegacyChargeClass::Reexpansion => CausalWorkClass::Restoration,
        LegacyChargeClass::Recovery => CausalWorkClass::Verification,
        LegacyChargeClass::Fallback => CausalWorkClass::Fallback,
    };
    LegacyClassMapping {
        legacy,
        suggested_causal_class,
        requires_remeasurement: true,
        measured_fact: false,
    }
}

fn validate_identity(identity: &ParentCounterIdentity) -> Result<(), CausalWorkError> {
    validate_text("counter_id", &identity.counter_id)
}

fn validate_text(field: &'static str, text: &str) -> Result<(), CausalWorkError> {
    if text.is_empty()
        || text.len() > CAUSAL_WORK_MAX_ID_BYTES
        || text.chars().any(char::is_control)
    {
        return Err(CausalWorkError::InvalidText(field));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalWorkFailureCode {
    UnsupportedVersion,
    CounterRegressed,
    CounterOverflow,
    InvalidText,
    TooManyCharges,
    DoubleClassifiedWorkUnit,
    ZeroAmountCharge,
    ChargesForUnmeasuredCounter,
    ResidueWithoutPolicy,
    UnclassifiedWork,
    NonConservation,
    NonCanonicalCharges,
    ClassTotalsMismatch,
    ReceiptDigestMismatch,
    CounterCorrespondenceMismatch,
    Json,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CausalWorkError {
    UnsupportedVersion,
    CounterRegressed { start: u64, end: u64 },
    CounterOverflow,
    InvalidText(&'static str),
    TooManyCharges,
    DoubleClassifiedWorkUnit(Sha256Digest),
    ZeroAmountCharge(Sha256Digest),
    ChargesForUnmeasuredCounter,
    ResidueWithoutPolicy,
    UnclassifiedWork { amount: u64 },
    NonConservation { observed: u64, classified: u64 },
    NonCanonicalCharges,
    ClassTotalsMismatch,
    ReceiptDigestMismatch,
    CounterCorrespondenceMismatch,
    Json(String),
}

impl CausalWorkError {
    pub const fn code(&self) -> CausalWorkFailureCode {
        match self {
            Self::UnsupportedVersion => CausalWorkFailureCode::UnsupportedVersion,
            Self::CounterRegressed { .. } => CausalWorkFailureCode::CounterRegressed,
            Self::CounterOverflow => CausalWorkFailureCode::CounterOverflow,
            Self::InvalidText(_) => CausalWorkFailureCode::InvalidText,
            Self::TooManyCharges => CausalWorkFailureCode::TooManyCharges,
            Self::DoubleClassifiedWorkUnit(_) => CausalWorkFailureCode::DoubleClassifiedWorkUnit,
            Self::ZeroAmountCharge(_) => CausalWorkFailureCode::ZeroAmountCharge,
            Self::ChargesForUnmeasuredCounter => CausalWorkFailureCode::ChargesForUnmeasuredCounter,
            Self::ResidueWithoutPolicy => CausalWorkFailureCode::ResidueWithoutPolicy,
            Self::UnclassifiedWork { .. } => CausalWorkFailureCode::UnclassifiedWork,
            Self::NonConservation { .. } => CausalWorkFailureCode::NonConservation,
            Self::NonCanonicalCharges => CausalWorkFailureCode::NonCanonicalCharges,
            Self::ClassTotalsMismatch => CausalWorkFailureCode::ClassTotalsMismatch,
            Self::ReceiptDigestMismatch => CausalWorkFailureCode::ReceiptDigestMismatch,
            Self::CounterCorrespondenceMismatch => {
                CausalWorkFailureCode::CounterCorrespondenceMismatch
            }
            Self::Json(_) => CausalWorkFailureCode::Json,
        }
    }
}

impl fmt::Display for CausalWorkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "causal-work ledger failure: {:?}", self.code())
    }
}

impl Error for CausalWorkError {}

pub fn causal_work_contract_manifest() -> Value {
    json!({
        "contract": "zerostack.causal_work",
        "taxonomy_version": CAUSAL_WORK_TAXONOMY_VERSION,
        "receipt_schema": CAUSAL_WORK_RECEIPT_SCHEMA,
        "classes": CausalWorkClass::ALL.map(CausalWorkClass::as_str),
        "arithmetic": "checked_u64_integer_only",
        "classification": "one_unique_work_unit_id_one_class",
        "wire_decode": "all_receipt_invariants_validated_during_deserialization",
        "correspondence_decode": "validated_constructor_and_deserializer_only",
        "measurement_namespace": "parent_counter_observation",
        "estimate_namespace": "declared_estimate_nonconvertible_to_measurement",
        "unavailable_counter": "unmeasured_not_zero",
        "residue": "reject_or_preregistered_assign_to_residue",
        "residue_work_unit_id": "every_residue_charge_equals_preregistered_id",
        "legacy": "readable_mapping_requires_remeasurement_and_is_not_fact"
    })
}

pub fn causal_work_contract_digest() -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(
        canonical_json(&causal_work_contract_manifest()).as_bytes(),
    ))
}
