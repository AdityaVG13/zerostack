//! Store segment schema version stamps and version-skew rules.
//!
//! CacheZero / ZeroStack contract (shared across engines): every segment written
//! into `.zerostack` stamps `{schema_major, schema_minor, writer_version}`.
//!
//! Skew rules:
//! - **Newer major** (found > supported): refuse loudly; never guess.
//! - **Older major** (found < supported): refuse loudly (incompatible layout).
//! - **Same major, older minor**: admit with graceful degrade.
//! - **Same major, equal or newer minor**: admit (minor bumps are additive).
//!
//! Writers never downgrade a segment in place; they emit the producer stamp.

use std::fmt;

use serde::{Deserialize, Serialize};

/// GraphZero store segments that participate in the shared-schema contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreSegmentKind {
    /// Worktree / structural fingerprint sidecars.
    Fingerprint,
    /// Dirty-set outputs (`dirty --since` / `DirtyReport` envelopes).
    DirtySet,
    /// Published snapshot metadata + shard layout companion stamp.
    Snapshot,
}

impl StoreSegmentKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fingerprint => "fingerprint",
            Self::DirtySet => "dirty_set",
            Self::Snapshot => "snapshot",
        }
    }
}

impl fmt::Display for StoreSegmentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Current GraphZero store schema (all three segments start at 1.0).
pub const GRAPHZERO_STORE_SCHEMA_MAJOR: u32 = 1;
pub const GRAPHZERO_STORE_SCHEMA_MINOR: u32 = 0;

/// Wire stamp carried on every store segment (CacheZero §9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaVersionStamp {
    pub schema_major: u32,
    pub schema_minor: u32,
    pub writer_version: String,
}

impl SchemaVersionStamp {
    /// Build a producer stamp for the current GraphZero store schema.
    #[must_use]
    pub fn current(writer_version: impl Into<String>) -> Self {
        Self {
            schema_major: GRAPHZERO_STORE_SCHEMA_MAJOR,
            schema_minor: GRAPHZERO_STORE_SCHEMA_MINOR,
            writer_version: writer_version.into(),
        }
    }

    /// Stamp for a concrete major/minor (tests and forward writers).
    #[must_use]
    pub fn new(schema_major: u32, schema_minor: u32, writer_version: impl Into<String>) -> Self {
        Self {
            schema_major,
            schema_minor,
            writer_version: writer_version.into(),
        }
    }
}

/// Result of admitting a segment stamp for local read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmitOutcome {
    /// Same major; fully supported (equal minor, or newer additive minor).
    Compatible,
    /// Same major, older minor: read known fields; ignore unknowns on write path.
    DegradedOlderMinor,
}

/// Loud refusal when a segment cannot be interpreted safely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SchemaVersionError {
    pub segment: StoreSegmentKind,
    pub found_major: u32,
    pub found_minor: u32,
    pub supported_major: u32,
    pub supported_minor: u32,
    pub reason: SchemaVersionRefuseReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaVersionRefuseReason {
    /// Found major is newer than this reader supports.
    NewerMajor,
    /// Found major is older than this reader (layout no longer understood).
    OlderMajor,
}

impl SchemaVersionRefuseReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NewerMajor => "newer_major",
            Self::OlderMajor => "older_major",
        }
    }
}

impl fmt::Display for SchemaVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "schema_version_refused segment={} reason={} found={}.{} supported={}.{}: refuse loudly, never guess",
            self.segment,
            self.reason.as_str(),
            self.found_major,
            self.found_minor,
            self.supported_major,
            self.supported_minor
        )
    }
}

impl std::error::Error for SchemaVersionError {}

/// Admit a found stamp against the local supported major/minor.
///
/// # Errors
///
/// Returns [`SchemaVersionError`] when majors differ (newer or older).
pub fn admit_read(
    segment: StoreSegmentKind,
    found: &SchemaVersionStamp,
    supported_major: u32,
    supported_minor: u32,
) -> Result<AdmitOutcome, SchemaVersionError> {
    if found.schema_major > supported_major {
        return Err(SchemaVersionError {
            segment,
            found_major: found.schema_major,
            found_minor: found.schema_minor,
            supported_major,
            supported_minor,
            reason: SchemaVersionRefuseReason::NewerMajor,
        });
    }
    if found.schema_major < supported_major {
        return Err(SchemaVersionError {
            segment,
            found_major: found.schema_major,
            found_minor: found.schema_minor,
            supported_major,
            supported_minor,
            reason: SchemaVersionRefuseReason::OlderMajor,
        });
    }
    // Same major.
    if found.schema_minor < supported_minor {
        Ok(AdmitOutcome::DegradedOlderMinor)
    } else {
        // Equal minor, or newer minor (additive / forward-compatible fields).
        Ok(AdmitOutcome::Compatible)
    }
}

/// Admit against the current GraphZero store schema constants.
pub fn admit_current(
    segment: StoreSegmentKind,
    found: &SchemaVersionStamp,
) -> Result<AdmitOutcome, SchemaVersionError> {
    admit_read(
        segment,
        found,
        GRAPHZERO_STORE_SCHEMA_MAJOR,
        GRAPHZERO_STORE_SCHEMA_MINOR,
    )
}

/// Durable companion for a published snapshot under a store root.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotSchemaSegment {
    pub schema_major: u32,
    pub schema_minor: u32,
    pub writer_version: String,
    /// Snapshot id this stamp was written with (tip at publish time).
    pub snapshot_id: u64,
    pub segment: StoreSegmentKind,
}

impl SnapshotSchemaSegment {
    #[must_use]
    pub fn for_snapshot(snapshot_id: u64, writer_version: impl Into<String>) -> Self {
        let stamp = SchemaVersionStamp::current(writer_version);
        Self {
            schema_major: stamp.schema_major,
            schema_minor: stamp.schema_minor,
            writer_version: stamp.writer_version,
            snapshot_id,
            segment: StoreSegmentKind::Snapshot,
        }
    }

    #[must_use]
    pub fn stamp(&self) -> SchemaVersionStamp {
        SchemaVersionStamp {
            schema_major: self.schema_major,
            schema_minor: self.schema_minor,
            writer_version: self.writer_version.clone(),
        }
    }
}

/// Relative path of the snapshot schema stamp under a graphzero store root.
pub const SNAPSHOT_SCHEMA_FILE: &str = "snapshot_schema.json";

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-types/schema_version_tests.rs"]
mod tests;
