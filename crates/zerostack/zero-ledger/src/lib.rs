#![forbid(unsafe_code)]

//! RACC resource gauge, T8 exposure/replay identity, and exact-phase dominance receipt.
//!
//! This crate is the single home for RACC token accounting arithmetic so that
//! TokenZero, FSZero, GraphZero and the CodeMode host all account identically.
//! The crate implements the accounting model documented in `docs/racc/RACC.md`.
//!
//! Properties enforced here:
//!
//! - **Locked tokenizer gauge.** Every charge carries a [`TokenizerIdentity`];
//!   mixing identities is a typed error ([`LedgerError::TokenizerIdentityMismatch`]).
//! - **Append-only monotone counters.** [`ResourceGauge`] exposes no API to
//!   decrement or rewrite history; overflow is a typed error, never a wrap.
//! - **Legacy v2 token surface stays readable.** Every model-visible call
//!   declares its locked-tokenizer input once in [`TokenCharge::input_tokens`]
//!   and splits it across the six [`ChargeClass`] variants. This is the archived
//!   v2 per-surface report and remains wire-compatible, but it is **not** the
//!   complete causal authority: the six classes can omit or double-class
//!   fallback, restoration, prewarm, and residue work, and declared estimates
//!   can masquerade as measured facts. Unclassified and double-counted input
//!   remain typed errors on this surface ([`LedgerError::UnclassifiedInput`],
//!   [`LedgerError::DoubleCountedInput`]).
//! - **Complete causal authority is versioned and exclusive.** [`causal_work`]
//!   replaces the six token classes as complete authority with exactly-one
//!   classification across candidate, verification, comparison, baseline,
//!   fallback, restoration, prewarm, and residue. Receipts bind one
//!   parent-measured integer counter window and a preregistered residue policy;
//!   declared estimates live in a separate namespace ([`DeclaredEstimate`])
//!   that can never construct a measured receipt, and an unavailable counter is
//!   [`ParentCounterObservation::Unmeasured`], never zero. Legacy v2 classes
//!   map readably without rewriting archives ([`map_legacy_class`]).
//! - **Integer-only arithmetic.** Retained fractions are parts-per-million
//!   integers widened to u128; there are no floats and no percentage strings.
//!   [`RetainedFractionPpm`] is range-validated at construction and on the wire.
//! - **Unforgeable exactness gates.** [`ExactnessGates`] has no public boolean
//!   setter: each gate is raised only by presenting a verified evidence handle
//!   ([`ArchiveAttestation`], [`PolicyEvidence`], [`TaskAcceptanceReceipt`]).
//! - **V6 metric completion.** [`resource_classes`] adds typed non-token
//!   ledger classes with honest exactness labels and provider-bill
//!   reconciliation; [`charging_maps`] stores the disjoint six-phase
//!   lower-bound maps with overlap checking and Gamma closure;
//!   [`campaign`] allocates cold-build cost over reuse campaigns;
//!   [`frontier`] computes the normalized Frontier Closure decomposition.
//! - **No I/O, sync only.** charge() is a handful of integer adds and performs
//!   no allocation.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

pub mod campaign;
pub mod causal_work;
pub mod charging_maps;
pub mod fresh_work;
pub mod frontier;
pub mod resource_classes;
pub mod usage;

pub use campaign::{CampaignError, ReuseCampaign};

pub use causal_work::{
    CAUSAL_WORK_MAX_CHARGES, CAUSAL_WORK_MAX_ID_BYTES, CAUSAL_WORK_RECEIPT_SCHEMA,
    CAUSAL_WORK_TAXONOMY_VERSION, CausalClassTotals, CausalCounterUnit, CausalWorkCharge,
    CausalWorkClass, CausalWorkError, CausalWorkFailureCode, CausalWorkOutcome,
    CausalWorkReceipt, CounterCorrespondenceReceipt, CounterEvidenceMode, DeclaredEstimate,
    LegacyChargeClass, LegacyClassMapping, ParentCounterIdentity, ParentCounterObservation,
    ParentCounterWindow, ResiduePolicy, causal_work_contract_digest,
    causal_work_contract_manifest, map_legacy_class,
};

pub use charging_maps::{
    ChargingEntry, ChargingMap, ChargingMapError, ChargingMapSet, ChargingPhase, ClosureReport,
    PhasePolicy,
};

pub use frontier::{FrontierClosure, FrontierError, FrontierTerm, LimitingBurden};

pub use resource_classes::{
    BillLineReconciliation, BillLineStatus, MeasurementSource, ProviderBillLine,
    ProviderBillReconciliation, ReconciliationState, ResourceClass, ResourceClassParseError,
    ResourceLedger, ResourceRow, ResourceTotal,
};

pub use fresh_work::{ActionFreshWork, FreshWorkComponent, FreshWorkVector, SessionFreshWork};

/// Parts per million denominator used by every retained-fraction comparison.
pub const PPM_ONE: u32 = 1_000_000;

/// Canonical-JSON receipt schema version.
///
/// Version 2 adds schema_version, the per-class charge breakdown and the derived
/// racc_input_tokens total; version 1 carried a single caller-supplied
/// racc_input_tokens field inside the ledger.
pub const RECEIPT_SCHEMA_VERSION: u32 = 2;

/// A 32-byte content digest, wired as lowercase hex.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Default)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    /// Renders the digest as 64 lowercase hex characters.
    pub fn to_hex(self) -> String {
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(nibble(byte >> 4));
            out.push(nibble(byte & 0x0f));
        }
        out
    }

    /// Parses 64 lowercase hex characters into a digest.
    pub fn from_hex(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 64 {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, chunk) in bytes.chunks_exact(2).enumerate() {
            let hi = unnibble(chunk[0])?;
            let lo = unnibble(chunk[1])?;
            out[i] = (hi << 4) | lo;
        }
        Some(Self(out))
    }
}

fn nibble(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'a' + value - 10) as char,
    }
}

fn unnibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Digest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct HexVisitor;
        impl Visitor<'_> for HexVisitor {
            type Value = Digest;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("64 lowercase hex characters")
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Digest, E> {
                Digest::from_hex(value)
                    .ok_or_else(|| de::Error::invalid_value(de::Unexpected::Str(value), &self))
            }
        }
        deserializer.deserialize_str(HexVisitor)
    }
}

/// Per-call input token budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TokenBudget(pub u64);

