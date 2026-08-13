//! Shared install-time surface and domain-adapter registration contract.
//!
//! A worker supplies domain semantics. This module supplies the one selected
//! execution face and validates the metadata that the host or a compatibility
//! carrier may expose. It deliberately contains no JavaScript runtime, MCP
//! transport, or engine-domain dispatch.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CanonicalRegistry, EngineIdentity, RefOwnership, RegistryEngine, TelemetrySchema};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityDescriptor {
    pub surface: String,
    pub method: String,
}

impl CapabilityDescriptor {
    pub fn new(surface: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            surface: surface.into(),
            method: method.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobalRegistration {
    pub root: String,
    pub capabilities: Vec<CapabilityDescriptor>,
}

impl GlobalRegistration {
    pub fn zero(capabilities: Vec<CapabilityDescriptor>) -> Self {
        Self {
            root: "zero".to_owned(),
            capabilities,
        }
    }

    pub fn validate(&self) -> Result<(), RegistrationError> {
        validate_identifier(&self.root)
            .map_err(|_| RegistrationError::InvalidGlobal(self.root.clone()))?;
        let mut seen = BTreeSet::new();
        for capability in &self.capabilities {
            if validate_identifier(&capability.surface).is_err()
                || validate_identifier(&capability.method).is_err()
            {
                return Err(RegistrationError::InvalidCapability(capability.clone()));
            }
            if !seen.insert(capability.clone()) {
                return Err(RegistrationError::DuplicateCapability(capability.clone()));
            }
        }
        Ok(())
    }
}

fn validate_identifier(value: &str) -> Result<(), ()> {
    if matches!(value, "__proto__" | "prototype" | "constructor") {
        return Err(());
    }
    let mut chars = value.chars();
    let first = chars.next().ok_or(())?;
    if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
        return Err(());
    }
    if chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()) {
        Ok(())
    } else {
        Err(())
    }
}

pub const SURFACE_CONTRACT_VERSION: &str = "zerostack.surface/v1";

/// The single public face selected for one installed artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceKind {
    #[serde(rename = "codemode")]
    CodeMode,
    Mcp,
}

impl SurfaceKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CodeMode => "codemode",
            Self::Mcp => "mcp",
        }
    }

    pub const fn opposite(self) -> Self {
        match self {
            Self::CodeMode => Self::Mcp,
            Self::Mcp => Self::CodeMode,
        }
    }

    pub fn parse(value: &str) -> Result<Self, SurfaceContractError> {
        match value {
            "codemode" | "code-mode" | "code_mode" => Ok(Self::CodeMode),
            "mcp" => Ok(Self::Mcp),
            _ => Err(SurfaceContractError::InvalidSurface(value.to_owned())),
        }
    }
}

/// Engine-owned semantics presented to a shared surface.
///
/// `CanonicalRegistry` carries typed operation dispatch and effect/approval
/// policy. `RefOwnership` and `TelemetrySchema` carry the other two pieces of
/// metadata that must not be reconstructed by a transport adapter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DomainAdapterRegistration {
    pub engine: EngineIdentity,
    pub registry: CanonicalRegistry,
    pub ref_ownership: RefOwnership,
    pub telemetry_schema: TelemetrySchema,
    pub capabilities: Vec<CapabilityDescriptor>,
}

impl DomainAdapterRegistration {
    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        self.registry
            .validate()
            .map_err(|error| SurfaceContractError::InvalidAdapter(error.to_string()))?;
        if !registry_matches_engine(self.registry.engine, self.engine) {
            return Err(SurfaceContractError::RegistryEngineMismatch {
                expected: self.engine,
                actual: self.registry.engine,
            });
        }
        if self.ref_ownership.engine != self.engine {
            return Err(SurfaceContractError::EngineMismatch {
                field: "ref_ownership.engine",
                expected: self.engine,
                actual: self.ref_ownership.engine,
            });
        }
        GlobalRegistration::zero(self.capabilities.clone())
            .validate()
            .map_err(SurfaceContractError::InvalidCapabilities)?;

        let canonical_ids: BTreeSet<String> = self
            .registry
            .operations
            .iter()
            .map(|operation| operation.canonical_id.clone())
            .collect();
        let capability_ids: BTreeSet<String> = self
            .capabilities
            .iter()
            .map(|capability| format!("{}.{}", capability.surface, capability.method))
            .collect();
        let missing = canonical_ids
            .difference(&capability_ids)
            .cloned()
            .collect::<Vec<_>>();
        let extra = capability_ids
            .difference(&canonical_ids)
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() || !extra.is_empty() {
            return Err(SurfaceContractError::CapabilityCatalogMismatch { missing, extra });
        }
        Ok(())
    }

    /// The canonical digest of the typed domain operation registry.
    pub fn registry_digest_hex(&self) -> Result<String, SurfaceContractError> {
        self.validate()?;
        self.registry
            .contract_digest_hex()
            .map_err(|error| SurfaceContractError::InvalidAdapter(error.to_string()))
    }
}

