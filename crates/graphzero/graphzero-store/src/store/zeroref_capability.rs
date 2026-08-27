//! ZeroRef v1 capability descriptor and peer validation
//! (docs/adr/002-zeroref-v1.md §11, bead graphzero-zeroref-v1-shared-cas-1ghi.6).
//!
//! One descriptor source: every surface (CLI `capabilities`, `doctor`,
//! CodeMode capability manifest) serializes [`ZeroRefDescriptor::current`],
//! whose values come from the same constants the parser and store use, so
//! docs, list output, and runtime behavior cannot drift.
//!
//! The descriptor carries no secrets and no absolute private paths: shared
//! roots are reported as states/booleans, never as paths.

use serde::{Deserialize, Serialize};

use super::shared_cas::{CAS_LAYOUT, CAS_LAYOUT_VERSION, CAS_MAX_OBJECT_BYTES};
use super::zeroref::{
    HASH_ALGORITHM, HASH_HEX_LEN, ZEROREF_MAJOR, ZEROREF_MINOR, ZeroRefError, ZeroRefErrorClass,
};
use super::zerostack_store::{STORE_ROOT_ENVS, shared_store_opt_in_from_env};

/// Descriptor schema id. Major bump = incompatible; additive fields only
/// within a major.
pub const ZEROREF_CAPABILITY_SCHEMA: &str = "zeroref-capability/v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractVersion {
    pub major: u64,
    pub minor: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HashCapability {
    pub algorithm: String,
    pub hex_length: u64,
    pub accept_uppercase: bool,
    pub accept_prefixes: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemeCapability {
    pub accepted: Vec<String>,
    pub emitted: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FragmentCapability {
    pub canonical: Vec<String>,
    pub byte_span: String,
    pub line_span: String,
    pub clamps: bool,
    pub legacy_input_aliases: Vec<String>,
    pub emitted_aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedCasCapability {
    pub layout: String,
    pub layout_version: u64,
    pub read: bool,
    pub write: bool,
    pub max_object_bytes: u64,
}

/// Effective interop state, distinct from code support: a binary can carry
/// full v1 code while shared interoperability is disabled or misconfigured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedInteropState {
    /// Opt-in present, root configured and usable.
    Enabled,
    /// No shared-store opt-in: local-only scheme parsing and local CAS.
    Disabled,
    /// Opt-in present but the shared root is missing or not configured.
    Misconfigured,
    /// Opt-in and root present but the root is not usable (I/O failure).
    Unhealthy,
}

impl SharedInteropState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Misconfigured => "misconfigured",
            Self::Unhealthy => "unhealthy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveState {
    /// The code in this binary implements ZeroRef v1 (always true here).
    pub code_support: bool,
    pub shared_interop: SharedInteropState,
    pub shared_root_configured: bool,
    /// Actionable detail for non-enabled states. Never contains paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl EffectiveState {
    /// Probe env + shared-root health. Local-only support stays fully
    /// functional in every state; only cross-engine sharing varies.
    pub fn probe_from_env() -> Self {
        let opt_in = shared_store_opt_in_from_env();
        let root = STORE_ROOT_ENVS.iter().find_map(std::env::var_os);
        let (state, detail) = match (opt_in, &root) {
            (false, _) => (
                SharedInteropState::Disabled,
                Some("shared-store opt-in not set; operating local-only".to_string()),
            ),
            (true, None) => (
                SharedInteropState::Misconfigured,
                Some("shared-store opt-in set but no store-root env is configured".to_string()),
            ),
            (true, Some(path)) => {
                let dir = std::path::Path::new(path);
                if !dir.is_dir() {
                    (
                        SharedInteropState::Misconfigured,
                        Some("configured shared root does not exist".to_string()),
                    )
                } else if std::fs::read_dir(dir).is_err() {
                    (
                        SharedInteropState::Unhealthy,
                        Some("configured shared root is not readable".to_string()),
                    )
                } else {
                    (SharedInteropState::Enabled, None)
                }
            }
        };
        Self {
            code_support: true,
            shared_interop: state,
            shared_root_configured: root.is_some(),
            detail,
        }
    }
}

/// The machine-readable ZeroRef capability descriptor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ZeroRefDescriptor {
    pub schema: String,
    pub contract: ContractVersion,
    pub hash: HashCapability,
    pub schemes: SchemeCapability,
    pub fragments: FragmentCapability,
    pub shared_cas: SharedCasCapability,
    pub error_classes: Vec<String>,
    pub effective: EffectiveState,
}

