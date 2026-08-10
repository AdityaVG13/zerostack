//! Effect-closed transaction admission over the durable zero-store journal.
//!
//! This module owns no project bytes. FSZero remains the authority for exact
//! workspace snapshots, candidate clones, and candidate cleanup. ZeroStack
//! admits speculation only after every preregistered resource has a supported
//! isolation and restoration mode. The resulting journal receipt claims only
//! the declared effect-closed scope; it never upgrades filesystem rollback to
//! universal external-state restoration or native durability.

#![allow(clippy::result_large_err)]

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use zero_abi::{
    ArtifactOwnerV1, DigestV1, EffectClass, EffectProgramV1, EffectRollbackV1,
    TypedEffectOperationV1, canonical_json,
};
use zero_cert::EffectAcceptedV1;
use zero_store::{
    ContinuationCartridgeV1, DurableProfileIdV1, JournalBindingV1, JournalErrorV1,
    JournalFailureCodeV1, JournalPathsV1, RecoveryOutcomeV1, RecoveryReceiptV1, abort_journal_v1,
    commit_journal_v1, prepare_journal_v1, recover_journal_v1,
};

pub const TRANSACTION_CONTRACT_VERSION_V1: u16 = 1;
pub const TRANSACTION_MAX_RESOURCES_V1: usize = 256;
pub const TRANSACTION_MAX_CANONICAL_BYTES_V1: usize = 1_048_576;

const REQUEST_DOMAIN_V1: &[u8] = b"zerostack.effect_closure.request.v1\0";
const TRANSACTION_ID_DOMAIN_V1: &[u8] = b"zerostack.effect_transaction.id.v1\0";
const MANIFEST_DOMAIN_V1: &[u8] = b"zerostack.effect_closure.manifest.v1\0";
const EXTERNAL_INVENTORY_DOMAIN_V1: &[u8] = b"zerostack.effect_closure.external_inventory.v1\0";
const EXTERNAL_DEBT_DOMAIN_V1: &[u8] = b"zerostack.effect_closure.external_debt.v1\0";
const JOURNAL_RECEIPT_DOMAIN_V1: &[u8] = b"zerostack.effect_transaction.receipt.v1\0";
const CONTRACT_DOMAIN_V1: &[u8] = b"zerostack.transaction.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionResourceKindV1 {
    ProjectFilesystem,
    GraphIndex,
    Toolchain,
    RuntimeManifest,
    CapabilitySurface,
    ExternalDatabase,
    ExternalService,
    ObservationLog,
    Time,
    Randomness,
    Queue,
    RateLimit,
    Network,
    Approval,
    ProviderModelState,
    DecoderState,
    OtherExternal,
}

impl TransactionResourceKindV1 {
    pub const fn is_external(self) -> bool {
        !matches!(
            self,
            Self::ProjectFilesystem
                | Self::GraphIndex
                | Self::Toolchain
                | Self::RuntimeManifest
                | Self::CapabilitySurface
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionAccessV1 {
    Read,
    Write,
    ReadWrite,
}

impl TransactionAccessV1 {
    const fn writes(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceIsolationModeV1 {
    ImmutableSnapshot,
    RecordedReplay,
    Buffered,
    Journaled,
    Transactional,
    DelayedUntilCommit,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceRestorationModeV1 {
    NotNeeded,
    RecordedReplay,
    JournalRollback,
    TransactionRollback,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionResourceRequirementV1 {
    pub owner: ArtifactOwnerV1,
    pub kind: TransactionResourceKindV1,
    pub scope_digest: DigestV1,
    pub baseline_state_digest: DigestV1,
    pub access: TransactionAccessV1,
    pub authority_digest: DigestV1,
}

impl TransactionResourceRequirementV1 {
    fn key(self) -> (TransactionResourceKindV1, DigestV1) {
        (self.kind, self.scope_digest)
    }

    fn validate(self) -> Result<(), TransactionErrorV1> {
        if self.scope_digest == DigestV1::ZERO
            || self.baseline_state_digest == DigestV1::ZERO
            || self.authority_digest == DigestV1::ZERO
        {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::InvalidResource,
                Some(self),
                "resource scope, baseline state, and authority digests must be nonzero",
            ));
        }
        Ok(())
    }
}

/// Exact resource inventory required by the trusted controller for one effect.
///
/// Fields are private and this type is not deserializable. Callers must derive
/// it from a validated `EffectProgramV1` through `new`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectClosureRequestV1 {
    contract_version: u16,
    action_digest: DigestV1,
    baseline_state: DigestV1,
    rollback: EffectRollbackV1,
    resources: Vec<TransactionResourceRequirementV1>,
}

impl EffectClosureRequestV1 {
    pub fn new(
        program: &EffectProgramV1,
        mut resources: Vec<TransactionResourceRequirementV1>,
    ) -> Result<Self, TransactionErrorV1> {
        program.validate().map_err(|error| {
            TransactionErrorV1::new(
                TransactionFailureCodeV1::InvalidEffectProgram,
                None,
                error.to_string(),
            )
        })?;
        sort_and_validate_requirements(&mut resources)?;
        validate_program_resource_mapping(program, &resources)?;
        let request = Self {
            contract_version: TRANSACTION_CONTRACT_VERSION_V1,
            action_digest: program.action_digest(),
            baseline_state: program.base_state(),
            rollback: program.rollback(),
            resources,
        };
        request.validate()?;
        Ok(request)
    }

    pub const fn action_digest(&self) -> DigestV1 {
        self.action_digest
    }
    pub const fn baseline_state(&self) -> DigestV1 {
        self.baseline_state
    }
    pub const fn rollback(&self) -> EffectRollbackV1 {
        self.rollback
    }
    pub fn resources(&self) -> &[TransactionResourceRequirementV1] {
        &self.resources
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TransactionErrorV1> {
        self.validate()?;
        canonical_serialize(self)
    }

    pub fn digest(&self) -> Result<DigestV1, TransactionErrorV1> {
        Ok(domain_digest(REQUEST_DOMAIN_V1, &self.canonical_bytes()?))
    }

    fn validate(&self) -> Result<(), TransactionErrorV1> {
        if self.contract_version != TRANSACTION_CONTRACT_VERSION_V1 {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::SchemaVersionMismatch,
                None,
                "effect-closure request version is unsupported",
            ));
        }
        if self.action_digest == DigestV1::ZERO || self.baseline_state == DigestV1::ZERO {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::InvalidEffectProgram,
                None,
                "action and baseline digests must be nonzero",
            ));
        }
        validate_sorted_requirements(&self.resources)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectResourceClosureV1 {
    pub requirement: TransactionResourceRequirementV1,
    pub isolation: ResourceIsolationModeV1,
    pub restoration: ResourceRestorationModeV1,
}

impl EffectResourceClosureV1 {
    fn validate(self) -> Result<(), TransactionErrorV1> {
        if self.isolation == ResourceIsolationModeV1::Unsupported
            || self.restoration == ResourceRestorationModeV1::Unsupported
        {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::UnsupportedIsolation,
                Some(self.requirement),
                "unsupported isolation or restoration blocks speculation",
            ));
        }
        let access_ok = match self.requirement.access {
            TransactionAccessV1::Read => {
                !matches!(self.isolation, ResourceIsolationModeV1::DelayedUntilCommit)
            }
            TransactionAccessV1::Write | TransactionAccessV1::ReadWrite => !matches!(
                self.isolation,
                ResourceIsolationModeV1::ImmutableSnapshot
                    | ResourceIsolationModeV1::RecordedReplay
            ),
        };
        if !access_ok {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::IsolationAccessMismatch,
                Some(self.requirement),
                "resource access is incompatible with its isolation mode",
            ));
        }
        let expected = match self.isolation {
            ResourceIsolationModeV1::ImmutableSnapshot
            | ResourceIsolationModeV1::Buffered
            | ResourceIsolationModeV1::DelayedUntilCommit => ResourceRestorationModeV1::NotNeeded,
            ResourceIsolationModeV1::RecordedReplay => ResourceRestorationModeV1::RecordedReplay,
            ResourceIsolationModeV1::Journaled => ResourceRestorationModeV1::JournalRollback,
            ResourceIsolationModeV1::Transactional => {
                ResourceRestorationModeV1::TransactionRollback
            }
            ResourceIsolationModeV1::Unsupported => unreachable!(),
        };
        if self.restoration != expected {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::RestorationMismatch,
                Some(self.requirement),
                format!(
                    "isolation {:?} requires restoration {:?}",
                    self.isolation, expected
                ),
            ));
        }
        Ok(())
    }
}

