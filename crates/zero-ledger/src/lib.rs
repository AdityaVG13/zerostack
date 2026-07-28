#![forbid(unsafe_code)]

//! RACC resource gauge, T8 exposure/replay identity, and exact-phase dominance receipt.
//!
//! This crate is the single home for RACC token accounting arithmetic so that
//! TokenZero, FSZero, GraphZero and the CodeMode host all account identically.
//! It ports the ledger/receipt subset of docs/racc/RACC_CONTRACT.rs.
//!
//! Properties enforced here:
//!
//! - **Locked tokenizer gauge.** Every charge carries a [`TokenizerIdentity`];
//!   mixing identities is a typed error ([`LedgerError::TokenizerIdentityMismatch`]).
//! - **Append-only monotone counters.** [`ResourceGauge`] exposes no API to
//!   decrement or rewrite history; overflow is a typed error, never a wrap.
//! - **Integer-only arithmetic.** Retained fractions are parts-per-million
//!   integers widened to u128; there are no floats and no percentage strings.
//! - **No I/O, sync only.** charge() is a handful of integer adds and performs
//!   no allocation.

use std::fmt;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// Parts per million denominator used by every retained-fraction comparison.
pub const PPM_ONE: u32 = 1_000_000;

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
        for (i, chunk) in bytes.chunks(2).enumerate() {
            let hi = unnibble(chunk[0])?;
            let lo = unnibble(chunk[1])?;
            out[i] = (hi << 4) | lo;
        }
        Some(Self(out))
    }
}