/// Budget granted for the next expansion round.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NextBudget(pub u64);

/// Retained input fraction expressed in parts per million.
///
/// RetainedFractionPpm::new(30_000) means keep at most 3% of the raw input
/// tokens. The value is never a float and never a percentage string (T5). The
/// inner value is private: the only ways to obtain one are [`Self::new`] and
/// deserialization, and both reject anything above [`PPM_ONE`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RetainedFractionPpm(u32);

impl RetainedFractionPpm {
    /// Constructs a retained fraction, rejecting values above 1_000_000 ppm.
    pub fn new(ppm: u32) -> Result<Self, LedgerError> {
        if ppm > PPM_ONE {
            return Err(LedgerError::PpmOutOfRange { ppm });
        }
        Ok(Self(ppm))
    }

    /// The validated parts-per-million value.
    pub fn ppm(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for RetainedFractionPpm {
    /// Range-validates the wire value: out-of-range ppm never survives decoding.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let ppm = u32::deserialize(deserializer)?;
        Self::new(ppm).map_err(|_| {
            de::Error::invalid_value(
                de::Unexpected::Unsigned(u64::from(ppm)),
                &"a retained fraction in 0..=1000000 ppm",
            )
        })
    }
}

/// The tokenizer gauge that all counts in one ledger are measured against.
///
/// T4/T14 require a locked gauge: token counts produced by different
/// tokenizers are incommensurable and must never be summed.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TokenizerIdentity {
    /// Stable tokenizer name, e.g. cl100k_base.
    pub tokenizer_id: String,
    /// Digest of the exact tokenizer artifact (vocab + merges + config).
    pub tokenizer_version_digest: Digest,
}

impl TokenizerIdentity {
    /// Convenience constructor.
    pub fn new(tokenizer_id: impl Into<String>, tokenizer_version_digest: Digest) -> Self {
        Self {
            tokenizer_id: tokenizer_id.into(),
            tokenizer_version_digest,
        }
    }
}

/// Immutable configuration of one ledger: the locked tokenizer gauge.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LedgerConfig {
    /// The single tokenizer identity every charge must agree with.
    pub tokenizer: TokenizerIdentity,
}

impl LedgerConfig {
    /// Locks a ledger to one tokenizer identity.
    pub fn new(tokenizer: TokenizerIdentity) -> Self {
        Self { tokenizer }
    }
}

/// Legacy v2 input-token charge classes (T8, section 2.2).
///
/// These six classes are the archived v2 per-surface report and stay readable
/// and wire-compatible, but they are not the complete causal authority: they
/// can omit or double-class fallback, restoration, prewarm, and residue work,
/// and declared estimates can masquerade as measured facts. Complete exactly-one
/// causal accounting lives in [`causal_work`] ([`CausalWorkClass`],
/// [`CausalWorkReceipt`]); [`map_legacy_class`] maps these classes readably
/// without rewriting archives.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ChargeClass {
    /// Input tokens of an accepted, billed rendering.
    Billed,
    /// Input tokens of a trial rendering that was not accepted.
    FailedTrial,
    /// Input tokens re-sent by a retry of a failed call.
    Retry,
    /// Input tokens spent verifying or recovering evidence.
    Recovery,
    /// Input tokens spent re-expanding previously compressed spans.
    Reexpansion,
    /// Input tokens of a raw fallback view.
    Fallback,
}

/// Transactional task-attempt disposition used to classify mandatory cost.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskAttemptDisposition {
    Passed,
    RolledBack,
}

impl ChargeClass {
    /// Every charge class, in canonical order.
    pub const ALL: [ChargeClass; 6] = [
        ChargeClass::Billed,
        ChargeClass::FailedTrial,
        ChargeClass::Retry,
        ChargeClass::Recovery,
        ChargeClass::Reexpansion,
        ChargeClass::Fallback,
    ];

    /// Name of the ledger counter this class accumulates into.
    pub fn counter_name(self) -> &'static str {
        match self {
            Self::Billed => "billed_tokens",
            Self::FailedTrial => "failed_trial_tokens",
            Self::Retry => "retry_tokens",
            Self::Recovery => "recovery_tokens",
            Self::Reexpansion => "reexpansion_tokens",
            Self::Fallback => "fallback_tokens",
        }
    }
}

impl fmt::Display for ChargeClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.counter_name())
    }
}

/// Cumulative, append-only token counters, tagged with the locked gauge.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenLedger {
    /// Tokenizer gauge these counts were measured with.
    pub tokenizer: TokenizerIdentity,
    /// Input tokens a raw (uncompressed) replay of the history would cost.
    pub raw_input_tokens: u64,
    /// Every model-visible input token rendered under RACC, declared once.
    pub declared_input_tokens: u64,
    /// Class Billed: input tokens of accepted renderings.
    pub billed_tokens: u64,
    /// Class FailedTrial: input tokens of trials that were not accepted.
    pub failed_trial_tokens: u64,
    /// Class Retry: input tokens re-sent by retries.
    pub retry_tokens: u64,
    /// Class Recovery: input tokens spent verifying or recovering evidence.
    pub recovery_tokens: u64,
    /// Class Reexpansion: input tokens spent re-expanding compressed spans.
    pub reexpansion_tokens: u64,
    /// Class Fallback: input tokens spent on raw fallback views.
    pub fallback_tokens: u64,
    /// Model output tokens.
    pub model_output_tokens: u64,
    /// Number of model calls.
    pub model_calls: u64,
    /// Number of retried calls.
    pub retries: u64,
    /// Cumulative fresh-work decomposition of the declared input.
    #[serde(default)]
    pub fresh_work: FreshWorkVector,
    /// Number of charges that declared a fresh-work vector.
    #[serde(default)]
    pub fresh_work_actions: u64,
}

impl TokenLedger {
    /// An empty ledger tagged with the given tokenizer identity.
    pub fn empty(tokenizer: TokenizerIdentity) -> Self {
        Self {
            tokenizer,
            raw_input_tokens: 0,
            declared_input_tokens: 0,
            billed_tokens: 0,
            failed_trial_tokens: 0,
            retry_tokens: 0,
            recovery_tokens: 0,
            reexpansion_tokens: 0,
            fallback_tokens: 0,
            model_output_tokens: 0,
            model_calls: 0,
            retries: 0,
            fresh_work: FreshWorkVector::default(),
            fresh_work_actions: 0,
        }
    }

