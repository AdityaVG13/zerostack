//! Canonical generation-qualified ordinal refs and allocation gauges for ZeroStack.
//!
//! This crate defines syntax and checked integer arithmetic only. It does not cut
//! any engine over to ordinal refs. Token certification evaluates complete refs
//! under an exact provider lock; component atoms never certify a composition.

#![forbid(unsafe_code)]

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;

pub mod solver;

/// Closed set of engines that can own an ordinal reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EngineScheme {
    Fz,
    Gz,
    Tz,
}

impl EngineScheme {
    /// Canonical lowercase scheme.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fz => "fz",
            Self::Gz => "gz",
            Self::Tz => "tz",
        }
    }
}
impl fmt::Display for EngineScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl FromStr for EngineScheme {
    type Err = ParseOrdinalRefError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fz" => Ok(Self::Fz),
            "gz" => Ok(Self::Gz),
            "tz" => Ok(Self::Tz),
            _ => Err(ParseOrdinalRefError::InvalidScheme),
        }
    }
}
impl Serialize for EngineScheme {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for EngineScheme {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <&str>::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

/// Canonical '<scheme>://o/<generation>/<ordinal>' reference.
///
/// Coordinates are unsigned ASCII-decimal u64 values. Zero is valid syntax;
/// allocation gauges use one-based, nonzero coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OrdinalRef {
    scheme: EngineScheme,
    generation: u64,
    ordinal: u64,
}

impl OrdinalRef {
    pub const fn new(scheme: EngineScheme, generation: u64, ordinal: u64) -> Self {
        Self {
            scheme,
            generation,
            ordinal,
        }
    }
    pub const fn scheme(self) -> EngineScheme {
        self.scheme
    }
    pub const fn generation(self) -> u64 {
        self.generation
    }
    pub const fn ordinal(self) -> u64 {
        self.ordinal
    }
}
impl fmt::Display for OrdinalRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}://o/{}/{}",
            self.scheme, self.generation, self.ordinal
        )
    }
}
impl FromStr for OrdinalRef {
    type Err = ParseOrdinalRefError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if !value.is_ascii() {
            return Err(ParseOrdinalRefError::NonAscii);
        }
        if value.contains('?') || value.contains('#') {
            return Err(ParseOrdinalRefError::QueryOrFragment);
        }
        let (scheme, coordinates) = value
            .split_once("://o/")
            .ok_or(ParseOrdinalRefError::InvalidShape)?;
        let scheme = scheme.parse()?;
        let mut fields = coordinates.split('/');
        let generation =
            parse_decimal(fields.next().unwrap_or_default(), DecimalField::Generation)?;
        let ordinal = parse_decimal(fields.next().unwrap_or_default(), DecimalField::Ordinal)?;
        if fields.next().is_some() {
            return Err(ParseOrdinalRefError::InvalidShape);
        }
        Ok(Self::new(scheme, generation, ordinal))
    }
}
fn parse_decimal(value: &str, field: DecimalField) -> Result<u64, ParseOrdinalRefError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ParseOrdinalRefError::InvalidDecimal(field));
    }
    if value.len() > 1 && value.starts_with('0') {
        return Err(ParseOrdinalRefError::LeadingZero(field));
    }
    value
        .parse()
        .map_err(|_| ParseOrdinalRefError::DecimalOverflow(field))
}
impl Serialize for OrdinalRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for OrdinalRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        <&str>::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalField {
    Generation,
    Ordinal,
}

/// Strict grammar failure. Extra paths, queries, and fragments are refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseOrdinalRefError {
    NonAscii,
    InvalidScheme,
    InvalidShape,
    QueryOrFragment,
    InvalidDecimal(DecimalField),
    LeadingZero(DecimalField),
    DecimalOverflow(DecimalField),
}
impl fmt::Display for ParseOrdinalRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid ordinal reference: {self:?}")
    }
}
impl Error for ParseOrdinalRefError {}

/// Checked mapping between zero-based allocations and one-based coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gauge {
    capacity: NonZeroU64,
}
impl Gauge {
    /// Construct with a nonzero number of ordinal slots per generation.
    pub fn new(capacity: u64) -> Result<Self, GaugeError> {
        NonZeroU64::new(capacity)
            .map(|capacity| Self { capacity })
            .ok_or(GaugeError::ZeroCapacity)
    }
    pub const fn capacity(self) -> NonZeroU64 {
        self.capacity
    }

    /// Map a zero-based allocation to one-based generation and ordinal coordinates.
    pub fn allocate(self, scheme: EngineScheme, allocation: u64) -> Result<OrdinalRef, GaugeError> {
        let capacity = self.capacity.get();
        let generation = (allocation / capacity)
            .checked_add(1)
            .ok_or(GaugeError::ArithmeticOverflow)?;
        Ok(OrdinalRef::new(
            scheme,
            generation,
            allocation % capacity + 1,
        ))
    }

