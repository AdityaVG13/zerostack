//! Pack manifest v2: canonical-v2 JSON, content-addressed shards, and ed25519 signatures.

use std::fmt;
use std::path::Path;

use anyhow::{Context, Result};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize, Serializer};
use sha2::{Digest, Sha256};

pub const LEGACY_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const MANIFEST_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackShardEntry {
    pub file_name: String,
    pub content_sha256: String,
    pub blob_count: u32,
    pub file_hash64: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackSemanticSidecarEntry {
    pub gzsh_file_name: String,
    pub file_name: String,
    pub content_sha256: String,
    pub record_count: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackProvenance {
    pub lockfile_sha256: String,
    pub toolchain: String,
    pub built_at_unix_nanos: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PackManifest {
    pub schema_version: u32,
    pub pack_id: String,
    pub version: String,
    pub tier_a_coverage: f64,
    pub shards: Vec<PackShardEntry>,
    #[serde(default)]
    pub semantic_sidecars: Vec<PackSemanticSidecarEntry>,
    pub provenance: PackProvenance,
    /// Hex-encoded ed25519 signature over canonical-v2 unsigned JSON bytes.
    pub signature_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ManifestSchemaError {
    LegacyManifest,
    Unsupported(u32),
}

impl fmt::Display for ManifestSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LegacyManifest => write!(
                f,
                "LegacyManifest: schema v1 uses legacy canonical bytes; rebuild and explicitly re-sign as schema v2"
            ),
            Self::Unsupported(version) => write!(
                f,
                "unsupported pack manifest schema {version}, expected {MANIFEST_SCHEMA_VERSION}"
            ),
        }
    }
}

impl std::error::Error for ManifestSchemaError {}

#[derive(Serialize)]
struct UnsignedPackManifestRef<'a> {
    schema_version: u32,
    pack_id: &'a str,
    version: &'a str,
    tier_a_coverage: f64,
    shards: &'a [PackShardEntry],
    semantic_sidecars: &'a [PackSemanticSidecarEntry],
    provenance: &'a PackProvenance,
    #[serde(serialize_with = "serialize_empty_signature")]
    signature_hex: (),
}

fn serialize_empty_signature<S>(_: &(), serializer: S) -> std::result::Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str("")
}

impl<'a> From<&'a PackManifest> for UnsignedPackManifestRef<'a> {
    fn from(manifest: &'a PackManifest) -> Self {
        Self {
            schema_version: manifest.schema_version,
            pack_id: &manifest.pack_id,
            version: &manifest.version,
            tier_a_coverage: manifest.tier_a_coverage,
            shards: &manifest.shards,
            semantic_sidecars: &manifest.semantic_sidecars,
            provenance: &manifest.provenance,
            signature_hex: (),
        }
    }
}

impl PackManifest {
    pub fn validate_schema(&self) -> std::result::Result<(), ManifestSchemaError> {
        match self.schema_version {
            MANIFEST_SCHEMA_VERSION => Ok(()),
            LEGACY_MANIFEST_SCHEMA_VERSION => Err(ManifestSchemaError::LegacyManifest),
            version => Err(ManifestSchemaError::Unsupported(version)),
        }
    }

    /// The sole unsigned payload for v2 digesting and signing.
    pub fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>> {
        self.validate_schema()?;
        let value = serde_json::to_value(UnsignedPackManifestRef::from(self))
            .context("serialize unsigned manifest value")?;
        Ok(zero_abi::canonical_json(&value).into_bytes())
    }

    pub fn digest_sha256(&self) -> Result<String> {
        let mut h = Sha256::new();
        h.update(self.canonical_unsigned_bytes()?);
        Ok(hex::encode(h.finalize()))
    }

    pub fn write_json(&self, path: &Path) -> Result<()> {
        self.validate_schema()?;
        let json = serde_json::to_string_pretty(self).context("encode manifest")?;
        std::fs::write(path, json).with_context(|| format!("write {}", path.display()))
    }

    pub fn read_json(path: &Path) -> Result<Self> {
        let data = std::fs::read_to_string(path)
            .with_context(|| format!("read manifest {}", path.display()))?;
        let manifest: Self = serde_json::from_str(&data).context("parse manifest json")?;
        manifest.validate_schema()?;
        Ok(manifest)
    }
}

/// Dev/CI signing keypair (deterministic seed for reproducible tests).
#[derive(Clone)]
pub struct PackSignKey {
    signing: SigningKey,
}

impl PackSignKey {
    pub fn fixture() -> Self {
        let seed = [0x42u8; 32];
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    pub fn public(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    pub fn signing_key(&self) -> &SigningKey {
        &self.signing
    }
}
