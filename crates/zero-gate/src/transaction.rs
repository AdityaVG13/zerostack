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
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};
use zero_abi::{
    canonical_json, ArtifactOwner, Sha256Digest, EffectClass, EffectProgram, EffectRollback,
    TypedEffectOperation,
};
use zero_cert::EffectAccepted;
use zero_store::{
    abort_journal, commit_journal, prepare_journal, recover_journal,
    ContinuationCartridge, DurableProfileId, JournalBinding, JournalError,
    JournalFailureCode, JournalPaths, RecoveryOutcome, RecoveryReceipt,
};

pub const TRANSACTION_CONTRACT_VERSION: u16 = 1;
pub const TRANSACTION_MAX_RESOURCES: usize = 256;
pub const TRANSACTION_MAX_CANONICAL_BYTES: usize = 1_048_576;

const REQUEST_DOMAIN: &[u8] = b"zerostack.effect_closure.request.v1\0";
const TRANSACTION_ID_DOMAIN: &[u8] = b"zerostack.effect_transaction.id.v1\0";
const MANIFEST_DOMAIN: &[u8] = b"zerostack.effect_closure.manifest.v1\0";
const EXTERNAL_INVENTORY_DOMAIN: &[u8] = b"zerostack.effect_closure.external_inventory.v1\0";
const EXTERNAL_DEBT_DOMAIN: &[u8] = b"zerostack.effect_closure.external_debt.v1\0";
const JOURNAL_RECEIPT_DOMAIN: &[u8] = b"zerostack.effect_transaction.receipt.v1\0";
const CONTRACT_DOMAIN: &[u8] = b"zerostack.transaction.contract.v1\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionResourceKind {
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