/// Canonical declaration of isolation and restoration for every required resource.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectClosureManifestV1 {
    contract_version: u16,
    request_digest: DigestV1,
    resources: Vec<EffectResourceClosureV1>,
}

impl EffectClosureManifestV1 {
    pub fn new(
        request: &EffectClosureRequestV1,
        mut resources: Vec<EffectResourceClosureV1>,
    ) -> Result<Self, TransactionErrorV1> {
        resources.sort_by_key(|entry| entry.requirement.key());
        let manifest = Self {
            contract_version: TRANSACTION_CONTRACT_VERSION_V1,
            request_digest: request.digest()?,
            resources,
        };
        manifest.validate_body()?;
        Ok(manifest)
    }

    pub fn resources(&self) -> &[EffectResourceClosureV1] {
        &self.resources
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TransactionErrorV1> {
        self.validate_body()?;
        canonical_serialize(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, TransactionErrorV1> {
        if bytes.len() > TRANSACTION_MAX_CANONICAL_BYTES_V1 {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::CanonicalPayloadTooLarge,
                None,
                format!("effect-closure manifest has {} bytes", bytes.len()),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::NonCanonicalEncoding,
                None,
                "effect-closure manifest is not exact canonical JSON",
            ));
        }
        let manifest: Self = serde_json::from_value(value).map_err(serialization_error)?;
        manifest.validate_body()?;
        Ok(manifest)
    }

    pub fn digest(&self) -> Result<DigestV1, TransactionErrorV1> {
        Ok(domain_digest(MANIFEST_DOMAIN_V1, &self.canonical_bytes()?))
    }

    fn validate_body(&self) -> Result<(), TransactionErrorV1> {
        if self.contract_version != TRANSACTION_CONTRACT_VERSION_V1 {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::SchemaVersionMismatch,
                None,
                "effect-closure manifest version is unsupported",
            ));
        }
        if self.request_digest == DigestV1::ZERO {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::RequestMismatch,
                None,
                "effect-closure request digest is zero",
            ));
        }
        if self.resources.is_empty() {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::EmptyResourceInventory,
                None,
                "effect-closure inventory must be explicit and nonempty",
            ));
        }
        if self.resources.len() > TRANSACTION_MAX_RESOURCES_V1 {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::TooManyResources,
                None,
                format!(
                    "effect-closure inventory has {} resources",
                    self.resources.len()
                ),
            ));
        }
        let mut previous = None;
        for entry in &self.resources {
            entry.requirement.validate()?;
            let key = entry.requirement.key();
            if previous.is_some_and(|prior| prior >= key) {
                return Err(TransactionErrorV1::new(
                    TransactionFailureCodeV1::NonCanonicalResourceInventory,
                    Some(entry.requirement),
                    "resource closures must be strictly sorted without duplicate scopes",
                ));
            }
            previous = Some(key);
        }
        Ok(())
    }
}

/// Private capability proving that the exact requested inventory is effect-closed.
#[derive(Debug)]
pub struct ClosedEffectBoundaryV1 {
    request_digest: DigestV1,
    manifest_digest: DigestV1,
    action_digest: DigestV1,
    baseline_state: DigestV1,
    rollback: EffectRollbackV1,
    external_inventory_digest: DigestV1,
    external_restoration_debt_digest: DigestV1,
    resource_count: u16,
    external_resource_count: u16,
    external_restoration_debt_count: u16,
}

impl ClosedEffectBoundaryV1 {
    pub const fn request_digest(&self) -> DigestV1 {
        self.request_digest
    }
    pub const fn manifest_digest(&self) -> DigestV1 {
        self.manifest_digest
    }
    pub const fn action_digest(&self) -> DigestV1 {
        self.action_digest
    }
    pub const fn baseline_state(&self) -> DigestV1 {
        self.baseline_state
    }
    pub const fn rollback(&self) -> EffectRollbackV1 {
        self.rollback
    }
    pub const fn external_inventory_digest(&self) -> DigestV1 {
        self.external_inventory_digest
    }
    pub const fn resource_count(&self) -> u16 {
        self.resource_count
    }
    pub const fn external_resource_count(&self) -> u16 {
        self.external_resource_count
    }
    pub const fn external_restoration_debt_digest(&self) -> DigestV1 {
        self.external_restoration_debt_digest
    }
    pub const fn external_restoration_debt_count(&self) -> u16 {
        self.external_restoration_debt_count
    }
}

pub fn validate_effect_closure_v1(
    request: &EffectClosureRequestV1,
    manifest: &EffectClosureManifestV1,
) -> Result<ClosedEffectBoundaryV1, TransactionErrorV1> {
    request.validate()?;
    manifest.validate_body()?;
    if manifest.request_digest != request.digest()? {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::RequestMismatch,
            None,
            "effect-closure manifest binds another request",
        ));
    }
    let mut requested = request.resources.iter();
    let mut declared = manifest.resources.iter();
    loop {
        match (requested.next(), declared.next()) {
            (Some(required), Some(entry)) if *required == entry.requirement => {
                entry.validate()?;
            }
            (Some(required), Some(entry)) => {
                let code = if required.key() < entry.requirement.key() {
                    TransactionFailureCodeV1::MissingResource
                } else {
                    TransactionFailureCodeV1::UnexpectedResource
                };
                return Err(TransactionErrorV1::new(
                    code,
                    Some(if code == TransactionFailureCodeV1::MissingResource {
                        *required
                    } else {
                        entry.requirement
                    }),
                    "effect-closure inventory differs from the preregistered request",
                ));
            }
            (Some(required), None) => {
                return Err(TransactionErrorV1::new(
                    TransactionFailureCodeV1::MissingResource,
                    Some(*required),
                    "effect-closure manifest omitted a required resource",
                ));
            }
            (None, Some(entry)) => {
                return Err(TransactionErrorV1::new(
                    TransactionFailureCodeV1::UnexpectedResource,
                    Some(entry.requirement),
                    "effect-closure manifest added an unrequested resource",
                ));
            }
            (None, None) => break,
        }
    }
    validate_rollback_coverage(request.rollback, &manifest.resources)?;
    let external = manifest
        .resources
        .iter()
        .filter(|entry| entry.requirement.kind.is_external())
        .copied()
        .collect::<Vec<_>>();
    // Buffered and delayed effects never leave the isolation boundary before commit.
    let external_debt = external
        .iter()
        .filter(|entry| {
            entry.requirement.access.writes()
                && matches!(
                    entry.isolation,
                    ResourceIsolationModeV1::Journaled | ResourceIsolationModeV1::Transactional
                )
        })
        .copied()
        .collect::<Vec<_>>();
    let external_bytes = canonical_serialize(&external)?;
    let external_debt_bytes = canonical_serialize(&external_debt)?;
    Ok(ClosedEffectBoundaryV1 {
        request_digest: request.digest()?,
        manifest_digest: manifest.digest()?,
        action_digest: request.action_digest,
        baseline_state: request.baseline_state,
        rollback: request.rollback,
        external_inventory_digest: domain_digest(EXTERNAL_INVENTORY_DOMAIN_V1, &external_bytes),
        external_restoration_debt_digest: domain_digest(
            EXTERNAL_DEBT_DOMAIN_V1,
            &external_debt_bytes,
        ),
        resource_count: request.resources.len() as u16,
        external_resource_count: external.len() as u16,
        external_restoration_debt_count: external_debt.len() as u16,
    })
}

