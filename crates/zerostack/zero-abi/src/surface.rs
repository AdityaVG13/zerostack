//! Direct ZeroKernel capability registration. Domain engines register typed capabilities under the
//! single `z` global. Transport catalogs and engine-local model-facing surfaces are outside this
//! contract.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

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
    pub fn z(capabilities: Vec<CapabilityDescriptor>) -> Self {
        Self {
            root: "z".to_owned(),
            capabilities,
        }
    }

    pub fn validate(&self) -> Result<(), RegistrationError> {
        validate_identifier(&self.root)
            .map_err(|_| RegistrationError::InvalidGlobal(self.root.clone()))?;
        if self.root != "z" {
            return Err(RegistrationError::InvalidGlobal(self.root.clone()));
        }

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistrationError {
    InvalidGlobal(String),
    InvalidCapability(CapabilityDescriptor),
    DuplicateCapability(CapabilityDescriptor),
}

impl fmt::Display for RegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidGlobal(name) => write!(formatter, "invalid global name: {name}"),
            Self::InvalidCapability(capability) => write!(
                formatter,
                "invalid capability: {}.{}",
                capability.surface, capability.method
            ),
            Self::DuplicateCapability(capability) => write!(
                formatter,
                "duplicate capability: {}.{}",
                capability.surface, capability.method
            ),
        }
    }
}

impl std::error::Error for RegistrationError {}