    /// Cumulative tokens recorded under one charge class.
    pub fn class_tokens(&self, class: ChargeClass) -> u64 {
        match class {
            ChargeClass::Billed => self.billed_tokens,
            ChargeClass::FailedTrial => self.failed_trial_tokens,
            ChargeClass::Retry => self.retry_tokens,
            ChargeClass::Recovery => self.recovery_tokens,
            ChargeClass::Reexpansion => self.reexpansion_tokens,
            ChargeClass::Fallback => self.fallback_tokens,
        }
    }

    /// Total RACC input exposure: the sum over every charge class.
    ///
    /// This is the C of the exact-phase certificate. Nothing model-visible is
    /// excluded, so hiding cost in a side counter cannot lower it.
    pub fn racc_input_tokens(&self) -> Result<u64, LedgerError> {
        let mut total = 0u64;
        for class in ChargeClass::ALL {
            total = add(total, self.class_tokens(class), class.counter_name())?;
        }
        Ok(total)
    }

    /// Verifies that every declared input token is classified exactly once.
    ///
    /// Returns the class total on success. Declared input above the classified
    /// sum means a call was left unclassified; a classified sum above the
    /// declared input means a call was counted twice.
    pub fn check_accounting_complete(&self) -> Result<u64, LedgerError> {
        let classified = self.racc_input_tokens()?;
        if self.declared_input_tokens > classified {
            return Err(LedgerError::UnclassifiedInput {
                declared: self.declared_input_tokens,
                classified,
            });
        }
        if classified > self.declared_input_tokens {
            return Err(LedgerError::DoubleCountedInput {
                declared: self.declared_input_tokens,
                classified,
            });
        }
        Ok(classified)
    }

    fn apply_charge_totals(&mut self, totals: TokenCharge) {
        self.raw_input_tokens = totals.raw_input_tokens;
        self.declared_input_tokens = totals.input_tokens;
        self.billed_tokens = totals.billed_tokens;
        self.failed_trial_tokens = totals.failed_trial_tokens;
        self.retry_tokens = totals.retry_tokens;
        self.recovery_tokens = totals.recovery_tokens;
        self.reexpansion_tokens = totals.reexpansion_tokens;
        self.fallback_tokens = totals.fallback_tokens;
        self.model_output_tokens = totals.model_output_tokens;
        self.model_calls = totals.model_calls;
        self.retries = totals.retries;
        self.fresh_work = totals.fresh_work;
    }
}

/// One append-only charge against a resource gauge.
///
/// input_tokens is the total locked-tokenizer input this call made visible to
/// the model, and must equal the sum of the per-class fields. All fields default
/// to zero, so an empty charge is trivially reconciled.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TokenCharge {
    /// Raw-replay input tokens attributable to this call.
    pub raw_input_tokens: u64,
    /// Declared model-visible input tokens for this call.
    pub input_tokens: u64,
    /// Class Billed.
    pub billed_tokens: u64,
    /// Class FailedTrial.
    pub failed_trial_tokens: u64,
    /// Class Retry.
    pub retry_tokens: u64,
    /// Class Recovery.
    pub recovery_tokens: u64,
    /// Class Reexpansion.
    pub reexpansion_tokens: u64,
    /// Class Fallback.
    pub fallback_tokens: u64,
    /// Output tokens produced by this call.
    pub model_output_tokens: u64,
    /// Model calls represented (normally 1).
    pub model_calls: u64,
    /// Retries represented.
    pub retries: u64,
    /// Fresh-work decomposition of this call's declared input.
    ///
    /// Either the all-zero (undeclared) vector, or a vector whose component
    /// sum equals `input_tokens`.
    pub fresh_work: FreshWorkVector,
}

impl TokenCharge {
    /// Tokens this charge attributes to one class.
    pub fn class_tokens(&self, class: ChargeClass) -> u64 {
        match class {
            ChargeClass::Billed => self.billed_tokens,
            ChargeClass::FailedTrial => self.failed_trial_tokens,
            ChargeClass::Retry => self.retry_tokens,
            ChargeClass::Recovery => self.recovery_tokens,
            ChargeClass::Reexpansion => self.reexpansion_tokens,
            ChargeClass::Fallback => self.fallback_tokens,
        }
    }

    /// Sum of the per-class attributions.
    pub fn classified_tokens(&self) -> Result<u64, LedgerError> {
        let mut total = 0u64;
        for class in ChargeClass::ALL {
            total = add(total, self.class_tokens(class), class.counter_name())?;
        }
        Ok(total)
    }

    /// Verifies the declared input equals the per-class attribution.
    pub fn check_classification(&self) -> Result<u64, LedgerError> {
        let classified = self.classified_tokens()?;
        if self.input_tokens > classified {
            return Err(LedgerError::UnclassifiedInput {
                declared: self.input_tokens,
                classified,
            });
        }
        if classified > self.input_tokens {
            return Err(LedgerError::DoubleCountedInput {
                declared: self.input_tokens,
                classified,
            });
        }
        self.check_fresh_work(classified)?;
        Ok(classified)
    }

    /// Verifies the fresh-work vector, when declared, decomposes exactly the
    /// declared input of this call.
    pub fn check_fresh_work(&self, declared: u64) -> Result<(), LedgerError> {
        if !self.fresh_work.is_declared() {
            return Ok(());
        }
        let decomposed = self.fresh_work.component_sum()?;
        if decomposed != declared {
            return Err(LedgerError::FreshWorkTotalMismatch {
                declared,
                decomposed,
            });
        }
        Ok(())
    }

    fn accumulate(&self, ledger: &TokenLedger) -> Result<Self, LedgerError> {
        let mut totals = Self::default();
        self.accumulate_input_totals(ledger, &mut totals)?;
        self.accumulate_class_totals(ledger, &mut totals)?;
        self.accumulate_result_totals(ledger, &mut totals)?;
        Ok(totals)
    }

    fn accumulate_input_totals(
        &self,
        ledger: &TokenLedger,
        totals: &mut Self,
    ) -> Result<(), LedgerError> {
        totals.raw_input_tokens = add(
            ledger.raw_input_tokens,
            self.raw_input_tokens,
            "raw_input_tokens",
        )?;
        totals.input_tokens = add(
            ledger.declared_input_tokens,
            self.input_tokens,
            "declared_input_tokens",
        )?;
        Ok(())
    }