fn validate_rollback_coverage(
    rollback: EffectRollbackV1,
    resources: &[EffectResourceClosureV1],
) -> Result<(), TransactionErrorV1> {
    let writes = resources
        .iter()
        .filter(|entry| entry.requirement.access.writes())
        .collect::<Vec<_>>();
    if rollback == EffectRollbackV1::RawFallback {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::RawFallbackIsNotSpeculation,
            None,
            "raw fallback is the frozen baseline, not a speculative transaction",
        ));
    }
    if rollback == EffectRollbackV1::ReadOnly && !writes.is_empty() {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::RollbackMismatch,
            Some(writes[0].requirement),
            "read-only rollback cannot cover a write resource",
        ));
    }
    if rollback != EffectRollbackV1::ReadOnly && writes.is_empty() {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::RollbackMismatch,
            None,
            "mutating rollback class requires at least one write resource",
        ));
    }
    if rollback == EffectRollbackV1::SingleAtomic && writes.len() != 1 {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::RollbackMismatch,
            None,
            "single-atomic rollback requires exactly one isolated write resource",
        ));
    }
    for entry in writes {
        let mode = entry.isolation;
        let compatible = match rollback {
            EffectRollbackV1::ReadOnly | EffectRollbackV1::RawFallback => false,
            EffectRollbackV1::SingleAtomic | EffectRollbackV1::Journaled => matches!(
                mode,
                ResourceIsolationModeV1::Buffered
                    | ResourceIsolationModeV1::Journaled
                    | ResourceIsolationModeV1::Transactional
                    | ResourceIsolationModeV1::DelayedUntilCommit
            ),
            EffectRollbackV1::WorkspaceClone => matches!(
                mode,
                ResourceIsolationModeV1::Buffered | ResourceIsolationModeV1::DelayedUntilCommit
            ),
            EffectRollbackV1::ExternalTransaction => matches!(
                mode,
                ResourceIsolationModeV1::Buffered
                    | ResourceIsolationModeV1::Transactional
                    | ResourceIsolationModeV1::DelayedUntilCommit
            ),
        };
        if !compatible {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::RollbackMismatch,
                Some(entry.requirement),
                format!("rollback {rollback:?} cannot cover isolation {mode:?}"),
            ));
        }
    }
    Ok(())
}

/// Live linear handle for one prepared zero-store journal transaction.
#[derive(Debug)]
pub struct EffectTransactionV1 {
    paths: JournalPathsV1,
    binding: JournalBindingV1,
    cartridge: ContinuationCartridgeV1,
    closure: ClosureBindingV1,
}

#[derive(Clone, Copy, Debug)]
struct ClosureBindingV1 {
    request_digest: DigestV1,
    manifest_digest: DigestV1,
    action_digest: DigestV1,
    baseline_state: DigestV1,
    external_inventory_digest: DigestV1,
    external_restoration_debt_digest: DigestV1,
    resource_count: u16,
    external_resource_count: u16,
    external_restoration_debt_count: u16,
}

impl ClosureBindingV1 {
    fn from_closed(closed: &ClosedEffectBoundaryV1) -> Self {
        Self {
            request_digest: closed.request_digest,
            manifest_digest: closed.manifest_digest,
            action_digest: closed.action_digest,
            baseline_state: closed.baseline_state,
            external_inventory_digest: closed.external_inventory_digest,
            external_restoration_debt_digest: closed.external_restoration_debt_digest,
            resource_count: closed.resource_count,
            external_resource_count: closed.external_resource_count,
            external_restoration_debt_count: closed.external_restoration_debt_count,
        }
    }
}

pub fn effect_journal_binding_v1(
    closed: &ClosedEffectBoundaryV1,
    assembly_manifest_digest: DigestV1,
    durable_profile_id: DurableProfileIdV1,
    candidate_root: DigestV1,
    owner_identity_digest: DigestV1,
) -> Result<JournalBindingV1, TransactionErrorV1> {
    for (label, digest) in [
        ("assembly manifest", assembly_manifest_digest),
        ("candidate root", candidate_root),
        ("owner identity", owner_identity_digest),
    ] {
        if digest == DigestV1::ZERO {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::JournalBindingMismatch,
                None,
                format!("{label} digest is zero"),
            ));
        }
    }
    let transaction_id = expected_transaction_id(
        closed,
        assembly_manifest_digest,
        durable_profile_id,
        candidate_root,
        owner_identity_digest,
    );
    let binding = JournalBindingV1::new(
        transaction_id,
        assembly_manifest_digest,
        durable_profile_id,
        closed.baseline_state,
        candidate_root,
        owner_identity_digest,
    );
    binding.validate().map_err(journal_error)?;
    Ok(binding)
}

fn expected_transaction_id(
    closed: &ClosedEffectBoundaryV1,
    assembly_manifest_digest: DigestV1,
    durable_profile_id: DurableProfileIdV1,
    candidate_root: DigestV1,
    owner_identity_digest: DigestV1,
) -> DigestV1 {
    let mut bytes = Vec::with_capacity(32 * 6 + 2);
    bytes.extend_from_slice(closed.request_digest.as_bytes());
    bytes.extend_from_slice(closed.manifest_digest.as_bytes());
    bytes.extend_from_slice(closed.action_digest.as_bytes());
    bytes.extend_from_slice(assembly_manifest_digest.as_bytes());
    let profile = durable_profile_id.as_str().as_bytes();
    bytes.extend_from_slice(&(profile.len() as u16).to_be_bytes());
    bytes.extend_from_slice(profile);
    bytes.extend_from_slice(candidate_root.as_bytes());
    bytes.extend_from_slice(owner_identity_digest.as_bytes());
    domain_digest(TRANSACTION_ID_DOMAIN_V1, &bytes)
}

pub fn begin_effect_transaction_v1(
    paths: JournalPathsV1,
    binding: JournalBindingV1,
    closed: &ClosedEffectBoundaryV1,
) -> Result<EffectTransactionV1, TransactionErrorV1> {
    validate_journal_binding(&binding, closed)?;
    let cartridge = prepare_journal_v1(&paths, binding.clone()).map_err(journal_error)?;
    Ok(EffectTransactionV1 {
        paths,
        binding,
        cartridge,
        closure: ClosureBindingV1::from_closed(closed),
    })
}

impl EffectTransactionV1 {
    /// Commits only after zero-cert has minted an acceptance for this exact action and baseline.
    pub fn commit(
        self,
        acceptance: &EffectAcceptedV1,
    ) -> Result<TransactionReceiptV1, TransactionErrorV1> {
        let acceptance_digest = validate_acceptance(acceptance, self.closure)?;
        let recovery = commit_journal_v1(&self.paths, &self.cartridge).map_err(journal_error)?;
        TransactionReceiptV1::from_recovery(
            &self.binding,
            self.closure,
            &recovery,
            Some(acceptance_digest),
        )
    }

    pub fn abort(self) -> Result<TransactionReceiptV1, TransactionErrorV1> {
        let recovery = abort_journal_v1(&self.paths, &self.cartridge).map_err(journal_error)?;
        TransactionReceiptV1::from_recovery(&self.binding, self.closure, &recovery, None)
    }
}

