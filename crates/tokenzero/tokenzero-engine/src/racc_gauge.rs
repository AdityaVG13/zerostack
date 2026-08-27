//! Live RACC gauge: TokenZero accounting charged through hub `zero-ledger`.
//!
//! Gate ownership stays in the hub (`zero-gate` is not a TokenZero
//! dependency). Uncertified lossy is routed here as Expand/RawFallback
//! eligible, never a silent commit.

use std::collections::HashSet;

use serde::Serialize;
use tokenzero_core::{Accounting, Mode, sha256_hex};
use zero_ledger::{
    ArchiveAttestation, Digest, DominanceReceipt, ExactnessGates, LedgerConfig, LedgerError,
    PolicyDecision, PolicyEvidence, ReceiptError, ReceiptRoots, ResourceGauge, RetainedFractionPpm,
    TaskAcceptanceReceipt, TaskOutcome, TokenCharge, TokenizerIdentity,
};

/// Locked lexical tokenizer identity used when no provider lock is attached.
pub fn lexical_tokenizer_identity() -> TokenizerIdentity {
    let digest = Digest::from_hex(&sha256_hex("tokenzero-lexical-v1"))
        .expect("sha256 hex is a valid ledger digest");
    TokenizerIdentity::new("tokenzero-lexical", digest)
}

/// Classify one Accounting block into a zero-ledger TokenCharge.
///
/// Expand is Recovery on first serve of a ref and Reexpansion afterwards
/// (T8 replay identity). Other tools bill visible tokens and attach any
/// recovery debit as Recovery, never double-classing the same mass.
pub fn charge_from_accounting(tool: &str, accounting: &Accounting, reexpand: bool) -> TokenCharge {
    let raw_input_tokens = u64::try_from(accounting.raw_tokens).unwrap_or(u64::MAX);
    let visible = u64::try_from(accounting.visible_tokens).unwrap_or(u64::MAX);
    let recovery = u64::try_from(accounting.recovery_tokens).unwrap_or(u64::MAX);
    let billed_declared = u64::try_from(accounting.billed_tokens).unwrap_or(u64::MAX);

    if tool == "expand" {
        let mut charge = TokenCharge {
            raw_input_tokens,
            input_tokens: recovery,
            ..TokenCharge::default()
        };
        if reexpand {
            charge.reexpansion_tokens = recovery;
        } else {
            charge.recovery_tokens = recovery;
        }
        return charge;
    }

    let billed = if billed_declared > 0 {
        billed_declared
    } else {
        visible
    };
    TokenCharge {
        raw_input_tokens,
        input_tokens: billed.saturating_add(recovery),
        billed_tokens: billed,
        recovery_tokens: recovery,
        ..TokenCharge::default()
    }
}

/// Compression decision TokenZero can take without owning zero-gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionRoute {
    Passthrough,
    Compact,
    /// Lossy is legal only when the caller can expand or fall back to raw.
    LossyExpandEligible,
}

/// Uncertified lossy without a recovery ref is a silent commit and is refused.
pub fn classify_compression(
    mode: Mode,
    has_recovery_ref: bool,
) -> Result<CompressionRoute, &'static str> {
    match mode {
        Mode::Passthrough | Mode::Exact => Ok(CompressionRoute::Passthrough),
        Mode::Lossy if has_recovery_ref => Ok(CompressionRoute::LossyExpandEligible),
        Mode::Lossy => Err("uncertified lossy without Expand/RawFallback"),
        _ => Ok(CompressionRoute::Compact),
    }
}

/// Per-response receipt fragment taken from the accepted charge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChargeReceiptFragment {
    pub schema: &'static str,
    pub tokenizer_id: String,
    pub raw_input_tokens: u64,
    pub input_tokens: u64,
    pub billed_tokens: u64,
    pub recovery_tokens: u64,
    pub reexpansion_tokens: u64,
    pub charge_count: u64,
}

