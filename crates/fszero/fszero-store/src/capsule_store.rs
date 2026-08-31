//! Work Capsule byte storage, exact projections, and storage economics. FSZero stores
//! opaque canonical manifests and projection bytes in the existing CAS. GC planning
//! returns `Unknown` unless support closure is complete; it never performs deletion.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;

use serde::{Deserialize, Serialize};
use zero_abi::work_capsule::WorkCapsule;

use crate::{CasError, CasStore};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CapsuleEnvelope {
    capsule_root: String,
    manifest: WorkCapsule,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapsuleObjectReceipt {
    pub capsule_root: String,
    pub object_hash: String,
    pub created: bool,
}

/// Typed capsule object-store failures. Classes stay distinct end to end so
/// the engine never reports a clean miss as corruption or a root mismatch as
/// a storage fault.
#[derive(Debug)]
pub enum CapsuleStoreError {
    /// CAS-layer failure (malformed hash, clean miss, corrupted bytes, I/O).
    Cas(CasError),
    /// Object bytes are not a canonical capsule envelope (or the envelope
    /// could not be produced).
    Envelope(String),
    /// Envelope manifest is not a valid WorkCapsule.
    Manifest(String),
    /// The envelope-declared root does not match the manifest's own root.
    RootMismatch { expected: String, actual: String },
    /// The stored capsule root does not match the caller-expected root.
    ExactRootMismatch { expected: String, actual: String },
}

impl fmt::Display for CapsuleStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapsuleStoreError::Cas(error) => write!(f, "cas: {error}"),
            CapsuleStoreError::Envelope(detail) => {
                write!(f, "capsule envelope invalid: {detail}")
            }
            CapsuleStoreError::Manifest(detail) => {
                write!(f, "capsule manifest invalid: {detail}")
            }
            CapsuleStoreError::RootMismatch { expected, actual } => write!(
                f,
                "capsule envelope root mismatch: declared {expected}, manifest {actual}"
            ),
            CapsuleStoreError::ExactRootMismatch { expected, actual } => {
                write!(
                    f,
                    "capsule root mismatch: expected {expected}, stored {actual}"
                )
            }
        }
    }
}

impl std::error::Error for CapsuleStoreError {}

pub struct CapsuleObjectStore {
    cas: CasStore,
}

impl CapsuleObjectStore {
    pub fn new(cas: CasStore) -> Self {
        Self { cas }
    }

    pub fn put(&self, capsule: &WorkCapsule) -> Result<CapsuleObjectReceipt, CapsuleStoreError> {
        let capsule_root = capsule.root().map_err(CapsuleStoreError::Manifest)?;
        let envelope = CapsuleEnvelope {
            capsule_root: capsule_root.clone(),
            manifest: capsule.clone(),
        };
        let value = serde_json::to_value(&envelope)
            .map_err(|error| CapsuleStoreError::Envelope(error.to_string()))?;
        let bytes = zero_abi::canonical_json(&value).into_bytes();
        let outcome = self.cas.put(&bytes).map_err(CapsuleStoreError::Cas)?;
        Ok(CapsuleObjectReceipt {
            capsule_root,
            object_hash: outcome.hash,
            created: outcome.created,
        })
    }

    pub fn get(&self, object_hash: &str) -> Result<WorkCapsule, CapsuleStoreError> {
        let bytes = self.cas.get(object_hash).map_err(CapsuleStoreError::Cas)?;
        let envelope: CapsuleEnvelope = serde_json::from_slice(&bytes)
            .map_err(|error| CapsuleStoreError::Envelope(error.to_string()))?;
        let actual = envelope
            .manifest
            .root()
            .map_err(CapsuleStoreError::Manifest)?;
        if actual != envelope.capsule_root {
            return Err(CapsuleStoreError::RootMismatch {
                expected: envelope.capsule_root.clone(),
                actual,
            });
        }
        Ok(envelope.manifest)
    }

    /// Exact publication read. The CAS verifies the object hash before any
    /// byte escapes, the envelope must be internally consistent, and the
    /// stored capsule root must equal `expected_capsule_root` exactly.
    pub fn get_expected(
        &self,
        object_hash: &str,
        expected_capsule_root: &str,
    ) -> Result<WorkCapsule, CapsuleStoreError> {
        let capsule = self.get(object_hash)?;
        let actual = capsule.root().map_err(CapsuleStoreError::Manifest)?;
        if actual != expected_capsule_root {
            return Err(CapsuleStoreError::ExactRootMismatch {
                expected: expected_capsule_root.to_owned(),
                actual,
            });
        }
        Ok(capsule)
    }

