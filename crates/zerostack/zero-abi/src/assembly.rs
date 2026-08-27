//! Canonical ZeroStack assembly identity and pre-dispatch validation.
//!
//! An assembly manifest binds the exact host, workers, profiles, target,
//! verifiers, receipt schema, runtime generation, epoch, and capability catalogs
//! that were assembled together. Canonical bytes are sorted-key JSON with
//! canonically ordered vectors. The digest is domain separated from other JSON
//! contracts.

use std::{error::Error, fmt};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use serde_json::{Value, json};

use crate::{
    canonical_json,
    cwir::{CWIR_CONTRACT_VERSION, cwir_contract_digest},
    effect::{EFFECT_IR_CONTRACT_VERSION, effect_ir_contract_digest},
    raw_worker::EngineIdentity,
    reasoning::{REASONING_CONTRACT_VERSION, reasoning_contract_digest},
    sha256,
    zbf::{ZBF_CONTRACT_VERSION, zbf_contract_digest},
};

pub const ASSEMBLY_MANIFEST_SCHEMA_VERSION: u16 = 1;
pub const ASSEMBLY_ABI_CONTRACT_VERSION: u16 = 1;
pub const ASSEMBLY_MANIFEST_DOMAIN: &[u8] = b"zerostack.assembly_manifest\0";
pub const MAX_ASSEMBLY_MANIFEST_BYTES: usize = 1_048_576;
pub const MAX_ASSEMBLY_ITEMS: usize = 64;
pub const MAX_ASSEMBLY_STRING_BYTES: usize = 512;

/// A fixed SHA-256 identity. Wire form is exactly 64 lowercase hexadecimal bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub const ZERO: Self = Self([0; 32]);

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_hex(value: &str) -> Result<Self, DigestParseError> {
        if value.len() != 64 {
            return Err(DigestParseError::WrongLength(value.len()));
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = lower_hex_nibble(pair[0]).ok_or(DigestParseError::InvalidHex {
                index: index * 2,
                byte: pair[0],
            })?;
            let low = lower_hex_nibble(pair[1]).ok_or(DigestParseError::InvalidHex {
                index: index * 2 + 1,
                byte: pair[1],
            })?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }

    pub fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for byte in self.0 {
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        out
    }
}

fn lower_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for Sha256Digest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DigestParseError {
    WrongLength(usize),
    InvalidHex { index: usize, byte: u8 },
}

impl fmt::Display for DigestParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongLength(actual) => write!(
                f,
                "digest must be 64 lowercase hexadecimal bytes, got {actual}"
            ),
            Self::InvalidHex { index, byte } => write!(
                f,
                "digest contains non-lowercase-hex byte 0x{byte:02x} at index {index}"
            ),
        }
    }
}