    fn accumulate_class_totals(
        &self,
        ledger: &TokenLedger,
        totals: &mut Self,
    ) -> Result<(), LedgerError> {
        totals.billed_tokens = add(ledger.billed_tokens, self.billed_tokens, "billed_tokens")?;
        totals.failed_trial_tokens = add(
            ledger.failed_trial_tokens,
            self.failed_trial_tokens,
            "failed_trial_tokens",
        )?;
        totals.retry_tokens = add(ledger.retry_tokens, self.retry_tokens, "retry_tokens")?;
        totals.recovery_tokens = add(
            ledger.recovery_tokens,
            self.recovery_tokens,
            "recovery_tokens",
        )?;
        totals.reexpansion_tokens = add(
            ledger.reexpansion_tokens,
            self.reexpansion_tokens,
            "reexpansion_tokens",
        )?;
        totals.fallback_tokens = add(
            ledger.fallback_tokens,
            self.fallback_tokens,
            "fallback_tokens",
        )?;
        Ok(())
    }

    fn accumulate_result_totals(
        &self,
        ledger: &TokenLedger,
        totals: &mut Self,
    ) -> Result<(), LedgerError> {
        totals.model_output_tokens = add(
            ledger.model_output_tokens,
            self.model_output_tokens,
            "model_output_tokens",
        )?;
        totals.model_calls = add(ledger.model_calls, self.model_calls, "model_calls")?;
        totals.retries = add(ledger.retries, self.retries, "retries")?;
        totals.fresh_work = ledger.fresh_work.merge(&self.fresh_work)?;
        Ok(())
    }
}

/// The RACC resource gauge: a locked tokenizer plus monotone counters.
///
/// There is deliberately no API to decrement, reset, or rewrite history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceGauge {
    config: LedgerConfig,
    ledger: TokenLedger,
    charges: u64,
}

impl ResourceGauge {
    /// Creates a gauge locked to the config's tokenizer identity.
    pub fn new(config: LedgerConfig) -> Self {
        let ledger = TokenLedger::empty(config.tokenizer.clone());
        Self {
            config,
            ledger,
            charges: 0,
        }
    }

    /// The locked configuration.
    pub fn config(&self) -> &LedgerConfig {
        &self.config
    }

    /// Read-only view of the cumulative counters.
    pub fn ledger(&self) -> &TokenLedger {
        &self.ledger
    }

    /// Number of accepted charges; monotone, one per successful charge().
    pub fn charge_count(&self) -> u64 {
        self.charges
    }

    /// Applies one append-only charge.
    ///
    /// Rejects counts measured with a different tokenizer identity, rejects a
    /// charge whose declared input is not classified exactly once, and rejects
    /// overflow rather than wrapping. On any error the ledger is unchanged, so
    /// the counters stay monotone. Performs no allocation and no I/O.
    /// Charges one successful or rolled-back task attempt through the existing
    /// exhaustive token accounting. A zero charge is rejected so speculation
    /// cannot become free; checked charge() arithmetic keeps counters monotone.
    pub fn charge_task_attempt(
        &mut self,
        tokenizer: &TokenizerIdentity,
        attempt_cost: u64,
        disposition: TaskAttemptDisposition,
    ) -> Result<(), LedgerError> {
        if attempt_cost == 0 {
            return Err(LedgerError::ZeroTaskAttemptCost);
        }
        let mut charge = TokenCharge {
            raw_input_tokens: attempt_cost,
            input_tokens: attempt_cost,
            ..TokenCharge::default()
        };
        match disposition {
            TaskAttemptDisposition::Passed => charge.billed_tokens = attempt_cost,
            TaskAttemptDisposition::RolledBack => charge.failed_trial_tokens = attempt_cost,
        }
        self.charge(tokenizer, &charge)
    }

    pub fn charge(
        &mut self,
        tokenizer: &TokenizerIdentity,
        charge: &TokenCharge,
    ) -> Result<(), LedgerError> {
        self.check_tokenizer(tokenizer)?;
        charge.check_classification()?;

        let totals = charge.accumulate(&self.ledger)?;
        let fresh_work_actions = if charge.fresh_work.is_declared() {
            add(self.ledger.fresh_work_actions, 1, "fresh_work_actions")?
        } else {
            self.ledger.fresh_work_actions
        };
        let charges = add(self.charges, 1, "charges")?;
        self.ledger.apply_charge_totals(totals);
        self.ledger.fresh_work_actions = fresh_work_actions;
        self.charges = charges;
        Ok(())
    }

    fn check_tokenizer(&self, tokenizer: &TokenizerIdentity) -> Result<(), LedgerError> {
        if tokenizer != &self.config.tokenizer {
            return Err(LedgerError::TokenizerIdentityMismatch {
                expected_id: self.config.tokenizer.tokenizer_id.clone(),
                expected_digest: self.config.tokenizer.tokenizer_version_digest,
                actual_id: tokenizer.tokenizer_id.clone(),
                actual_digest: tokenizer.tokenizer_version_digest,
            });
        }
        Ok(())
    }

    /// Seals the gauge into an ex-post dominance receipt.
    pub fn finalize_receipt(
        &self,
        target_retained_ppm: RetainedFractionPpm,
        roots: ReceiptRoots,
        exactness: ExactnessGates,
    ) -> Result<DominanceReceipt, ReceiptError> {
        if self.charges == 0 {
            return Err(ReceiptError::IncompleteLedger);
        }
        DominanceReceipt::seal(self.ledger.clone(), target_retained_ppm, roots, exactness)
    }
}

fn add(lhs: u64, rhs: u64, counter: &'static str) -> Result<u64, LedgerError> {
    lhs.checked_add(rhs)
        .ok_or(LedgerError::CounterOverflow { counter })
}

fn fold_root(tag: &str, leaves: &[Digest]) -> Digest {
    let mut buf = Vec::with_capacity(tag.len() + 1 + leaves.len() * 32);
    buf.extend_from_slice(tag.as_bytes());
    buf.push(0);
    for leaf in leaves {
        buf.extend_from_slice(&leaf.0);
    }
    Digest::from_hex(&zero_abi::sha256_hex(&buf)).expect("sha256_hex yields 64 lowercase hex")
}

