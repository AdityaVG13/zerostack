//! Canonical ZeroStack assembly identity and pre-dispatch validation.
//!
//! An assembly manifest binds the exact host, workers, profiles, target,
//! verifiers, receipt schema, runtime generation, epoch, and capability catalogs
//! that were assembled together. Canonical bytes are sorted-key JSON with
//! canonically ordered vectors. The digest is domain separated from other JSON
//! contracts.

use std::{error::Error, fmt};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};

use crate::{
    canonical_json,
    cwir::{cwir_contract_digest_v1, CWIR_CONTRACT_VERSION_V1},
    effect::{effect_ir_contract_digest_v1, EFFECT_IR_CONTRACT_VERSION_V1},
    raw_worker::EngineIdentity,
    reasoning::{reasoning_contract_digest_v1, REASONING_CONTRACT_VERSION_V1},
    sha256,
    zbf::{zbf_contract_digest_v1, ZBF_CONTRACT_VERSION_V1},
};

pub const ASSEMBLY_MANIFEST_SCHEMA_VERSION: u16 = 1;
pub const ASSEMBLY_ABI_CONTRACT_VERSION: u16 = 1;
pub const ASSEMBLY_MANIFEST_DOMAIN_V1: &[u8] = b"zerostack.assembly_manifest.v1\0";
pub const MAX_ASSEMBLY_MANIFEST_BYTES: usize = 1_048_576;
pub const MAX_ASSEMBLY_ITEMS: usize = 64;
pub const MAX_ASSEMBLY_STRING_BYTES: usize = 512;

/// A fixed SHA-256 identity. Wire form is exactly 64 lowercase hexadecimal bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DigestV1([u8; 32]);