impl Error for DigestParseError {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOwner {
    ZeroStack,
    FsZero,
    GraphZero,
    TokenZero,
    PiZeroStack,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKind {
    Platform,
    Runtime,
    Storage,
    Verification,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedArtifact {
    pub artifact_id: String,
    pub owner: ArtifactOwner,
    pub artifact_version: String,
    pub source_repository: String,
    pub source_revision: String,
    pub artifact_digest: Sha256Digest,
    pub contract_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedProfile {
    pub profile_kind: ProfileKind,
    pub profile_id: String,
    pub profile_version: String,
    pub profile_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetIdentity {
    pub target_triple: String,
    pub architecture: String,
    pub operating_system: String,
    pub abi: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformIdentity {
    pub profile_id: String,
    pub profile_version: String,
    pub profile_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierIdentity {
    pub verifier_id: String,
    pub verifier_version: String,
    pub verifier_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSchemaIdentity {
    pub schema_id: String,
    pub schema_version: String,
    pub schema_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentity {
    pub engine: EngineIdentity,
    pub artifact_digest: Sha256Digest,
    pub worker_protocol_digest: Sha256Digest,
    pub semantic_contract_digest: Sha256Digest,
    pub operation_registry_digest: Sha256Digest,
    pub capability_catalog_digest: Sha256Digest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssemblyManifest {
    pub schema_version: u16,
    pub required_abi_contract_version: u16,
    pub abi_contract_digest: Sha256Digest,
    pub linked_artifacts: Vec<LinkedArtifact>,
    pub linked_profiles: Vec<LinkedProfile>,
    pub target: TargetIdentity,
    pub platform: PlatformIdentity,
    pub verifiers: Vec<VerifierIdentity>,
    pub receipt_schema: ReceiptSchemaIdentity,
    pub runtime_generation: u64,
    pub assembly_epoch: u64,
    pub workers: Vec<WorkerIdentity>,
    pub aggregate_capability_catalog_digest: Sha256Digest,
}

impl AssemblyManifest {
    pub fn validate(&self) -> Result<(), AssemblyManifestError> {
        require_version(
            "schema_version",
            ASSEMBLY_MANIFEST_SCHEMA_VERSION,
            self.schema_version,
        )?;
        require_version(
            "required_abi_contract_version",
            ASSEMBLY_ABI_CONTRACT_VERSION,
            self.required_abi_contract_version,
        )?;
        let expected_contract = assembly_abi_contract_digest();
        if self.abi_contract_digest != expected_contract {
            return Err(AssemblyManifestError::ContractDigestMismatch {
                expected: expected_contract,
                actual: self.abi_contract_digest,
            });
        }

        require_items("linked_artifacts", &self.linked_artifacts)?;
        require_items("linked_profiles", &self.linked_profiles)?;
        require_items("verifiers", &self.verifiers)?;
        require_items("workers", &self.workers)?;
        require_canonical_order(
            "linked_artifacts",
            &self
                .linked_artifacts
                .iter()
                .map(|item| item.artifact_id.as_str())
                .collect::<Vec<_>>(),
        )?;
        let profile_keys = self
            .linked_profiles
            .iter()
            .map(|item| format!("{:?}:{}", item.profile_kind, item.profile_id))
            .collect::<Vec<_>>();
        require_canonical_order(
            "linked_profiles",
            &profile_keys.iter().map(String::as_str).collect::<Vec<_>>(),
        )?;
        require_canonical_order(
            "verifiers",
            &self
                .verifiers
                .iter()
                .map(|item| item.verifier_id.as_str())
                .collect::<Vec<_>>(),
        )?;

        for artifact in &self.linked_artifacts {
            for (field, value) in [
                ("artifact_id", artifact.artifact_id.as_str()),
                ("artifact_version", artifact.artifact_version.as_str()),
                ("source_repository", artifact.source_repository.as_str()),
                ("source_revision", artifact.source_revision.as_str()),
            ] {
                require_string(field, value)?;
            }
        }
        for profile in &self.linked_profiles {
            require_string("profile_id", &profile.profile_id)?;
            require_string("profile_version", &profile.profile_version)?;
        }
        for (field, value) in [
            ("target.target_triple", self.target.target_triple.as_str()),
            ("target.architecture", self.target.architecture.as_str()),
            (
                "target.operating_system",
                self.target.operating_system.as_str(),
            ),
            ("target.abi", self.target.abi.as_str()),
            ("platform.profile_id", self.platform.profile_id.as_str()),
            (
                "platform.profile_version",
                self.platform.profile_version.as_str(),
            ),
            (
                "receipt_schema.schema_id",
                self.receipt_schema.schema_id.as_str(),
            ),
            (
                "receipt_schema.schema_version",
                self.receipt_schema.schema_version.as_str(),
            ),
        ] {
            require_string(field, value)?;
        }
        for verifier in &self.verifiers {
            require_string("verifier_id", &verifier.verifier_id)?;
            require_string("verifier_version", &verifier.verifier_version)?;
        }
        if self.runtime_generation == 0 {
            return Err(AssemblyManifestError::ZeroCounter("runtime_generation"));
        }
        if self.assembly_epoch == 0 {
            return Err(AssemblyManifestError::ZeroCounter("assembly_epoch"));
        }

        let platform_linked = self.linked_profiles.iter().any(|profile| {
            profile.profile_kind == ProfileKind::Platform
                && profile.profile_id == self.platform.profile_id
                && profile.profile_version == self.platform.profile_version
                && profile.profile_digest == self.platform.profile_digest
        });
        if !platform_linked {
            return Err(AssemblyManifestError::UnlinkedIdentity("platform"));
        }

        let expected_engines = [
            EngineIdentity::FsZero,
            EngineIdentity::GraphZero,
            EngineIdentity::TokenZero,
        ];
        let actual_engines = self
            .workers
            .iter()
            .map(|worker| worker.engine)
            .collect::<Vec<_>>();
        if actual_engines != expected_engines {
            return Err(AssemblyManifestError::WorkerSetMismatch);
        }
        for worker in &self.workers {
            if !self
                .linked_artifacts
                .iter()
                .any(|artifact| artifact.artifact_digest == worker.artifact_digest)
            {
                return Err(AssemblyManifestError::UnlinkedWorkerArtifact(worker.engine));
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AssemblyManifestError> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|error| AssemblyManifestError::Json(error.to_string()))?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn digest(&self) -> Result<Sha256Digest, AssemblyManifestError> {
        let canonical = self.canonical_bytes()?;
        let mut domain_bound = Vec::with_capacity(ASSEMBLY_MANIFEST_DOMAIN.len() + canonical.len());
        domain_bound.extend_from_slice(ASSEMBLY_MANIFEST_DOMAIN);
        domain_bound.extend_from_slice(&canonical);
        Ok(Sha256Digest::from_bytes(sha256(&domain_bound)))
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssemblyManifestError> {
        if bytes.len() > MAX_ASSEMBLY_MANIFEST_BYTES {
            return Err(AssemblyManifestError::ManifestTooLarge {
                actual: bytes.len(),
                maximum: MAX_ASSEMBLY_MANIFEST_BYTES,
            });
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| AssemblyManifestError::Json(error.to_string()))?;
        manifest.validate()?;
        let canonical = manifest.canonical_bytes()?;
        if canonical != bytes {
            return Err(AssemblyManifestError::NonCanonicalEncoding);
        }
        Ok(manifest)
    }

    pub fn expectation(&self) -> Result<AssemblyExpectation, AssemblyManifestError> {
        Ok(AssemblyExpectation {
            required_schema_version: ASSEMBLY_MANIFEST_SCHEMA_VERSION,
            manifest_digest: self.digest()?,
            abi_contract_digest: self.abi_contract_digest,
            linked_artifacts: self.linked_artifacts.clone(),
            linked_profiles: self.linked_profiles.clone(),
            target: self.target.clone(),
            platform: self.platform.clone(),
            verifiers: self.verifiers.clone(),
            receipt_schema: self.receipt_schema.clone(),
            runtime_generation: self.runtime_generation,
            assembly_epoch: self.assembly_epoch,
            workers: self.workers.clone(),
            aggregate_capability_catalog_digest: self.aggregate_capability_catalog_digest,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssemblyExpectation {
    pub required_schema_version: u16,
    pub manifest_digest: Sha256Digest,
    pub abi_contract_digest: Sha256Digest,
    pub linked_artifacts: Vec<LinkedArtifact>,
    pub linked_profiles: Vec<LinkedProfile>,
    pub target: TargetIdentity,
    pub platform: PlatformIdentity,
    pub verifiers: Vec<VerifierIdentity>,
    pub receipt_schema: ReceiptSchemaIdentity,
    pub runtime_generation: u64,
    pub assembly_epoch: u64,
    pub workers: Vec<WorkerIdentity>,
    pub aggregate_capability_catalog_digest: Sha256Digest,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssemblyFailureCode {
    InvalidManifest,
    UnsupportedRequiredVersion,
    AbiContractDigestMismatch,
    LinkedArtifactMismatch,
    LinkedProfileMismatch,
    TargetMismatch,
    PlatformMismatch,
    VerifierMismatch,
    ReceiptSchemaMismatch,
    RuntimeGenerationMismatch,
    AssemblyEpochMismatch,
    WorkerSetMismatch,
    WorkerDigestMismatch,
    WorkerIdentityMismatch,
    CapabilityCatalogDigestMismatch,
    ManifestDigestMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssemblyPreDispatchError {
    InvalidManifest(AssemblyManifestError),
    UnsupportedRequiredVersion { supported: u16, actual: u16 },
    AbiContractDigestMismatch,
    LinkedArtifactMismatch,
    LinkedProfileMismatch,
    TargetMismatch,
    PlatformMismatch,
    VerifierMismatch,
    ReceiptSchemaMismatch,
    RuntimeGenerationMismatch,
    AssemblyEpochMismatch,
    WorkerSetMismatch,
    WorkerDigestMismatch { engine: EngineIdentity },
    WorkerIdentityMismatch { engine: EngineIdentity },
    CapabilityCatalogDigestMismatch { scope: String },
    ManifestDigestMismatch,
}

impl AssemblyPreDispatchError {
    pub const fn code(&self) -> AssemblyFailureCode {
        match self {
            Self::InvalidManifest(_) => AssemblyFailureCode::InvalidManifest,
            Self::UnsupportedRequiredVersion { .. } => {
                AssemblyFailureCode::UnsupportedRequiredVersion
            }
            Self::AbiContractDigestMismatch => AssemblyFailureCode::AbiContractDigestMismatch,
            Self::LinkedArtifactMismatch => AssemblyFailureCode::LinkedArtifactMismatch,
            Self::LinkedProfileMismatch => AssemblyFailureCode::LinkedProfileMismatch,
            Self::TargetMismatch => AssemblyFailureCode::TargetMismatch,
            Self::PlatformMismatch => AssemblyFailureCode::PlatformMismatch,
            Self::VerifierMismatch => AssemblyFailureCode::VerifierMismatch,
            Self::ReceiptSchemaMismatch => AssemblyFailureCode::ReceiptSchemaMismatch,
            Self::RuntimeGenerationMismatch => AssemblyFailureCode::RuntimeGenerationMismatch,
            Self::AssemblyEpochMismatch => AssemblyFailureCode::AssemblyEpochMismatch,
            Self::WorkerSetMismatch => AssemblyFailureCode::WorkerSetMismatch,
            Self::WorkerDigestMismatch { .. } => AssemblyFailureCode::WorkerDigestMismatch,
            Self::WorkerIdentityMismatch { .. } => AssemblyFailureCode::WorkerIdentityMismatch,
            Self::CapabilityCatalogDigestMismatch { .. } => {
                AssemblyFailureCode::CapabilityCatalogDigestMismatch
            }
            Self::ManifestDigestMismatch => AssemblyFailureCode::ManifestDigestMismatch,
        }
    }
}

impl fmt::Display for AssemblyPreDispatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "assembly pre-dispatch validation failed: {:?}",
            self.code()
        )
    }
}

impl Error for AssemblyPreDispatchError {}

/// Validate every assembly-bound identity before an operation reaches a worker.
pub fn validate_assembly_pre_dispatch(
    manifest: &AssemblyManifest,
    expected: &AssemblyExpectation,
) -> Result<(), AssemblyPreDispatchError> {
    manifest
        .validate()
        .map_err(AssemblyPreDispatchError::InvalidManifest)?;
    if expected.required_schema_version != ASSEMBLY_MANIFEST_SCHEMA_VERSION {
        return Err(AssemblyPreDispatchError::UnsupportedRequiredVersion {
            supported: ASSEMBLY_MANIFEST_SCHEMA_VERSION,
            actual: expected.required_schema_version,
        });
    }
    if manifest.abi_contract_digest != expected.abi_contract_digest {
        return Err(AssemblyPreDispatchError::AbiContractDigestMismatch);
    }
    if manifest.linked_artifacts != expected.linked_artifacts {
        return Err(AssemblyPreDispatchError::LinkedArtifactMismatch);
    }
    if manifest.linked_profiles != expected.linked_profiles {
        return Err(AssemblyPreDispatchError::LinkedProfileMismatch);
    }
    if manifest.target != expected.target {
        return Err(AssemblyPreDispatchError::TargetMismatch);
    }
    if manifest.platform != expected.platform {
        return Err(AssemblyPreDispatchError::PlatformMismatch);
    }
    if manifest.verifiers != expected.verifiers {
        return Err(AssemblyPreDispatchError::VerifierMismatch);
    }
    if manifest.receipt_schema != expected.receipt_schema {
        return Err(AssemblyPreDispatchError::ReceiptSchemaMismatch);
    }
    if manifest.runtime_generation != expected.runtime_generation {
        return Err(AssemblyPreDispatchError::RuntimeGenerationMismatch);
    }
    if manifest.assembly_epoch != expected.assembly_epoch {
        return Err(AssemblyPreDispatchError::AssemblyEpochMismatch);
    }
    if manifest.workers.len() != expected.workers.len()
        || manifest
            .workers
            .iter()
            .zip(&expected.workers)
            .any(|(actual, wanted)| actual.engine != wanted.engine)
    {
        return Err(AssemblyPreDispatchError::WorkerSetMismatch);
    }
    for (actual, wanted) in manifest.workers.iter().zip(&expected.workers) {
        if actual.artifact_digest != wanted.artifact_digest {
            return Err(AssemblyPreDispatchError::WorkerDigestMismatch {
                engine: actual.engine,
            });
        }
        if actual.capability_catalog_digest != wanted.capability_catalog_digest {
            return Err(AssemblyPreDispatchError::CapabilityCatalogDigestMismatch {
                scope: actual.engine.as_str().to_owned(),
            });
        }
        if actual != wanted {
            return Err(AssemblyPreDispatchError::WorkerIdentityMismatch {
                engine: actual.engine,
            });
        }
    }
    if manifest.aggregate_capability_catalog_digest != expected.aggregate_capability_catalog_digest
    {
        return Err(AssemblyPreDispatchError::CapabilityCatalogDigestMismatch {
            scope: "aggregate".to_owned(),
        });
    }
    let actual_digest = manifest
        .digest()
        .map_err(AssemblyPreDispatchError::InvalidManifest)?;
    if actual_digest != expected.manifest_digest {
        return Err(AssemblyPreDispatchError::ManifestDigestMismatch);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssemblyManifestError {
    UnsupportedVersion {
        field: &'static str,
        supported: u16,
        actual: u16,
    },
    ContractDigestMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    EmptyCollection(&'static str),
    TooManyItems {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    EmptyString(&'static str),
    StringTooLong {
        field: &'static str,
        actual: usize,
        maximum: usize,
    },
    ControlCharacter(&'static str),
    NonCanonicalOrder(&'static str),
    DuplicateIdentity {
        field: &'static str,
        identity: String,
    },
    ZeroCounter(&'static str),
    UnlinkedIdentity(&'static str),
    UnlinkedWorkerArtifact(EngineIdentity),
    WorkerSetMismatch,
    ManifestTooLarge {
        actual: usize,
        maximum: usize,
    },
    NonCanonicalEncoding,
    Json(String),
}

impl fmt::Display for AssemblyManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion {
                field,
                supported,
                actual,
            } => write!(
                f,
                "unsupported required version for {field}: supported={supported}, actual={actual}"
            ),
            Self::ContractDigestMismatch { expected, actual } => write!(
                f,
                "assembly ABI contract digest mismatch: expected={expected}, actual={actual}"
            ),
            Self::EmptyCollection(field) => write!(f, "{field} must not be empty"),
            Self::TooManyItems {
                field,
                actual,
                maximum,
            } => write!(f, "{field} has {actual} items; maximum is {maximum}"),
            Self::EmptyString(field) => write!(f, "{field} must not be empty"),
            Self::StringTooLong {
                field,
                actual,
                maximum,
            } => write!(f, "{field} has {actual} bytes; maximum is {maximum}"),
            Self::ControlCharacter(field) => {
                write!(f, "{field} must not contain control characters")
            }
            Self::NonCanonicalOrder(field) => {
                write!(f, "{field} is not in canonical ascending order")
            }
            Self::DuplicateIdentity { field, identity } => {
                write!(f, "{field} contains duplicate identity {identity}")
            }
            Self::ZeroCounter(field) => write!(f, "{field} must be nonzero"),
            Self::UnlinkedIdentity(field) => write!(f, "{field} is not linked by the manifest"),
            Self::UnlinkedWorkerArtifact(engine) => write!(
                f,
                "worker artifact for {} is not present in linked_artifacts",
                engine.as_str()
            ),
            Self::WorkerSetMismatch => {
                f.write_str("workers must contain fszero, graphzero, tokenzero in canonical order")
            }
            Self::ManifestTooLarge { actual, maximum } => {
                write!(f, "manifest has {actual} bytes; maximum is {maximum}")
            }
            Self::NonCanonicalEncoding => f.write_str("manifest bytes are not canonical"),
            Self::Json(error) => write!(f, "manifest JSON is invalid: {error}"),
        }
    }
}

impl Error for AssemblyManifestError {}

fn require_version(
    field: &'static str,
    supported: u16,
    actual: u16,
) -> Result<(), AssemblyManifestError> {
    if actual == supported {
        Ok(())
    } else {
        Err(AssemblyManifestError::UnsupportedVersion {
            field,
            supported,
            actual,
        })
    }
}

fn require_items<T>(field: &'static str, values: &[T]) -> Result<(), AssemblyManifestError> {
    if values.is_empty() {
        return Err(AssemblyManifestError::EmptyCollection(field));
    }
    if values.len() > MAX_ASSEMBLY_ITEMS {
        return Err(AssemblyManifestError::TooManyItems {
            field,
            actual: values.len(),
            maximum: MAX_ASSEMBLY_ITEMS,
        });
    }
    Ok(())
}

fn require_string(field: &'static str, value: &str) -> Result<(), AssemblyManifestError> {
    if value.is_empty() {
        return Err(AssemblyManifestError::EmptyString(field));
    }
    if value.len() > MAX_ASSEMBLY_STRING_BYTES {
        return Err(AssemblyManifestError::StringTooLong {
            field,
            actual: value.len(),
            maximum: MAX_ASSEMBLY_STRING_BYTES,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(AssemblyManifestError::ControlCharacter(field));
    }
    Ok(())
}

fn require_canonical_order(
    field: &'static str,
    identities: &[&str],
) -> Result<(), AssemblyManifestError> {
    for pair in identities.windows(2) {
        match pair[0].cmp(pair[1]) {
            std::cmp::Ordering::Greater => {
                return Err(AssemblyManifestError::NonCanonicalOrder(field));
            }
            std::cmp::Ordering::Equal => {
                return Err(AssemblyManifestError::DuplicateIdentity {
                    field,
                    identity: pair[0].to_owned(),
                });
            }
            std::cmp::Ordering::Less => {}
        }
    }
    Ok(())
}

/// Semantic schema hashed into every AssemblyManifest.
pub fn assembly_abi_contract_manifest() -> Value {
    json!({
        "contract": "zerostack.assembly_abi",
        "contract_version": ASSEMBLY_ABI_CONTRACT_VERSION,
        "manifest_schema_version": ASSEMBLY_MANIFEST_SCHEMA_VERSION,
        "digest": {"algorithm": "sha256", "domain": "zerostack.assembly_manifest\u{0}"},
        "encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "linked_contracts": {
            "cwir_contract_version": CWIR_CONTRACT_VERSION,
            "cwir_contract_digest": cwir_contract_digest(),
            "effect_ir_contract_version": EFFECT_IR_CONTRACT_VERSION,
            "effect_ir_contract_digest": effect_ir_contract_digest(),
            "reasoning_contract_version": REASONING_CONTRACT_VERSION,
            "reasoning_contract_digest": reasoning_contract_digest(),
            "zbf_contract_version": ZBF_CONTRACT_VERSION,
            "zbf_contract_digest": zbf_contract_digest()
        },
        "bounds": {
            "max_manifest_bytes": MAX_ASSEMBLY_MANIFEST_BYTES,
            "max_items_per_vector": MAX_ASSEMBLY_ITEMS,
            "max_string_bytes": MAX_ASSEMBLY_STRING_BYTES
        },
        "manifest_fields": [
            "schema_version", "required_abi_contract_version", "abi_contract_digest",
            "linked_artifacts", "linked_profiles", "target", "platform", "verifiers",
            "receipt_schema", "runtime_generation", "assembly_epoch", "workers",
            "aggregate_capability_catalog_digest"
        ],
        "linked_artifact_fields": [
            "artifact_id", "owner", "artifact_version", "source_repository",
            "source_revision", "artifact_digest", "contract_digest"
        ],
        "linked_profile_fields": [
            "profile_kind", "profile_id", "profile_version", "profile_digest"
        ],
        "target_fields": ["target_triple", "architecture", "operating_system", "abi"],
        "platform_fields": ["profile_id", "profile_version", "profile_digest"],
        "verifier_fields": ["verifier_id", "verifier_version", "verifier_digest"],
        "receipt_schema_fields": ["schema_id", "schema_version", "schema_digest"],
        "worker_fields": [
            "engine", "artifact_digest", "worker_protocol_digest",
            "semantic_contract_digest", "operation_registry_digest",
            "capability_catalog_digest"
        ],
        "canonical_vector_keys": ["artifact_id", "profile_kind+profile_id", "verifier_id", "engine"],
        "required_engines": ["fszero", "graphzero", "tokenzero"],
        "unknown_fields": "reject",
        "unknown_required_versions": "reject_before_dispatch",
        "predispatch_failure_codes": [
            "invalid_manifest", "unsupported_required_version",
            "abi_contract_digest_mismatch", "linked_artifact_mismatch",
            "linked_profile_mismatch", "target_mismatch", "platform_mismatch",
            "verifier_mismatch", "receipt_schema_mismatch",
            "runtime_generation_mismatch", "assembly_epoch_mismatch",
            "worker_set_mismatch", "worker_digest_mismatch", "worker_identity_mismatch",
            "capability_catalog_digest_mismatch", "manifest_digest_mismatch"
        ]
    })
}

pub fn assembly_abi_contract_digest() -> Sha256Digest {
    let canonical = canonical_json(&assembly_abi_contract_manifest());
    Sha256Digest::from_bytes(sha256(canonical.as_bytes()))
}
