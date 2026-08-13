//! Canonical cache-entry schema shared by GraphZero witness caches and FSZero memoization.
//!
//! Normative prose: conformance/contracts/cache-entry-v1.md.
//!
//! A cache hit can only be built from a key carrying a completeness witness.
//! Constructors and deserializers validate that witness before accepting an
//! entry, making the unsound direction (under-invalidation) fail closed.

use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{canonical_json, sha256_hex};

/// Wire schema identifier for this shared cache-entry contract.
pub const CACHE_ENTRY_SCHEMA_V1: &str = "cache-entry/v1";

/// A non-empty content-addressed root used by a cache key or value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CacheRootV1(String);

impl CacheRootV1 {
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
pub struct OperatorIdentityV1 {
    id: String,
    version: String,
}

impl OperatorIdentityV1 {
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
pub struct CompletenessWitnessV1 {
    proof_root: CacheRootV1,
    checked_roots: Vec<CacheRootV1>,
}

impl CompletenessWitnessV1 {
    pub fn new(
        proof_root: CacheRootV1,
        checked_roots: Vec<CacheRootV1>,
    ) -> Result<Self, CacheEntryError> {
        let mut witness = Self {
            proof_root,
            checked_roots,
        };
        witness.normalize_and_validate()?;
        Ok(witness)
    }

    pub fn proof_root(&self) -> &CacheRootV1 {
        &self.proof_root
    }