impl ZeroRefDescriptor {
    /// Build the descriptor from the same constants the parser/store use.
    pub fn current(effective: EffectiveState) -> Self {
        Self {
            schema: ZEROREF_CAPABILITY_SCHEMA.to_string(),
            contract: ContractVersion {
                major: ZEROREF_MAJOR,
                minor: ZEROREF_MINOR,
            },
            hash: HashCapability {
                algorithm: HASH_ALGORITHM.to_string(),
                hex_length: HASH_HEX_LEN as u64,
                accept_uppercase: false,
                accept_prefixes: false,
            },
            schemes: SchemeCapability {
                accepted: vec!["fz".to_string(), "gz".to_string(), "tz".to_string()],
                emitted: "gz".to_string(),
            },
            fragments: FragmentCapability {
                canonical: vec!["#B<start>-<end>".to_string(), "#L<start>-<end>".to_string()],
                byte_span: "zero-based-half-open".to_string(),
                line_span: "one-based-inclusive".to_string(),
                clamps: false,
                legacy_input_aliases: vec!["#B<start>+<len>".to_string()],
                emitted_aliases: Vec::new(),
            },
            shared_cas: SharedCasCapability {
                layout: CAS_LAYOUT.to_string(),
                layout_version: CAS_LAYOUT_VERSION,
                read: true,
                write: true,
                max_object_bytes: CAS_MAX_OBJECT_BYTES,
            },
            error_classes: [
                ZeroRefErrorClass::Malformed,
                ZeroRefErrorClass::Unsupported,
                ZeroRefErrorClass::RangeOutOfBounds,
                ZeroRefErrorClass::NotUtf8,
                ZeroRefErrorClass::Missing,
                ZeroRefErrorClass::Io,
                ZeroRefErrorClass::DigestMismatch,
                ZeroRefErrorClass::PolicyDenied,
                ZeroRefErrorClass::IncompatibleVersion,
                ZeroRefErrorClass::LegacyAmbiguity,
            ]
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
            effective,
        }
    }

    /// Descriptor with the effective state probed from the environment.
    pub fn from_env() -> Self {
        Self::current(EffectiveState::probe_from_env())
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("descriptor serializes")
    }
}

/// Outcome of validating a peer descriptor against ours.
#[derive(Debug, Clone, PartialEq)]
pub struct PeerCompatibility {
    /// True when both sides can exchange shared-CAS objects right now.
    pub shared_interop: bool,
    /// Human-readable notes explaining any restriction.
    pub notes: Vec<String>,
}

fn incompatible(message: impl Into<String>) -> ZeroRefError {
    ZeroRefError {
        class: ZeroRefErrorClass::IncompatibleVersion,
        message: message.into(),
    }
}

fn peer_malformed(message: impl Into<String>) -> ZeroRefError {
    ZeroRefError {
        class: ZeroRefErrorClass::Malformed,
        message: message.into(),
    }
}

/// Strictly validate a peer's descriptor before any payload work.
///
/// Unknown major versions and incompatible hash/layout semantics fail with
/// actionable `incompatible_version` errors; missing or type-broken
/// descriptors fail as `malformed`. Additive minor fields are ignored, so
/// newer-minor peers remain forward-compatible.
pub fn validate_peer_descriptor(
    peer: &serde_json::Value,
    ours: &ZeroRefDescriptor,
) -> Result<PeerCompatibility, ZeroRefError> {
    if peer.is_null() {
        return Err(peer_malformed(
            "peer sent no ZeroRef descriptor; treat it as legacy local-only and do not pass refs",
        ));
    }
    let schema = peer
        .get("schema")
        .and_then(|v| v.as_str())
        .ok_or_else(|| peer_malformed("peer descriptor missing string field 'schema'"))?;
    if !schema.starts_with("zeroref-capability/") {
        return Err(peer_malformed(format!(
            "peer descriptor schema '{schema}' is not a zeroref-capability document"
        )));
    }
    let major = peer
        .pointer("/contract/major")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| peer_malformed("peer descriptor missing numeric 'contract.major'"))?;
    if major != ours.contract.major {
        return Err(incompatible(format!(
            "peer speaks ZeroRef contract major {major}; this build supports major {} — \
             upgrade the older side before passing refs",
            ours.contract.major
        )));
    }
    let algorithm = peer
        .pointer("/hash/algorithm")
        .and_then(|v| v.as_str())
        .ok_or_else(|| peer_malformed("peer descriptor missing 'hash.algorithm'"))?;
    let hex_length = peer
        .pointer("/hash/hex_length")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| peer_malformed("peer descriptor missing 'hash.hex_length'"))?;
    if algorithm != ours.hash.algorithm || hex_length != ours.hash.hex_length {
        return Err(incompatible(format!(
            "peer identity is {algorithm}/{hex_length}; this build requires {}/{} — \
             identities would not interoperate",
            ours.hash.algorithm, ours.hash.hex_length
        )));
    }
    let layout_version = peer
        .pointer("/shared_cas/layout_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| peer_malformed("peer descriptor missing 'shared_cas.layout_version'"))?;
    if layout_version != ours.shared_cas.layout_version {
        return Err(incompatible(format!(
            "peer shared-CAS layout version {layout_version} differs from ours {} — \
             objects would land at different paths",
            ours.shared_cas.layout_version
        )));
    }

    let mut notes = Vec::new();
    let peer_state = peer
        .pointer("/effective/shared_interop")
        .and_then(|v| v.as_str())
        .unwrap_or("disabled");
    if peer_state != "enabled" {
        notes.push(format!("peer shared interop is {peer_state}"));
    }
    if ours.effective.shared_interop != SharedInteropState::Enabled {
        let mut note = format!(
            "local shared interop is {}",
            ours.effective.shared_interop.as_str()
        );
        if let Some(detail) = &ours.effective.detail {
            note.push_str(&format!(": {detail}"));
        }
        notes.push(note);
    }
    let shared_interop =
        peer_state == "enabled" && ours.effective.shared_interop == SharedInteropState::Enabled;
    Ok(PeerCompatibility {
        shared_interop,
        notes,
    })
}
