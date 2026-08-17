//! Canonical cache-entry schema shared by GraphZero witness caches and FSZero memoization.
//!
//! Normative prose: conformance/contracts/cache-entry.md.
//!
//! A cache hit can only be built from a key carrying a completeness witness.
//! Constructors and deserializers validate that witness before accepting an
//! entry, making the unsound direction (under-invalidation) fail closed.

use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{canonical_json, sha256_hex};

/// Wire schema identifier for this shared cache-entry contract.
pub const CACHE_ENTRY_SCHEMA: &str = "cache-entry/v1";

/// A non-empty content-addressed root used by a cache key or value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CacheRoot(String);

impl CacheRoot {
    /// Construct a root. Empty roots are never valid cache evidence.
    pub fn new(root: impl Into<String>) -> Result<Self, CacheEntryError> {
        let root = root.into();
        if root.is_empty() {
            return Err(CacheEntryError::EmptyField("cache root"));
        }
        Ok(Self(root))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Operator identity is versioned independently of its parameter payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorIdentity {
    id: String,
    version: String,
}

impl OperatorIdentity {
    pub fn new(id: impl Into<String>, version: impl Into<String>) -> Result<Self, CacheEntryError> {
        let id = id.into();
        let version = version.into();
        require_nonempty("operator id", &id)?;
        require_nonempty("operator version", &version)?;
        Ok(Self { id, version })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

/// Durable proof for the dependency cone examined by the operator.
/// checked_roots covers every root whose absence makes a negative result
/// complete; proof_root identifies the durable witness itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheCompletenessWitness {
    proof_root: CacheRoot,
    checked_roots: Vec<CacheRoot>,
}

impl CacheCompletenessWitness {
    pub fn new(
        proof_root: CacheRoot,
        checked_roots: Vec<CacheRoot>,
    ) -> Result<Self, CacheEntryError> {
        let mut witness = Self {
            proof_root,
            checked_roots,
        };
        witness.normalize_and_validate()?;
        Ok(witness)
    }

    pub fn proof_root(&self) -> &CacheRoot {
        &self.proof_root
    }

    pub fn checked_roots(&self) -> &[CacheRoot] {
        &self.checked_roots
    }

    fn normalize_and_validate(&mut self) -> Result<(), CacheEntryError> {
        validate_root("completeness proof root", &self.proof_root)?;
        normalize_roots(&mut self.checked_roots);
        validate_roots("checked dependency root", &self.checked_roots)
    }
}

/// Optional durable receipt from an independent verifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifierReceipt {
    verifier: String,
    receipt_root: CacheRoot,
}

impl VerifierReceipt {
    pub fn new(
        verifier: impl Into<String>,
        receipt_root: CacheRoot,
    ) -> Result<Self, CacheEntryError> {
        let verifier = verifier.into();
        require_nonempty("verifier", &verifier)?;
        validate_root("verifier receipt root", &receipt_root)?;
        Ok(Self {
            verifier,
            receipt_root,
        })
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    pub fn receipt_root(&self) -> &CacheRoot {
        &self.receipt_root
    }
}

/// Complete cache key. scope_roots is empty for a positive hit and non-empty
/// for a negative/no-matches entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheKey {
    operator: OperatorIdentity,
    canonical_parameters: Value,
    minimum_dependency_roots: Vec<CacheRoot>,
    environment_roots: Vec<CacheRoot>,
    toolchain_roots: Vec<CacheRoot>,
    completeness_witness: CacheCompletenessWitness,
    #[serde(default)]
    scope_roots: Vec<CacheRoot>,
}

#[derive(Debug, Deserialize)]
struct CacheKeyWire {
    operator: OperatorIdentity,
    canonical_parameters: Value,
    minimum_dependency_roots: Vec<CacheRoot>,
    environment_roots: Vec<CacheRoot>,
    toolchain_roots: Vec<CacheRoot>,
    completeness_witness: CacheCompletenessWitness,
    #[serde(default)]
    scope_roots: Vec<CacheRoot>,
}

impl<'de> Deserialize<'de> for CacheKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CacheKeyWire::deserialize(deserializer)?;
        Self::build(
            wire.operator,
            wire.canonical_parameters,
            wire.minimum_dependency_roots,
            wire.environment_roots,
            wire.toolchain_roots,
            wire.completeness_witness,
            wire.scope_roots,
        )
        .map_err(D::Error::custom)
    }
}