/// Recovers a prepared transaction. A new-root outcome requires the exact
/// zero-cert acceptance to be revalidated; old-root recovery never claims one.
pub fn recover_effect_transaction_v1(
    paths: &JournalPathsV1,
    binding: &JournalBindingV1,
    closed: &ClosedEffectBoundaryV1,
    acceptance: Option<&EffectAcceptedV1>,
) -> Result<TransactionReceiptV1, TransactionErrorV1> {
    validate_journal_binding(binding, closed)?;
    let closure = ClosureBindingV1::from_closed(closed);
    let acceptance_digest = acceptance
        .map(|accepted| validate_acceptance(accepted, closure))
        .transpose()?;
    let recovery = recover_journal_v1(paths, binding).map_err(journal_error)?;
    TransactionReceiptV1::from_recovery(binding, closure, &recovery, acceptance_digest)
}

fn validate_acceptance(
    acceptance: &EffectAcceptedV1,
    closure: ClosureBindingV1,
) -> Result<DigestV1, TransactionErrorV1> {
    acceptance.validate().map_err(|error| {
        TransactionErrorV1::new(
            TransactionFailureCodeV1::InvalidEffectAcceptance,
            None,
            error.to_string(),
        )
    })?;
    if acceptance.action_digest() != closure.action_digest
        || acceptance.state_snapshot() != closure.baseline_state
    {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::EffectAcceptanceMismatch,
            None,
            "effect acceptance binds another action or baseline snapshot",
        ));
    }
    Ok(acceptance.acceptance_digest())
}