/// Session-cumulative hub gauge locked to one tokenizer identity.
#[derive(Debug)]
pub struct SessionRaccGauge {
    identity: TokenizerIdentity,
    gauge: ResourceGauge,
    expand_refs: HashSet<String>,
}

impl SessionRaccGauge {
    pub fn new(identity: TokenizerIdentity) -> Self {
        Self {
            gauge: ResourceGauge::new(LedgerConfig::new(identity.clone())),
            identity,
            expand_refs: HashSet::new(),
        }
    }

    pub fn with_lexical_identity() -> Self {
        Self::new(lexical_tokenizer_identity())
    }

    pub fn identity(&self) -> &TokenizerIdentity {
        &self.identity
    }

    pub fn charge_count(&self) -> u64 {
        self.gauge.charge_count()
    }

    /// Charge one served response. `expand_ref` keys T8 re-expansion.
    pub fn charge_response(
        &mut self,
        tool: &str,
        accounting: &Accounting,
        expand_ref: Option<&str>,
    ) -> Result<ChargeReceiptFragment, LedgerError> {
        let reexpand = match expand_ref {
            Some(ref_id) if tool == "expand" => !self.expand_refs.insert(ref_id.to_string()),
            _ => false,
        };
        let charge = charge_from_accounting(tool, accounting, reexpand);
        self.gauge.charge(&self.identity, &charge)?;
        Ok(ChargeReceiptFragment {
            schema: "tokenzero.racc_charge.v1",
            tokenizer_id: self.identity.tokenizer_id.clone(),
            raw_input_tokens: charge.raw_input_tokens,
            input_tokens: charge.input_tokens,
            billed_tokens: charge.billed_tokens,
            recovery_tokens: charge.recovery_tokens,
            reexpansion_tokens: charge.reexpansion_tokens,
            charge_count: self.gauge.charge_count(),
        })
    }

    pub fn finalize_receipt(
        &self,
        target_retained_ppm: u32,
        roots: ReceiptRoots,
        exactness: ExactnessGates,
    ) -> Result<DominanceReceipt, ReceiptError> {
        let target = RetainedFractionPpm::new(target_retained_ppm)
            .map_err(|_| ReceiptError::IncompleteLedger)?;
        self.gauge.finalize_receipt(target, roots, exactness)
    }
}

/// Build verified exactness evidence from labeled spans so tests and
/// session-end sealing can recompute `exact_phase_valid` from the same handles.
pub fn seal_with_labeled_evidence(
    gauge: &SessionRaccGauge,
    target_retained_ppm: u32,
    archive_label: &str,
    policy_label: &str,
    task_label: &str,
) -> Result<DominanceReceipt, ReceiptError> {
    let span = digest_label(archive_label);
    let archive_root = ArchiveAttestation::root_of(&[span]);
    let attestation = ArchiveAttestation::verify(archive_root, &[span])
        .map_err(|_| ReceiptError::IncompleteLedger)?;
    let decision = PolicyDecision::RawFallbackServed {
        view_digest: digest_label(policy_label),
    };
    let certificate_root = PolicyEvidence::root_of(&[decision]);
    let policy = PolicyEvidence::verify(certificate_root, &[decision])
        .map_err(|_| ReceiptError::IncompleteLedger)?;
    let outcome = TaskOutcome::Accepted {
        task_digest: digest_label(task_label),
    };
    let acceptance_root = TaskAcceptanceReceipt::root_of(&[outcome]);
    let task = TaskAcceptanceReceipt::verify(acceptance_root, &[outcome])
        .map_err(|_| ReceiptError::IncompleteLedger)?;
    let exactness = ExactnessGates::default()
        .with_byte_exact(&attestation)
        .with_policy_exact_or_fallback(&policy)
        .with_task_verified(&task);
    gauge.finalize_receipt(
        target_retained_ppm,
        ReceiptRoots {
            archive_root,
            certificate_root,
        },
        exactness,
    )
}

fn digest_label(label: &str) -> Digest {
    Digest::from_hex(&sha256_hex(label)).expect("sha256 hex is a valid ledger digest")
}