fn nibble(value: u8) -> char {
    char::from_digit(u32::from(value), 16).unwrap_or('0')
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
/// RetainedFractionPpm(30_000) means keep at most 3% of the raw input tokens.
/// The value is never a float and never a percentage string (T5).
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RetainedFractionPpm(pub u32);

impl RetainedFractionPpm {
    /// Constructs a retained fraction, rejecting values above 1_000_000 ppm.
    pub fn new(ppm: u32) -> Result<Self, LedgerError> {
        if ppm > PPM_ONE {
            return Err(LedgerError::PpmOutOfRange { ppm });
        }
        Ok(Self(ppm))
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

/// Cumulative, append-only token counters, tagged with the locked gauge.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenLedger {
    /// Tokenizer gauge these counts were measured with.
    pub tokenizer: TokenizerIdentity,
    /// Input tokens a raw (uncompressed) replay of the history would cost.
    pub raw_input_tokens: u64,
    /// Input tokens actually billed under RACC.
    pub racc_input_tokens: u64,
    /// Model output tokens.
    pub model_output_tokens: u64,
    /// Number of model calls.
    pub model_calls: u64,
    /// Input tokens spent on raw fallback views.
    pub fallback_tokens: u64,
    /// Input tokens spent recovering evidence (certificate fetch, expansion).
    pub recovery_tokens: u64,
    /// Input tokens spent re-expanding previously compressed spans.
    pub reexpansion_tokens: u64,
    /// Number of retried calls.
    pub retries: u64,
}

impl TokenLedger {
    /// An empty ledger tagged with the given tokenizer identity.
    pub fn empty(tokenizer: TokenizerIdentity) -> Self {
        Self {
            tokenizer,
            raw_input_tokens: 0,
            racc_input_tokens: 0,
            model_output_tokens: 0,
            model_calls: 0,
            fallback_tokens: 0,
            recovery_tokens: 0,
            reexpansion_tokens: 0,
            retries: 0,
        }
    }

    /// Total RACC-side input exposure: billed input plus recovery, re-expansion
    /// and raw fallback surcharges.
    pub fn total_racc_exposure(&self) -> Result<u64, LedgerError> {
        let mut total = self.racc_input_tokens;
        for (name, value) in [
            ("recovery_tokens", self.recovery_tokens),
            ("reexpansion_tokens", self.reexpansion_tokens),
            ("fallback_tokens", self.fallback_tokens),
        ] {
            total = total
                .checked_add(value)
                .ok_or(LedgerError::CounterOverflow { counter: name })?;
        }
        Ok(total)
    }
}

/// One append-only charge against a resource gauge.
///
/// All fields default to zero, so a caller charges only the counters it
/// observed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TokenCharge {
    /// Raw-replay input tokens attributable to this call.
    pub raw_input_tokens: u64,
    /// Billed RACC input tokens for this call.
    pub racc_input_tokens: u64,
    /// Output tokens produced by this call.
    pub model_output_tokens: u64,
    /// Model calls represented (normally 1).
    pub model_calls: u64,
    /// Raw fallback input tokens.
    pub fallback_tokens: u64,
    /// Evidence recovery input tokens.
    pub recovery_tokens: u64,
    /// Re-expansion input tokens.
    pub reexpansion_tokens: u64,
    /// Retries represented.
    pub retries: u64,
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
    /// Rejects counts measured with a different tokenizer identity and rejects
    /// overflow rather than wrapping. On any error the ledger is unchanged, so
    /// the counters stay monotone. Performs no allocation and no I/O.
    pub fn charge(
        &mut self,
        tokenizer: &TokenizerIdentity,
        charge: &TokenCharge,
    ) -> Result<(), LedgerError> {
        if tokenizer != &self.config.tokenizer {
            return Err(LedgerError::TokenizerIdentityMismatch {
                expected_id: self.config.tokenizer.tokenizer_id.clone(),
                expected_digest: self.config.tokenizer.tokenizer_version_digest,
                actual_id: tokenizer.tokenizer_id.clone(),
                actual_digest: tokenizer.tokenizer_version_digest,
            });
        }

        let raw_input_tokens = add(
            self.ledger.raw_input_tokens,
            charge.raw_input_tokens,
            "raw_input_tokens",
        )?;
        let racc_input_tokens = add(
            self.ledger.racc_input_tokens,
            charge.racc_input_tokens,
            "racc_input_tokens",
        )?;
        let model_output_tokens = add(
            self.ledger.model_output_tokens,
            charge.model_output_tokens,
            "model_output_tokens",
        )?;
        let model_calls = add(self.ledger.model_calls, charge.model_calls, "model_calls")?;
        let fallback_tokens = add(
            self.ledger.fallback_tokens,
            charge.fallback_tokens,
            "fallback_tokens",
        )?;
        let recovery_tokens = add(
            self.ledger.recovery_tokens,
            charge.recovery_tokens,
            "recovery_tokens",
        )?;
        let reexpansion_tokens = add(
            self.ledger.reexpansion_tokens,
            charge.reexpansion_tokens,
            "reexpansion_tokens",
        )?;
        let retries = add(self.ledger.retries, charge.retries, "retries")?;
        let charges = add(self.charges, 1, "charges")?;

        self.ledger.raw_input_tokens = raw_input_tokens;
        self.ledger.racc_input_tokens = racc_input_tokens;
        self.ledger.model_output_tokens = model_output_tokens;
        self.ledger.model_calls = model_calls;
        self.ledger.fallback_tokens = fallback_tokens;
        self.ledger.recovery_tokens = recovery_tokens;
        self.ledger.reexpansion_tokens = reexpansion_tokens;
        self.ledger.retries = retries;
        self.charges = charges;
        Ok(())
    }

    /// Seals the gauge into an ex-post dominance receipt.
    pub fn finalize_receipt(
        &self,
        target_retained_ppm: RetainedFractionPpm,
        roots: ReceiptRoots,
        exactness: ExactnessGates,
    ) -> Result<DominanceReceipt, ReceiptError> {
        if target_retained_ppm.0 > PPM_ONE {
            return Err(ReceiptError::TargetOutOfRange {
                ppm: target_retained_ppm.0,
            });
        }
        if self.charges == 0 || self.ledger.raw_input_tokens == 0 {
            return Err(ReceiptError::IncompleteLedger);
        }
        Ok(DominanceReceipt {
            ledger: self.ledger.clone(),
            target_retained_ppm,
            archive_root: roots.archive_root,
            certificate_root: roots.certificate_root,
            byte_exact: exactness.byte_exact,
            policy_exact_or_fallback: exactness.policy_exact_or_fallback,
            task_verified: exactness.task_verified,
        })
    }
}

fn add(lhs: u64, rhs: u64, counter: &'static str) -> Result<u64, LedgerError> {
    lhs.checked_add(rhs)
        .ok_or(LedgerError::CounterOverflow { counter })
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExactnessGates {
    /// Every retained span is byte-identical to the archived original.
    pub byte_exact: bool,
    /// Policy sufficiency was proven, or a raw fallback was served.
    pub policy_exact_or_fallback: bool,
    /// The downstream task acceptance check passed.
    pub task_verified: bool,
}

/// Ex-post exact-phase certificate: C <= epsilon * R plus exactness gates.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DominanceReceipt {
    /// Cumulative counters, tagged with the locked tokenizer gauge.
    pub ledger: TokenLedger,
    /// Preregistered retained-fraction target, in ppm.
    pub target_retained_ppm: RetainedFractionPpm,
    /// Root of the byte-exact archive.
    pub archive_root: Digest,
    /// Root of the evidence certificate set.
    pub certificate_root: Digest,
    /// Every retained span is byte-identical to the archived original.
    pub byte_exact: bool,
    /// Policy sufficiency was proven, or a raw fallback was served.
    pub policy_exact_or_fallback: bool,
    /// The downstream task acceptance check passed.
    pub task_verified: bool,
}

impl DominanceReceipt {
    /// Pure arithmetic part of the ex-post phase certificate.
    ///
    /// Checks racc_input_tokens * 1_000_000 <= raw_input_tokens * target_ppm
    /// with u128 widening, exactly as RACC_CONTRACT.rs specifies.
    pub fn meets_token_target(&self) -> bool {
        let lhs = u128::from(self.ledger.racc_input_tokens) * u128::from(PPM_ONE);
        let rhs = u128::from(self.ledger.raw_input_tokens) * u128::from(self.target_retained_ppm.0);
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
    /// Returns None when the raw baseline is zero (no ratio is defined).
    pub fn achieved_retained_ppm_ceil(&self) -> Option<u128> {
        let raw = u128::from(self.ledger.raw_input_tokens);
        if raw == 0 {
            return None;
        }
        let numerator = u128::from(self.ledger.racc_input_tokens) * u128::from(PPM_ONE);
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

/// Full exposure account behind a ledger: C = H + sum_i m_i b_i (T8).
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

    /// Verifies the T8 exact exposure/replay identity against a ledger.
    ///
    /// The ledger's raw_input_tokens must equal C_raw exactly and its total
    /// RACC exposure (billed + recovery + re-expansion + fallback) must equal
    /// C_tz exactly. Any drift means the ledger and the exposure account
    /// disagree about history, which invalidates every downstream saving claim.
    pub fn check_replay_identity(&self, ledger: &TokenLedger) -> Result<(), LedgerError> {
        let raw_expected = self.raw_cost()?;
        let racc_expected = self.racc_cost()?;
        let raw_actual = u128::from(ledger.raw_input_tokens);
        let racc_actual = u128::from(ledger.total_racc_exposure()?);
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
            Self::EmptyExposureAccount => f.write_str("exposure account has no blocks"),
            Self::ReplayIdentityMismatch { side, expected, actual } => write!(
                f,
                "replay identity mismatch on {side} side: exposure account implies {expected}, ledger recorded {actual}"
            ),
        }
    }
}

impl std::error::Error for LedgerError {}

/// Failures when sealing a receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceiptError {
    /// No charges were recorded, or the raw baseline is zero.
    IncompleteLedger,
    /// The requested target exceeded 1_000_000 ppm.
    TargetOutOfRange {
        /// The rejected value.
        ppm: u32,
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
            Self::TargetOutOfRange { ppm } => {
                write!(
                    f,
                    "target retained fraction {ppm} ppm exceeds {PPM_ONE} ppm"
                )
            }
            Self::Encoding => f.write_str("receipt could not be encoded as canonical JSON"),
        }
    }
}

impl std::error::Error for ReceiptError {}