/// One install-time surface registration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceRegistration {
    pub contract_version: String,
    pub surface: SurfaceKind,
    pub root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub adapter: DomainAdapterRegistration,
}

impl SurfaceRegistration {
    pub fn new(
        surface: SurfaceKind,
        root: impl Into<String>,
        adapter: DomainAdapterRegistration,
    ) -> Self {
        Self {
            contract_version: SURFACE_CONTRACT_VERSION.to_owned(),
            surface,
            root: root.into(),
            instructions: None,
            adapter,
        }
    }

    pub fn validate(&self) -> Result<(), SurfaceContractError> {
        if self.contract_version != SURFACE_CONTRACT_VERSION {
            return Err(SurfaceContractError::InvalidVersion(
                self.contract_version.clone(),
            ));
        }
        self.adapter.validate()?;
        GlobalRegistration {
            root: self.root.clone(),
            capabilities: self.adapter.capabilities.clone(),
        }
        .validate()
        .map_err(SurfaceContractError::InvalidCapabilities)
    }

    /// Convert only a CodeMode registration to the host's global tree.
    ///
    /// An MCP compatibility carrier must not silently become a CodeMode host.
    pub fn global_registration(&self) -> Result<GlobalRegistration, SurfaceContractError> {
        self.validate()?;
        if self.surface != SurfaceKind::CodeMode {
            return Err(SurfaceContractError::WrongSurface {
                requested: SurfaceKind::CodeMode,
                actual: self.surface,
            });
        }
        Ok(GlobalRegistration {
            root: self.root.clone(),
            capabilities: self.adapter.capabilities.clone(),
        })
    }

    /// Stable machine-readable catalog projection.
    pub fn catalog(&self) -> Result<Value, SurfaceContractError> {
        self.validate()?;
        serde_json::to_value(self)
            .map_err(|error| SurfaceContractError::Encoding(error.to_string()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistrationError {
    InvalidGlobal(String),
    InvalidCapability(CapabilityDescriptor),
    DuplicateCapability(CapabilityDescriptor),
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGlobal(name) => write!(f, "invalid global name: {name}"),
            Self::InvalidCapability(cap) => {
                write!(f, "invalid capability: {}.{}", cap.surface, cap.method)
            }
            Self::DuplicateCapability(cap) => {
                write!(f, "duplicate capability: {}.{}", cap.surface, cap.method)
            }
        }
    }
}

impl std::error::Error for RegistrationError {}

#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceContractError {
    InvalidVersion(String),
    InvalidSurface(String),
    InvalidAdapter(String),
    InvalidCapabilities(RegistrationError),
    RegistryEngineMismatch {
        expected: EngineIdentity,
        actual: RegistryEngine,
    },
    EngineMismatch {
        field: &'static str,
        expected: EngineIdentity,
        actual: EngineIdentity,
    },
    WrongSurface {
        requested: SurfaceKind,
        actual: SurfaceKind,
    },
    CapabilityCatalogMismatch {
        missing: Vec<String>,
        extra: Vec<String>,
    },
    Encoding(String),
}

impl fmt::Display for SurfaceContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVersion(version) => {
                write!(
                    formatter,
                    "unsupported surface contract version {version:?}"
                )
            }
            Self::InvalidSurface(surface) => {
                write!(formatter, "unsupported surface {surface:?}")
            }
            Self::InvalidAdapter(error) => write!(formatter, "invalid domain adapter: {error}"),
            Self::InvalidCapabilities(error) => write!(formatter, "invalid capabilities: {error}"),
            Self::RegistryEngineMismatch { expected, actual } => write!(
                formatter,
                "registry belongs to {actual:?}, expected {}",
                expected.as_str()
            ),
            Self::EngineMismatch {
                field,
                expected,
                actual,
            } => write!(
                formatter,
                "{field} belongs to {}, expected {}",
                actual.as_str(),
                expected.as_str()
            ),
            Self::WrongSurface { requested, actual } => write!(
                formatter,
                "cannot expose {} registration as {}; artifact is {}",
                actual.as_str(),
                requested.as_str(),
                actual.as_str()
            ),
            Self::CapabilityCatalogMismatch { missing, extra } => write!(
                formatter,
                "capability catalog must exactly match canonical operations; missing={missing:?}, extra={extra:?}"
            ),
            Self::Encoding(error) => write!(formatter, "failed to encode surface catalog: {error}"),
        }
    }
}

impl std::error::Error for SurfaceContractError {}

fn registry_matches_engine(registry: RegistryEngine, engine: EngineIdentity) -> bool {
    matches!(
        (registry, engine),
        (RegistryEngine::FsZero, EngineIdentity::FsZero)
            | (RegistryEngine::GraphZero, EngineIdentity::GraphZero)
            | (RegistryEngine::TokenZero, EngineIdentity::TokenZero)
    )
}

#[cfg(test)]
#[path = "../../../tests/rust/zero-abi/unit/surface.rs"]
mod tests;