impl DigestV1 {
    pub const ZERO: Self = Self([0; 32]);

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn from_hex(value: &str) -> Result<Self, DigestParseErrorV1> {
        if value.len() != 64 {
            return Err(DigestParseErrorV1::WrongLength(value.len()));
        }
        let mut bytes = [0_u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = lower_hex_nibble(pair[0]).ok_or(DigestParseErrorV1::InvalidHex {
                index: index * 2,
                byte: pair[0],
            })?;
            let low = lower_hex_nibble(pair[1]).ok_or(DigestParseErrorV1::InvalidHex {
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

impl fmt::Display for DigestV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for DigestV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for DigestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_hex(&value).map_err(de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DigestParseErrorV1 {
    WrongLength(usize),
    InvalidHex { index: usize, byte: u8 },
}

impl fmt::Display for DigestParseErrorV1 {
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

impl Error for DigestParseErrorV1 {}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOwnerV1 {
    ZeroStack,
    FsZero,
    GraphZero,
    TokenZero,
    PiZeroStack,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileKindV1 {
    Platform,
    Runtime,
    Storage,
    Verification,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedArtifactV1 {
    pub artifact_id: String,
    pub owner: ArtifactOwnerV1,
    pub artifact_version: String,
    pub source_repository: String,
    pub source_revision: String,
    pub artifact_digest: DigestV1,
    pub contract_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedProfileV1 {
    pub profile_kind: ProfileKindV1,
    pub profile_id: String,
    pub profile_version: String,
    pub profile_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetIdentityV1 {
    pub target_triple: String,
    pub architecture: String,
    pub operating_system: String,
    pub abi: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformIdentityV1 {
    pub profile_id: String,
    pub profile_version: String,
    pub profile_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerifierIdentityV1 {
    pub verifier_id: String,
    pub verifier_version: String,
    pub verifier_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSchemaIdentityV1 {
    pub schema_id: String,
    pub schema_version: String,
    pub schema_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerIdentityV1 {
    pub engine: EngineIdentity,
    pub artifact_digest: DigestV1,
    pub worker_protocol_digest: DigestV1,
    pub semantic_contract_digest: DigestV1,
    pub operation_registry_digest: DigestV1,
    pub capability_catalog_digest: DigestV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssemblyManifestV1 {
    pub schema_version: u16,
    pub required_abi_contract_version: u16,
    pub abi_contract_digest: DigestV1,
    pub linked_artifacts: Vec<LinkedArtifactV1>,
    pub linked_profiles: Vec<LinkedProfileV1>,
    pub target: TargetIdentityV1,
    pub platform: PlatformIdentityV1,
    pub verifiers: Vec<VerifierIdentityV1>,
    pub receipt_schema: ReceiptSchemaIdentityV1,
    pub runtime_generation: u64,
    pub assembly_epoch: u64,
    pub workers: Vec<WorkerIdentityV1>,
    pub aggregate_capability_catalog_digest: DigestV1,
}

impl AssemblyManifestV1 {
    pub fn validate(&self) -> Result<(), AssemblyManifestErrorV1> {
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
        let expected_contract = assembly_abi_contract_digest_v1();
        if self.abi_contract_digest != expected_contract {
            return Err(AssemblyManifestErrorV1::ContractDigestMismatch {
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
            return Err(AssemblyManifestErrorV1::ZeroCounter("runtime_generation"));
        }
        if self.assembly_epoch == 0 {
            return Err(AssemblyManifestErrorV1::ZeroCounter("assembly_epoch"));
        }

        let platform_linked = self.linked_profiles.iter().any(|profile| {
            profile.profile_kind == ProfileKindV1::Platform
                && profile.profile_id == self.platform.profile_id
                && profile.profile_version == self.platform.profile_version
                && profile.profile_digest == self.platform.profile_digest
        });
        if !platform_linked {
            return Err(AssemblyManifestErrorV1::UnlinkedIdentity("platform"));
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
            return Err(AssemblyManifestErrorV1::WorkerSetMismatch);
        }
        for worker in &self.workers {
            if !self
                .linked_artifacts
                .iter()
                .any(|artifact| artifact.artifact_digest == worker.artifact_digest)
            {
                return Err(AssemblyManifestErrorV1::UnlinkedWorkerArtifact(
                    worker.engine,
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AssemblyManifestErrorV1> {
        self.validate()?;
        let value = serde_json::to_value(self)
            .map_err(|error| AssemblyManifestErrorV1::Json(error.to_string()))?;
        Ok(canonical_json(&value).into_bytes())
    }

    pub fn digest(&self) -> Result<DigestV1, AssemblyManifestErrorV1> {
        let canonical = self.canonical_bytes()?;
        let mut domain_bound =
            Vec::with_capacity(ASSEMBLY_MANIFEST_DOMAIN_V1.len() + canonical.len());
        domain_bound.extend_from_slice(ASSEMBLY_MANIFEST_DOMAIN_V1);
        domain_bound.extend_from_slice(&canonical);
        Ok(DigestV1::from_bytes(sha256(&domain_bound)))
    }

    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssemblyManifestErrorV1> {
        if bytes.len() > MAX_ASSEMBLY_MANIFEST_BYTES {
            return Err(AssemblyManifestErrorV1::ManifestTooLarge {
                actual: bytes.len(),
                maximum: MAX_ASSEMBLY_MANIFEST_BYTES,
            });
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| AssemblyManifestErrorV1::Json(error.to_string()))?;
        manifest.validate()?;
        let canonical = manifest.canonical_bytes()?;
        if canonical != bytes {
            return Err(AssemblyManifestErrorV1::NonCanonicalEncoding);
        }
        Ok(manifest)
    }

    pub fn expectation(&self) -> Result<AssemblyExpectationV1, AssemblyManifestErrorV1> {
        Ok(AssemblyExpectationV1 {
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
pub struct AssemblyExpectationV1 {
    pub required_schema_version: u16,
    pub manifest_digest: DigestV1,
    pub abi_contract_digest: DigestV1,
    pub linked_artifacts: Vec<LinkedArtifactV1>,
    pub linked_profiles: Vec<LinkedProfileV1>,
    pub target: TargetIdentityV1,
    pub platform: PlatformIdentityV1,
    pub verifiers: Vec<VerifierIdentityV1>,
    pub receipt_schema: ReceiptSchemaIdentityV1,
    pub runtime_generation: u64,
    pub assembly_epoch: u64,
    pub workers: Vec<WorkerIdentityV1>,
    pub aggregate_capability_catalog_digest: DigestV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssemblyFailureCodeV1 {
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
pub enum AssemblyPreDispatchErrorV1 {
    InvalidManifest(AssemblyManifestErrorV1),
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

impl AssemblyPreDispatchErrorV1 {
    pub const fn code(&self) -> AssemblyFailureCodeV1 {
        match self {
            Self::InvalidManifest(_) => AssemblyFailureCodeV1::InvalidManifest,
            Self::UnsupportedRequiredVersion { .. } => {
                AssemblyFailureCodeV1::UnsupportedRequiredVersion
            }
            Self::AbiContractDigestMismatch => AssemblyFailureCodeV1::AbiContractDigestMismatch,
            Self::LinkedArtifactMismatch => AssemblyFailureCodeV1::LinkedArtifactMismatch,
            Self::LinkedProfileMismatch => AssemblyFailureCodeV1::LinkedProfileMismatch,
            Self::TargetMismatch => AssemblyFailureCodeV1::TargetMismatch,
            Self::PlatformMismatch => AssemblyFailureCodeV1::PlatformMismatch,
            Self::VerifierMismatch => AssemblyFailureCodeV1::VerifierMismatch,
            Self::ReceiptSchemaMismatch => AssemblyFailureCodeV1::ReceiptSchemaMismatch,
            Self::RuntimeGenerationMismatch => AssemblyFailureCodeV1::RuntimeGenerationMismatch,
            Self::AssemblyEpochMismatch => AssemblyFailureCodeV1::AssemblyEpochMismatch,
            Self::WorkerSetMismatch => AssemblyFailureCodeV1::WorkerSetMismatch,
            Self::WorkerDigestMismatch { .. } => AssemblyFailureCodeV1::WorkerDigestMismatch,
            Self::WorkerIdentityMismatch { .. } => AssemblyFailureCodeV1::WorkerIdentityMismatch,
            Self::CapabilityCatalogDigestMismatch { .. } => {
                AssemblyFailureCodeV1::CapabilityCatalogDigestMismatch
            }
            Self::ManifestDigestMismatch => AssemblyFailureCodeV1::ManifestDigestMismatch,
        }
    }
}

impl fmt::Display for AssemblyPreDispatchErrorV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "assembly pre-dispatch validation failed: {:?}",
            self.code()
        )
    }
}

impl Error for AssemblyPreDispatchErrorV1 {}

/// Validate every assembly-bound identity before an operation reaches a worker.
pub fn validate_assembly_pre_dispatch_v1(
    manifest: &AssemblyManifestV1,
    expected: &AssemblyExpectationV1,
) -> Result<(), AssemblyPreDispatchErrorV1> {
    manifest
        .validate()
        .map_err(AssemblyPreDispatchErrorV1::InvalidManifest)?;
    if expected.required_schema_version != ASSEMBLY_MANIFEST_SCHEMA_VERSION {
        return Err(AssemblyPreDispatchErrorV1::UnsupportedRequiredVersion {
            supported: ASSEMBLY_MANIFEST_SCHEMA_VERSION,
            actual: expected.required_schema_version,
        });
    }
    if manifest.abi_contract_digest != expected.abi_contract_digest {
        return Err(AssemblyPreDispatchErrorV1::AbiContractDigestMismatch);
    }
    if manifest.linked_artifacts != expected.linked_artifacts {
        return Err(AssemblyPreDispatchErrorV1::LinkedArtifactMismatch);
    }
    if manifest.linked_profiles != expected.linked_profiles {
        return Err(AssemblyPreDispatchErrorV1::LinkedProfileMismatch);
    }
    if manifest.target != expected.target {
        return Err(AssemblyPreDispatchErrorV1::TargetMismatch);
    }
    if manifest.platform != expected.platform {
        return Err(AssemblyPreDispatchErrorV1::PlatformMismatch);
    }
    if manifest.verifiers != expected.verifiers {
        return Err(AssemblyPreDispatchErrorV1::VerifierMismatch);
    }
    if manifest.receipt_schema != expected.receipt_schema {
        return Err(AssemblyPreDispatchErrorV1::ReceiptSchemaMismatch);
    }
    if manifest.runtime_generation != expected.runtime_generation {
        return Err(AssemblyPreDispatchErrorV1::RuntimeGenerationMismatch);
    }
    if manifest.assembly_epoch != expected.assembly_epoch {
        return Err(AssemblyPreDispatchErrorV1::AssemblyEpochMismatch);
    }
    if manifest.workers.len() != expected.workers.len()
        || manifest
            .workers
            .iter()
            .zip(&expected.workers)
            .any(|(actual, wanted)| actual.engine != wanted.engine)
    {
        return Err(AssemblyPreDispatchErrorV1::WorkerSetMismatch);
    }
    for (actual, wanted) in manifest.workers.iter().zip(&expected.workers) {
        if actual.artifact_digest != wanted.artifact_digest {
            return Err(AssemblyPreDispatchErrorV1::WorkerDigestMismatch {
                engine: actual.engine,
            });
        }
        if actual.capability_catalog_digest != wanted.capability_catalog_digest {
            return Err(
                AssemblyPreDispatchErrorV1::CapabilityCatalogDigestMismatch {
                    scope: actual.engine.as_str().to_owned(),
                },
            );
        }
        if actual != wanted {
            return Err(AssemblyPreDispatchErrorV1::WorkerIdentityMismatch {
                engine: actual.engine,
            });
        }
    }
    if manifest.aggregate_capability_catalog_digest != expected.aggregate_capability_catalog_digest
    {
        return Err(
            AssemblyPreDispatchErrorV1::CapabilityCatalogDigestMismatch {
                scope: "aggregate".to_owned(),
            },
        );
    }
    let actual_digest = manifest
        .digest()
        .map_err(AssemblyPreDispatchErrorV1::InvalidManifest)?;
    if actual_digest != expected.manifest_digest {
        return Err(AssemblyPreDispatchErrorV1::ManifestDigestMismatch);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssemblyManifestErrorV1 {
    UnsupportedVersion {
        field: &'static str,
        supported: u16,
        actual: u16,
    },
    ContractDigestMismatch {
        expected: DigestV1,
        actual: DigestV1,
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

impl fmt::Display for AssemblyManifestErrorV1 {
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

impl Error for AssemblyManifestErrorV1 {}

fn require_version(
    field: &'static str,
    supported: u16,
    actual: u16,
) -> Result<(), AssemblyManifestErrorV1> {
    if actual == supported {
        Ok(())
    } else {
        Err(AssemblyManifestErrorV1::UnsupportedVersion {
            field,
            supported,
            actual,
        })
    }
}

fn require_items<T>(field: &'static str, values: &[T]) -> Result<(), AssemblyManifestErrorV1> {
    if values.is_empty() {
        return Err(AssemblyManifestErrorV1::EmptyCollection(field));
    }
    if values.len() > MAX_ASSEMBLY_ITEMS {
        return Err(AssemblyManifestErrorV1::TooManyItems {
            field,
            actual: values.len(),
            maximum: MAX_ASSEMBLY_ITEMS,
        });
    }
    Ok(())
}

fn require_string(field: &'static str, value: &str) -> Result<(), AssemblyManifestErrorV1> {
    if value.is_empty() {
        return Err(AssemblyManifestErrorV1::EmptyString(field));
    }
    if value.len() > MAX_ASSEMBLY_STRING_BYTES {
        return Err(AssemblyManifestErrorV1::StringTooLong {
            field,
            actual: value.len(),
            maximum: MAX_ASSEMBLY_STRING_BYTES,
        });
    }
    if value.chars().any(char::is_control) {
        return Err(AssemblyManifestErrorV1::ControlCharacter(field));
    }
    Ok(())
}

fn require_canonical_order(
    field: &'static str,
    identities: &[&str],
) -> Result<(), AssemblyManifestErrorV1> {
    for pair in identities.windows(2) {
        match pair[0].cmp(pair[1]) {
            std::cmp::Ordering::Greater => {
                return Err(AssemblyManifestErrorV1::NonCanonicalOrder(field));
            }
            std::cmp::Ordering::Equal => {
                return Err(AssemblyManifestErrorV1::DuplicateIdentity {
                    field,
                    identity: pair[0].to_owned(),
                });
            }
            std::cmp::Ordering::Less => {}
        }
    }
    Ok(())
}

/// Semantic schema hashed into every AssemblyManifestV1.
pub fn assembly_abi_contract_manifest_v1() -> Value {
    json!({
        "contract": "zerostack.assembly_abi",
        "contract_version": ASSEMBLY_ABI_CONTRACT_VERSION,
        "manifest_schema_version": ASSEMBLY_MANIFEST_SCHEMA_VERSION,
        "digest": {"algorithm": "sha256", "domain": "zerostack.assembly_manifest.v1\u{0}"},
        "encoding": "rfc8259_json_sorted_object_keys_no_whitespace",
        "linked_contracts": {
            "cwir_contract_version": CWIR_CONTRACT_VERSION_V1,
            "cwir_contract_digest": cwir_contract_digest_v1(),
            "effect_ir_contract_version": EFFECT_IR_CONTRACT_VERSION_V1,
            "effect_ir_contract_digest": effect_ir_contract_digest_v1(),
            "reasoning_contract_version": REASONING_CONTRACT_VERSION_V1,
            "reasoning_contract_digest": reasoning_contract_digest_v1(),
            "zbf_contract_version": ZBF_CONTRACT_VERSION_V1,
            "zbf_contract_digest": zbf_contract_digest_v1()
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

pub fn assembly_abi_contract_digest_v1() -> DigestV1 {
    let canonical = canonical_json(&assembly_abi_contract_manifest_v1());
    DigestV1::from_bytes(sha256(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn artifact(artifact_id: &str, owner: ArtifactOwnerV1, byte: u8) -> LinkedArtifactV1 {
        LinkedArtifactV1 {
            artifact_id: artifact_id.into(),
            owner,
            artifact_version: "1.0.0".into(),
            source_repository: format!("https://example.invalid/{artifact_id}"),
            source_revision: format!("{byte:02x}").repeat(20),
            artifact_digest: digest(byte),
            contract_digest: digest(byte.wrapping_add(16)),
        }
    }

    fn worker(engine: EngineIdentity, byte: u8) -> WorkerIdentityV1 {
        WorkerIdentityV1 {
            engine,
            artifact_digest: digest(byte),
            worker_protocol_digest: digest(byte.wrapping_add(32)),
            semantic_contract_digest: digest(byte.wrapping_add(48)),
            operation_registry_digest: digest(byte.wrapping_add(64)),
            capability_catalog_digest: digest(byte.wrapping_add(80)),
        }
    }

    fn assembly_manifest_fixture() -> AssemblyManifestV1 {
        AssemblyManifestV1 {
            schema_version: ASSEMBLY_MANIFEST_SCHEMA_VERSION,
            required_abi_contract_version: ASSEMBLY_ABI_CONTRACT_VERSION,
            abi_contract_digest: assembly_abi_contract_digest_v1(),
            linked_artifacts: vec![
                artifact("fszero.worker", ArtifactOwnerV1::FsZero, 1),
                artifact("graphzero.worker", ArtifactOwnerV1::GraphZero, 2),
                artifact("tokenzero.worker", ArtifactOwnerV1::TokenZero, 3),
                artifact("zerostack.host", ArtifactOwnerV1::ZeroStack, 4),
            ],
            linked_profiles: vec![
                LinkedProfileV1 {
                    profile_kind: ProfileKindV1::Platform,
                    profile_id: "linux-x86_64-v1".into(),
                    profile_version: "1".into(),
                    profile_digest: digest(101),
                },
                LinkedProfileV1 {
                    profile_kind: ProfileKindV1::Runtime,
                    profile_id: "quickjs-v1".into(),
                    profile_version: "2025-09-13".into(),
                    profile_digest: digest(102),
                },
            ],
            target: TargetIdentityV1 {
                target_triple: "x86_64-unknown-linux-gnu".into(),
                architecture: "x86_64".into(),
                operating_system: "linux".into(),
                abi: "gnu".into(),
            },
            platform: PlatformIdentityV1 {
                profile_id: "linux-x86_64-v1".into(),
                profile_version: "1".into(),
                profile_digest: digest(101),
            },
            verifiers: vec![VerifierIdentityV1 {
                verifier_id: "zero-testkit.assembly-kat".into(),
                verifier_version: "1".into(),
                verifier_digest: digest(103),
            }],
            receipt_schema: ReceiptSchemaIdentityV1 {
                schema_id: "zerostack.proof_receipt".into(),
                schema_version: "1".into(),
                schema_digest: digest(104),
            },
            runtime_generation: 7,
            assembly_epoch: 42,
            workers: vec![
                worker(EngineIdentity::FsZero, 1),
                worker(EngineIdentity::GraphZero, 2),
                worker(EngineIdentity::TokenZero, 3),
            ],
            aggregate_capability_catalog_digest: digest(105),
        }
    }

    #[test]
    fn assembly_manifest_canonical_vector_is_stable() {
        let manifest = assembly_manifest_fixture();
        let bytes = manifest.canonical_bytes().unwrap();
        let decoded = AssemblyManifestV1::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(decoded, manifest);
        assert_eq!(
            manifest.digest().unwrap().to_hex(),
            "7a5d8c5a6bfd4e8990510d9f4129f734bd07f4cc3a2603068ce5bb3d80246b92"
        );
    }

    #[test]
    fn assembly_manifest_contract_digest_is_stable() {
        let contract = assembly_abi_contract_manifest_v1();
        assert_eq!(
            contract["linked_contracts"]["zbf_contract_digest"],
            zbf_contract_digest_v1().to_hex()
        );
        assert_eq!(
            assembly_abi_contract_digest_v1().to_hex(),
            "f9320787ce17676c1eff1b2e38f1897ca40f9a72a02d5d72ffba37d70aa70d70"
        );
    }

    #[test]
    fn assembly_manifest_rejects_noncanonical_unknown_and_unlinked_values() {
        let manifest = assembly_manifest_fixture();
        let mut bytes = manifest.canonical_bytes().unwrap();
        bytes.push(b'\n');
        assert_eq!(
            AssemblyManifestV1::from_canonical_bytes(&bytes).unwrap_err(),
            AssemblyManifestErrorV1::NonCanonicalEncoding
        );

        let mut unknown = assembly_manifest_fixture();
        unknown.schema_version += 1;
        assert!(matches!(
            unknown.validate(),
            Err(AssemblyManifestErrorV1::UnsupportedVersion {
                field: "schema_version",
                ..
            })
        ));

        let mut unlinked = assembly_manifest_fixture();
        unlinked.workers[0].artifact_digest = digest(200);
        assert_eq!(
            unlinked.validate().unwrap_err(),
            AssemblyManifestErrorV1::UnlinkedWorkerArtifact(EngineIdentity::FsZero)
        );
    }

    #[test]
    fn assembly_manifest_predispatch_skew_is_typed() {
        let manifest = assembly_manifest_fixture();
        let mut expected = manifest.expectation().unwrap();
        expected.workers[0].artifact_digest = digest(200);
        assert_eq!(
            validate_assembly_pre_dispatch_v1(&manifest, &expected)
                .unwrap_err()
                .code(),
            AssemblyFailureCodeV1::WorkerDigestMismatch
        );

        let mut expected = manifest.expectation().unwrap();
        expected.workers[1].capability_catalog_digest = digest(201);
        assert_eq!(
            validate_assembly_pre_dispatch_v1(&manifest, &expected)
                .unwrap_err()
                .code(),
            AssemblyFailureCodeV1::CapabilityCatalogDigestMismatch
        );

        let mut expected = manifest.expectation().unwrap();
        expected.required_schema_version += 1;
        assert_eq!(
            validate_assembly_pre_dispatch_v1(&manifest, &expected)
                .unwrap_err()
                .code(),
            AssemblyFailureCodeV1::UnsupportedRequiredVersion
        );
    }

    #[test]
    fn assembly_manifest_digest_wire_form_is_strict() {
        let value = digest(0xab);
        let encoded = serde_json::to_string(&value).unwrap();
        assert_eq!(encoded, format!("\"{}\"", "ab".repeat(32)));
        assert_eq!(serde_json::from_str::<DigestV1>(&encoded).unwrap(), value);
        assert!(serde_json::from_str::<DigestV1>(&format!("\"{}\"", "AB".repeat(32))).is_err());
    }
}