    pub fn checked_roots(&self) -> &[CacheRootV1] {
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
pub struct VerifierReceiptV1 {
    verifier: String,
    receipt_root: CacheRootV1,
}

impl VerifierReceiptV1 {
    pub fn new(
        verifier: impl Into<String>,
        receipt_root: CacheRootV1,
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

    pub fn receipt_root(&self) -> &CacheRootV1 {
        &self.receipt_root
    }
}

/// Complete cache key. scope_roots is empty for a positive hit and non-empty
/// for a negative/no-matches entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheKeyV1 {
    operator: OperatorIdentityV1,
    canonical_parameters: Value,
    minimum_dependency_roots: Vec<CacheRootV1>,
    environment_roots: Vec<CacheRootV1>,
    toolchain_roots: Vec<CacheRootV1>,
    completeness_witness: CompletenessWitnessV1,
    #[serde(default)]
    scope_roots: Vec<CacheRootV1>,
}

#[derive(Debug, Deserialize)]
struct CacheKeyWire {
    operator: OperatorIdentityV1,
    canonical_parameters: Value,
    minimum_dependency_roots: Vec<CacheRootV1>,
    environment_roots: Vec<CacheRootV1>,
    toolchain_roots: Vec<CacheRootV1>,
    completeness_witness: CompletenessWitnessV1,
    #[serde(default)]
    scope_roots: Vec<CacheRootV1>,
}

impl<'de> Deserialize<'de> for CacheKeyV1 {
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

impl CacheKeyV1 {
    /// Construct a positive-result key with no anti-dependency scope roots.
    pub fn new(
        operator: OperatorIdentityV1,
        canonical_parameters: Value,
        minimum_dependency_roots: Vec<CacheRootV1>,
        environment_roots: Vec<CacheRootV1>,
        toolchain_roots: Vec<CacheRootV1>,
        completeness_witness: CompletenessWitnessV1,
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
        operator: OperatorIdentityV1,
        canonical_parameters: Value,
        minimum_dependency_roots: Vec<CacheRootV1>,
        environment_roots: Vec<CacheRootV1>,
        toolchain_roots: Vec<CacheRootV1>,
        completeness_witness: CompletenessWitnessV1,
        scope_roots: Vec<CacheRootV1>,
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
        operator: OperatorIdentityV1,
        canonical_parameters: Value,
        mut minimum_dependency_roots: Vec<CacheRootV1>,
        mut environment_roots: Vec<CacheRootV1>,
        mut toolchain_roots: Vec<CacheRootV1>,
        mut completeness_witness: CompletenessWitnessV1,
        mut scope_roots: Vec<CacheRootV1>,
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

    pub fn operator(&self) -> &OperatorIdentityV1 {
        &self.operator
    }

    pub fn canonical_parameters(&self) -> &Value {
        &self.canonical_parameters
    }

    pub fn minimum_dependency_roots(&self) -> &[CacheRootV1] {
        &self.minimum_dependency_roots
    }

    pub fn environment_roots(&self) -> &[CacheRootV1] {
        &self.environment_roots
    }

    pub fn toolchain_roots(&self) -> &[CacheRootV1] {
        &self.toolchain_roots
    }

    pub fn completeness_witness(&self) -> &CompletenessWitnessV1 {
        &self.completeness_witness
    }

    pub fn scope_roots(&self) -> &[CacheRootV1] {
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
pub enum CacheValueV1 {
    Hit {
        output_root: CacheRootV1,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        verifier_receipt: Option<VerifierReceiptV1>,
    },
    NoMatches,
}

/// One validated cache entry shared by GraphZero and FSZero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheEntryV1 {
    schema: String,
    key: CacheKeyV1,
    value: CacheValueV1,
}

#[derive(Debug, Deserialize)]
struct CacheEntryWire {
    schema: String,
    key: CacheKeyV1,
    value: CacheValueV1,
}

impl<'de> Deserialize<'de> for CacheEntryV1 {
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

impl CacheEntryV1 {
    /// Build a positive hit. A scope-bearing key is reserved for no-matches.
    pub fn positive(
        key: CacheKeyV1,
        output_root: CacheRootV1,
        verifier_receipt: Option<VerifierReceiptV1>,
    ) -> Result<Self, CacheEntryError> {
        let entry = Self {
            schema: CACHE_ENTRY_SCHEMA_V1.to_owned(),
            key,
            value: CacheValueV1::Hit {
                output_root,
                verifier_receipt,
            },
        };
        entry.validate()?;
        Ok(entry)
    }

    /// Build a certified negative/no-matches entry. Its key must carry scope
    /// roots covered by the completeness witness.
    pub fn negative(key: CacheKeyV1) -> Result<Self, CacheEntryError> {
        let entry = Self {
            schema: CACHE_ENTRY_SCHEMA_V1.to_owned(),
            key,
            value: CacheValueV1::NoMatches,
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn schema(&self) -> &str {
        &self.schema
    }

    pub fn key(&self) -> &CacheKeyV1 {
        &self.key
    }

    pub fn value(&self) -> &CacheValueV1 {
        &self.value
    }

    pub fn key_hash_hex(&self) -> String {
        self.key.key_hash_hex()
    }

    fn validate(&self) -> Result<(), CacheEntryError> {
        if self.schema != CACHE_ENTRY_SCHEMA_V1 {
            return Err(CacheEntryError::UnsupportedSchema(self.schema.clone()));
        }
        self.key.validate()?;
        match &self.value {
            CacheValueV1::Hit {
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
            CacheValueV1::NoMatches => {
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

fn validate_root(field: &str, root: &CacheRootV1) -> Result<(), CacheEntryError> {
    if root.0.is_empty() {
        Err(CacheEntryError::InvalidRoot(field.to_owned()))
    } else {
        Ok(())
    }
}

fn validate_roots(field: &str, roots: &[CacheRootV1]) -> Result<(), CacheEntryError> {
    for root in roots {
        validate_root(field, root)?;
    }
    Ok(())
}

fn validate_operator(operator: &OperatorIdentityV1) -> Result<(), CacheEntryError> {
    require_nonempty("operator id", &operator.id)?;
    require_nonempty("operator version", &operator.version)
}

fn normalize_roots(roots: &mut Vec<CacheRootV1>) {
    roots.sort();
    roots.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn root(name: &str) -> CacheRootV1 {
        CacheRootV1::new(name).unwrap()
    }

    fn witness() -> CompletenessWitnessV1 {
        CompletenessWitnessV1::new(
            root("proof"),
            vec![
                root("scope"),
                root("dep-a"),
                root("dep-b"),
                root("env"),
                root("toolchain"),
            ],
        )
        .unwrap()
    }

    fn positive_key(parameters: Value) -> CacheKeyV1 {
        CacheKeyV1::new(
            OperatorIdentityV1::new("graph.snap", "2").unwrap(),
            parameters,
            vec![root("dep-b"), root("dep-a")],
            vec![root("env")],
            vec![root("toolchain")],
            witness(),
        )
        .unwrap()
    }

    #[test]
    fn round_trip_positive_and_negative_entries() {
        let positive = CacheEntryV1::positive(
            positive_key(json!({"query": "needle"})),
            root("output"),
            Some(VerifierReceiptV1::new("graph.verifier", root("receipt")).unwrap()),
        )
        .unwrap();
        let negative = CacheEntryV1::negative(
            CacheKeyV1::with_scope_roots(
                OperatorIdentityV1::new("fs.search", "1").unwrap(),
                json!({"query": "absent"}),
                vec![root("dep-a")],
                vec![root("env")],
                vec![root("toolchain")],
                witness(),
                vec![root("scope")],
            )
            .unwrap(),
        )
        .unwrap();

        for entry in [positive, negative] {
            let encoded = serde_json::to_string(&entry).unwrap();
            let decoded: CacheEntryV1 = serde_json::from_str(&encoded).unwrap();
            assert_eq!(entry, decoded);
        }
    }

    #[test]
    fn canonical_hash_is_stable_for_semantically_equal_keys() {
        let mut params_a = Map::new();
        params_a.insert("z".to_owned(), json!(1));
        params_a.insert("a".to_owned(), json!(2));
        let mut params_b = Map::new();
        params_b.insert("a".to_owned(), json!(2));
        params_b.insert("z".to_owned(), json!(1));
        let a = positive_key(Value::Object(params_a));
        let b = CacheKeyV1::new(
            OperatorIdentityV1::new("graph.snap", "2").unwrap(),
            Value::Object(params_b),
            vec![root("dep-a"), root("dep-b")],
            vec![root("env")],
            vec![root("toolchain")],
            CompletenessWitnessV1::new(
                root("proof"),
                vec![
                    root("dep-a"),
                    root("dep-b"),
                    root("scope"),
                    root("env"),
                    root("toolchain"),
                ],
            )
            .unwrap(),
        )
        .unwrap();

        assert_eq!(a.canonical_key_json(), b.canonical_key_json());
        assert_eq!(a.key_hash_hex(), b.key_hash_hex());
    }

    #[test]
    fn negative_entry_requires_and_carries_scope_roots() {
        let key = CacheKeyV1::with_scope_roots(
            OperatorIdentityV1::new("fs.search", "1").unwrap(),
            json!({"query": "absent"}),
            vec![],
            vec![root("env")],
            vec![root("toolchain")],
            witness(),
            vec![root("scope")],
        )
        .unwrap();
        let entry = CacheEntryV1::negative(key).unwrap();
        assert!(matches!(entry.value(), CacheValueV1::NoMatches));
        assert_eq!(entry.key().scope_roots(), &[root("scope")]);
        assert!(CacheEntryV1::negative(positive_key(json!({}))).is_err());
    }

    #[test]
    fn missing_completeness_witness_is_rejected() {
        let entry = CacheEntryV1::positive(positive_key(json!({})), root("output"), None).unwrap();
        let mut wire = serde_json::to_value(entry).unwrap();
        wire["key"]
            .as_object_mut()
            .unwrap()
            .remove("completeness_witness");
        assert!(serde_json::from_value::<CacheEntryV1>(wire).is_err());
    }

    #[test]
    fn mutation_without_witness_update_is_rejected() {
        let entry = CacheEntryV1::positive(positive_key(json!({})), root("output"), None).unwrap();
        let mut wire = serde_json::to_value(entry).unwrap();
        wire["key"]["minimum_dependency_roots"][0] = json!("dep-mutated");
        assert!(serde_json::from_value::<CacheEntryV1>(wire).is_err());
    }
}