impl CacheKey {
    /// Construct a positive-result key with no anti-dependency scope roots.
    pub fn new(
        operator: OperatorIdentity,
        canonical_parameters: Value,
        minimum_dependency_roots: Vec<CacheRoot>,
        environment_roots: Vec<CacheRoot>,
        toolchain_roots: Vec<CacheRoot>,
        completeness_witness: CacheCompletenessWitness,
    ) -> Result<Self, CacheEntryError> {
        Self::build(
            operator,
            canonical_parameters,
            minimum_dependency_roots,
            environment_roots,
            toolchain_roots,
            completeness_witness,
            Vec::new(),
        )
    }

    /// Construct a negative-result key. Scope roots are anti-dependencies: any
    /// change to one invalidates the no-matches answer.
    pub fn with_scope_roots(
        operator: OperatorIdentity,
        canonical_parameters: Value,
        minimum_dependency_roots: Vec<CacheRoot>,
        environment_roots: Vec<CacheRoot>,
        toolchain_roots: Vec<CacheRoot>,
        completeness_witness: CacheCompletenessWitness,
        scope_roots: Vec<CacheRoot>,
    ) -> Result<Self, CacheEntryError> {
        if scope_roots.is_empty() {
            return Err(CacheEntryError::EmptyField("negative scope roots"));
        }
        Self::build(
            operator,
            canonical_parameters,
            minimum_dependency_roots,
            environment_roots,
            toolchain_roots,
            completeness_witness,
            scope_roots,
        )
    }

    fn build(
        operator: OperatorIdentity,
        canonical_parameters: Value,
        mut minimum_dependency_roots: Vec<CacheRoot>,
        mut environment_roots: Vec<CacheRoot>,
        mut toolchain_roots: Vec<CacheRoot>,
        mut completeness_witness: CacheCompletenessWitness,
        mut scope_roots: Vec<CacheRoot>,
    ) -> Result<Self, CacheEntryError> {
        validate_operator(&operator)?;
        normalize_roots(&mut minimum_dependency_roots);
        normalize_roots(&mut environment_roots);
        normalize_roots(&mut toolchain_roots);
        normalize_roots(&mut scope_roots);
        validate_roots("minimum dependency root", &minimum_dependency_roots)?;
        validate_roots("environment root", &environment_roots)?;
        validate_roots("toolchain root", &toolchain_roots)?;
        validate_roots("scope root", &scope_roots)?;
        completeness_witness.normalize_and_validate()?;
        for root in minimum_dependency_roots
            .iter()
            .chain(environment_roots.iter())
            .chain(toolchain_roots.iter())
            .chain(scope_roots.iter())
        {
            if !completeness_witness.checked_roots.contains(root) {
                return Err(CacheEntryError::RootNotWitnessed(root.as_str().to_owned()));
            }
        }
        Ok(Self {
            operator,
            canonical_parameters,
            minimum_dependency_roots,
            environment_roots,
            toolchain_roots,
            completeness_witness,
            scope_roots,
        })
    }

    pub fn operator(&self) -> &OperatorIdentity {
        &self.operator
    }

    pub fn canonical_parameters(&self) -> &Value {
        &self.canonical_parameters
    }

    pub fn minimum_dependency_roots(&self) -> &[CacheRoot] {
        &self.minimum_dependency_roots
    }

    pub fn environment_roots(&self) -> &[CacheRoot] {
        &self.environment_roots
    }

    pub fn toolchain_roots(&self) -> &[CacheRoot] {
        &self.toolchain_roots
    }

    pub fn completeness_witness(&self) -> &CacheCompletenessWitness {
        &self.completeness_witness
    }

    pub fn scope_roots(&self) -> &[CacheRoot] {
        &self.scope_roots
    }

    /// Stable key bytes: sorted JSON object keys plus normalized root sets.
    pub fn canonical_key_json(&self) -> String {
        let value = serde_json::to_value(self).expect("cache key is JSON-serializable");
        canonical_json(&value)
    }

    /// SHA-256 of canonical_key_json.
    pub fn key_hash_hex(&self) -> String {
        sha256_hex(self.canonical_key_json().as_bytes())
    }

    fn validate(&self) -> Result<(), CacheEntryError> {
        Self::build(
            self.operator.clone(),
            self.canonical_parameters.clone(),
            self.minimum_dependency_roots.clone(),
            self.environment_roots.clone(),
            self.toolchain_roots.clone(),
            self.completeness_witness.clone(),
            self.scope_roots.clone(),
        )
        .map(|_| ())
    }
}

/// Cache value: either an output root or a certified no-matches answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CacheValue {
    Hit {
        output_root: CacheRoot,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verifier_receipt: Option<VerifierReceipt>,
    },
    NoMatches,
}

