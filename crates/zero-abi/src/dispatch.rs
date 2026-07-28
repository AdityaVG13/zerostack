//! Shared canonical operation dispatch metadata and fail-closed enforcement.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::EffectClass;

pub const CANONICAL_DISPATCH_VERSION: &str = "zerostack.canonical_dispatch.v1";

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
    pub aliases: Vec<String>,
    pub args_schema: Value,
    pub effect_policy: EffectPolicy,
    pub errors: Vec<DispatchErrorClass>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalRegistry {
    pub version: String,
    pub engine: RegistryEngine,
    pub operations: Vec<CanonicalOperation>,
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
        Ok(())
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
        Ok(self
            .operation()
            .expect("resolved operation remains in registry"))
    }

    pub fn validate_arguments(&mut self, args: &Value) -> Result<(), DispatchContractError> {
        self.require_stage(DispatchStage::ValidateArguments)?;
        let schema = self
            .operation()
            .expect("validate stage always has an operation")
            .args_schema
            .clone();
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
        let required = self
            .operation()
            .expect("authorize stage always has an operation")
            .effect_policy
            .effect_class;
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
        let operation = self
            .operation()
            .expect("acquisition stage always has an operation");
        let canonical_operation_id = operation.canonical_id.clone();
        let policy = operation.effect_policy;

        if matches!(policy.permit, PermitRequirement::Required) {
            let permit = match permit.as_ref() {
                Some(grant) if !grant.permit_id.trim().is_empty() => grant,
                _ => {
                    return Err(self.poison(DispatchContractError::new(
                        DispatchErrorClass::PermitRequired,
                        "a non-empty permit is required",
                    )))
                }
            };
            if permit.canonical_operation_id != canonical_operation_id
                || permit.effect_class != policy.effect_class
            {
                return Err(self.poison(DispatchContractError::new(
                    DispatchErrorClass::PermitRequired,
                    format!(
                        "permit is not bound to operation {:?} and effect {:?}",
                        canonical_operation_id, policy.effect_class
                    ),
                )));
            }
        } else if permit.is_some() {
            return Err(self.poison(DispatchContractError::new(
                DispatchErrorClass::InvalidStageTransition,
                "permit supplied for an operation that does not accept one",
            )));
        }

        if matches!(policy.approval, ApprovalRequirement::Required) {
            let approval = match approval.as_ref() {
                Some(grant) if !grant.approval_id.trim().is_empty() => grant,
                _ => {
                    return Err(self.poison(DispatchContractError::new(
                        DispatchErrorClass::ApprovalRequired,
                        "a non-empty approval is required",
                    )))
                }
            };
            if approval.canonical_operation_id != canonical_operation_id
                || approval.effect_class != policy.effect_class
            {
                return Err(self.poison(DispatchContractError::new(
                    DispatchErrorClass::ApprovalRequired,
                    format!(
                        "approval is not bound to operation {:?} and effect {:?}",
                        canonical_operation_id, policy.effect_class
                    ),
                )));
            }
        } else if approval.is_some() {
            return Err(self.poison(DispatchContractError::new(
                DispatchErrorClass::InvalidStageTransition,
                "approval supplied for an operation that does not accept one",
            )));
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

fn validate_schema_value(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or_else(|| format!("unsupported non-object schema at {path}"))?;
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
    if let Some(keyword) = object.keys().find(|key| !SUPPORTED.contains(&key.as_str())) {
        return Err(format!("unsupported schema keyword {keyword:?} at {path}"));
    }

    if let Some(allowed) = object.get("enum") {
        let allowed = allowed
            .as_array()
            .ok_or_else(|| format!("enum must be an array at {path}"))?;
        if !allowed.contains(value) {
            return Err(format!("value at {path} is not in the declared enum"));
        }
    }

    let expected = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("schema type is required at {path}"))?;
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

    if expected == "object" {
        let value_object = value.as_object().expect("object type was checked");
        let properties = object
            .get("properties")
            .map(|properties| {
                properties
                    .as_object()
                    .ok_or_else(|| format!("properties must be an object at {path}"))
            })
            .transpose()?
            .cloned()
            .unwrap_or_default();
        if let Some(required) = object.get("required") {
            let required = required
                .as_array()
                .ok_or_else(|| format!("required must be an array at {path}"))?;
            for key in required {
                let key = key
                    .as_str()
                    .ok_or_else(|| format!("required entries must be strings at {path}"))?;
                if !value_object.contains_key(key) {
                    return Err(format!("missing required argument {path}.{key}"));
                }
            }
        }
        for (key, child) in value_object {
            if let Some(child_schema) = properties.get(key) {
                validate_schema_value(child_schema, child, &format!("{path}.{key}"))?;
            } else if object.get("additionalProperties") == Some(&Value::Bool(false)) {
                return Err(format!("unknown argument {path}.{key}"));
            }
        }
    }

    if expected == "array" {
        if let Some(item_schema) = object.get("items") {
            for (index, item) in value
                .as_array()
                .expect("array type was checked")
                .iter()
                .enumerate()
            {
                validate_schema_value(item_schema, item, &format!("{path}[{index}]"))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn operation(canonical_id: &str, alias: &str) -> CanonicalOperation {
        CanonicalOperation {
            canonical_id: canonical_id.to_owned(),
            aliases: vec![alias.to_owned()],
            args_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            effect_policy: EffectPolicy {
                effect_class: EffectClass::ReadOnly,
                permit: PermitRequirement::NotRequired,
                approval: ApprovalRequirement::NotRequired,
            },
            errors: vec![DispatchErrorClass::InvalidStageTransition],
        }
    }

    #[test]
    fn registry_rejects_alias_collisions() {
        let registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![
                operation("fs.first", "shared"),
                operation("fs.second", "shared"),
            ],
        };
        assert_eq!(
            registry.validate().unwrap_err().class,
            DispatchErrorClass::InvalidRegistry
        );
    }

    #[test]
    fn out_of_order_transition_poison_is_permanent() {
        let registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::FsZero,
            operations: vec![operation("fs.read", "read")],
        };
        let mut machine = DispatchMachine::new(registry).unwrap();
        assert_eq!(
            machine.dispatched().unwrap_err().class,
            DispatchErrorClass::InvalidStageTransition
        );
        assert_eq!(machine.stage(), DispatchStage::Failed);
        assert!(machine.resolve("read").is_err());
        assert_eq!(machine.stage(), DispatchStage::Failed);
    }

    fn authority_machine() -> DispatchMachine {
        let mut operation = operation("fixture.operation", "fixture_alias");
        operation.effect_policy = EffectPolicy {
            effect_class: EffectClass::Irreversible,
            permit: PermitRequirement::Required,
            approval: ApprovalRequirement::Required,
        };
        let registry = CanonicalRegistry {
            version: CANONICAL_DISPATCH_VERSION.to_owned(),
            engine: RegistryEngine::TokenZero,
            operations: vec![operation],
        };
        let mut machine = DispatchMachine::new(registry).unwrap();
        machine.resolve("fixture_alias").unwrap();
        machine.validate_arguments(&json!({})).unwrap();
        machine.authorize_effect(EffectGrant::Irreversible).unwrap();
        machine
    }

    fn valid_permit() -> PermitGrant {
        PermitGrant {
            permit_id: "permit-valid".to_owned(),
            canonical_operation_id: "fixture.operation".to_owned(),
            effect_class: EffectClass::Irreversible,
        }
    }

    fn valid_approval() -> ApprovalGrant {
        ApprovalGrant {
            approval_id: "approval-valid".to_owned(),
            canonical_operation_id: "fixture.operation".to_owned(),
            effect_class: EffectClass::Irreversible,
        }
    }

    #[test]
    fn authority_grants_reject_wrong_operation_and_effect_bindings() {
        let mut wrong_operation = authority_machine();
        let mut permit = valid_permit();
        permit.canonical_operation_id = "other.operation".to_owned();
        assert_eq!(
            wrong_operation
                .acquire_authority(Some(permit), Some(valid_approval()))
                .unwrap_err()
                .class,
            DispatchErrorClass::PermitRequired
        );
        assert_eq!(wrong_operation.stage(), DispatchStage::Failed);

        let mut wrong_effect = authority_machine();
        let mut approval = valid_approval();
        approval.effect_class = EffectClass::ReadOnly;
        assert_eq!(
            wrong_effect
                .acquire_authority(Some(valid_permit()), Some(approval))
                .unwrap_err()
                .class,
            DispatchErrorClass::ApprovalRequired
        );
        assert_eq!(wrong_effect.stage(), DispatchStage::Failed);
    }
}
