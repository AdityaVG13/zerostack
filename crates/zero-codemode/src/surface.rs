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
use zero_abi::{CanonicalRegistry, EngineIdentity, RefOwnership, TelemetrySchema};

use crate::{CapabilityDescriptor, GlobalRegistration, RegistrationError};

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

#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceContractError {
    InvalidVersion(String),
    InvalidSurface(String),
    InvalidAdapter(String),
    InvalidCapabilities(RegistrationError),
    RegistryEngineMismatch {
        expected: EngineIdentity,
        actual: zero_abi::RegistryEngine,
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

fn registry_matches_engine(registry: zero_abi::RegistryEngine, engine: EngineIdentity) -> bool {
    matches!(
        (registry, engine),
        (zero_abi::RegistryEngine::FsZero, EngineIdentity::FsZero)
            | (
                zero_abi::RegistryEngine::GraphZero,
                EngineIdentity::GraphZero
            )
            | (
                zero_abi::RegistryEngine::TokenZero,
                EngineIdentity::TokenZero
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zero_abi::{
        ALL_DISPATCH_ERROR_CLASSES, ApprovalRequirement, CasLayout, EffectClass, EffectPolicy,
        HashAlgorithm, LayoutVersion, PermitRequirement, RegistryEngine, SharedCapability,
    };

    fn adapter() -> DomainAdapterRegistration {
        DomainAdapterRegistration {
            engine: EngineIdentity::FsZero,
            registry: CanonicalRegistry {
                version: zero_abi::CANONICAL_DISPATCH_VERSION.to_owned(),
                engine: RegistryEngine::FsZero,
                operations: vec![zero_abi::CanonicalOperation {
                    canonical_id: "fs.read".into(),
                    aliases: vec!["read".into()],
                    args_schema: json!({"type": "object", "additionalProperties": false}),
                    effect_policy: EffectPolicy {
                        effect_class: EffectClass::ReadOnly,
                        permit: PermitRequirement::NotRequired,
                        approval: ApprovalRequirement::NotRequired,
                    },
                    errors: ALL_DISPATCH_ERROR_CLASSES.to_vec(),
                }],
            },
            ref_ownership: zero_abi::RefOwnership {
                engine: EngineIdentity::FsZero,
                session_id: "session-1".into(),
                refs: vec!["fz://ref-1".into()],
                snapshot: None,
            },
            telemetry_schema: TelemetrySchema::V1,
            capabilities: vec![CapabilityDescriptor::new("fs", "read")],
        }
    }

    #[test]
    fn codemode_registration_projects_to_one_valid_global_tree() {
        let registration = SurfaceRegistration::new(SurfaceKind::CodeMode, "zero", adapter());
        let global = registration.global_registration().unwrap();
        assert_eq!(global.root, "zero");
        assert_eq!(global.capabilities.len(), 1);
        assert_eq!(registration.surface.opposite(), SurfaceKind::Mcp);
        assert_eq!(
            registration.adapter.registry_digest_hex().unwrap().len(),
            64
        );
    }

    #[test]
    fn mcp_registration_cannot_be_used_as_codemode() {
        let registration = SurfaceRegistration::new(SurfaceKind::Mcp, "zero", adapter());
        assert!(matches!(
            registration.global_registration(),
            Err(SurfaceContractError::WrongSurface {
                requested: SurfaceKind::CodeMode,
                actual: SurfaceKind::Mcp
            })
        ));
    }

    #[test]
    fn adapter_engine_bindings_and_unknown_metadata_fail_closed() {
        let mut wrong = adapter();
        wrong.ref_ownership.engine = EngineIdentity::GraphZero;
        assert!(matches!(
            wrong.validate(),
            Err(SurfaceContractError::EngineMismatch {
                field: "ref_ownership.engine",
                ..
            })
        ));

        let mut wire = serde_json::to_value(SurfaceRegistration::new(
            SurfaceKind::CodeMode,
            "zero",
            adapter(),
        ))
        .unwrap();
        wire["unexpected"] = json!(true);
        assert!(serde_json::from_value::<SurfaceRegistration>(wire).is_err());
    }

    #[test]
    fn capability_catalog_must_match_canonical_operations_exactly() {
        let mut missing = adapter();
        missing.capabilities.clear();
        assert!(matches!(
            missing.validate(),
            Err(SurfaceContractError::CapabilityCatalogMismatch { missing, extra })
                if missing == vec!["fs.read"] && extra.is_empty()
        ));

        let mut extra = adapter();
        extra.capabilities = vec![
            CapabilityDescriptor::new("fs", "read"),
            CapabilityDescriptor::new("fs", "write"),
        ];
        assert!(matches!(
            extra.validate(),
            Err(SurfaceContractError::CapabilityCatalogMismatch { missing, extra })
                if missing.is_empty() && extra == vec!["fs.write"]
        ));

        let mut duplicate = adapter();
        duplicate
            .capabilities
            .push(CapabilityDescriptor::new("fs", "read"));
        assert!(matches!(
            duplicate.validate(),
            Err(SurfaceContractError::InvalidCapabilities(
                crate::RegistrationError::DuplicateCapability(_)
            ))
        ));
    }

    #[test]
    fn surface_parser_accepts_only_canonical_faces_and_aliases() {
        assert_eq!(
            SurfaceKind::parse("codemode").unwrap(),
            SurfaceKind::CodeMode
        );
        assert_eq!(
            SurfaceKind::parse("code-mode").unwrap(),
            SurfaceKind::CodeMode
        );
        assert_eq!(SurfaceKind::parse("mcp").unwrap(), SurfaceKind::Mcp);
        assert!(SurfaceKind::parse("both").is_err());
        let capability = SharedCapability::zeroref_v1(
            HashAlgorithm::Sha256,
            CasLayout::BlobsSha256Hh,
            LayoutVersion::V1,
        );
        assert_eq!(capability.hash.algorithm, HashAlgorithm::Sha256);
    }
}