    /// Recover a zero-based allocation from one-based gauge coordinates.
    pub fn allocation(self, reference: OrdinalRef) -> Result<u64, GaugeError> {
        if reference.generation == 0 || reference.ordinal == 0 {
            return Err(GaugeError::ZeroCoordinate);
        }
        if reference.ordinal > self.capacity.get() {
            return Err(GaugeError::OrdinalOutOfRange {
                ordinal: reference.ordinal,
                capacity: self.capacity.get(),
            });
        }
        (reference.generation - 1)
            .checked_mul(self.capacity.get())
            .and_then(|base| base.checked_add(reference.ordinal - 1))
            .ok_or(GaugeError::ArithmeticOverflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaugeError {
    ZeroCapacity,
    ZeroCoordinate,
    OrdinalOutOfRange { ordinal: u64, capacity: u64 },
    ArithmeticOverflow,
}
impl fmt::Display for GaugeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid gauge mapping: {self:?}")
    }
}
impl Error for GaugeError {}

/// Exact provider, model, and tokenizer revision identity used for certification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderLock {
    pub provider: String,
    pub model: String,
    /// Lowercase SHA-256 digest of the tokenizer revision manifest.
    pub tokenizer_revision_digest: String,
}

/// One complete rendered grammar instance, never a component atom.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureInstance {
    pub rendered: String,
    /// Reviewed count, or explicit null when runtime certification is required.
    #[serde(deserialize_with = "deserialize_expected_token_count")]
    pub expected_token_count: Option<u64>,
}
fn deserialize_expected_token_count<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u64>, D::Error> {
    Option::<u64>::deserialize(deserializer)
}

/// Versioned provider-locked complete-instance fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AtomFixture {
    pub schema: String,
    pub provider_lock: ProviderLock,
    pub instances: Vec<FixtureInstance>,
}

/// Proof that all listed complete refs counted as one token under one exact lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certification {
    provider_lock: ProviderLock,
    certified_instances: usize,
}
impl Certification {
    pub fn provider_lock(&self) -> &ProviderLock {
        &self.provider_lock
    }
    pub const fn certified_instances(&self) -> usize {
        self.certified_instances
    }
}

/// Generic public fixtures. Null expected counts make runtime certification mandatory.
pub const BUNDLED_ATOM_FIXTURE_JSON: &str = include_str!("../fixtures/atoms.json");

/// Parse a fixture while requiring expected_token_count to be explicitly present.
pub fn parse_fixture(json: &str) -> Result<AtomFixture, CertificationError> {
    serde_json::from_str(json).map_err(|error| CertificationError::InvalidJson(error.to_string()))
}

/// Certify every complete canonical fixture rendering with a provider-locked callback.
///
/// The callback receives the validated lock and complete ref verbatim. Every
/// callback count must be one. Proof never transfers to another lock or an
/// untested composition.
pub fn certify_fixture<F>(
    fixture: &AtomFixture,
    expected_lock: &ProviderLock,
    mut count_tokens: F,
) -> Result<Certification, CertificationError>
where
    F: FnMut(&ProviderLock, &str) -> Result<u64, String>,
{
    validate_lock(&fixture.provider_lock)?;
    if &fixture.provider_lock != expected_lock {
        return Err(CertificationError::ProviderLockMismatch);
    }
    if fixture.schema != "zerostack.zero_gauge.complete_atoms.v1" {
        return Err(CertificationError::UnsupportedSchema);
    }
    if fixture.instances.is_empty() {
        return Err(CertificationError::EmptyFixture);
    }

    let mut unique = BTreeSet::new();
    for instance in &fixture.instances {
        if instance.rendered.is_empty() {
            return Err(CertificationError::EmptyInstance);
        }
        if !unique.insert(instance.rendered.as_str()) {
            return Err(CertificationError::DuplicateInstance(
                instance.rendered.clone(),
            ));
        }
        let parsed: OrdinalRef = instance
            .rendered
            .parse()
            .map_err(|_| CertificationError::NoncanonicalInstance(instance.rendered.clone()))?;
        if parsed.to_string() != instance.rendered {
            return Err(CertificationError::NoncanonicalInstance(
                instance.rendered.clone(),
            ));
        }
        if let Some(count) = instance.expected_token_count
            && count != 1
        {
            return Err(CertificationError::ExpectedCountNotOne {
                rendered: instance.rendered.clone(),
                count,
            });
        }
        let count = count_tokens(&fixture.provider_lock, &instance.rendered)
            .map_err(CertificationError::Tokenizer)?;
        if count != 1 {
            return Err(CertificationError::RuntimeCountNotOne {
                rendered: instance.rendered.clone(),
                count,
            });
        }
    }
    Ok(Certification {
        provider_lock: fixture.provider_lock.clone(),
        certified_instances: fixture.instances.len(),
    })
}

fn validate_lock(lock: &ProviderLock) -> Result<(), CertificationError> {
    if lock.provider.is_empty() || lock.model.is_empty() {
        return Err(CertificationError::EmptyProviderLockField);
    }
    let digest = lock.tokenizer_revision_digest.as_bytes();
    if digest.len() != 64
        || !digest
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(CertificationError::InvalidTokenizerRevisionDigest);
    }
    Ok(())
}

/// Fixture validation or tokenizer proof failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertificationError {
    InvalidJson(String),
    UnsupportedSchema,
    EmptyProviderLockField,
    InvalidTokenizerRevisionDigest,
    ProviderLockMismatch,
    EmptyFixture,
    EmptyInstance,
    DuplicateInstance(String),
    NoncanonicalInstance(String),
    ExpectedCountNotOne { rendered: String, count: u64 },
    RuntimeCountNotOne { rendered: String, count: u64 },
    Tokenizer(String),
}
impl fmt::Display for CertificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ordinal atom certification failed: {self:?}")
    }
}
impl Error for CertificationError {}

#[cfg(test)]
#[path = "../../../tests/rust/zero-gauge/unit/lib.rs"]
mod tests;