/// One validated cache entry shared by GraphZero and FSZero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheEntry {
    schema: String,
    key: CacheKey,
    value: CacheValue,
}

#[derive(Debug, Deserialize)]
struct CacheEntryWire {
    schema: String,
    key: CacheKey,
    value: CacheValue,
}

impl<'de> Deserialize<'de> for CacheEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CacheEntryWire::deserialize(deserializer)?;
        let entry = Self {
            schema: wire.schema,
            key: wire.key,
            value: wire.value,
        };
        entry.validate().map_err(D::Error::custom)?;
        Ok(entry)
    }
}

impl CacheEntry {
    /// Build a positive hit. A scope-bearing key is reserved for no-matches.
    pub fn positive(
        key: CacheKey,
        output_root: CacheRoot,
        verifier_receipt: Option<VerifierReceipt>,
    ) -> Result<Self, CacheEntryError> {
        let entry = Self {
            schema: CACHE_ENTRY_SCHEMA.to_owned(),
            key,
            value: CacheValue::Hit {
                output_root,
                verifier_receipt,
            },
        };
        entry.validate()?;
        Ok(entry)
    }

    /// Build a certified negative/no-matches entry. Its key must carry scope
    /// roots covered by the completeness witness.
    pub fn negative(key: CacheKey) -> Result<Self, CacheEntryError> {
        let entry = Self {
            schema: CACHE_ENTRY_SCHEMA.to_owned(),
            key,
            value: CacheValue::NoMatches,
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn key(&self) -> &CacheKey {
        &self.key
    }

    pub fn value(&self) -> &CacheValue {
        &self.value
    }

    pub fn key_hash_hex(&self) -> String {
        self.key.key_hash_hex()
    }

    fn validate(&self) -> Result<(), CacheEntryError> {
        if self.schema != CACHE_ENTRY_SCHEMA {
            return Err(CacheEntryError::UnsupportedSchema(self.schema.clone()));
        }
        self.key.validate()?;
        match &self.value {
            CacheValue::Hit {
                output_root,
                verifier_receipt,
            } => {
                if !self.key.scope_roots.is_empty() {
                    return Err(CacheEntryError::UnexpectedScopeRoots);
                }
                validate_root("output root", output_root)?;
                if let Some(receipt) = verifier_receipt {
                    require_nonempty("verifier", &receipt.verifier)?;
                    validate_root("verifier receipt root", &receipt.receipt_root)?;
                }
            }
            CacheValue::NoMatches => {
                if self.key.scope_roots.is_empty() {
                    return Err(CacheEntryError::MissingScopeRoots);
                }
            }
        }
        Ok(())
    }
}

/// Validation failures are stable enough for conformance consumers to classify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheEntryError {
    EmptyField(&'static str),
    InvalidRoot(String),
    RootNotWitnessed(String),
    MissingScopeRoots,
    UnexpectedScopeRoots,
    UnsupportedSchema(String),
}

impl std::fmt::Display for CacheEntryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyField(field) => write!(f, "{field} must not be empty"),
            Self::InvalidRoot(field) => write!(f, "{field} is invalid"),
            Self::RootNotWitnessed(root) => {
                write!(f, "root is not covered by completeness witness: {root}")
            }
            Self::MissingScopeRoots => write!(f, "negative entry requires scope roots"),
            Self::UnexpectedScopeRoots => write!(f, "positive entry cannot carry scope roots"),
            Self::UnsupportedSchema(schema) => write!(f, "unsupported cache schema: {schema}"),
        }
    }
}

impl std::error::Error for CacheEntryError {}

fn require_nonempty(field: &'static str, value: &str) -> Result<(), CacheEntryError> {
    if value.is_empty() {
        Err(CacheEntryError::EmptyField(field))
    } else {
        Ok(())
    }
}

fn validate_root(field: &str, root: &CacheRoot) -> Result<(), CacheEntryError> {
    if root.0.is_empty() {
        Err(CacheEntryError::InvalidRoot(field.to_owned()))
    } else {
        Ok(())
    }
}

fn validate_roots(field: &str, roots: &[CacheRoot]) -> Result<(), CacheEntryError> {
    for root in roots {
        validate_root(field, root)?;
    }
    Ok(())
}

fn validate_operator(operator: &OperatorIdentity) -> Result<(), CacheEntryError> {
    require_nonempty("operator id", &operator.id)?;
    require_nonempty("operator version", &operator.version)
}

fn normalize_roots(roots: &mut Vec<CacheRoot>) {
    roots.sort();
    roots.dedup();
}

