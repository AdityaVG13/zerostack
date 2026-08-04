//! Versioned exclusive causal-work accounting.
//!
//! Legacy token classes remain readable but are not complete causal authority.
//! V3 receipts bind one parent-measured integer counter window, exactly one class
//! per unique work unit, and an explicit residue policy. Declared estimates use a
//! distinct type and can never construct a measured receipt.

use std::{collections::BTreeSet, error::Error, fmt};

use serde::{de, Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use zero_abi::{canonical_json, sha256, DigestV1};

pub const CAUSAL_WORK_TAXONOMY_VERSION_V1: u16 = 3;
pub const CAUSAL_WORK_RECEIPT_SCHEMA_V1: u16 = 1;
pub const CAUSAL_WORK_MAX_CHARGES: usize = 65_536;
pub const CAUSAL_WORK_MAX_ID_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalWorkClassV1 {
    Candidate,
    Verification,
    Comparison,
    Baseline,
    Fallback,
    Restoration,
    Prewarm,
    Residue,
}

impl CausalWorkClassV1 {
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
pub enum CausalCounterUnitV1 {
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
pub struct ParentCounterIdentityV1 {
    pub counter_id: String,
    pub unit: CausalCounterUnitV1,
    pub boundary_digest: DigestV1,
    pub adapter_digest: DigestV1,
    pub platform_profile_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParentCounterWindowV1 {
    pub identity: ParentCounterIdentityV1,
    pub start: u64,
    pub end: u64,
}

impl ParentCounterWindowV1 {
    pub fn delta(&self) -> Result<u64, CausalWorkErrorV1> {
        self.end
            .checked_sub(self.start)
            .ok_or(CausalWorkErrorV1::CounterRegressed {
                start: self.start,
                end: self.end,
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "availability", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParentCounterObservationV1 {
    Measured {
        window: ParentCounterWindowV1,
    },
    Unmeasured {
        identity: ParentCounterIdentityV1,
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredEstimateV1 {
    pub estimator_id: String,
    pub identity: ParentCounterIdentityV1,
    pub declared_value: u64,
    pub assumptions_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalWorkChargeV1 {
    pub work_unit_id: DigestV1,
    pub class: CausalWorkClassV1,
    pub amount: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResiduePolicyV1 {
    RejectUnclassified,
    AssignToResidue {
        policy_id: String,
        policy_digest: DigestV1,
        residue_work_unit_id: DigestV1,
    },
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalClassTotalsV1 {
    pub candidate: u64,
    pub verification: u64,
    pub comparison: u64,
    pub baseline: u64,
    pub fallback: u64,
    pub restoration: u64,
    pub prewarm: u64,
    pub residue: u64,
}

impl CausalClassTotalsV1 {
    pub fn class_total(&self, class: CausalWorkClassV1) -> u64 {
        match class {
            CausalWorkClassV1::Candidate => self.candidate,
            CausalWorkClassV1::Verification => self.verification,
            CausalWorkClassV1::Comparison => self.comparison,
            CausalWorkClassV1::Baseline => self.baseline,
            CausalWorkClassV1::Fallback => self.fallback,
            CausalWorkClassV1::Restoration => self.restoration,
            CausalWorkClassV1::Prewarm => self.prewarm,
            CausalWorkClassV1::Residue => self.residue,
        }
    }

    fn add(&mut self, class: CausalWorkClassV1, amount: u64) -> Result<(), CausalWorkErrorV1> {
        let target = match class {
            CausalWorkClassV1::Candidate => &mut self.candidate,
            CausalWorkClassV1::Verification => &mut self.verification,
            CausalWorkClassV1::Comparison => &mut self.comparison,
            CausalWorkClassV1::Baseline => &mut self.baseline,
            CausalWorkClassV1::Fallback => &mut self.fallback,
            CausalWorkClassV1::Restoration => &mut self.restoration,
            CausalWorkClassV1::Prewarm => &mut self.prewarm,
            CausalWorkClassV1::Residue => &mut self.residue,
        };
        *target = target
            .checked_add(amount)
            .ok_or(CausalWorkErrorV1::CounterOverflow)?;
        Ok(())
    }

    pub fn checked_total(&self) -> Result<u64, CausalWorkErrorV1> {
        CausalWorkClassV1::ALL
            .into_iter()
            .try_fold(0_u64, |total, class| {
                total
                    .checked_add(self.class_total(class))
                    .ok_or(CausalWorkErrorV1::CounterOverflow)
            })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CausalWorkReceiptV1 {
    pub schema_version: u16,
    pub taxonomy_version: u16,
    pub assembly_manifest_digest: DigestV1,
    pub measurement: ParentCounterWindowV1,
    pub residue_policy: ResiduePolicyV1,
    pub charges: Vec<CausalWorkChargeV1>,
    pub class_totals: CausalClassTotalsV1,
    pub classified_total: u64,
    pub observed_total: u64,
    pub receipt_digest: DigestV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CausalWorkReceiptWireV1 {
    schema_version: u16,
    taxonomy_version: u16,
    assembly_manifest_digest: DigestV1,
    measurement: ParentCounterWindowV1,
    residue_policy: ResiduePolicyV1,
    charges: Vec<CausalWorkChargeV1>,
    class_totals: CausalClassTotalsV1,
    classified_total: u64,
    observed_total: u64,
    receipt_digest: DigestV1,
}

impl<'de> Deserialize<'de> for CausalWorkReceiptV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CausalWorkReceiptWireV1::deserialize(deserializer)?;
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum CausalWorkOutcomeV1 {
    Measured {
        receipt: CausalWorkReceiptV1,
    },
    Unmeasured {
        identity: ParentCounterIdentityV1,
        reason: String,
    },
}

impl CausalWorkReceiptV1 {
    pub fn build(
        assembly_manifest_digest: DigestV1,
        observation: ParentCounterObservationV1,
        mut charges: Vec<CausalWorkChargeV1>,
        residue_policy: ResiduePolicyV1,
    ) -> Result<CausalWorkOutcomeV1, CausalWorkErrorV1> {
        let window = match observation {
            ParentCounterObservationV1::Unmeasured { identity, reason } => {
                validate_identity(&identity)?;
                validate_text("unmeasured.reason", &reason)?;
                if !charges.is_empty() {
                    return Err(CausalWorkErrorV1::ChargesForUnmeasuredCounter);
                }
                return Ok(CausalWorkOutcomeV1::Unmeasured { identity, reason });
            }
            ParentCounterObservationV1::Measured { window } => window,
        };
        validate_identity(&window.identity)?;
        if charges.len() > CAUSAL_WORK_MAX_CHARGES {
            return Err(CausalWorkErrorV1::TooManyCharges);
        }
        charges.sort_by_key(|charge| charge.work_unit_id);
        for pair in charges.windows(2) {
            if pair[0].work_unit_id == pair[1].work_unit_id {
                return Err(CausalWorkErrorV1::DoubleClassifiedWorkUnit(
                    pair[0].work_unit_id,
                ));
            }
        }
        let mut totals = CausalClassTotalsV1::default();
        for charge in &charges {
            if charge.amount == 0 {
                return Err(CausalWorkErrorV1::ZeroAmountCharge(charge.work_unit_id));
            }
            if charge.class == CausalWorkClassV1::Residue
                && !matches!(residue_policy, ResiduePolicyV1::AssignToResidue { .. })
            {
                return Err(CausalWorkErrorV1::ResidueWithoutPolicy);
            }
            totals.add(charge.class, charge.amount)?;
        }
        let observed_total = window.delta()?;
        let classified_before_residue = totals.checked_total()?;
        if classified_before_residue > observed_total {
            return Err(CausalWorkErrorV1::NonConservation {
                observed: observed_total,
                classified: classified_before_residue,
            });
        }
        let missing = observed_total - classified_before_residue;
        if missing != 0 {
            match &residue_policy {
                ResiduePolicyV1::RejectUnclassified => {
                    return Err(CausalWorkErrorV1::UnclassifiedWork { amount: missing });
                }
                ResiduePolicyV1::AssignToResidue {
                    policy_id,
                    residue_work_unit_id,
                    ..
                } => {
                    validate_text("residue_policy.policy_id", policy_id)?;
                    if charges
                        .binary_search_by_key(residue_work_unit_id, |charge| charge.work_unit_id)
                        .is_ok()
                    {
                        return Err(CausalWorkErrorV1::DoubleClassifiedWorkUnit(
                            *residue_work_unit_id,
                        ));
                    }
                    charges.push(CausalWorkChargeV1 {
                        work_unit_id: *residue_work_unit_id,
                        class: CausalWorkClassV1::Residue,
                        amount: missing,
                    });
                    charges.sort_by_key(|charge| charge.work_unit_id);
                    totals.add(CausalWorkClassV1::Residue, missing)?;
                }
            }
        } else if let ResiduePolicyV1::AssignToResidue { policy_id, .. } = &residue_policy {
            validate_text("residue_policy.policy_id", policy_id)?;
        }
        let classified_total = totals.checked_total()?;
        if classified_total != observed_total {
            return Err(CausalWorkErrorV1::NonConservation {
                observed: observed_total,
                classified: classified_total,
            });
        }
        let mut receipt = Self {
            schema_version: CAUSAL_WORK_RECEIPT_SCHEMA_V1,
            taxonomy_version: CAUSAL_WORK_TAXONOMY_VERSION_V1,
            assembly_manifest_digest,
            measurement: window,
            residue_policy,
            charges,
            class_totals: totals,
            classified_total,
            observed_total,
            receipt_digest: DigestV1::ZERO,
        };
        receipt.receipt_digest = receipt.compute_digest()?;
        receipt.validate()?;
        Ok(CausalWorkOutcomeV1::Measured { receipt })
    }

    pub fn validate(&self) -> Result<(), CausalWorkErrorV1> {
        if self.schema_version != CAUSAL_WORK_RECEIPT_SCHEMA_V1
            || self.taxonomy_version != CAUSAL_WORK_TAXONOMY_VERSION_V1
        {
            return Err(CausalWorkErrorV1::UnsupportedVersion);
        }
        validate_identity(&self.measurement.identity)?;
        if self.charges.len() > CAUSAL_WORK_MAX_CHARGES {
            return Err(CausalWorkErrorV1::TooManyCharges);
        }
        if let ResiduePolicyV1::AssignToResidue { policy_id, .. } = &self.residue_policy {
            validate_text("residue_policy.policy_id", policy_id)?;
        }
        let mut seen = BTreeSet::new();
        let mut totals = CausalClassTotalsV1::default();
        for charge in &self.charges {
            if charge.amount == 0 {
                return Err(CausalWorkErrorV1::ZeroAmountCharge(charge.work_unit_id));
            }
            if !seen.insert(charge.work_unit_id) {
                return Err(CausalWorkErrorV1::DoubleClassifiedWorkUnit(
                    charge.work_unit_id,
                ));
            }
            if charge.class == CausalWorkClassV1::Residue {
                match &self.residue_policy {
                    ResiduePolicyV1::AssignToResidue {
                        residue_work_unit_id,
                        ..
                    } if charge.work_unit_id == *residue_work_unit_id => {}
                    _ => return Err(CausalWorkErrorV1::ResidueWithoutPolicy),
                }
            }
            totals.add(charge.class, charge.amount)?;
        }
        if self.charges.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(CausalWorkErrorV1::NonCanonicalCharges);
        }
        if totals != self.class_totals {
            return Err(CausalWorkErrorV1::ClassTotalsMismatch);
        }
        let observed = self.measurement.delta()?;
        let classified = totals.checked_total()?;
        if observed != self.observed_total
            || classified != self.classified_total
            || observed != classified
        {
            return Err(CausalWorkErrorV1::NonConservation {
                observed,
                classified,
            });
        }
        if self.compute_digest()? != self.receipt_digest {
            return Err(CausalWorkErrorV1::ReceiptDigestMismatch);
        }
        Ok(())
    }

    pub fn compute_digest(&self) -> Result<DigestV1, CausalWorkErrorV1> {
        let mut value = serde_json::to_value(self)
            .map_err(|error| CausalWorkErrorV1::Json(error.to_string()))?;
        value
            .as_object_mut()
            .ok_or_else(|| CausalWorkErrorV1::Json("receipt must be object".into()))?
            .remove("receipt_digest");
        let mut bytes = b"zerostack.causal_work_receipt.v1\0".to_vec();
        bytes.extend_from_slice(canonical_json(&value).as_bytes());
        Ok(DigestV1::from_bytes(sha256(&bytes)))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterEvidenceModeV1 {
    Synthetic,
    RchCompilation,
    Native,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CounterCorrespondenceReceiptV1 {
    platform_profile: String,
    evidence_mode: CounterEvidenceModeV1,
    identity: ParentCounterIdentityV1,
    parent_window: ParentCounterWindowV1,
    adapter_observed_delta: u64,
    adapter_binary_digest: DigestV1,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CounterCorrespondenceReceiptWireV1 {
    platform_profile: String,
    evidence_mode: CounterEvidenceModeV1,
    identity: ParentCounterIdentityV1,
    parent_window: ParentCounterWindowV1,
    adapter_observed_delta: u64,
    adapter_binary_digest: DigestV1,
}

impl<'de> Deserialize<'de> for CounterCorrespondenceReceiptV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = CounterCorrespondenceReceiptWireV1::deserialize(deserializer)?;
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

impl CounterCorrespondenceReceiptV1 {
    pub fn new(
        platform_profile: String,
        evidence_mode: CounterEvidenceModeV1,
        identity: ParentCounterIdentityV1,
        parent_window: ParentCounterWindowV1,
        adapter_observed_delta: u64,
        adapter_binary_digest: DigestV1,
    ) -> Result<Self, CausalWorkErrorV1> {
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

    pub fn validate(&self) -> Result<(), CausalWorkErrorV1> {
        validate_text("platform_profile", &self.platform_profile)?;
        validate_identity(&self.identity)?;
        if self.parent_window.identity != self.identity
            || self.parent_window.delta()? != self.adapter_observed_delta
            || self.identity.adapter_digest != self.adapter_binary_digest
        {
            return Err(CausalWorkErrorV1::CounterCorrespondenceMismatch);
        }
        Ok(())
    }

    pub const fn is_native_evidence(&self) -> bool {
        matches!(self.evidence_mode, CounterEvidenceModeV1::Native)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyChargeClassV2 {
    Billed,
    FailedTrial,
    Retry,
    Recovery,
    Reexpansion,
    Fallback,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyClassMappingV1 {
    pub legacy: LegacyChargeClassV2,
    pub suggested_causal_class: CausalWorkClassV1,
    pub requires_remeasurement: bool,
    pub measured_fact: bool,
}

pub const fn map_legacy_class_v2(legacy: LegacyChargeClassV2) -> LegacyClassMappingV1 {
    let suggested_causal_class = match legacy {
        LegacyChargeClassV2::Billed | LegacyChargeClassV2::FailedTrial => {
            CausalWorkClassV1::Candidate
        }
        LegacyChargeClassV2::Retry | LegacyChargeClassV2::Reexpansion => {
            CausalWorkClassV1::Restoration
        }
        LegacyChargeClassV2::Recovery => CausalWorkClassV1::Verification,
        LegacyChargeClassV2::Fallback => CausalWorkClassV1::Fallback,
    };
    LegacyClassMappingV1 {
        legacy,
        suggested_causal_class,
        requires_remeasurement: true,
        measured_fact: false,
    }
}

fn validate_identity(identity: &ParentCounterIdentityV1) -> Result<(), CausalWorkErrorV1> {
    validate_text("counter_id", &identity.counter_id)
}

fn validate_text(field: &'static str, text: &str) -> Result<(), CausalWorkErrorV1> {
    if text.is_empty()
        || text.len() > CAUSAL_WORK_MAX_ID_BYTES
        || text.chars().any(char::is_control)
    {
        return Err(CausalWorkErrorV1::InvalidText(field));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CausalWorkFailureCodeV1 {
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
pub enum CausalWorkErrorV1 {
    UnsupportedVersion,
    CounterRegressed { start: u64, end: u64 },
    CounterOverflow,
    InvalidText(&'static str),
    TooManyCharges,
    DoubleClassifiedWorkUnit(DigestV1),
    ZeroAmountCharge(DigestV1),
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

impl CausalWorkErrorV1 {
    pub const fn code(&self) -> CausalWorkFailureCodeV1 {
        match self {
            Self::UnsupportedVersion => CausalWorkFailureCodeV1::UnsupportedVersion,
            Self::CounterRegressed { .. } => CausalWorkFailureCodeV1::CounterRegressed,
            Self::CounterOverflow => CausalWorkFailureCodeV1::CounterOverflow,
            Self::InvalidText(_) => CausalWorkFailureCodeV1::InvalidText,
            Self::TooManyCharges => CausalWorkFailureCodeV1::TooManyCharges,
            Self::DoubleClassifiedWorkUnit(_) => CausalWorkFailureCodeV1::DoubleClassifiedWorkUnit,
            Self::ZeroAmountCharge(_) => CausalWorkFailureCodeV1::ZeroAmountCharge,
            Self::ChargesForUnmeasuredCounter => {
                CausalWorkFailureCodeV1::ChargesForUnmeasuredCounter
            }
            Self::ResidueWithoutPolicy => CausalWorkFailureCodeV1::ResidueWithoutPolicy,
            Self::UnclassifiedWork { .. } => CausalWorkFailureCodeV1::UnclassifiedWork,
            Self::NonConservation { .. } => CausalWorkFailureCodeV1::NonConservation,
            Self::NonCanonicalCharges => CausalWorkFailureCodeV1::NonCanonicalCharges,
            Self::ClassTotalsMismatch => CausalWorkFailureCodeV1::ClassTotalsMismatch,
            Self::ReceiptDigestMismatch => CausalWorkFailureCodeV1::ReceiptDigestMismatch,
            Self::CounterCorrespondenceMismatch => {
                CausalWorkFailureCodeV1::CounterCorrespondenceMismatch
            }
            Self::Json(_) => CausalWorkFailureCodeV1::Json,
        }
    }
}

impl fmt::Display for CausalWorkErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "causal-work ledger failure: {:?}", self.code())
    }
}

impl Error for CausalWorkErrorV1 {}

pub fn causal_work_contract_manifest_v1() -> Value {
    json!({
        "contract": "zerostack.causal_work",
        "taxonomy_version": CAUSAL_WORK_TAXONOMY_VERSION_V1,
        "receipt_schema": CAUSAL_WORK_RECEIPT_SCHEMA_V1,
        "classes": CausalWorkClassV1::ALL.map(CausalWorkClassV1::as_str),
        "arithmetic": "checked_u64_integer_only",
        "classification": "one_unique_work_unit_id_one_class",
        "wire_decode": "all_receipt_invariants_validated_during_deserialization",
        "correspondence_decode": "validated_constructor_and_deserializer_only",
        "measurement_namespace": "parent_counter_observation",
        "estimate_namespace": "declared_estimate_nonconvertible_to_measurement",
        "unavailable_counter": "unmeasured_not_zero",
        "residue": "reject_or_preregistered_assign_to_residue",
        "residue_work_unit_id": "every_residue_charge_equals_preregistered_id",
        "legacy_v2": "readable_mapping_requires_remeasurement_and_is_not_fact"
    })
}

pub fn causal_work_contract_digest_v1() -> DigestV1 {
    DigestV1::from_bytes(sha256(
        canonical_json(&causal_work_contract_manifest_v1()).as_bytes(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn identity() -> ParentCounterIdentityV1 {
        ParentCounterIdentityV1 {
            counter_id: "parent.cpu_ns".into(),
            unit: CausalCounterUnitV1::CpuNanoseconds,
            boundary_digest: d(1),
            adapter_digest: d(2),
            platform_profile_digest: d(3),
        }
    }

    fn measured(total: u64) -> ParentCounterObservationV1 {
        ParentCounterObservationV1::Measured {
            window: ParentCounterWindowV1 {
                identity: identity(),
                start: 100,
                end: 100 + total,
            },
        }
    }

    fn charge(byte: u8, class: CausalWorkClassV1, amount: u64) -> CausalWorkChargeV1 {
        CausalWorkChargeV1 {
            work_unit_id: d(byte),
            class,
            amount,
        }
    }

    #[test]
    fn causal_classes_are_exactly_eight_and_contract_is_stable() {
        assert_eq!(CausalWorkClassV1::ALL.len(), 8);
        assert_eq!(
            causal_work_contract_digest_v1().to_hex(),
            "094be0570d982ab1817b8296e403db516fd43cfa5162014a9532e645b4a2eb82"
        );
    }

    #[test]
    fn causal_classes_conserve_and_preregistered_residue_closes() {
        let outcome = CausalWorkReceiptV1::build(
            d(9),
            measured(10),
            vec![
                charge(1, CausalWorkClassV1::Candidate, 3),
                charge(2, CausalWorkClassV1::Verification, 2),
            ],
            ResiduePolicyV1::AssignToResidue {
                policy_id: "unattributed-parent-delta.v1".into(),
                policy_digest: d(4),
                residue_work_unit_id: d(8),
            },
        )
        .unwrap();
        let CausalWorkOutcomeV1::Measured { receipt } = outcome else {
            panic!("measurement must produce receipt")
        };
        assert_eq!(receipt.class_totals.residue, 5);
        assert_eq!(receipt.classified_total, 10);
        receipt.validate().unwrap();
        assert_eq!(
            receipt.receipt_digest.to_hex(),
            "33763c6ab2d3c9374d3238ed54d2930e014cd49b61befb42eca8ab623fef72bf"
        );

        let valid_wire = serde_json::to_value(&receipt).unwrap();
        let mut bad_version = valid_wire.clone();
        bad_version["schema_version"] = json!(2);
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_version).is_err());
        let mut bad_total = valid_wire.clone();
        bad_total["observed_total"] = json!(9);
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_total).is_err());
        let mut bad_order = valid_wire.clone();
        bad_order["charges"].as_array_mut().unwrap().reverse();
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_order).is_err());
        let mut bad_digest = valid_wire;
        bad_digest["receipt_digest"] = json!(d(0));
        assert!(serde_json::from_value::<CausalWorkReceiptV1>(bad_digest).is_err());
    }

    #[test]
    fn causal_classes_reject_dual_missing_overflow_and_estimate_alias() {
        assert_eq!(
            CausalWorkReceiptV1::build(
                d(9),
                measured(2),
                vec![
                    charge(1, CausalWorkClassV1::Candidate, 1),
                    charge(1, CausalWorkClassV1::Fallback, 1),
                ],
                ResiduePolicyV1::RejectUnclassified,
            )
            .unwrap_err()
            .code(),
            CausalWorkFailureCodeV1::DoubleClassifiedWorkUnit
        );
        assert_eq!(
            CausalWorkReceiptV1::build(
                d(9),
                measured(2),
                vec![charge(1, CausalWorkClassV1::Candidate, 1)],
                ResiduePolicyV1::RejectUnclassified,
            )
            .unwrap_err()
            .code(),
            CausalWorkFailureCodeV1::UnclassifiedWork
        );
        assert_eq!(
            CausalClassTotalsV1 {
                candidate: u64::MAX,
                verification: 1,
                ..Default::default()
            }
            .checked_total()
            .unwrap_err()
            .code(),
            CausalWorkFailureCodeV1::CounterOverflow
        );
        let estimate = json!({
            "estimator_id": "declared",
            "identity": identity(),
            "declared_value": 1.5,
            "assumptions_digest": d(5)
        });
        assert!(serde_json::from_value::<DeclaredEstimateV1>(estimate.clone()).is_err());
        assert!(serde_json::from_value::<ParentCounterObservationV1>(estimate).is_err());
    }

    #[test]
    fn causal_classes_unavailable_is_unmeasured_not_zero() {
        let outcome = CausalWorkReceiptV1::build(
            d(9),
            ParentCounterObservationV1::Unmeasured {
                identity: identity(),
                reason: "counter unavailable".into(),
            },
            Vec::new(),
            ResiduePolicyV1::RejectUnclassified,
        )
        .unwrap();
        assert!(matches!(outcome, CausalWorkOutcomeV1::Unmeasured { .. }));
    }

    #[test]
    fn causal_classes_archived_v2_fixture_stays_readable_without_rewrite() {
        const ARCHIVE: &[u8] = include_bytes!("../tests/fixtures/token-ledger-v2-archive.json");
        let preserved = ARCHIVE.to_vec();
        let ledger: crate::TokenLedger = serde_json::from_slice(ARCHIVE).unwrap();
        assert_eq!(ledger.billed_tokens, 6);
        assert_eq!(ledger.failed_trial_tokens, 3);
        assert_eq!(ledger.retry_tokens, 2);
        assert_eq!(ledger.recovery_tokens, 4);
        assert_eq!(ledger.reexpansion_tokens, 1);
        assert_eq!(ledger.fallback_tokens, 5);
        assert_eq!(ledger.check_accounting_complete().unwrap(), 21);
        assert_eq!(ARCHIVE, preserved.as_slice());
        assert_eq!(
            DigestV1::from_bytes(sha256(ARCHIVE)).to_hex(),
            "650b5e225689e57a142d815b4b6e709b02b58f2c5ed81b8d30405ede8cbd331d"
        );
    }

    #[test]
    fn causal_classes_legacy_mapping_never_becomes_fact() {
        for legacy in [
            LegacyChargeClassV2::Billed,
            LegacyChargeClassV2::FailedTrial,
            LegacyChargeClassV2::Retry,
            LegacyChargeClassV2::Recovery,
            LegacyChargeClassV2::Reexpansion,
            LegacyChargeClassV2::Fallback,
        ] {
            let mapping = map_legacy_class_v2(legacy);
            assert!(mapping.requires_remeasurement);
            assert!(!mapping.measured_fact);
        }
    }
}