/// Evidence that every retained span is byte-identical to the archived original.
///
/// The handle can only be built by presenting the span digests that fold to the
/// declared archive root, so a caller cannot assert byte-exactness it cannot
/// exhibit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveAttestation {
    archive_root: Digest,
}

impl ArchiveAttestation {
    /// Canonical archive root over the retained span digests.
    pub fn root_of(retained_span_digests: &[Digest]) -> Digest {
        fold_root("racc.archive", retained_span_digests)
    }

    /// Verifies the retained spans against the declared archive root.
    pub fn verify(
        archive_root: Digest,
        retained_span_digests: &[Digest],
    ) -> Result<Self, EvidenceError> {
        if retained_span_digests.is_empty() {
            return Err(EvidenceError::NoEvidence { kind: "archive" });
        }
        let actual = Self::root_of(retained_span_digests);
        if actual != archive_root {
            return Err(EvidenceError::RootMismatch {
                kind: "archive",
                declared: archive_root,
                actual,
            });
        }
        Ok(Self { archive_root })
    }

    /// The verified archive root.
    pub fn archive_root(&self) -> Digest {
        self.archive_root
    }
}

/// One per-decision policy outcome behind the policy-exactness gate.
///
/// These are policy-sufficiency or raw-fallback receipts (paper 8.2). They are
/// deliberately distinct from the T13 task-no-regret receipts of 8.3 / non-claim
/// 14.9: proving policy sufficiency is not proving task acceptance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyDecision {
    /// Policy sufficiency was proven for this decision.
    SufficiencyProven {
        /// Digest of the sufficiency witness.
        witness_digest: Digest,
    },
    /// No sufficiency proof was available, so a raw fallback view was served.
    RawFallbackServed {
        /// Digest of the raw view that was served.
        view_digest: Digest,
    },
}

impl PolicyDecision {
    fn leaf(self) -> Digest {
        match self {
            Self::SufficiencyProven { witness_digest } => {
                fold_root("racc.policy.proven", &[witness_digest])
            }
            Self::RawFallbackServed { view_digest } => {
                fold_root("racc.policy.fallback", &[view_digest])
            }
        }
    }
}

/// Evidence that every decision was policy-exact or served a raw fallback.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyEvidence {
    certificate_root: Digest,
}

impl PolicyEvidence {
    /// Canonical certificate root over the per-decision receipts.
    pub fn root_of(decisions: &[PolicyDecision]) -> Digest {
        let leaves: Vec<Digest> = decisions.iter().map(|d| d.leaf()).collect();
        fold_root("racc.policy", &leaves)
    }

    /// Verifies the per-decision receipts against the declared certificate root.
    pub fn verify(
        certificate_root: Digest,
        decisions: &[PolicyDecision],
    ) -> Result<Self, EvidenceError> {
        if decisions.is_empty() {
            return Err(EvidenceError::NoEvidence { kind: "policy" });
        }
        let actual = Self::root_of(decisions);
        if actual != certificate_root {
            return Err(EvidenceError::RootMismatch {
                kind: "policy",
                declared: certificate_root,
                actual,
            });
        }
        Ok(Self { certificate_root })
    }

    /// The verified certificate root.
    pub fn certificate_root(&self) -> Digest {
        self.certificate_root
    }
}

/// Outcome of one downstream task acceptance check (T13).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskOutcome {
    /// The task was accepted against its preregistered criterion.
    Accepted {
        /// Digest of the task acceptance record.
        task_digest: Digest,
    },
    /// The task regressed; no acceptance receipt may be minted.
    Regressed {
        /// Digest of the task acceptance record.
        task_digest: Digest,
    },
}

impl TaskOutcome {
    fn leaf(self) -> Digest {
        match self {
            Self::Accepted { task_digest } => fold_root("racc.task.accepted", &[task_digest]),
            Self::Regressed { task_digest } => fold_root("racc.task.regressed", &[task_digest]),
        }
    }

    fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted { .. })
    }
}

/// T13 task-acceptance receipt: every checked task was accepted.
///
/// Trusted boundary: the outcomes are minted by the transactional T13 protocol
/// (zero-gate). Until zero-gate lands, the caller that presents the outcomes is
/// the trusted party; this type is the seam that will be narrowed to zero-gate.
/// A task-acceptance receipt never substitutes for policy evidence (8.2/8.3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskAcceptanceReceipt {
    acceptance_root: Digest,
}

impl TaskAcceptanceReceipt {
    /// Canonical acceptance root over the per-task outcomes.
    pub fn root_of(outcomes: &[TaskOutcome]) -> Digest {
        let leaves: Vec<Digest> = outcomes.iter().map(|o| o.leaf()).collect();
        fold_root("racc.task", &leaves)
    }

    /// Verifies the outcomes against the declared root and rejects regressions.
    pub fn verify(
        acceptance_root: Digest,
        outcomes: &[TaskOutcome],
    ) -> Result<Self, EvidenceError> {
        if outcomes.is_empty() {
            return Err(EvidenceError::NoEvidence { kind: "task" });
        }
        if !outcomes.iter().all(|o| o.is_accepted()) {
            return Err(EvidenceError::TaskRegressed);
        }
        let actual = Self::root_of(outcomes);
        if actual != acceptance_root {
            return Err(EvidenceError::RootMismatch {
                kind: "task",
                declared: acceptance_root,
                actual,
            });
        }
        Ok(Self { acceptance_root })
    }

    /// The verified acceptance root.
    pub fn acceptance_root(&self) -> Digest {
        self.acceptance_root
    }
}

/// Merkle roots a receipt is anchored to.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReceiptRoots {
    /// Root of the byte-exact archive.
    pub archive_root: Digest,
    /// Root of the evidence certificate set.
    pub certificate_root: Digest,
}

/// The three non-arithmetic exactness gates of the phase certificate.
///
/// The booleans are private and there is no public setter: a gate is raised only
/// by presenting the corresponding verified evidence handle. Default is all-false.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExactnessGates {
    byte_exact: Option<Digest>,
    policy_exact_or_fallback: Option<Digest>,
    task_verified: Option<Digest>,
}

impl ExactnessGates {
    /// Raises the byte-exactness gate from a verified archive attestation.
    pub fn with_byte_exact(mut self, attestation: &ArchiveAttestation) -> Self {
        self.byte_exact = Some(attestation.archive_root());
        self
    }

