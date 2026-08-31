//! GraphZero store segment schema stamps (CacheZero / ZeroStack contract). Types and skew
//! rules live in `graphzero_types::schema_version`. This module writes/reads the durable
//! snapshot companion stamp under a store root and re-exports the shared contract for store callers.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

pub use graphzero_types::{
    AdmitOutcome, GRAPHZERO_STORE_SCHEMA_MAJOR, GRAPHZERO_STORE_SCHEMA_MINOR, SNAPSHOT_SCHEMA_FILE,
    SchemaVersionError, SchemaVersionRefuseReason, SchemaVersionStamp, SnapshotSchemaSegment,
    StoreSegmentKind, admit_current, admit_read,
};

/// Producer identity for graphzero-store writers.
#[must_use]
pub fn store_writer_version() -> String {
    format!("graphzero-store@{}", env!("CARGO_PKG_VERSION"))
}

/// Current fingerprint / dirty-set / snapshot producer stamp.
#[must_use]
pub fn current_store_stamp() -> SchemaVersionStamp {
    SchemaVersionStamp::current(store_writer_version())
}

/// Path of the snapshot schema companion under `store_root`.
#[must_use]
pub fn snapshot_schema_path(store_root: &Path) -> std::path::PathBuf {
    store_root.join(SNAPSHOT_SCHEMA_FILE)
}

/// Atomically publish the snapshot schema stamp for `snapshot_id`.
pub fn write_snapshot_schema_stamp(store_root: &Path, snapshot_id: u64) -> Result<()> {
    let segment = SnapshotSchemaSegment::for_snapshot(snapshot_id, store_writer_version());
    let text = serde_json::to_string_pretty(&segment).context("serialize snapshot schema stamp")?;
    let path = snapshot_schema_path(store_root);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text.as_bytes())
        .with_context(|| format!("write snapshot schema temp {}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .with_context(|| format!("publish snapshot schema stamp {}", path.display()))?;
    Ok(())
}

/// Load and admit the snapshot schema stamp. Missing stamp (legacy stores) is treated
/// as same-major older content and admitted with degrade. Newer major refuses loudly.
pub fn admit_snapshot_schema_stamp(store_root: &Path) -> Result<AdmitOutcome> {
    let path = snapshot_schema_path(store_root);
    match fs::read_to_string(&path) {
        Ok(text) => {
            let segment: SnapshotSchemaSegment = serde_json::from_str(&text)
                .with_context(|| format!("parse snapshot schema stamp {}", path.display()))?;
            match admit_current(StoreSegmentKind::Snapshot, &segment.stamp()) {
                Ok(outcome) => Ok(outcome),
                Err(err) => bail!("{err}"),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Pre-stamp stores: same major, treat as older-minor degrade.
            Ok(AdmitOutcome::DegradedOlderMinor)
        }
        Err(error) => {
            Err(error).with_context(|| format!("read snapshot schema stamp {}", path.display()))
        }
    }
}

/// Admit a fingerprint sidecar stamp. Missing major (legacy JSON) degrades.
pub fn admit_fingerprint_stamp(
    schema_major: Option<u32>,
    schema_minor: Option<u32>,
    writer_version: Option<&str>,
) -> Result<AdmitOutcome> {
    match schema_major {
        None | Some(0) => Ok(AdmitOutcome::DegradedOlderMinor),
        Some(major) => {
            let stamp = SchemaVersionStamp {
                schema_major: major,
                schema_minor: schema_minor.unwrap_or(0),
                writer_version: writer_version.unwrap_or("unknown").to_string(),
            };
            admit_current(StoreSegmentKind::Fingerprint, &stamp).map_err(|e| anyhow::anyhow!("{e}"))
        }
    }
}