    pub fn put_projection(
        &self,
        manifest: &DirectProjectionManifest,
        source: &[u8],
    ) -> Result<ProjectionReceipt, String> {
        let projected = manifest.project(source)?;
        let source_outcome = self
            .cas
            .put_prehashed(&manifest.source_hash, source)
            .map_err(|error| error.to_string())?;
        let manifest_value = serde_json::to_value(manifest).map_err(|error| error.to_string())?;
        let manifest_bytes = zero_abi::canonical_json(&manifest_value).into_bytes();
        let manifest_outcome = self
            .cas
            .put(&manifest_bytes)
            .map_err(|error| error.to_string())?;
        let projection_outcome = self
            .cas
            .put(&projected)
            .map_err(|error| error.to_string())?;
        Ok(ProjectionReceipt {
            source_hash: source_outcome.hash,
            source_created: source_outcome.created,
            manifest_hash: manifest_outcome.hash,
            projection_hash: projection_outcome.hash,
            projected_bytes: projected.len() as u64,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectionReceipt {
    pub source_hash: String,
    pub source_created: bool,
    pub manifest_hash: String,
    pub projection_hash: String,
    pub projected_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectionRange {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DirectProjectionManifest {
    pub source_hash: String,
    pub projection_kind: String,
    pub ranges: Vec<ProjectionRange>,
}

impl DirectProjectionManifest {
    pub fn project(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        if self.projection_kind.is_empty() {
            return Err("projection kind must not be empty".into());
        }
        let actual = crate::access_log::content_hash_bytes(bytes);
        if actual != self.source_hash {
            return Err("projection source hash mismatch".into());
        }
        let mut previous_end = 0_u64;
        let mut projected_len = 0_u64;
        for range in &self.ranges {
            if range.start > range.end
                || range.start < previous_end
                || range.end > bytes.len() as u64
            {
                return Err("projection range is invalid, overlapping, or outside source".into());
            }
            projected_len = projected_len
                .checked_add(range.end - range.start)
                .ok_or("projection length overflow")?;
            previous_end = range.end;
        }
        let capacity =
            usize::try_from(projected_len).map_err(|_| "projection does not fit memory")?;
        let mut output = Vec::with_capacity(capacity);
        for range in &self.ranges {
            output.extend_from_slice(&bytes[range.start as usize..range.end as usize]);
        }
        Ok(output)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GcSupportEdge {
    pub from: String,
    pub to: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GcDryRun {
    Complete {
        retained: Vec<String>,
        reclaimable: Vec<String>,
    },
    Unknown {
        missing_objects: Vec<String>,
        incomplete_sources: Vec<String>,
    },
}

pub fn plan_gc_dry_run(
    all_objects: impl IntoIterator<Item = String>,
    protected_roots: impl IntoIterator<Item = String>,
    support_edges: impl IntoIterator<Item = GcSupportEdge>,
    support_edges_complete: bool,
) -> GcDryRun {
    let all: BTreeSet<String> = all_objects.into_iter().collect();
    let roots: BTreeSet<String> = protected_roots.into_iter().collect();
    let mut adjacency: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for edge in support_edges {
        adjacency.entry(edge.from).or_default().push(edge.to);
    }
    let mut retained = BTreeSet::new();
    let mut queue: VecDeque<String> = roots.iter().cloned().collect();
    while let Some(object) = queue.pop_front() {
        if !retained.insert(object.clone()) {
            continue;
        }
        if let Some(children) = adjacency.get(&object) {
            queue.extend(children.iter().cloned());
        }
    }
    let missing_objects: Vec<String> = retained.difference(&all).cloned().collect();
    let incomplete_sources = if support_edges_complete {
        Vec::new()
    } else {
        retained.iter().cloned().collect()
    };
    if !missing_objects.is_empty() || !incomplete_sources.is_empty() {
        return GcDryRun::Unknown {
            missing_objects,
            incomplete_sources,
        };
    }
    GcDryRun::Complete {
        retained: retained.iter().cloned().collect(),
        reclaimable: all.difference(&retained).cloned().collect(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PackTierResources {
    pub tier: String,
    pub exact: bool,
    pub resident_bytes: u64,
    pub metadata_bytes: u64,
    pub expected_read_bytes: u64,
    pub expected_decode_work: u64,
}

impl PackTierResources {
    fn dominates(&self, other: &Self) -> bool {
        let no_worse = self.resident_bytes <= other.resident_bytes
            && self.metadata_bytes <= other.metadata_bytes
            && self.expected_read_bytes <= other.expected_read_bytes
            && self.expected_decode_work <= other.expected_decode_work;
        let strictly_better = !other.exact
            || self.resident_bytes < other.resident_bytes
            || self.metadata_bytes < other.metadata_bytes
            || self.expected_read_bytes < other.expected_read_bytes
            || self.expected_decode_work < other.expected_decode_work;
        self.exact && no_worse && strictly_better
    }
}

pub fn nondominated_pack_tiers(tiers: &[PackTierResources]) -> Vec<PackTierResources> {
    let mut frontier: Vec<_> = tiers
        .iter()
        .filter(|candidate| {
            candidate.exact
                && !tiers
                    .iter()
                    .any(|other| other.tier != candidate.tier && other.dominates(candidate))
        })
        .cloned()
        .collect();
    frontier.sort_by(|left, right| left.tier.cmp(&right.tier));
    frontier
}
