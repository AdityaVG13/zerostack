//! Shared canonical operation dispatch metadata and fail-closed enforcement.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    EffectClass,
    digest::{contract_digest as digest_manifest, contract_digest_hex as digest_manifest_hex},
    schema::normalize_schema,
};

pub const CANONICAL_DISPATCH_VERSION: &str = "zerostack.canonical_dispatch.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryEngine {
    TokenZero,
    FsZero,
    GraphZero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermitRequirement {
    NotRequired,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalRequirement {
    NotRequired,
    Required,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectPolicy {
    pub effect_class: EffectClass,
    pub permit: PermitRequirement,
    pub approval: ApprovalRequirement,
}

impl EffectPolicy {
    pub fn validate(self) -> Result<(), DispatchContractError> {
        let valid = matches!(
            (self.effect_class, self.permit, self.approval),
            (
                EffectClass::ReadOnly,
                PermitRequirement::NotRequired,
                ApprovalRequirement::NotRequired
            ) | (
                EffectClass::ReversibleMutation,
                PermitRequirement::Required,
                ApprovalRequirement::NotRequired
            ) | (
                EffectClass::ApprovalRequiredMutation,
                PermitRequirement::Required,
                ApprovalRequirement::Required
            ) | (
                EffectClass::Irreversible,
                PermitRequirement::Required,
                ApprovalRequirement::Required
            )
        );
        if valid {
            Ok(())
        } else {
            Err(DispatchContractError::new(
                DispatchErrorClass::InvalidRegistry,
                "effect policy is inconsistent with its effect class",
            ))
        }
    }
}

/// A typed authorization decision. It conveys policy authorization only;
/// permits and approvals are acquired in the next dispatch stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectGrant {
    ReadOnly,
    ReversibleMutation,
    ApprovalRequiredMutation,
    Irreversible,
}

impl EffectGrant {
    pub fn effect_class(self) -> EffectClass {
        match self {
            Self::ReadOnly => EffectClass::ReadOnly,
            Self::ReversibleMutation => EffectClass::ReversibleMutation,
            Self::ApprovalRequiredMutation => EffectClass::ApprovalRequiredMutation,
            Self::Irreversible => EffectClass::Irreversible,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchErrorClass {
    MissingMetadata,
    UnknownMetadata,
    InvalidRegistry,
    UnknownOperation,
    InvalidArguments,
    UnauthorizedEffect,
    PermitRequired,
    ApprovalRequired,
    DispatchFailed,
    JournalFailed,
    InvalidStageTransition,
}

pub const ALL_DISPATCH_ERROR_CLASSES: [DispatchErrorClass; 11] = [
    DispatchErrorClass::MissingMetadata,
    DispatchErrorClass::UnknownMetadata,
    DispatchErrorClass::InvalidRegistry,
    DispatchErrorClass::UnknownOperation,
    DispatchErrorClass::InvalidArguments,
    DispatchErrorClass::UnauthorizedEffect,
    DispatchErrorClass::PermitRequired,
    DispatchErrorClass::ApprovalRequired,
    DispatchErrorClass::DispatchFailed,
    DispatchErrorClass::JournalFailed,
    DispatchErrorClass::InvalidStageTransition,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchContractError {
    pub class: DispatchErrorClass,
    pub message: String,
}

impl DispatchContractError {
    pub fn new(class: DispatchErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
}

impl fmt::Display for DispatchContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.class, self.message)
    }
}

impl std::error::Error for DispatchContractError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalOperation {
    pub canonical_id: String,
    /// Engine-owned prose. Empty means no description was declared and is
    /// omitted from legacy wire manifests.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub aliases: Vec<String>,
    pub args_schema: Value,
    /// Engine-owned JSON Schema for successful results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    /// Primary name exposed by an MCP catalog. Dispatch still resolves the
    /// canonical id and aliases independently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_tool_name: Option<String>,
    pub effect_policy: EffectPolicy,
    pub errors: Vec<DispatchErrorClass>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalResource {
    pub uri: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRegistry {
    pub version: String,
    pub engine: RegistryEngine,
    pub operations: Vec<CanonicalOperation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<CanonicalResource>,
}

impl CanonicalRegistry {
    pub fn decode_str(encoded: &str) -> Result<Self, DispatchContractError> {
        let registry: Self = serde_json::from_str(encoded).map_err(classify_decode_error)?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn decode(value: &Value) -> Result<Self, DispatchContractError> {
        let registry: Self =
            serde_json::from_value(value.clone()).map_err(classify_decode_error)?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn validate(&self) -> Result<(), DispatchContractError> {
        if self.version != CANONICAL_DISPATCH_VERSION {
            return Err(DispatchContractError::new(
                DispatchErrorClass::UnknownMetadata,
                format!("unsupported dispatch metadata version {:?}", self.version),
            ));
        }
        if self.operations.is_empty() {
            return Err(DispatchContractError::new(
                DispatchErrorClass::InvalidRegistry,
                "registry must contain at least one operation",
            ));
        }

        let mut names = BTreeMap::<&str, &str>::new();
        for operation in &self.operations {
            validate_operation(operation)?;
            register_name(&mut names, &operation.canonical_id, &operation.canonical_id)?;
            for alias in &operation.aliases {
                register_name(&mut names, alias, &operation.canonical_id)?;
            }
        }
        for operation in &self.operations {
            if let Some(tool_name) = operation.mcp_tool_name.as_deref() {
                // The default primary MCP name is the canonical id. It is the
                // only intentional duplicate in the shared name namespace.
                if tool_name != operation.canonical_id {
                    register_name(&mut names, tool_name, &operation.canonical_id)?;
                }
            }
        }
        let mut resources = BTreeSet::new();
        for resource in &self.resources {
            validate_resource(resource)?;
            if !resources.insert(resource.uri.as_str()) {
                return Err(DispatchContractError::new(
                    DispatchErrorClass::InvalidRegistry,
                    format!("duplicate resource URI {:?}", resource.uri),
                ));
            }
        }
        Ok(())
    }

    /// Canonical manifest for this registry's dispatch contract.
    ///
    /// Operation declaration order, aliases, declared error classes, and the
    /// set-like parts of JSON Schema do not affect dispatch behavior. Sort and
    /// normalize those fields before hashing, while preserving order inside
    /// schema arrays whose order is semantically significant. Engine-specific
    /// semantic manifests remain responsible for their additional fields.
    pub fn canonical_manifest(&self) -> Result<Value, DispatchContractError> {
        self.validate()?;

        let mut registry = self.clone();
        for operation in &mut registry.operations {
            operation.aliases.sort();
            operation.args_schema = normalize_schema(&operation.args_schema);
            if let Some(output_schema) = &mut operation.output_schema {
                *output_schema = normalize_schema(output_schema);
            }
            operation.errors.sort_unstable();
        }
        registry
            .operations
            .sort_by(|left, right| left.canonical_id.cmp(&right.canonical_id));
        registry
            .resources
            .sort_by(|left, right| left.uri.cmp(&right.uri));

        serde_json::to_value(registry).map_err(|error| {
            DispatchContractError::new(
                DispatchErrorClass::InvalidRegistry,
                format!("failed to encode canonical registry: {error}"),
            )
        })
    }

    /// Raw SHA-256 digest of the canonical dispatch registry manifest.
    pub fn contract_digest(&self) -> Result<[u8; 32], DispatchContractError> {
        Ok(digest_manifest(&self.canonical_manifest()?))
    }

    /// Lowercase hexadecimal SHA-256 of the canonical dispatch registry manifest.
    pub fn contract_digest_hex(&self) -> Result<String, DispatchContractError> {
        Ok(digest_manifest_hex(&self.canonical_manifest()?))
    }

    pub fn resolve(
        &self,
        invoked_name: &str,
    ) -> Result<&CanonicalOperation, DispatchContractError> {
        self.operations
            .iter()
            .find(|operation| {
                operation.canonical_id == invoked_name
                    || operation.aliases.iter().any(|alias| alias == invoked_name)
            })
            .ok_or_else(|| {
                DispatchContractError::new(
                    DispatchErrorClass::UnknownOperation,
                    format!("unknown canonical operation or alias {invoked_name:?}"),
                )
            })
    }
}

fn classify_decode_error(error: serde_json::Error) -> DispatchContractError {
    let message = error.to_string();
    let class = if message.contains("missing field") {
        DispatchErrorClass::MissingMetadata
    } else if message.contains("unknown field") || message.contains("unknown variant") {
        DispatchErrorClass::UnknownMetadata
    } else {
        DispatchErrorClass::InvalidRegistry
    };
    DispatchContractError::new(class, message)
}

fn validate_operation(operation: &CanonicalOperation) -> Result<(), DispatchContractError> {
    validate_name(&operation.canonical_id, "canonical operation id")?;
    if let Some(tool_name) = operation.mcp_tool_name.as_deref() {
        validate_name(tool_name, "MCP tool name")?;
    }
    if let Some(output_schema) = &operation.output_schema
        && !output_schema.is_object()
    {
        return Err(DispatchContractError::new(
            DispatchErrorClass::InvalidRegistry,
            format!("{} output_schema must be an object", operation.canonical_id),
        ));
    }
    operation.effect_policy.validate()?;
    if !operation.args_schema.is_object() {
        return Err(DispatchContractError::new(
            DispatchErrorClass::InvalidRegistry,
            format!("{} args_schema must be an object", operation.canonical_id),
        ));
    }
    if operation.args_schema.get("type").is_none() {
        return Err(DispatchContractError::new(
            DispatchErrorClass::MissingMetadata,
            format!("{} args_schema is missing type", operation.canonical_id),
        ));
    }
    if operation.errors.is_empty() {
        return Err(DispatchContractError::new(
            DispatchErrorClass::MissingMetadata,
            format!("{} must declare error classes", operation.canonical_id),
        ));
    }
    let unique_errors: BTreeSet<_> = operation.errors.iter().copied().collect();
    if unique_errors.len() != operation.errors.len() {
        return Err(DispatchContractError::new(
            DispatchErrorClass::InvalidRegistry,
            format!(
                "{} declares duplicate error classes",
                operation.canonical_id
            ),
        ));
    }
    for alias in &operation.aliases {
        validate_name(alias, "operation alias")?;
    }
    Ok(())
}

fn validate_resource(resource: &CanonicalResource) -> Result<(), DispatchContractError> {
    validate_name(&resource.uri, "resource URI")?;
    validate_display_name(&resource.name, "resource name")?;
    Ok(())
}

fn validate_display_name(name: &str, label: &str) -> Result<(), DispatchContractError> {
    if name.is_empty() || name.trim() != name {
        return Err(DispatchContractError::new(
            DispatchErrorClass::InvalidRegistry,
            format!("invalid {label} {name:?}"),
        ));
    }
    Ok(())
}

fn validate_name(name: &str, label: &str) -> Result<(), DispatchContractError> {
    if name.is_empty() || name.trim() != name || name.chars().any(char::is_whitespace) {
        return Err(DispatchContractError::new(
            DispatchErrorClass::InvalidRegistry,
            format!("invalid {label} {name:?}"),
        ));
    }
    Ok(())
}

fn register_name<'a>(
    names: &mut BTreeMap<&'a str, &'a str>,
    name: &'a str,
    canonical_id: &'a str,
) -> Result<(), DispatchContractError> {
    if let Some(previous) = names.insert(name, canonical_id) {
        return Err(DispatchContractError::new(
            DispatchErrorClass::InvalidRegistry,
            format!("name {name:?} collides between {previous:?} and {canonical_id:?}"),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceForm {
    Canonical,
    DirectAlias,
    ComputedProperty,
    ObfuscatedComma,
}

/// Source text is retained only for diagnostics. Resolution uses invoked_name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceDiagnostic {
    pub form: SourceForm,
    pub source_text: String,
    pub invoked_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermitGrant {
    pub permit_id: String,
    pub canonical_operation_id: String,
    pub effect_class: EffectClass,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalGrant {
    pub approval_id: String,
    pub canonical_operation_id: String,
    pub effect_class: EffectClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchStage {
    Resolve,
    ValidateArguments,
    AuthorizeEffect,
    AcquireAuthority,
    Dispatch,
    Journal,
    Result,
    Complete,
    Failed,
}

/// Fail-closed ordered dispatch state machine. Any rejected action poisons the
/// machine, preventing a later transition from bypassing the failed stage.
#[derive(Debug, Clone)]
pub struct DispatchMachine {
    registry: CanonicalRegistry,
    stage: DispatchStage,
    canonical_id: Option<String>,
    diagnostic: Option<SourceDiagnostic>,
}

impl DispatchMachine {
    pub fn new(registry: CanonicalRegistry) -> Result<Self, DispatchContractError> {
        registry.validate()?;
        Ok(Self {
            registry,
            stage: DispatchStage::Resolve,
            canonical_id: None,
            diagnostic: None,
        })
    }

    pub fn stage(&self) -> DispatchStage {
        self.stage
    }

    pub fn operation(&self) -> Option<&CanonicalOperation> {
        self.canonical_id
            .as_deref()
            .and_then(|canonical_id| self.registry.resolve(canonical_id).ok())
    }

    pub fn diagnostic(&self) -> Option<&SourceDiagnostic> {
        self.diagnostic.as_ref()
    }

    pub fn resolve(
        &mut self,
        invoked_name: &str,
    ) -> Result<&CanonicalOperation, DispatchContractError> {
        self.resolve_with_diagnostic(SourceDiagnostic {
            form: SourceForm::Canonical,
            source_text: invoked_name.to_owned(),
            invoked_name: invoked_name.to_owned(),
        })
    }

    pub fn resolve_with_diagnostic(
        &mut self,
        diagnostic: SourceDiagnostic,
    ) -> Result<&CanonicalOperation, DispatchContractError> {
        self.require_stage(DispatchStage::Resolve)?;
        let canonical_id = match self.registry.resolve(&diagnostic.invoked_name) {
            Ok(operation) => operation.canonical_id.clone(),
            Err(error) => return Err(self.poison(error)),
        };
        self.canonical_id = Some(canonical_id);
        self.diagnostic = Some(diagnostic);
        self.stage = DispatchStage::ValidateArguments;
        if self.operation().is_none() {
            return Err(self.poison(DispatchContractError::new(
                DispatchErrorClass::InvalidStageTransition,
                "resolved operation is missing from the registry",
            )));
        }
        self.required_operation()
    }

    pub fn validate_arguments(&mut self, args: &Value) -> Result<(), DispatchContractError> {
        self.require_stage(DispatchStage::ValidateArguments)?;
        let schema = match self.required_operation() {
            Ok(operation) => operation.args_schema.clone(),
            Err(error) => return Err(self.poison(error)),
        };
        if let Err(message) = validate_schema_value(&schema, args, "$") {
            return Err(self.poison(DispatchContractError::new(
                DispatchErrorClass::InvalidArguments,
                message,
            )));
        }
        self.stage = DispatchStage::AuthorizeEffect;
        Ok(())
    }

    pub fn authorize_effect(&mut self, grant: EffectGrant) -> Result<(), DispatchContractError> {
        self.require_stage(DispatchStage::AuthorizeEffect)?;
        let required = match self.required_operation() {
            Ok(operation) => operation.effect_policy.effect_class,
            Err(error) => return Err(self.poison(error)),
        };
        if grant.effect_class() != required {
            return Err(self.poison(DispatchContractError::new(
                DispatchErrorClass::UnauthorizedEffect,
                format!(
                    "grant {:?} does not authorize {:?}",
                    grant.effect_class(),
                    required
                ),
            )));
        }
        self.stage = DispatchStage::AcquireAuthority;
        Ok(())
    }

    pub fn acquire_authority(
        &mut self,
        permit: Option<PermitGrant>,
        approval: Option<ApprovalGrant>,
    ) -> Result<(), DispatchContractError> {
        self.require_stage(DispatchStage::AcquireAuthority)?;
        let (canonical_operation_id, policy) = match self.required_operation() {
            Ok(operation) => (operation.canonical_id.clone(), operation.effect_policy),
            Err(error) => return Err(self.poison(error)),
        };

        if let Err(error) =
            validate_permit_authority(policy, &canonical_operation_id, permit.as_ref())
        {
            return Err(self.poison(error));
        }
        if let Err(error) =
            validate_approval_authority(policy, &canonical_operation_id, approval.as_ref())
        {
            return Err(self.poison(error));
        }
        self.stage = DispatchStage::Dispatch;
        Ok(())
    }

    pub fn dispatched(&mut self) -> Result<(), DispatchContractError> {
        self.require_stage(DispatchStage::Dispatch)?;
        self.stage = DispatchStage::Journal;
        Ok(())
    }

    pub fn journaled(&mut self) -> Result<(), DispatchContractError> {
        self.require_stage(DispatchStage::Journal)?;
        self.stage = DispatchStage::Result;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), DispatchContractError> {
        self.require_stage(DispatchStage::Result)?;
        self.stage = DispatchStage::Complete;
        Ok(())
    }

    fn required_operation(&self) -> Result<&CanonicalOperation, DispatchContractError> {
        self.operation().ok_or_else(|| {
            DispatchContractError::new(
                DispatchErrorClass::InvalidStageTransition,
                "operation missing after successful stage transition",
            )
        })
    }

    fn require_stage(&mut self, expected: DispatchStage) -> Result<(), DispatchContractError> {
        if self.stage == expected {
            return Ok(());
        }
        let actual = self.stage;
        Err(self.poison(DispatchContractError::new(
            DispatchErrorClass::InvalidStageTransition,
            format!("expected stage {expected:?}, found {actual:?}"),
        )))
    }

    fn poison(&mut self, error: DispatchContractError) -> DispatchContractError {
        self.stage = DispatchStage::Failed;
        error
    }
}

fn validate_permit_authority(
    policy: EffectPolicy,
    canonical_operation_id: &str,
    permit: Option<&PermitGrant>,
) -> Result<(), DispatchContractError> {
    match policy.permit {
        PermitRequirement::Required => {
            let permit = require_non_empty_permit(permit)?;
            validate_permit_binding(permit, canonical_operation_id, policy.effect_class)
        }
        PermitRequirement::NotRequired => reject_unexpected_permit(permit),
    }
}

fn require_non_empty_permit(
    permit: Option<&PermitGrant>,
) -> Result<&PermitGrant, DispatchContractError> {
    let Some(permit) = permit else {
        return Err(DispatchContractError::new(
            DispatchErrorClass::PermitRequired,
            "a non-empty permit is required",
        ));
    };
    if permit.permit_id.trim().is_empty() {
        return Err(DispatchContractError::new(
            DispatchErrorClass::PermitRequired,
            "a non-empty permit is required",
        ));
    }
    Ok(permit)
}

fn validate_permit_binding(
    permit: &PermitGrant,
    canonical_operation_id: &str,
    effect_class: EffectClass,
) -> Result<(), DispatchContractError> {
    if permit.canonical_operation_id != canonical_operation_id {
        return Err(invalid_permit_binding(canonical_operation_id, effect_class));
    }
    if permit.effect_class != effect_class {
        return Err(invalid_permit_binding(canonical_operation_id, effect_class));
    }
    Ok(())
}

fn invalid_permit_binding(
    canonical_operation_id: &str,
    effect_class: EffectClass,
) -> DispatchContractError {
    DispatchContractError::new(
        DispatchErrorClass::PermitRequired,
        format!(
            "permit is not bound to operation {:?} and effect {:?}",
            canonical_operation_id, effect_class
        ),
    )
}

fn reject_unexpected_permit(permit: Option<&PermitGrant>) -> Result<(), DispatchContractError> {
    if permit.is_some() {
        return Err(DispatchContractError::new(
            DispatchErrorClass::InvalidStageTransition,
            "permit supplied for an operation that does not accept one",
        ));
    }
    Ok(())
}

fn validate_approval_authority(
    policy: EffectPolicy,
    canonical_operation_id: &str,
    approval: Option<&ApprovalGrant>,
) -> Result<(), DispatchContractError> {
    match policy.approval {
        ApprovalRequirement::Required => {
            let approval = require_non_empty_approval(approval)?;
            validate_approval_binding(approval, canonical_operation_id, policy.effect_class)
        }
        ApprovalRequirement::NotRequired => reject_unexpected_approval(approval),
    }
}

fn require_non_empty_approval(
    approval: Option<&ApprovalGrant>,
) -> Result<&ApprovalGrant, DispatchContractError> {
    let Some(approval) = approval else {
        return Err(DispatchContractError::new(
            DispatchErrorClass::ApprovalRequired,
            "a non-empty approval is required",
        ));
    };
    if approval.approval_id.trim().is_empty() {
        return Err(DispatchContractError::new(
            DispatchErrorClass::ApprovalRequired,
            "a non-empty approval is required",
        ));
    }
    Ok(approval)
}

fn validate_approval_binding(
    approval: &ApprovalGrant,
    canonical_operation_id: &str,
    effect_class: EffectClass,
) -> Result<(), DispatchContractError> {
    if approval.canonical_operation_id != canonical_operation_id {
        return Err(invalid_approval_binding(
            canonical_operation_id,
            effect_class,
        ));
    }
    if approval.effect_class != effect_class {
        return Err(invalid_approval_binding(
            canonical_operation_id,
            effect_class,
        ));
    }
    Ok(())
}

fn invalid_approval_binding(
    canonical_operation_id: &str,
    effect_class: EffectClass,
) -> DispatchContractError {
    DispatchContractError::new(
        DispatchErrorClass::ApprovalRequired,
        format!(
            "approval is not bound to operation {:?} and effect {:?}",
            canonical_operation_id, effect_class
        ),
    )
}

fn reject_unexpected_approval(
    approval: Option<&ApprovalGrant>,
) -> Result<(), DispatchContractError> {
    if approval.is_some() {
        return Err(DispatchContractError::new(
            DispatchErrorClass::InvalidStageTransition,
            "approval supplied for an operation that does not accept one",
        ));
    }
    Ok(())
}

fn validate_schema_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let schema = schema
        .as_object()
        .ok_or_else(|| format!("unsupported non-object schema at {path}"))?;
    validate_supported_schema_keywords(schema, path)?;
    validate_enum_constraint(schema, value, path)?;
    let expected = required_schema_type(schema, path)?;
    validate_schema_type(expected, value, path)?;
    validate_typed_constraints(schema, value, expected, path)
}

fn validate_supported_schema_keywords(
    schema: &serde_json::Map<String, Value>,
    path: &str,
) -> Result<(), String> {
    const SUPPORTED: &[&str] = &[
        "type",
        "properties",
        "required",
        "additionalProperties",
        "items",
        "enum",
        "description",
        "title",
        "$schema",
    ];
    if let Some(keyword) = schema.keys().find(|key| !SUPPORTED.contains(&key.as_str())) {
        return Err(format!("unsupported schema keyword {keyword:?} at {path}"));
    }
    Ok(())
}

fn validate_enum_constraint(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
    path: &str,
) -> Result<(), String> {
    let Some(allowed) = schema.get("enum") else {
        return Ok(());
    };
    let allowed = allowed
        .as_array()
        .ok_or_else(|| format!("enum must be an array at {path}"))?;
    if !allowed.contains(value) {
        return Err(format!("value at {path} is not in the declared enum"));
    }
    Ok(())
}

fn required_schema_type<'a>(
    schema: &'a serde_json::Map<String, Value>,
    path: &str,
) -> Result<&'a str, String> {
    schema
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("schema type is required at {path}"))
}

fn validate_schema_type(expected: &str, value: &Value, path: &str) -> Result<(), String> {
    let type_matches = match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value
            .as_number()
            .is_some_and(|number| number.is_i64() || number.is_u64()),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        other => return Err(format!("unsupported schema type {other:?} at {path}")),
    };
    if !type_matches {
        return Err(format!("expected {expected} at {path}"));
    }
    Ok(())
}

fn validate_typed_constraints(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
    expected: &str,
    path: &str,
) -> Result<(), String> {
    match expected {
        "object" => validate_object_constraints(schema, value, path),
        "array" => validate_array_constraints(schema, value, path),
        "string" | "number" | "integer" => validate_scalar_constraints(),
        "boolean" | "null" => Ok(()),
        other => Err(format!("unsupported schema type {other:?} at {path}")),
    }
}

fn validate_scalar_constraints() -> Result<(), String> {
    // String and number schemas currently support only type and enum constraints.
    Ok(())
}

fn validate_object_constraints(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
    path: &str,
) -> Result<(), String> {
    let Some(value) = value.as_object() else {
        return Err(format!("expected object at {path}"));
    };
    let properties = schema_properties(schema, path)?;
    validate_required_properties(schema, value, path)?;
    for (key, child) in value {
        validate_object_property(schema, properties, key, child, path)?;
    }
    Ok(())
}

fn schema_properties<'a>(
    schema: &'a serde_json::Map<String, Value>,
    path: &str,
) -> Result<Option<&'a serde_json::Map<String, Value>>, String> {
    schema
        .get("properties")
        .map(|properties| {
            properties
                .as_object()
                .ok_or_else(|| format!("properties must be an object at {path}"))
        })
        .transpose()
}

fn validate_required_properties(
    schema: &serde_json::Map<String, Value>,
    value: &serde_json::Map<String, Value>,
    path: &str,
) -> Result<(), String> {
    let Some(required) = schema.get("required") else {
        return Ok(());
    };
    let required = required
        .as_array()
        .ok_or_else(|| format!("required must be an array at {path}"))?;
    for key in required {
        let key = key
            .as_str()
            .ok_or_else(|| format!("required entries must be strings at {path}"))?;
        if !value.contains_key(key) {
            return Err(format!("missing required argument {path}.{key}"));
        }
    }
    Ok(())
}

fn validate_object_property(
    schema: &serde_json::Map<String, Value>,
    properties: Option<&serde_json::Map<String, Value>>,
    key: &str,
    child: &Value,
    path: &str,
) -> Result<(), String> {
    if let Some(child_schema) = properties.and_then(|properties| properties.get(key)) {
        return validate_schema_value(child_schema, child, &format!("{path}.{key}"));
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        return Err(format!("unknown argument {path}.{key}"));
    }
    Ok(())
}

fn validate_array_constraints(
    schema: &serde_json::Map<String, Value>,
    value: &Value,
    path: &str,
) -> Result<(), String> {
    let Some(item_schema) = schema.get("items") else {
        return Ok(());
    };
    let Some(items) = value.as_array() else {
        return Err(format!("expected array at {path}"));
    };
    for (index, item) in items.iter().enumerate() {
        validate_schema_value(item_schema, item, &format!("{path}[{index}]"))?;
    }
    Ok(())
}