    /// Raises the policy-exactness gate from verified per-decision receipts.
    pub fn with_policy_exact_or_fallback(mut self, evidence: &PolicyEvidence) -> Self {
        self.policy_exact_or_fallback = Some(evidence.certificate_root());
        self
    }

    /// Raises the task gate from a T13 task-acceptance receipt.
    pub fn with_task_verified(mut self, receipt: &TaskAcceptanceReceipt) -> Self {
        self.task_verified = Some(receipt.acceptance_root());
        self
    }

    /// Whether the byte-exactness gate is backed by evidence.
    pub fn byte_exact(&self) -> bool {
        self.byte_exact.is_some()
    }

    /// Whether the policy-exactness gate is backed by evidence.
    pub fn policy_exact_or_fallback(&self) -> bool {
        self.policy_exact_or_fallback.is_some()
    }

    /// Whether the task gate is backed by evidence.
    pub fn task_verified(&self) -> bool {
        self.task_verified.is_some()
    }
}

/// Ex-post exact-phase certificate: C <= epsilon * R plus exactness gates.
///
/// Constructed only by [`DominanceReceipt::seal`] or [`ResourceGauge::finalize_receipt`].
/// A receipt decoded from the wire is a claim, not a proof: its gates are only
/// meaningful once the archive and certificate roots are re-verified against
/// evidence handles.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DominanceReceipt {
    /// Canonical-JSON schema version.
    pub schema_version: u32,
    /// Cumulative counters, tagged with the locked tokenizer gauge.
    pub ledger: TokenLedger,
    /// Total RACC input exposure, summed over every charge class.
    pub racc_input_tokens: u64,
    /// Preregistered retained-fraction target, in ppm.
    pub target_retained_ppm: RetainedFractionPpm,
    /// Root of the byte-exact archive.
    pub archive_root: Digest,
    /// Root of the evidence certificate set.
    pub certificate_root: Digest,
    byte_exact: bool,
    policy_exact_or_fallback: bool,
    task_verified: bool,
}

impl DominanceReceipt {
    /// Seals a ledger into a receipt.
    ///
    /// Rejects a zero raw baseline (R = 0 certifies nothing), rejects a ledger
    /// whose declared input is not classified exactly once, and rejects gates
    /// whose evidence roots disagree with the receipt roots.
    pub fn seal(
        ledger: TokenLedger,
        target_retained_ppm: RetainedFractionPpm,
        roots: ReceiptRoots,
        exactness: ExactnessGates,
    ) -> Result<Self, ReceiptError> {
        if ledger.raw_input_tokens == 0 {
            return Err(ReceiptError::IncompleteLedger);
        }
        let racc_input_tokens = ledger
            .check_accounting_complete()
            .map_err(ReceiptError::Accounting)?;
        if let Some(root) = exactness.byte_exact
            && root != roots.archive_root
        {
            return Err(ReceiptError::EvidenceRootMismatch {
                kind: "archive",
                receipt: roots.archive_root,
                evidence: root,
            });
        }
        if let Some(root) = exactness.policy_exact_or_fallback
            && root != roots.certificate_root
        {
            return Err(ReceiptError::EvidenceRootMismatch {
                kind: "policy",
                receipt: roots.certificate_root,
                evidence: root,
            });
        }
        Ok(Self {
            schema_version: RECEIPT_SCHEMA_VERSION,
            ledger,
            racc_input_tokens,
            target_retained_ppm,
            archive_root: roots.archive_root,
            certificate_root: roots.certificate_root,
            byte_exact: exactness.byte_exact(),
            policy_exact_or_fallback: exactness.policy_exact_or_fallback(),
            task_verified: exactness.task_verified(),
        })
    }

    /// Whether every retained span was attested byte-identical.
    pub fn byte_exact(&self) -> bool {
        self.byte_exact
    }

    /// Whether every decision was policy-exact or served a raw fallback.
    pub fn policy_exact_or_fallback(&self) -> bool {
        self.policy_exact_or_fallback
    }

    /// Whether the downstream T13 task acceptance check passed.
    pub fn task_verified(&self) -> bool {
        self.task_verified
    }

    /// Pure arithmetic part of the ex-post phase certificate.
    ///
    /// Checks racc_input_tokens * 1_000_000 <= raw_input_tokens * target_ppm
    /// with u128 widening, exactly as RACC_CONTRACT.rs specifies. A zero raw
    /// baseline is always false: with R = 0 there is no phase to certify.
    pub fn meets_token_target(&self) -> bool {
        if self.ledger.raw_input_tokens == 0 {
            return false;
        }
        let exposure = match self.ledger.racc_input_tokens() {
            Ok(exposure) => exposure,
            Err(_) => return false,
        };
        if exposure != self.racc_input_tokens {
            return false;
        }
        let lhs = u128::from(exposure) * u128::from(PPM_ONE);
        let rhs =
            u128::from(self.ledger.raw_input_tokens) * u128::from(self.target_retained_ppm.ppm());
        lhs <= rhs
    }

    /// The full exact-phase predicate: arithmetic target plus exactness gates.
    pub fn exact_phase_valid(&self) -> bool {
        self.byte_exact
            && self.policy_exact_or_fallback
            && self.task_verified
            && self.meets_token_target()
    }

    /// Achieved retained fraction, rounded up to the next ppm.
    ///
    /// Returns None when the raw baseline is zero (no ratio is defined) or the
    /// exposure sum overflows.
    pub fn achieved_retained_ppm_ceil(&self) -> Option<u128> {
        let raw = u128::from(self.ledger.raw_input_tokens);
        if raw == 0 {
            return None;
        }
        let exposure = self.ledger.racc_input_tokens().ok()?;
        let numerator = u128::from(exposure) * u128::from(PPM_ONE);
        Some(numerator.div_ceil(raw))
    }

    /// Canonical JSON encoding of the receipt, via zero-abi key ordering.
    pub fn to_canonical_json(&self) -> Result<String, ReceiptError> {
        let value = serde_json::to_value(self).map_err(|_| ReceiptError::Encoding)?;
        Ok(zero_abi::canonical_json(&value))
    }

    /// Stable digest of the canonical JSON encoding.
    pub fn canonical_digest_hex(&self) -> Result<String, ReceiptError> {
        Ok(zero_abi::sha256_hex(self.to_canonical_json()?.as_bytes()))
    }
}