fn validate_journal_binding(
    binding: &JournalBindingV1,
    closed: &ClosedEffectBoundaryV1,
) -> Result<(), TransactionErrorV1> {
    binding.validate().map_err(journal_error)?;
    if binding.old_root != closed.baseline_state {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::BaselineMismatch,
            None,
            "journal old root differs from the effect-closure baseline",
        ));
    }
    let expected_id = expected_transaction_id(
        closed,
        binding.assembly_manifest_digest,
        binding.durable_profile_id,
        binding.new_root,
        binding.owner_identity_digest,
    );
    if binding.transaction_id != expected_id {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::JournalBindingMismatch,
            None,
            "journal transaction id does not bind the effect closure",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionDispositionV1 {
    CandidateCommitted,
    BaselineRootRecovered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorationScopeV1 {
    NotApplicableCandidateCommit,
    DeclaredEffectClosure,
    ProjectJournalRootOnly,
}

/// Journal-bound outcome record. It does not claim native durability or
/// restoration outside the preregistered effect-closure inventory.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionReceiptV1 {
    contract_version: u16,
    disposition: TransactionDispositionV1,
    restoration_scope: RestorationScopeV1,
    request_digest: DigestV1,
    closure_manifest_digest: DigestV1,
    action_digest: DigestV1,
    acceptance_digest: Option<DigestV1>,
    baseline_state: DigestV1,
    candidate_state: DigestV1,
    external_inventory_digest: DigestV1,
    external_restoration_debt_digest: DigestV1,
    resource_count: u16,
    external_resource_count: u16,
    external_restoration_debt_count: u16,
    journal_binding_digest: DigestV1,
    journal_recovery_digest: DigestV1,
    recovery_outcome: RecoveryOutcomeV1,
    observed_root: DigestV1,
    receipt_digest: DigestV1,
}

impl TransactionReceiptV1 {
    fn from_recovery(
        binding: &JournalBindingV1,
        closure: ClosureBindingV1,
        recovery: &RecoveryReceiptV1,
        acceptance_digest: Option<DigestV1>,
    ) -> Result<Self, TransactionErrorV1> {
        recovery.canonical_bytes().map_err(journal_error)?;
        let binding_digest = binding.digest().map_err(journal_error)?;
        if recovery.binding_digest != binding_digest {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::JournalBindingMismatch,
                None,
                "recovery receipt binds another durable journal",
            ));
        }
        let (disposition, restoration_scope, expected_root) = match recovery.outcome {
            RecoveryOutcomeV1::NewRootCommitted | RecoveryOutcomeV1::AlreadyCommitted => (
                TransactionDispositionV1::CandidateCommitted,
                RestorationScopeV1::NotApplicableCandidateCommit,
                binding.new_root,
            ),
            RecoveryOutcomeV1::NotStartedOldRoot
            | RecoveryOutcomeV1::OldRootAborted
            | RecoveryOutcomeV1::AlreadyAborted => (
                TransactionDispositionV1::BaselineRootRecovered,
                if closure.external_restoration_debt_count == 0 {
                    RestorationScopeV1::DeclaredEffectClosure
                } else {
                    RestorationScopeV1::ProjectJournalRootOnly
                },
                binding.old_root,
            ),
        };
        if disposition == TransactionDispositionV1::CandidateCommitted
            && acceptance_digest.is_none()
        {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::MissingEffectAcceptance,
                None,
                "candidate commit recovery requires zero-cert effect acceptance",
            ));
        }
        let acceptance_digest = if disposition == TransactionDispositionV1::CandidateCommitted {
            acceptance_digest
        } else {
            None
        };
        if recovery.observed_root != expected_root || closure.baseline_state != binding.old_root {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::JournalRootMismatch,
                None,
                "journal recovery outcome and observed root disagree",
            ));
        }
        let mut receipt = Self {
            contract_version: TRANSACTION_CONTRACT_VERSION_V1,
            disposition,
            restoration_scope,
            request_digest: closure.request_digest,
            closure_manifest_digest: closure.manifest_digest,
            action_digest: closure.action_digest,
            acceptance_digest,
            baseline_state: binding.old_root,
            candidate_state: binding.new_root,
            external_inventory_digest: closure.external_inventory_digest,
            external_restoration_debt_digest: closure.external_restoration_debt_digest,
            resource_count: closure.resource_count,
            external_resource_count: closure.external_resource_count,
            external_restoration_debt_count: closure.external_restoration_debt_count,
            journal_binding_digest: binding_digest,
            journal_recovery_digest: recovery.digest().map_err(journal_error)?,
            recovery_outcome: recovery.outcome,
            observed_root: recovery.observed_root,
            receipt_digest: DigestV1::ZERO,
        };
        receipt.receipt_digest = receipt.expected_digest()?;
        Ok(receipt)
    }

    pub const fn disposition(&self) -> TransactionDispositionV1 {
        self.disposition
    }
    pub const fn action_digest(&self) -> DigestV1 {
        self.action_digest
    }
    pub const fn baseline_state(&self) -> DigestV1 {
        self.baseline_state
    }
    pub const fn candidate_state(&self) -> DigestV1 {
        self.candidate_state
    }
    pub const fn acceptance_digest(&self) -> Option<DigestV1> {
        self.acceptance_digest
    }
    pub const fn restoration_scope(&self) -> RestorationScopeV1 {
        self.restoration_scope
    }
    pub const fn closure_manifest_digest(&self) -> DigestV1 {
        self.closure_manifest_digest
    }
    pub const fn external_inventory_digest(&self) -> DigestV1 {
        self.external_inventory_digest
    }
    pub const fn resource_count(&self) -> u16 {
        self.resource_count
    }
    pub const fn external_resource_count(&self) -> u16 {
        self.external_resource_count
    }
    pub const fn external_restoration_debt_digest(&self) -> DigestV1 {
        self.external_restoration_debt_digest
    }
    pub const fn external_restoration_debt_count(&self) -> u16 {
        self.external_restoration_debt_count
    }
    pub const fn observed_root(&self) -> DigestV1 {
        self.observed_root
    }
    pub const fn recovery_outcome(&self) -> RecoveryOutcomeV1 {
        self.recovery_outcome
    }
    pub const fn receipt_digest(&self) -> DigestV1 {
        self.receipt_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TransactionErrorV1> {
        if self.receipt_digest != self.expected_digest()? {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::ReceiptDigestMismatch,
                None,
                "transaction receipt digest does not match its canonical body",
            ));
        }
        canonical_serialize(self)
    }

    fn expected_digest(&self) -> Result<DigestV1, TransactionErrorV1> {
        let body = json!({
            "acceptance_digest": self.acceptance_digest,
            "action_digest": self.action_digest,
            "baseline_state": self.baseline_state,
            "candidate_state": self.candidate_state,
            "closure_manifest_digest": self.closure_manifest_digest,
            "contract_version": self.contract_version,
            "disposition": self.disposition,
            "external_inventory_digest": self.external_inventory_digest,
            "external_resource_count": self.external_resource_count,
            "external_restoration_debt_count": self.external_restoration_debt_count,
            "external_restoration_debt_digest": self.external_restoration_debt_digest,
            "journal_binding_digest": self.journal_binding_digest,
            "journal_recovery_digest": self.journal_recovery_digest,
            "observed_root": self.observed_root,
            "recovery_outcome": self.recovery_outcome,
            "request_digest": self.request_digest,
            "resource_count": self.resource_count,
            "restoration_scope": self.restoration_scope,
        });
        Ok(domain_digest(
            JOURNAL_RECEIPT_DOMAIN_V1,
            canonical_json(&body).as_bytes(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionFailureCodeV1 {
    SchemaVersionMismatch,
    InvalidEffectProgram,
    InvalidResource,
    MissingOperationResource,
    EmptyResourceInventory,
    TooManyResources,
    NonCanonicalResourceInventory,
    RequestMismatch,
    MissingResource,
    UnexpectedResource,
    UnsupportedIsolation,
    IsolationAccessMismatch,
    RestorationMismatch,
    RollbackMismatch,
    RawFallbackIsNotSpeculation,
    InvalidEffectAcceptance,
    EffectAcceptanceMismatch,
    MissingEffectAcceptance,
    BaselineMismatch,
    JournalBindingMismatch,
    JournalRootMismatch,
    ReceiptDigestMismatch,
    CanonicalPayloadTooLarge,
    NonCanonicalEncoding,
    SerializationFailure,
    JournalFailure,
}

impl TransactionFailureCodeV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaVersionMismatch => "schema_version_mismatch",
            Self::InvalidEffectProgram => "invalid_effect_program",
            Self::InvalidResource => "invalid_resource",
            Self::MissingOperationResource => "missing_operation_resource",
            Self::EmptyResourceInventory => "empty_resource_inventory",
            Self::TooManyResources => "too_many_resources",
            Self::NonCanonicalResourceInventory => "noncanonical_resource_inventory",
            Self::RequestMismatch => "request_mismatch",
            Self::MissingResource => "missing_resource",
            Self::UnexpectedResource => "unexpected_resource",
            Self::UnsupportedIsolation => "unsupported_isolation",
            Self::IsolationAccessMismatch => "isolation_access_mismatch",
            Self::RestorationMismatch => "restoration_mismatch",
            Self::RollbackMismatch => "rollback_mismatch",
            Self::RawFallbackIsNotSpeculation => "raw_fallback_is_not_speculation",
            Self::InvalidEffectAcceptance => "invalid_effect_acceptance",
            Self::EffectAcceptanceMismatch => "effect_acceptance_mismatch",
            Self::MissingEffectAcceptance => "missing_effect_acceptance",
            Self::BaselineMismatch => "baseline_mismatch",
            Self::JournalBindingMismatch => "journal_binding_mismatch",
            Self::JournalRootMismatch => "journal_root_mismatch",
            Self::ReceiptDigestMismatch => "receipt_digest_mismatch",
            Self::CanonicalPayloadTooLarge => "canonical_payload_too_large",
            Self::NonCanonicalEncoding => "noncanonical_encoding",
            Self::SerializationFailure => "serialization_failure",
            Self::JournalFailure => "journal_failure",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransactionErrorV1 {
    pub code: TransactionFailureCodeV1,
    pub resource: Option<TransactionResourceRequirementV1>,
    pub journal_code: Option<JournalFailureCodeV1>,
    pub detail: String,
}

impl TransactionErrorV1 {
    fn new(
        code: TransactionFailureCodeV1,
        resource: Option<TransactionResourceRequirementV1>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            resource,
            journal_code: None,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> TransactionFailureCodeV1 {
        self.code
    }
}

impl fmt::Display for TransactionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for TransactionErrorV1 {}

fn validate_program_resource_mapping(
    program: &EffectProgramV1,
    resources: &[TransactionResourceRequirementV1],
) -> Result<(), TransactionErrorV1> {
    if let Some(resource) = resources.iter().find(|resource| {
        resource.kind == TransactionResourceKindV1::ProjectFilesystem
            && resource.baseline_state_digest != program.base_state()
    }) {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::BaselineMismatch,
            Some(*resource),
            "project-filesystem resource baseline differs from the effect base state",
        ));
    }
    let covers = |owner: ArtifactOwnerV1, writes: bool, project_filesystem: bool| {
        resources.iter().any(|resource| {
            resource.owner == owner
                && (!project_filesystem
                    || resource.kind == TransactionResourceKindV1::ProjectFilesystem)
                && (!writes || resource.access.writes())
        })
    };
    for target in program.targets() {
        if !covers(target.owner, true, true) {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::MissingOperationResource,
                None,
                "effect target has no matching project-filesystem write resource",
            ));
        }
    }
    for operation in program.operations() {
        let owner_and_class = match operation {
            TypedEffectOperationV1::RecoverExact { owner, .. } => {
                Some((*owner, EffectClass::ReadOnly))
            }
            TypedEffectOperationV1::DeterministicTransform {
                owner,
                effect_class,
                ..
            }
            | TypedEffectOperationV1::InvokeCapability {
                owner,
                effect_class,
                ..
            } => Some((*owner, *effect_class)),
            TypedEffectOperationV1::ReplaceExactFile { .. }
            | TypedEffectOperationV1::CopyExact { .. }
            | TypedEffectOperationV1::ReturnLiteral { .. }
            | TypedEffectOperationV1::RawFallback => None,
        };
        if let Some((owner, effect_class)) = owner_and_class {
            let writes = effect_class != EffectClass::ReadOnly;
            if !covers(owner, writes, false) {
                return Err(TransactionErrorV1::new(
                    TransactionFailureCodeV1::MissingOperationResource,
                    None,
                    "effect capability has no matching owned resource scope",
                ));
            }
        }
    }
    Ok(())
}

fn sort_and_validate_requirements(
    resources: &mut [TransactionResourceRequirementV1],
) -> Result<(), TransactionErrorV1> {
    resources.sort_by_key(|resource| resource.key());
    validate_sorted_requirements(resources)
}

fn validate_sorted_requirements(
    resources: &[TransactionResourceRequirementV1],
) -> Result<(), TransactionErrorV1> {
    if resources.is_empty() {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::EmptyResourceInventory,
            None,
            "effect-closure inventory must be explicit and nonempty",
        ));
    }
    if resources.len() > TRANSACTION_MAX_RESOURCES_V1 {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::TooManyResources,
            None,
            format!("effect-closure inventory has {} resources", resources.len()),
        ));
    }
    let mut previous = None;
    for resource in resources {
        resource.validate()?;
        let key = resource.key();
        if previous.is_some_and(|prior| prior >= key) {
            return Err(TransactionErrorV1::new(
                TransactionFailureCodeV1::NonCanonicalResourceInventory,
                Some(*resource),
                "resource requirements must have unique kind/scope keys",
            ));
        }
        previous = Some(key);
    }
    Ok(())
}