impl TransactionResourceKind {
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
pub enum TransactionAccess {
    Read,
    Write,
    ReadWrite,
}

impl TransactionAccess {
    const fn writes(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceIsolationMode {
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
pub enum ResourceRestorationMode {
    NotNeeded,
    RecordedReplay,
    JournalRollback,
    TransactionRollback,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionResourceRequirement {
    pub owner: ArtifactOwner,
    pub kind: TransactionResourceKind,
    pub scope_digest: Sha256Digest,
    pub baseline_state_digest: Sha256Digest,
    pub access: TransactionAccess,
    pub authority_digest: Sha256Digest,
}

impl TransactionResourceRequirement {
    fn key(self) -> (TransactionResourceKind, Sha256Digest) {
        (self.kind, self.scope_digest)
    }

    fn validate(self) -> Result<(), TransactionError> {
        if self.scope_digest == Sha256Digest::ZERO
            || self.baseline_state_digest == Sha256Digest::ZERO
            || self.authority_digest == Sha256Digest::ZERO
        {
            return Err(TransactionError::new(
                TransactionFailureCode::InvalidResource,
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
/// it from a validated `EffectProgram` through `new`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectClosureRequest {
    contract_version: u16,
    action_digest: Sha256Digest,
    baseline_state: Sha256Digest,
    rollback: EffectRollback,
    resources: Vec<TransactionResourceRequirement>,
}

impl EffectClosureRequest {
    pub fn new(
        program: &EffectProgram,
        mut resources: Vec<TransactionResourceRequirement>,
    ) -> Result<Self, TransactionError> {
        program.validate().map_err(|error| {
            TransactionError::new(
                TransactionFailureCode::InvalidEffectProgram,
                None,
                error.to_string(),
            )
        })?;
        sort_and_validate_requirements(&mut resources)?;
        validate_program_resource_mapping(program, &resources)?;
        let request = Self {
            contract_version: TRANSACTION_CONTRACT_VERSION,
            action_digest: program.action_digest(),
            baseline_state: program.base_state(),
            rollback: program.rollback(),
            resources,
        };
        request.validate()?;
        Ok(request)
    }

    pub const fn action_digest(&self) -> Sha256Digest {
        self.action_digest
    }
    pub const fn baseline_state(&self) -> Sha256Digest {
        self.baseline_state
    }
    pub const fn rollback(&self) -> EffectRollback {
        self.rollback
    }
    pub fn resources(&self) -> &[TransactionResourceRequirement] {
        &self.resources
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        self.validate()?;
        canonical_serialize(self)
    }

    pub fn digest(&self) -> Result<Sha256Digest, TransactionError> {
        Ok(domain_digest(REQUEST_DOMAIN, &self.canonical_bytes()?))
    }

    fn validate(&self) -> Result<(), TransactionError> {
        if self.contract_version != TRANSACTION_CONTRACT_VERSION {
            return Err(TransactionError::new(
                TransactionFailureCode::SchemaVersionMismatch,
                None,
                "effect-closure request version is unsupported",
            ));
        }
        if self.action_digest == Sha256Digest::ZERO || self.baseline_state == Sha256Digest::ZERO {
            return Err(TransactionError::new(
                TransactionFailureCode::InvalidEffectProgram,
                None,
                "action and baseline digests must be nonzero",
            ));
        }
        validate_sorted_requirements(&self.resources)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectResourceClosure {
    pub requirement: TransactionResourceRequirement,
    pub isolation: ResourceIsolationMode,
    pub restoration: ResourceRestorationMode,
}

impl EffectResourceClosure {
    fn validate(self) -> Result<(), TransactionError> {
        if self.isolation == ResourceIsolationMode::Unsupported
            || self.restoration == ResourceRestorationMode::Unsupported
        {
            return Err(TransactionError::new(
                TransactionFailureCode::UnsupportedIsolation,
                Some(self.requirement),
                "unsupported isolation or restoration blocks speculation",
            ));
        }
        let access_ok = match self.requirement.access {
            TransactionAccess::Read => {
                !matches!(self.isolation, ResourceIsolationMode::DelayedUntilCommit)
            }
            TransactionAccess::Write | TransactionAccess::ReadWrite => !matches!(
                self.isolation,
                ResourceIsolationMode::ImmutableSnapshot
                    | ResourceIsolationMode::RecordedReplay
            ),
        };
        if !access_ok {
            return Err(TransactionError::new(
                TransactionFailureCode::IsolationAccessMismatch,
                Some(self.requirement),
                "resource access is incompatible with its isolation mode",
            ));
        }
        let expected = match self.isolation {
            ResourceIsolationMode::ImmutableSnapshot
            | ResourceIsolationMode::Buffered
            | ResourceIsolationMode::DelayedUntilCommit => ResourceRestorationMode::NotNeeded,
            ResourceIsolationMode::RecordedReplay => ResourceRestorationMode::RecordedReplay,
            ResourceIsolationMode::Journaled => ResourceRestorationMode::JournalRollback,
            ResourceIsolationMode::Transactional => {
                ResourceRestorationMode::TransactionRollback
            }
            ResourceIsolationMode::Unsupported => {
                return Err(TransactionError::new(
                    TransactionFailureCode::UnsupportedIsolation,
                    Some(self.requirement),
                    "unsupported isolation or restoration blocks speculation",
                ));
            }
        };
        if self.restoration != expected {
            return Err(TransactionError::new(
                TransactionFailureCode::RestorationMismatch,
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
pub struct EffectClosureManifest {
    contract_version: u16,
    request_digest: Sha256Digest,
    resources: Vec<EffectResourceClosure>,
}

impl EffectClosureManifest {
    pub fn new(
        request: &EffectClosureRequest,
        mut resources: Vec<EffectResourceClosure>,
    ) -> Result<Self, TransactionError> {
        resources.sort_by_key(|entry| entry.requirement.key());
        let manifest = Self {
            contract_version: TRANSACTION_CONTRACT_VERSION,
            request_digest: request.digest()?,
            resources,
        };
        manifest.validate_body()?;
        Ok(manifest)
    }

    pub fn resources(&self) -> &[EffectResourceClosure] {
        &self.resources
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        self.validate_body()?;
        canonical_serialize(self)
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, TransactionError> {
        if bytes.len() > TRANSACTION_MAX_CANONICAL_BYTES {
            return Err(TransactionError::new(
                TransactionFailureCode::CanonicalPayloadTooLarge,
                None,
                format!("effect-closure manifest has {} bytes", bytes.len()),
            ));
        }
        let value: Value = serde_json::from_slice(bytes).map_err(serialization_error)?;
        if canonical_json(&value).as_bytes() != bytes {
            return Err(TransactionError::new(
                TransactionFailureCode::NonCanonicalEncoding,
                None,
                "effect-closure manifest is not exact canonical JSON",
            ));
        }
        let manifest: Self = serde_json::from_value(value).map_err(serialization_error)?;
        manifest.validate_body()?;
        Ok(manifest)
    }

    pub fn digest(&self) -> Result<Sha256Digest, TransactionError> {
        Ok(domain_digest(MANIFEST_DOMAIN, &self.canonical_bytes()?))
    }

    fn validate_body(&self) -> Result<(), TransactionError> {
        if self.contract_version != TRANSACTION_CONTRACT_VERSION {
            return Err(TransactionError::new(
                TransactionFailureCode::SchemaVersionMismatch,
                None,
                "effect-closure manifest version is unsupported",
            ));
        }
        if self.request_digest == Sha256Digest::ZERO {
            return Err(TransactionError::new(
                TransactionFailureCode::RequestMismatch,
                None,
                "effect-closure request digest is zero",
            ));
        }
        if self.resources.is_empty() {
            return Err(TransactionError::new(
                TransactionFailureCode::EmptyResourceInventory,
                None,
                "effect-closure inventory must be explicit and nonempty",
            ));
        }
        if self.resources.len() > TRANSACTION_MAX_RESOURCES {
            return Err(TransactionError::new(
                TransactionFailureCode::TooManyResources,
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
                return Err(TransactionError::new(
                    TransactionFailureCode::NonCanonicalResourceInventory,
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
pub struct ClosedEffectBoundary {
    request_digest: Sha256Digest,
    manifest_digest: Sha256Digest,
    action_digest: Sha256Digest,
    baseline_state: Sha256Digest,
    rollback: EffectRollback,
    external_inventory_digest: Sha256Digest,
    external_restoration_debt_digest: Sha256Digest,
    resource_count: u16,
    external_resource_count: u16,
    external_restoration_debt_count: u16,
}

impl ClosedEffectBoundary {
    pub const fn request_digest(&self) -> Sha256Digest {
        self.request_digest
    }
    pub const fn manifest_digest(&self) -> Sha256Digest {
        self.manifest_digest
    }
    pub const fn action_digest(&self) -> Sha256Digest {
        self.action_digest
    }
    pub const fn baseline_state(&self) -> Sha256Digest {
        self.baseline_state
    }
    pub const fn rollback(&self) -> EffectRollback {
        self.rollback
    }
    pub const fn external_inventory_digest(&self) -> Sha256Digest {
        self.external_inventory_digest
    }
    pub const fn resource_count(&self) -> u16 {
        self.resource_count
    }
    pub const fn external_resource_count(&self) -> u16 {
        self.external_resource_count
    }
    pub const fn external_restoration_debt_digest(&self) -> Sha256Digest {
        self.external_restoration_debt_digest
    }
    pub const fn external_restoration_debt_count(&self) -> u16 {
        self.external_restoration_debt_count
    }
}

pub fn validate_effect_closure(
    request: &EffectClosureRequest,
    manifest: &EffectClosureManifest,
) -> Result<ClosedEffectBoundary, TransactionError> {
    request.validate()?;
    manifest.validate_body()?;
    if manifest.request_digest != request.digest()? {
        return Err(TransactionError::new(
            TransactionFailureCode::RequestMismatch,
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
                    TransactionFailureCode::MissingResource
                } else {
                    TransactionFailureCode::UnexpectedResource
                };
                return Err(TransactionError::new(
                    code,
                    Some(if code == TransactionFailureCode::MissingResource {
                        *required
                    } else {
                        entry.requirement
                    }),
                    "effect-closure inventory differs from the preregistered request",
                ));
            }
            (Some(required), None) => {
                return Err(TransactionError::new(
                    TransactionFailureCode::MissingResource,
                    Some(*required),
                    "effect-closure manifest omitted a required resource",
                ));
            }
            (None, Some(entry)) => {
                return Err(TransactionError::new(
                    TransactionFailureCode::UnexpectedResource,
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
                    ResourceIsolationMode::Journaled | ResourceIsolationMode::Transactional
                )
        })
        .copied()
        .collect::<Vec<_>>();
    let external_bytes = canonical_serialize(&external)?;
    let external_debt_bytes = canonical_serialize(&external_debt)?;
    Ok(ClosedEffectBoundary {
        request_digest: request.digest()?,
        manifest_digest: manifest.digest()?,
        action_digest: request.action_digest,
        baseline_state: request.baseline_state,
        rollback: request.rollback,
        external_inventory_digest: domain_digest(EXTERNAL_INVENTORY_DOMAIN, &external_bytes),
        external_restoration_debt_digest: domain_digest(
            EXTERNAL_DEBT_DOMAIN,
            &external_debt_bytes,
        ),
        resource_count: request.resources.len() as u16,
        external_resource_count: external.len() as u16,
        external_restoration_debt_count: external_debt.len() as u16,
    })
}

fn validate_rollback_coverage(
    rollback: EffectRollback,
    resources: &[EffectResourceClosure],
) -> Result<(), TransactionError> {
    let writes = resources
        .iter()
        .filter(|entry| entry.requirement.access.writes())
        .collect::<Vec<_>>();
    if rollback == EffectRollback::RawFallback {
        return Err(TransactionError::new(
            TransactionFailureCode::RawFallbackIsNotSpeculation,
            None,
            "raw fallback is the frozen baseline, not a speculative transaction",
        ));
    }
    if rollback == EffectRollback::ReadOnly && !writes.is_empty() {
        return Err(TransactionError::new(
            TransactionFailureCode::RollbackMismatch,
            Some(writes[0].requirement),
            "read-only rollback cannot cover a write resource",
        ));
    }
    if rollback != EffectRollback::ReadOnly && writes.is_empty() {
        return Err(TransactionError::new(
            TransactionFailureCode::RollbackMismatch,
            None,
            "mutating rollback class requires at least one write resource",
        ));
    }
    if rollback == EffectRollback::SingleAtomic && writes.len() != 1 {
        return Err(TransactionError::new(
            TransactionFailureCode::RollbackMismatch,
            None,
            "single-atomic rollback requires exactly one isolated write resource",
        ));
    }
    for entry in writes {
        let mode = entry.isolation;
        let compatible = match rollback {
            EffectRollback::ReadOnly | EffectRollback::RawFallback => false,
            EffectRollback::SingleAtomic | EffectRollback::Journaled => matches!(
                mode,
                ResourceIsolationMode::Buffered
                    | ResourceIsolationMode::Journaled
                    | ResourceIsolationMode::Transactional
                    | ResourceIsolationMode::DelayedUntilCommit
            ),
            EffectRollback::WorkspaceClone => matches!(
                mode,
                ResourceIsolationMode::Buffered | ResourceIsolationMode::DelayedUntilCommit
            ),
            EffectRollback::ExternalTransaction => matches!(
                mode,
                ResourceIsolationMode::Buffered
                    | ResourceIsolationMode::Transactional
                    | ResourceIsolationMode::DelayedUntilCommit
            ),
        };
        if !compatible {
            return Err(TransactionError::new(
                TransactionFailureCode::RollbackMismatch,
                Some(entry.requirement),
                format!("rollback {rollback:?} cannot cover isolation {mode:?}"),
            ));
        }
    }
    Ok(())
}

/// Live linear handle for one prepared zero-store journal transaction.
#[derive(Debug)]
pub struct EffectTransaction {
    paths: JournalPaths,
    binding: JournalBinding,
    cartridge: ContinuationCartridge,
    closure: ClosureBinding,
}

#[derive(Clone, Copy, Debug)]
struct ClosureBinding {
    request_digest: Sha256Digest,
    manifest_digest: Sha256Digest,
    action_digest: Sha256Digest,
    baseline_state: Sha256Digest,
    external_inventory_digest: Sha256Digest,
    external_restoration_debt_digest: Sha256Digest,
    resource_count: u16,
    external_resource_count: u16,
    external_restoration_debt_count: u16,
}

impl ClosureBinding {
    fn from_closed(closed: &ClosedEffectBoundary) -> Self {
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

pub fn effect_journal_binding(
    closed: &ClosedEffectBoundary,
    assembly_manifest_digest: Sha256Digest,
    durable_profile_id: DurableProfileId,
    candidate_root: Sha256Digest,
    owner_identity_digest: Sha256Digest,
) -> Result<JournalBinding, TransactionError> {
    for (label, digest) in [
        ("assembly manifest", assembly_manifest_digest),
        ("candidate root", candidate_root),
        ("owner identity", owner_identity_digest),
    ] {
        if digest == Sha256Digest::ZERO {
            return Err(TransactionError::new(
                TransactionFailureCode::JournalBindingMismatch,
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
    let binding = JournalBinding::new(
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
    closed: &ClosedEffectBoundary,
    assembly_manifest_digest: Sha256Digest,
    durable_profile_id: DurableProfileId,
    candidate_root: Sha256Digest,
    owner_identity_digest: Sha256Digest,
) -> Sha256Digest {
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
    domain_digest(TRANSACTION_ID_DOMAIN, &bytes)
}

pub fn begin_effect_transaction(
    paths: JournalPaths,
    binding: JournalBinding,
    closed: &ClosedEffectBoundary,
) -> Result<EffectTransaction, TransactionError> {
    validate_journal_binding(&binding, closed)?;
    let cartridge = prepare_journal(&paths, binding.clone()).map_err(journal_error)?;
    Ok(EffectTransaction {
        paths,
        binding,
        cartridge,
        closure: ClosureBinding::from_closed(closed),
    })
}

impl EffectTransaction {
    /// Commits only after zero-cert has minted an acceptance for this exact action and baseline.
    pub fn commit(
        self,
        acceptance: &EffectAccepted,
    ) -> Result<TransactionReceipt, TransactionError> {
        let acceptance_digest = validate_acceptance(acceptance, self.closure)?;
        let recovery = commit_journal(&self.paths, &self.cartridge).map_err(journal_error)?;
        TransactionReceipt::from_recovery(
            &self.binding,
            self.closure,
            &recovery,
            Some(acceptance_digest),
        )
    }

    pub fn abort(self) -> Result<TransactionReceipt, TransactionError> {
        let recovery = abort_journal(&self.paths, &self.cartridge).map_err(journal_error)?;
        TransactionReceipt::from_recovery(&self.binding, self.closure, &recovery, None)
    }
}

/// Recovers a prepared transaction. A new-root outcome requires the exact
/// zero-cert acceptance to be revalidated; old-root recovery never claims one.
pub fn recover_effect_transaction(
    paths: &JournalPaths,
    binding: &JournalBinding,
    closed: &ClosedEffectBoundary,
    acceptance: Option<&EffectAccepted>,
) -> Result<TransactionReceipt, TransactionError> {
    validate_journal_binding(binding, closed)?;
    let closure = ClosureBinding::from_closed(closed);
    let acceptance_digest = acceptance
        .map(|accepted| validate_acceptance(accepted, closure))
        .transpose()?;
    let recovery = recover_journal(paths, binding).map_err(journal_error)?;
    TransactionReceipt::from_recovery(binding, closure, &recovery, acceptance_digest)
}

fn validate_acceptance(
    acceptance: &EffectAccepted,
    closure: ClosureBinding,
) -> Result<Sha256Digest, TransactionError> {
    acceptance.validate().map_err(|error| {
        TransactionError::new(
            TransactionFailureCode::InvalidEffectAcceptance,
            None,
            error.to_string(),
        )
    })?;
    if acceptance.action_digest() != closure.action_digest
        || acceptance.state_snapshot() != closure.baseline_state
    {
        return Err(TransactionError::new(
            TransactionFailureCode::EffectAcceptanceMismatch,
            None,
            "effect acceptance binds another action or baseline snapshot",
        ));
    }
    Ok(acceptance.acceptance_digest())
}

fn validate_journal_binding(
    binding: &JournalBinding,
    closed: &ClosedEffectBoundary,
) -> Result<(), TransactionError> {
    binding.validate().map_err(journal_error)?;
    if binding.old_root != closed.baseline_state {
        return Err(TransactionError::new(
            TransactionFailureCode::BaselineMismatch,
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
        return Err(TransactionError::new(
            TransactionFailureCode::JournalBindingMismatch,
            None,
            "journal transaction id does not bind the effect closure",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionDisposition {
    CandidateCommitted,
    BaselineRootRecovered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RestorationScope {
    NotApplicableCandidateCommit,
    DeclaredEffectClosure,
    ProjectJournalRootOnly,
}

/// Journal-bound outcome record. It does not claim native durability or
/// restoration outside the preregistered effect-closure inventory.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionReceipt {
    contract_version: u16,
    disposition: TransactionDisposition,
    restoration_scope: RestorationScope,
    request_digest: Sha256Digest,
    closure_manifest_digest: Sha256Digest,
    action_digest: Sha256Digest,
    acceptance_digest: Option<Sha256Digest>,
    baseline_state: Sha256Digest,
    candidate_state: Sha256Digest,
    external_inventory_digest: Sha256Digest,
    external_restoration_debt_digest: Sha256Digest,
    resource_count: u16,
    external_resource_count: u16,
    external_restoration_debt_count: u16,
    journal_binding_digest: Sha256Digest,
    journal_recovery_digest: Sha256Digest,
    recovery_outcome: RecoveryOutcome,
    observed_root: Sha256Digest,
    receipt_digest: Sha256Digest,
}

impl TransactionReceipt {
    fn from_recovery(
        binding: &JournalBinding,
        closure: ClosureBinding,
        recovery: &RecoveryReceipt,
        acceptance_digest: Option<Sha256Digest>,
    ) -> Result<Self, TransactionError> {
        recovery.canonical_bytes().map_err(journal_error)?;
        let binding_digest = binding.digest().map_err(journal_error)?;
        if recovery.binding_digest != binding_digest {
            return Err(TransactionError::new(
                TransactionFailureCode::JournalBindingMismatch,
                None,
                "recovery receipt binds another durable journal",
            ));
        }
        let (disposition, restoration_scope, expected_root) = match recovery.outcome {
            RecoveryOutcome::NewRootCommitted | RecoveryOutcome::AlreadyCommitted => (
                TransactionDisposition::CandidateCommitted,
                RestorationScope::NotApplicableCandidateCommit,
                binding.new_root,
            ),
            RecoveryOutcome::NotStartedOldRoot
            | RecoveryOutcome::OldRootAborted
            | RecoveryOutcome::AlreadyAborted => (
                TransactionDisposition::BaselineRootRecovered,
                if closure.external_restoration_debt_count == 0 {
                    RestorationScope::DeclaredEffectClosure
                } else {
                    RestorationScope::ProjectJournalRootOnly
                },
                binding.old_root,
            ),
        };
        if disposition == TransactionDisposition::CandidateCommitted
            && acceptance_digest.is_none()
        {
            return Err(TransactionError::new(
                TransactionFailureCode::MissingEffectAcceptance,
                None,
                "candidate commit recovery requires zero-cert effect acceptance",
            ));
        }
        let acceptance_digest = if disposition == TransactionDisposition::CandidateCommitted {
            acceptance_digest
        } else {
            None
        };
        if recovery.observed_root != expected_root || closure.baseline_state != binding.old_root {
            return Err(TransactionError::new(
                TransactionFailureCode::JournalRootMismatch,
                None,
                "journal recovery outcome and observed root disagree",
            ));
        }
        let mut receipt = Self {
            contract_version: TRANSACTION_CONTRACT_VERSION,
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
            receipt_digest: Sha256Digest::ZERO,
        };
        receipt.receipt_digest = receipt.expected_digest()?;
        Ok(receipt)
    }

    pub const fn disposition(&self) -> TransactionDisposition {
        self.disposition
    }
    pub const fn action_digest(&self) -> Sha256Digest {
        self.action_digest
    }
    pub const fn baseline_state(&self) -> Sha256Digest {
        self.baseline_state
    }
    pub const fn candidate_state(&self) -> Sha256Digest {
        self.candidate_state
    }
    pub const fn acceptance_digest(&self) -> Option<Sha256Digest> {
        self.acceptance_digest
    }
    pub const fn restoration_scope(&self) -> RestorationScope {
        self.restoration_scope
    }
    pub const fn closure_manifest_digest(&self) -> Sha256Digest {
        self.closure_manifest_digest
    }
    pub const fn external_inventory_digest(&self) -> Sha256Digest {
        self.external_inventory_digest
    }
    pub const fn resource_count(&self) -> u16 {
        self.resource_count
    }
    pub const fn external_resource_count(&self) -> u16 {
        self.external_resource_count
    }
    pub const fn external_restoration_debt_digest(&self) -> Sha256Digest {
        self.external_restoration_debt_digest
    }
    pub const fn external_restoration_debt_count(&self) -> u16 {
        self.external_restoration_debt_count
    }
    pub const fn observed_root(&self) -> Sha256Digest {
        self.observed_root
    }
    pub const fn recovery_outcome(&self) -> RecoveryOutcome {
        self.recovery_outcome
    }
    pub const fn receipt_digest(&self) -> Sha256Digest {
        self.receipt_digest
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TransactionError> {
        if self.receipt_digest != self.expected_digest()? {
            return Err(TransactionError::new(
                TransactionFailureCode::ReceiptDigestMismatch,
                None,
                "transaction receipt digest does not match its canonical body",
            ));
        }
        canonical_serialize(self)
    }

    fn expected_digest(&self) -> Result<Sha256Digest, TransactionError> {
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
            JOURNAL_RECEIPT_DOMAIN,
            canonical_json(&body).as_bytes(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransactionFailureCode {
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

impl TransactionFailureCode {
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
pub struct TransactionError {
    pub code: TransactionFailureCode,
    pub resource: Option<TransactionResourceRequirement>,
    pub journal_code: Option<JournalFailureCode>,
    pub detail: String,
}

impl TransactionError {
    fn new(
        code: TransactionFailureCode,
        resource: Option<TransactionResourceRequirement>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code,
            resource,
            journal_code: None,
            detail: detail.into(),
        }
    }

    pub const fn failure_code(&self) -> TransactionFailureCode {
        self.code
    }
}

impl fmt::Display for TransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.detail)
    }
}
impl std::error::Error for TransactionError {}

fn validate_program_resource_mapping(
    program: &EffectProgram,
    resources: &[TransactionResourceRequirement],
) -> Result<(), TransactionError> {
    if let Some(resource) = resources.iter().find(|resource| {
        resource.kind == TransactionResourceKind::ProjectFilesystem
            && resource.baseline_state_digest != program.base_state()
    }) {
        return Err(TransactionError::new(
            TransactionFailureCode::BaselineMismatch,
            Some(*resource),
            "project-filesystem resource baseline differs from the effect base state",
        ));
    }
    let covers = |owner: ArtifactOwner, writes: bool, project_filesystem: bool| {
        resources.iter().any(|resource| {
            resource.owner == owner
                && (!project_filesystem
                    || resource.kind == TransactionResourceKind::ProjectFilesystem)
                && (!writes || resource.access.writes())
        })
    };
    for target in program.targets() {
        if !covers(target.owner, true, true) {
            return Err(TransactionError::new(
                TransactionFailureCode::MissingOperationResource,
                None,
                "effect target has no matching project-filesystem write resource",
            ));
        }
    }
    for operation in program.operations() {
        let owner_and_class = match operation {
            TypedEffectOperation::RecoverExact { owner, .. } => {
                Some((*owner, EffectClass::ReadOnly))
            }
            TypedEffectOperation::DeterministicTransform {
                owner,
                effect_class,
                ..
            }
            | TypedEffectOperation::InvokeCapability {
                owner,
                effect_class,
                ..
            } => Some((*owner, *effect_class)),
            TypedEffectOperation::ReplaceExactFile { .. }
            | TypedEffectOperation::CopyExact { .. }
            | TypedEffectOperation::ReturnLiteral { .. }
            | TypedEffectOperation::RawFallback => None,
        };
        if let Some((owner, effect_class)) = owner_and_class {
            let writes = effect_class != EffectClass::ReadOnly;
            if !covers(owner, writes, false) {
                return Err(TransactionError::new(
                    TransactionFailureCode::MissingOperationResource,
                    None,
                    "effect capability has no matching owned resource scope",
                ));
            }
        }
    }
    Ok(())
}

fn sort_and_validate_requirements(
    resources: &mut [TransactionResourceRequirement],
) -> Result<(), TransactionError> {
    resources.sort_by_key(|resource| resource.key());
    validate_sorted_requirements(resources)
}

fn validate_sorted_requirements(
    resources: &[TransactionResourceRequirement],
) -> Result<(), TransactionError> {
    if resources.is_empty() {
        return Err(TransactionError::new(
            TransactionFailureCode::EmptyResourceInventory,
            None,
            "effect-closure inventory must be explicit and nonempty",
        ));
    }
    if resources.len() > TRANSACTION_MAX_RESOURCES {
        return Err(TransactionError::new(
            TransactionFailureCode::TooManyResources,
            None,
            format!("effect-closure inventory has {} resources", resources.len()),
        ));
    }
    let mut previous = None;
    for resource in resources {
        resource.validate()?;
        let key = resource.key();
        if previous.is_some_and(|prior| prior >= key) {
            return Err(TransactionError::new(
                TransactionFailureCode::NonCanonicalResourceInventory,
                Some(*resource),
                "resource requirements must have unique kind/scope keys",
            ));
        }
        previous = Some(key);
    }
    Ok(())
}

fn canonical_serialize<T: Serialize>(value: &T) -> Result<Vec<u8>, TransactionError> {
    let value = serde_json::to_value(value).map_err(serialization_error)?;
    let bytes = canonical_json(&value).into_bytes();
    if bytes.len() > TRANSACTION_MAX_CANONICAL_BYTES {
        return Err(TransactionError::new(
            TransactionFailureCode::CanonicalPayloadTooLarge,
            None,
            format!(
                "transaction contract value has {} canonical bytes",
                bytes.len()
            ),
        ));
    }
    Ok(bytes)
}

fn serialization_error(error: serde_json::Error) -> TransactionError {
    TransactionError::new(
        TransactionFailureCode::SerializationFailure,
        None,
        error.to_string(),
    )
}

fn journal_error(error: JournalError) -> TransactionError {
    TransactionError {
        code: TransactionFailureCode::JournalFailure,
        resource: None,
        journal_code: Some(error.code),
        detail: error.to_string(),
    }
}

fn domain_digest(domain: &[u8], bytes: &[u8]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
    Sha256Digest::from_bytes(hasher.finalize().into())
}

pub fn transaction_contract() -> Value {
    json!({
        "canonical_encoding": "sorted_key_json",
        "contract_version": TRANSACTION_CONTRACT_VERSION,
        "domains": {
            "contract": String::from_utf8_lossy(CONTRACT_DOMAIN),
            "external_inventory": String::from_utf8_lossy(EXTERNAL_INVENTORY_DOMAIN),
            "external_restoration_debt": String::from_utf8_lossy(EXTERNAL_DEBT_DOMAIN),
            "journal_receipt": String::from_utf8_lossy(JOURNAL_RECEIPT_DOMAIN),
            "manifest": String::from_utf8_lossy(MANIFEST_DOMAIN),
            "request": String::from_utf8_lossy(REQUEST_DOMAIN),
            "transaction_id": String::from_utf8_lossy(TRANSACTION_ID_DOMAIN),
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
            "canonical_bytes": TRANSACTION_MAX_CANONICAL_BYTES,
            "resources": TRANSACTION_MAX_RESOURCES,
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

pub fn transaction_contract_digest() -> Sha256Digest {
    domain_digest(
        CONTRACT_DOMAIN,
        canonical_json(&transaction_contract()).as_bytes(),
    )
}