/// One block of archived content with its raw and RACC exposure multiplicities.
///
/// In the paper's notation a block contributes b_i tokens, is exposed r_i
/// times under a raw replay and d_i times under RACC.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExposureBlock {
    /// Token size of the block, b_i.
    pub block_tokens: u64,
    /// Raw replay exposure count, r_i.
    pub raw_exposures: u64,
    /// RACC exposure count, d_i.
    pub racc_exposures: u64,
}

/// Full exposure account behind a ledger: C = H + sum_i m_i b_i (T8, eq 5.7).
///
/// Framing and boundary interactions (system preamble, tool schemas, block
/// separators) are not blocks: they belong in the fixed overheads H_raw and
/// H_tz, so that the per-block sums stay exact multiplicities.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExposureAccount {
    /// Raw-side fixed overhead, H_raw.
    pub raw_fixed_overhead: u64,
    /// RACC-side fixed overhead, H_tz.
    pub racc_fixed_overhead: u64,
    /// Per-block exposure multiplicities.
    pub blocks: Vec<ExposureBlock>,
}

impl ExposureAccount {
    /// Raw replay cost C_raw = H_raw + sum r_i b_i.
    pub fn raw_cost(&self) -> Result<u128, LedgerError> {
        self.cost(self.raw_fixed_overhead, |block| block.raw_exposures)
    }

    /// RACC cost C_tz = H_tz + sum d_i b_i.
    pub fn racc_cost(&self) -> Result<u128, LedgerError> {
        self.cost(self.racc_fixed_overhead, |block| block.racc_exposures)
    }

    fn cost(
        &self,
        overhead: u64,
        multiplicity: impl Fn(&ExposureBlock) -> u64,
    ) -> Result<u128, LedgerError> {
        if self.blocks.is_empty() {
            return Err(LedgerError::EmptyExposureAccount);
        }
        let mut total = u128::from(overhead);
        for block in &self.blocks {
            total += u128::from(block.block_tokens) * u128::from(multiplicity(block));
        }
        Ok(total)
    }

    /// Total archived size sum_i b_i.
    pub fn block_tokens_total(&self) -> u128 {
        self.blocks.iter().map(|b| u128::from(b.block_tokens)).sum()
    }

    /// Verifies the T8 exact exposure/replay identity (eq 5.7) against a ledger.
    ///
    /// The ledger must account for its declared input exactly once, its
    /// raw_input_tokens must equal C_raw exactly, and its RACC exposure summed
    /// over every charge class must equal C_tz exactly. Any drift means the
    /// ledger and the exposure account disagree about history, which invalidates
    /// every downstream saving claim.
    pub fn check_replay_identity(&self, ledger: &TokenLedger) -> Result<(), LedgerError> {
        let racc_actual = u128::from(ledger.check_accounting_complete()?);
        let raw_expected = self.raw_cost()?;
        let racc_expected = self.racc_cost()?;
        let raw_actual = u128::from(ledger.raw_input_tokens);
        if raw_actual != raw_expected {
            return Err(LedgerError::ReplayIdentityMismatch {
                side: ExposureSide::Raw,
                expected: raw_expected,
                actual: raw_actual,
            });
        }
        if racc_actual != racc_expected {
            return Err(LedgerError::ReplayIdentityMismatch {
                side: ExposureSide::Racc,
                expected: racc_expected,
                actual: racc_actual,
            });
        }
        Ok(())
    }

    /// Exact saving, floor-rounded to ppm: floor((C_raw - C_tz) * 1e6 / C_raw).
    ///
    /// Returns zero when RACC costs at least as much as the raw replay.
    pub fn saving_ppm_floor(&self) -> Result<u32, LedgerError> {
        let raw = self.raw_cost()?;
        if raw == 0 {
            return Err(LedgerError::EmptyExposureAccount);
        }
        let racc = self.racc_cost()?;
        if racc >= raw {
            return Ok(0);
        }
        let ppm = (raw - racc) * u128::from(PPM_ONE) / raw;
        Ok(u32::try_from(ppm).unwrap_or(PPM_ONE))
    }

    /// Exact saving as the reduced-free rational pair (numerator, denominator),
    /// i.e. (C_raw - C_tz, C_raw). Lets callers compare savings exactly without
    /// floats or rounding.
    ///
    /// The numerator is zero when RACC costs at least as much as the raw replay.
    pub fn exact_saving_ratio(&self) -> Result<(u128, u128), LedgerError> {
        let raw = self.raw_cost()?;
        if raw == 0 {
            return Err(LedgerError::EmptyExposureAccount);
        }
        let racc = self.racc_cost()?;
        Ok((raw.saturating_sub(racc), raw))
    }

    /// Weighted RACC exposure sum_i d_i b_i, excluding fixed overhead.
    pub fn weighted_racc_exposure(&self) -> u128 {
        self.blocks
            .iter()
            .map(|b| u128::from(b.block_tokens) * u128::from(b.racc_exposures))
            .sum()
    }

    /// Weighted raw exposure sum_i r_i b_i, excluding fixed overhead.
    pub fn weighted_raw_exposure(&self) -> u128 {
        self.blocks
            .iter()
            .map(|b| u128::from(b.block_tokens) * u128::from(b.raw_exposures))
            .sum()
    }
}

/// Which side of the replay identity failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExposureSide {
    /// The raw replay baseline C_raw.
    Raw,
    /// The RACC exposure total C_tz.
    Racc,
}

impl fmt::Display for ExposureSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Raw => f.write_str("raw"),
            Self::Racc => f.write_str("racc"),
        }
    }
}