fn canonical_serialize<T: Serialize>(value: &T) -> Result<Vec<u8>, TransactionErrorV1> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > TRANSACTION_MAX_CANONICAL_BYTES_V1 {
        return Err(TransactionErrorV1::new(
            TransactionFailureCodeV1::CanonicalPayloadTooLarge,
            None,
            format!(
                "transaction contract value has {} canonical bytes",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
}

fn serialization_error(error: serde_json::Error) -> TransactionErrorV1 {
    TransactionErrorV1::new(
        TransactionFailureCodeV1::SerializationFailure,
        None,
        error.to_string(),
    )
}

fn journal_error(error: JournalErrorV1) -> TransactionErrorV1 {
    TransactionErrorV1 {
        code: TransactionFailureCodeV1::JournalFailure,
        resource: None,
        journal_code: Some(error.code),
        detail: error.to_string(),
    }
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    DigestV1::from_bytes(hasher.finalize().into())
}

pub fn transaction_contract_v1() -> Value {
    json!({
        "canonical_encoding": "sorted_key_json",
        "contract_version": TRANSACTION_CONTRACT_VERSION_V1,
        "domains": {
            "contract": String::from_utf8_lossy(CONTRACT_DOMAIN_V1),
            "external_inventory": String::from_utf8_lossy(EXTERNAL_INVENTORY_DOMAIN_V1),
            "external_restoration_debt": String::from_utf8_lossy(EXTERNAL_DEBT_DOMAIN_V1),
            "journal_receipt": String::from_utf8_lossy(JOURNAL_RECEIPT_DOMAIN_V1),
            "manifest": String::from_utf8_lossy(MANIFEST_DOMAIN_V1),
            "request": String::from_utf8_lossy(REQUEST_DOMAIN_V1),
            "transaction_id": String::from_utf8_lossy(TRANSACTION_ID_DOMAIN_V1),
        },
        "effect_closure": {
            "access": ["read", "write", "read_write"],
            "isolation": [
                "immutable_snapshot", "recorded_replay", "buffered", "journaled",
                "transactional", "delayed_until_commit", "unsupported"
            ],
            "resource_kinds": [
                "project_filesystem", "graph_index", "toolchain", "runtime_manifest",
                "capability_surface", "external_database", "external_service",
                "observation_log", "time", "randomness", "queue", "rate_limit",
                "network", "approval", "provider_model_state", "decoder_state",
                "other_external"
            ],
            "restoration": [
                "not_needed", "recorded_replay", "journal_rollback",
                "transaction_rollback", "unsupported"
            ],
            "unsupported_blocks_speculation": true,
        },
        "limits": {
            "canonical_bytes": TRANSACTION_MAX_CANONICAL_BYTES_V1,
            "resources": TRANSACTION_MAX_RESOURCES_V1,
        },
        "schemas": {
            "closure_manifest": ["contract_version", "request_digest", "resources"],
            "closure_resource": ["requirement", "isolation", "restoration"],
            "closure_request": [
                "contract_version", "action_digest", "baseline_state", "rollback", "resources"
            ],
            "resource_requirement": [
                "owner", "kind", "scope_digest", "baseline_state_digest", "access",
                "authority_digest"
            ],
            "transaction_receipt": [
                "contract_version", "disposition", "restoration_scope", "request_digest",
                "closure_manifest_digest", "action_digest", "acceptance_digest",
                "baseline_state", "candidate_state", "external_inventory_digest",
                "external_restoration_debt_digest", "resource_count",
                "external_resource_count", "external_restoration_debt_count",
                "journal_binding_digest", "journal_recovery_digest", "recovery_outcome",
                "observed_root", "receipt_digest"
            ],
        },
        "receipt_claim_scope": {
            "complete": "declared_effect_closure_only_when_external_restoration_debt_is_zero",
            "debt": "project_journal_root_only",
            "dispositions": ["candidate_committed", "baseline_root_recovered"],
            "scopes": [
                "not_applicable_candidate_commit", "declared_effect_closure",
                "project_journal_root_only"
            ]
        },
        "store_carrier": "zero_store.durable_journal.v2",
        "transaction_id_fields": [
            "request_digest", "manifest_digest", "action_digest", "assembly_manifest_digest",
            "length_prefixed_durable_profile_id", "candidate_root", "owner_identity_digest"
        ],
        "workspace_clone_authority": "fszero",
        "forbidden_claims": [
            "filesystem_rollback_implies_external_state_restoration",
            "journal_receipt_implies_native_durability",
            "workspace_clone_is_atomic_multi_file_publication"
        ],
        "failures": [
            "schema_version_mismatch", "invalid_effect_program", "invalid_resource",
            "missing_operation_resource", "empty_resource_inventory", "too_many_resources",
            "noncanonical_resource_inventory", "request_mismatch", "missing_resource",
            "unexpected_resource", "unsupported_isolation", "isolation_access_mismatch",
            "restoration_mismatch", "rollback_mismatch", "raw_fallback_is_not_speculation",
            "invalid_effect_acceptance", "effect_acceptance_mismatch",
            "missing_effect_acceptance", "baseline_mismatch", "journal_binding_mismatch",
            "journal_root_mismatch", "receipt_digest_mismatch", "canonical_payload_too_large",
            "noncanonical_encoding", "serialization_failure", "journal_failure"
        ],
    })
}

pub fn transaction_contract_digest_v1() -> DigestV1 {
    domain_digest(
        CONTRACT_DOMAIN_V1,
        canonical_json(&transaction_contract_v1()).as_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
    use tempfile::tempdir;
    use zero_abi::{
        CwirVerifierClassV1, EffectTargetV1, EffectVerificationPlanV1, EffectVerificationStepV1,
        TypedEffectOperationV1, sha256,
    };
    use zero_cert::{
        CompletenessWitness, EffectVerificationOutcomeV1, EvidenceCertificate, ObjectId,
        OperatorLock, Provenance, Query, Resolver, SpanRef, accept_effect_verification_v1, verify,
    };
    use zero_store::{DurableProfileIdV1, initialize_published_root_v1};

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    struct Resident<'a> {
        bytes: &'a [u8],
    }

    impl Resolver for Resident<'_> {
        fn resolve<'a>(&'a self, object_id: &ObjectId) -> Option<&'a [u8]> {
            (sha256(self.bytes) == object_id.0).then_some(self.bytes)
        }
        fn trusted_operator_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "read-span").then_some("1")
        }
        fn trusted_parser_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "tree-sitter").then_some("1")
        }
        fn trusted_index_version<'a>(&'a self, id: &str) -> Option<&'a str> {
            (id == "zero-index").then_some("2")
        }
    }

    fn accepted(program: &EffectProgramV1) -> zero_cert::EffectAcceptedV1 {
        let bytes = b"exact evidence";
        let object = sha256(bytes);
        let span = SpanRef {
            object_id: ObjectId(object),
            object_digest: object,
            byte_start: 0,
            byte_len: bytes.len() as u64,
            span_digest: object,
        };
        let certificate = EvidenceCertificate {
            query: Query::ReadSpan(span.clone()),
            spans: vec![span],
            payload: Cow::Borrowed(bytes),
            provenance: Provenance {
                parser_id: "tree-sitter".into(),
                parser_version: "1".into(),
                index_id: "zero-index".into(),
                index_version: "2".into(),
                operator_id: "read-span".into(),
                operator_version: "1".into(),
            },
            completeness: CompletenessWitness::ReadSpan {
                operator: OperatorLock {
                    operator_id: "read-span".into(),
                    operator_version: "1".into(),
                },
            },
            input_token_cost: 1,
            backend_work_units: 1,
        };
        let resident = Resident { bytes };
        let verified = verify(&certificate, &resident).unwrap();
        let outcome = accept_effect_verification_v1(
            digest(70),
            program,
            digest(71),
            digest(21),
            program.base_state(),
            digest(20),
            &verified,
        )
        .unwrap();
        let EffectVerificationOutcomeV1::Accepted(accepted) = outcome else {
            panic!("expected accepted effect");
        };
        accepted
    }

    fn program(snapshot: DigestV1, rollback: EffectRollbackV1) -> EffectProgramV1 {
        let target = EffectTargetV1 {
            owner: ArtifactOwnerV1::FsZero,
            target_digest: digest(10),
            required_snapshot: snapshot,
        };
        let step = EffectVerificationStepV1 {
            verifier_digest: digest(20),
            predicate_digest: digest(21),
            environment_digest: digest(22),
            required_snapshot: snapshot,
            verifier_class: CwirVerifierClassV1::ExactChecker,
        };
        let (targets, operations) = if rollback == EffectRollbackV1::ReadOnly {
            let bytes = b"literal".to_vec();
            (
                vec![],
                vec![TypedEffectOperationV1::ReturnLiteral {
                    payload_digest: DigestV1::from_bytes(sha256(&bytes)),
                    bytes,
                }],
            )
        } else {
            (
                vec![target],
                vec![TypedEffectOperationV1::ReplaceExactFile {
                    target: digest(10),
                    expected_before: digest(11),
                    replacement: digest(12),
                }],
            )
        };
        EffectProgramV1::new(
            snapshot,
            "transaction_test",
            targets,
            vec![],
            operations,
            vec![],
            EffectVerificationPlanV1::new(vec![step]).unwrap(),
            rollback,
        )
        .unwrap()
    }

    fn resource(
        kind: TransactionResourceKindV1,
        scope: u8,
        baseline: DigestV1,
        access: TransactionAccessV1,
    ) -> TransactionResourceRequirementV1 {
        TransactionResourceRequirementV1 {
            owner: if kind == TransactionResourceKindV1::ProjectFilesystem {
                ArtifactOwnerV1::FsZero
            } else {
                ArtifactOwnerV1::ZeroStack
            },
            kind,
            scope_digest: digest(scope),
            baseline_state_digest: baseline,
            access,
            authority_digest: digest(scope.wrapping_add(2)),
        }
    }

    fn closed(
        snapshot: DigestV1,
    ) -> (
        EffectClosureRequestV1,
        EffectClosureManifestV1,
        ClosedEffectBoundaryV1,
    ) {
        let project = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            30,
            snapshot,
            TransactionAccessV1::ReadWrite,
        );
        let time = resource(
            TransactionResourceKindV1::Time,
            40,
            digest(41),
            TransactionAccessV1::Read,
        );
        let request = EffectClosureRequestV1::new(
            &program(snapshot, EffectRollbackV1::Journaled),
            vec![time, project],
        )
        .unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![
                EffectResourceClosureV1 {
                    requirement: time,
                    isolation: ResourceIsolationModeV1::RecordedReplay,
                    restoration: ResourceRestorationModeV1::RecordedReplay,
                },
                EffectResourceClosureV1 {
                    requirement: project,
                    isolation: ResourceIsolationModeV1::Journaled,
                    restoration: ResourceRestorationModeV1::JournalRollback,
                },
            ],
        )
        .unwrap();
        let boundary = validate_effect_closure_v1(&request, &manifest).unwrap();
        (request, manifest, boundary)
    }

    fn paths(dir: &std::path::Path) -> JournalPathsV1 {
        JournalPathsV1::new(
            dir.join("root.json"),
            dir.join("journal.json"),
            dir.join("cartridge.json"),
            dir.join("owner-death.json"),
            dir.join("recovery.json"),
        )
        .unwrap()
    }

    fn binding(closed: &ClosedEffectBoundaryV1, new: DigestV1) -> JournalBindingV1 {
        effect_journal_binding_v1(
            closed,
            digest(61),
            DurableProfileIdV1::PortableStrict,
            new,
            digest(62),
        )
        .unwrap()
    }

    #[test]
    fn closure_inventory_is_canonical_and_externally_explicit() {
        let (request, manifest, boundary) = closed(digest(1));
        let bytes = manifest.canonical_bytes().unwrap();
        assert_eq!(
            EffectClosureManifestV1::from_canonical_bytes(&bytes).unwrap(),
            manifest
        );
        assert_eq!(boundary.request_digest(), request.digest().unwrap());
        assert_eq!(boundary.resource_count(), 2);
        assert_eq!(boundary.external_resource_count(), 1);
        assert_ne!(boundary.external_inventory_digest(), DigestV1::ZERO);
        let mut whitespace = bytes;
        whitespace.push(b'\n');
        assert_eq!(
            EffectClosureManifestV1::from_canonical_bytes(&whitespace)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn unsupported_missing_and_incompatible_resources_block_speculation() {
        let snapshot = digest(1);
        let project = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            30,
            snapshot,
            TransactionAccessV1::ReadWrite,
        );
        let time = resource(
            TransactionResourceKindV1::Time,
            40,
            digest(41),
            TransactionAccessV1::Read,
        );
        assert_eq!(
            EffectClosureRequestV1::new(
                &program(snapshot, EffectRollbackV1::Journaled),
                vec![time],
            )
            .unwrap_err()
            .failure_code(),
            TransactionFailureCodeV1::MissingOperationResource
        );
        let request = EffectClosureRequestV1::new(
            &program(snapshot, EffectRollbackV1::Journaled),
            vec![project, time],
        )
        .unwrap();
        let unsupported = EffectClosureManifestV1::new(
            &request,
            vec![
                EffectResourceClosureV1 {
                    requirement: project,
                    isolation: ResourceIsolationModeV1::Journaled,
                    restoration: ResourceRestorationModeV1::JournalRollback,
                },
                EffectResourceClosureV1 {
                    requirement: time,
                    isolation: ResourceIsolationModeV1::Unsupported,
                    restoration: ResourceRestorationModeV1::Unsupported,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &unsupported)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::UnsupportedIsolation
        );
        let missing = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: project,
                isolation: ResourceIsolationModeV1::Journaled,
                restoration: ResourceRestorationModeV1::JournalRollback,
            }],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &missing)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::MissingResource
        );
        let invalid_access = EffectClosureManifestV1::new(
            &request,
            vec![
                EffectResourceClosureV1 {
                    requirement: project,
                    isolation: ResourceIsolationModeV1::ImmutableSnapshot,
                    restoration: ResourceRestorationModeV1::NotNeeded,
                },
                EffectResourceClosureV1 {
                    requirement: time,
                    isolation: ResourceIsolationModeV1::RecordedReplay,
                    restoration: ResourceRestorationModeV1::RecordedReplay,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &invalid_access)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::IsolationAccessMismatch
        );
    }

    #[test]
    fn rollback_class_must_cover_writes_and_raw_fallback_never_speculates() {
        let snapshot = digest(1);
        let project = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            30,
            snapshot,
            TransactionAccessV1::ReadWrite,
        );
        let request = EffectClosureRequestV1::new(
            &program(snapshot, EffectRollbackV1::WorkspaceClone),
            vec![project],
        )
        .unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: project,
                isolation: ResourceIsolationModeV1::Journaled,
                restoration: ResourceRestorationModeV1::JournalRollback,
            }],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &manifest)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::RollbackMismatch
        );

        let raw = EffectProgramV1::new(
            snapshot,
            "raw",
            vec![],
            vec![],
            vec![TypedEffectOperationV1::RawFallback],
            vec![],
            EffectVerificationPlanV1::new(vec![]).unwrap(),
            EffectRollbackV1::RawFallback,
        )
        .unwrap();
        let read = resource(
            TransactionResourceKindV1::ProjectFilesystem,
            31,
            snapshot,
            TransactionAccessV1::Read,
        );
        let request = EffectClosureRequestV1::new(&raw, vec![read]).unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: read,
                isolation: ResourceIsolationModeV1::ImmutableSnapshot,
                restoration: ResourceRestorationModeV1::NotNeeded,
            }],
        )
        .unwrap();
        assert_eq!(
            validate_effect_closure_v1(&request, &manifest)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::RawFallbackIsNotSpeculation
        );
    }

    #[test]
    fn external_transactional_writes_remain_explicit_restoration_debt() {
        let snapshot = digest(1);
        let step = EffectVerificationStepV1 {
            verifier_digest: digest(20),
            predicate_digest: digest(21),
            environment_digest: digest(22),
            required_snapshot: snapshot,
            verifier_class: CwirVerifierClassV1::ExactChecker,
        };
        let effect = EffectProgramV1::new(
            snapshot,
            "external_tx",
            vec![],
            vec![],
            vec![TypedEffectOperationV1::InvokeCapability {
                owner: ArtifactOwnerV1::ZeroStack,
                capability: "external.database".into(),
                generation: 1,
                capability_contract_digest: digest(50),
                arguments_digest: digest(51),
                effect_class: EffectClass::ReversibleMutation,
            }],
            vec![],
            EffectVerificationPlanV1::new(vec![step]).unwrap(),
            EffectRollbackV1::ExternalTransaction,
        )
        .unwrap();
        let database = resource(
            TransactionResourceKindV1::ExternalDatabase,
            80,
            digest(81),
            TransactionAccessV1::ReadWrite,
        );
        let request = EffectClosureRequestV1::new(&effect, vec![database]).unwrap();
        let manifest = EffectClosureManifestV1::new(
            &request,
            vec![EffectResourceClosureV1 {
                requirement: database,
                isolation: ResourceIsolationModeV1::Transactional,
                restoration: ResourceRestorationModeV1::TransactionRollback,
            }],
        )
        .unwrap();
        let boundary = validate_effect_closure_v1(&request, &manifest).unwrap();
        assert_eq!(boundary.external_resource_count(), 1);
        assert_eq!(boundary.external_restoration_debt_count(), 1);
        assert_ne!(boundary.external_restoration_debt_digest(), DigestV1::ZERO);

        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        initialize_published_root_v1(&paths, snapshot).unwrap();
        let receipt = begin_effect_transaction_v1(paths, binding(&boundary, digest(2)), &boundary)
            .unwrap()
            .abort()
            .unwrap();
        assert_eq!(
            receipt.disposition(),
            TransactionDispositionV1::BaselineRootRecovered
        );
        assert_eq!(
            receipt.restoration_scope(),
            RestorationScopeV1::ProjectJournalRootOnly
        );
        assert_eq!(receipt.external_restoration_debt_count(), 1);
        assert_eq!(
            receipt.external_restoration_debt_digest(),
            boundary.external_restoration_debt_digest()
        );
    }

    #[test]
    fn journal_commit_binds_effect_closure_and_external_inventory() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let old = digest(1);
        let new = digest(2);
        initialize_published_root_v1(&paths, old).unwrap();
        let (_, _, boundary) = closed(old);
        let receipt = begin_effect_transaction_v1(paths, binding(&boundary, new), &boundary)
            .unwrap()
            .commit(&accepted(&program(old, EffectRollbackV1::Journaled)))
            .unwrap();
        assert_eq!(
            receipt.disposition(),
            TransactionDispositionV1::CandidateCommitted
        );
        assert_eq!(
            receipt.restoration_scope(),
            RestorationScopeV1::NotApplicableCandidateCommit
        );
        assert_eq!(receipt.observed_root(), new);
        assert_eq!(receipt.external_resource_count(), 1);
        assert!(receipt.acceptance_digest().is_some());
        assert_eq!(
            receipt.closure_manifest_digest(),
            boundary.manifest_digest()
        );
        assert_ne!(receipt.receipt_digest(), DigestV1::ZERO);
        receipt.canonical_bytes().unwrap();
    }

    #[test]
    fn candidate_commit_requires_matching_zero_cert_acceptance() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let old = digest(1);
        let new = digest(2);
        initialize_published_root_v1(&paths, old).unwrap();
        let (_, _, boundary) = closed(old);
        let transaction =
            begin_effect_transaction_v1(paths.clone(), binding(&boundary, new), &boundary).unwrap();
        let wrong = accepted(&program(old, EffectRollbackV1::SingleAtomic));
        assert_eq!(
            transaction.commit(&wrong).unwrap_err().failure_code(),
            TransactionFailureCodeV1::EffectAcceptanceMismatch
        );
        let receipt =
            recover_effect_transaction_v1(&paths, &binding(&boundary, new), &boundary, None)
                .unwrap();
        assert_eq!(
            receipt.disposition(),
            TransactionDispositionV1::BaselineRootRecovered
        );
    }

    #[test]
    fn committed_recovery_refuses_to_invent_missing_acceptance() {
        let temp = tempdir().unwrap();
        let paths = paths(temp.path());
        let old = digest(1);
        let new = digest(2);
        initialize_published_root_v1(&paths, old).unwrap();
        let (_, _, boundary) = closed(old);
        let binding = binding(&boundary, new);
        let transaction =
            begin_effect_transaction_v1(paths.clone(), binding.clone(), &boundary).unwrap();
        commit_journal_v1(&transaction.paths, &transaction.cartridge).unwrap();
        drop(transaction);
        assert_eq!(
            recover_effect_transaction_v1(&paths, &binding, &boundary, None)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::MissingEffectAcceptance
        );
        let accepted = accepted(&program(old, EffectRollbackV1::Journaled));
        let receipt =
            recover_effect_transaction_v1(&paths, &binding, &boundary, Some(&accepted)).unwrap();
        assert_eq!(
            receipt.disposition(),
            TransactionDispositionV1::CandidateCommitted
        );
        assert_eq!(
            receipt.acceptance_digest(),
            Some(accepted.acceptance_digest())
        );
    }

    #[test]
    fn journal_abort_and_recovery_claim_only_declared_effect_closure() {
        for recover in [false, true] {
            let temp = tempdir().unwrap();
            let paths = paths(temp.path());
            let old = digest(1);
            let new = digest(2);
            initialize_published_root_v1(&paths, old).unwrap();
            let (_, _, boundary) = closed(old);
            let binding = binding(&boundary, new);
            let transaction =
                begin_effect_transaction_v1(paths.clone(), binding.clone(), &boundary).unwrap();
            let receipt = if recover {
                drop(transaction);
                recover_effect_transaction_v1(&paths, &binding, &boundary, None).unwrap()
            } else {
                transaction.abort().unwrap()
            };
            assert_eq!(
                receipt.disposition(),
                TransactionDispositionV1::BaselineRootRecovered
            );
            assert_eq!(
                receipt.restoration_scope(),
                RestorationScopeV1::DeclaredEffectClosure
            );
            assert_eq!(receipt.observed_root(), old);
            assert!(matches!(
                receipt.recovery_outcome(),
                RecoveryOutcomeV1::OldRootAborted
            ));
        }
    }

    #[test]
    fn journal_binding_must_start_at_effect_baseline() {
        let temp = tempdir().unwrap();
        let journal_paths = paths(temp.path());
        initialize_published_root_v1(&journal_paths, digest(9)).unwrap();
        let (_, _, boundary) = closed(digest(1));
        let mismatched = JournalBindingV1::new(
            digest(60),
            digest(61),
            DurableProfileIdV1::PortableStrict,
            digest(9),
            digest(2),
            digest(62),
        );
        let error = begin_effect_transaction_v1(journal_paths, mismatched, &boundary).unwrap_err();
        assert_eq!(
            error.failure_code(),
            TransactionFailureCodeV1::BaselineMismatch
        );
        let substituted = JournalBindingV1::new(
            digest(60),
            digest(61),
            DurableProfileIdV1::PortableStrict,
            boundary.baseline_state(),
            digest(2),
            digest(62),
        );
        assert_eq!(
            begin_effect_transaction_v1(paths(temp.path()), substituted, &boundary)
                .unwrap_err()
                .failure_code(),
            TransactionFailureCodeV1::JournalBindingMismatch
        );
    }

    #[test]
    fn transaction_contract_digest_is_stable() {
        assert_eq!(
            transaction_contract_digest_v1().to_hex(),
            "bd07297dca414b7acdc680d0d5abd7543af92153e14aa0b844925d686889e491"
        );
    }
}