/// Typed ledger failures. None of these are recoverable by rewriting history.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LedgerError {
    /// A charge was measured with a tokenizer other than the locked gauge.
    TokenizerIdentityMismatch {
        /// Locked tokenizer id.
        expected_id: String,
        /// Locked tokenizer artifact digest.
        expected_digest: Digest,
        /// Offending tokenizer id.
        actual_id: String,
        /// Offending tokenizer artifact digest.
        actual_digest: Digest,
    },
    /// A monotone counter would have exceeded u64::MAX.
    CounterOverflow {
        /// Name of the counter that overflowed.
        counter: &'static str,
    },
    /// A retained fraction exceeded 1_000_000 ppm.
    PpmOutOfRange {
        /// The rejected value.
        ppm: u32,
    },
    /// Declared model-visible input was left unattributed to a charge class.
    UnclassifiedInput {
        /// Declared model-visible input tokens.
        declared: u64,
        /// Sum over the charge classes.
        classified: u64,
    },
    /// More tokens were attributed to charge classes than were declared.
    DoubleCountedInput {
        /// Declared model-visible input tokens.
        declared: u64,
        /// Sum over the charge classes.
        classified: u64,
    },
    /// The exposure account has no blocks, so no ratio is defined.
    EmptyExposureAccount,
    /// The T8 exposure/replay identity did not hold exactly.
    ReplayIdentityMismatch {
        /// Which side disagreed.
        side: ExposureSide,
        /// Cost implied by the exposure account.
        expected: u128,
        /// Cost recorded in the ledger.
        actual: u128,
    },
    /// Transactional speculation must never be free.
    ZeroTaskAttemptCost,
    /// A fresh-work vector did not decompose exactly the declared total.
    FreshWorkTotalMismatch {
        /// The total the vector claims to decompose.
        declared: u64,
        /// Sum over the fresh-work components.
        decomposed: u64,
    },
    /// An action record carried no action identity.
    EmptyActionId,
    /// A provider bill line carried an empty provider identity.
    EmptyBillProvider,
    /// A billed coordinate has no ledger rows: work went uncharged.
    HiddenUnchargedWork {
        /// Billing provider.
        provider: String,
        /// Billed resource class.
        class: &'static str,
        /// Amount the provider billed.
        billed: u64,
    },
    /// A ledger total deviated beyond a bill line's declared tolerance.
    OutOfTolerance {
        /// Billing provider.
        provider: String,
        /// Billed resource class.
        class: &'static str,
        /// Amount the provider billed.
        billed: u64,
        /// Ledger-derived total for the coordinate.
        ledger: u128,
        /// Declared tolerance in ppm.
        tolerance_ppm: u32,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TokenizerIdentityMismatch {
                expected_id,
                expected_digest,
                actual_id,
                actual_digest,
            } => write!(
                f,
                "tokenizer identity mismatch: ledger is locked to {expected_id}@{expected_digest}, charge used {actual_id}@{actual_digest}"
            ),
            Self::CounterOverflow { counter } => {
                write!(f, "monotone counter {counter} would overflow u64")
            }
            Self::PpmOutOfRange { ppm } => {
                write!(f, "retained fraction {ppm} ppm exceeds {PPM_ONE} ppm")
            }
            Self::UnclassifiedInput {
                declared,
                classified,
            } => write!(
                f,
                "{declared} declared input tokens but only {classified} classified: every model-visible call must be charged to a class"
            ),
            Self::DoubleCountedInput {
                declared,
                classified,
            } => write!(
                f,
                "{classified} classified input tokens exceed the {declared} declared: a call was counted more than once"
            ),
            Self::EmptyExposureAccount => f.write_str("exposure account has no blocks"),
            Self::ZeroTaskAttemptCost => {
                f.write_str("transactional task attempt cost must be nonzero")
            }
            Self::ReplayIdentityMismatch {
                side,
                expected,
                actual,
            } => write!(
                f,
                "replay identity mismatch on {side} side: exposure account implies {expected}, ledger recorded {actual}"
            ),
            Self::FreshWorkTotalMismatch {
                declared,
                decomposed,
            } => write!(
                f,
                "fresh-work components sum to {decomposed} but the declared total is {declared}: every token must sit in exactly one component"
            ),
            Self::EmptyActionId => {
                f.write_str("fresh-work action record requires a nonempty action id")
            }
            Self::EmptyBillProvider => {
                f.write_str("a provider bill line requires a nonempty provider identity")
            }
            Self::HiddenUnchargedWork {
                provider,
                class,
                billed,
            } => write!(
                f,
                "{provider} billed {billed} {class} but no ledger row charges that class: hidden uncharged work"
            ),
            Self::OutOfTolerance {
                provider,
                class,
                billed,
                ledger,
                tolerance_ppm,
            } => write!(
                f,
                "{provider} billed {billed} {class} but the ledger records {ledger}, beyond the declared {tolerance_ppm} ppm tolerance"
            ),
        }
    }
}

impl std::error::Error for LedgerError {}

/// Failures when verifying an exactness evidence handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceError {
    /// No evidence items were presented for the gate.
    NoEvidence {
        /// Which evidence kind was empty.
        kind: &'static str,
    },
    /// The presented evidence does not fold to the declared root.
    RootMismatch {
        /// Which evidence kind mismatched.
        kind: &'static str,
        /// Root the caller declared.
        declared: Digest,
        /// Root the presented evidence actually folds to.
        actual: Digest,
    },
    /// At least one task regressed, so no acceptance receipt exists.
    TaskRegressed,
}

impl fmt::Display for EvidenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoEvidence { kind } => write!(f, "no {kind} evidence was presented"),
            Self::RootMismatch {
                kind,
                declared,
                actual,
            } => write!(
                f,
                "{kind} evidence folds to {actual}, not the declared root {declared}"
            ),
            Self::TaskRegressed => {
                f.write_str("a task regressed: no acceptance receipt can be minted")
            }
        }
    }
}

impl std::error::Error for EvidenceError {}

/// Failures when sealing a receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptError {
    /// No charges were recorded, or the raw baseline is zero.
    IncompleteLedger,
    /// The ledger did not account for its declared input exactly once.
    Accounting(LedgerError),
    /// A gate was raised by evidence anchored to a different root.
    EvidenceRootMismatch {
        /// Which gate mismatched.
        kind: &'static str,
        /// Root recorded on the receipt.
        receipt: Digest,
        /// Root the evidence handle was verified against.
        evidence: Digest,
    },
    /// Canonical JSON encoding failed.
    Encoding,
}

impl fmt::Display for ReceiptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompleteLedger => {
                f.write_str("ledger is incomplete: no charges or zero raw baseline")
            }
            Self::Accounting(err) => write!(f, "incomplete charge accounting: {err}"),
            Self::EvidenceRootMismatch {
                kind,
                receipt,
                evidence,
            } => write!(
                f,
                "{kind} gate evidence is anchored to {evidence}, but the receipt records {receipt}"
            ),
            Self::Encoding => f.write_str("receipt could not be encoded as canonical JSON"),
        }
    }
}

impl std::error::Error for ReceiptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Accounting(err) => Some(err),
            _ => None,
        }
    }
}
