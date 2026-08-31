//! Incremental dirty closure from a bookmark and journal change feed.

use std::collections::BTreeSet;

use graphzero_types::{
    ContentHash, GRAPHZERO_STORE_SCHEMA_MAJOR, GRAPHZERO_STORE_SCHEMA_MINOR, SchemaVersionStamp,
    StoreSegmentKind, admit_current,
};

use crate::invalidation::{
    ArtifactId, DependencyClosureRecord, DependencyGraph, InfluenceClass, InvalidationCertificate,
    dirty_from_closure,
};

/// Journal event kinds accepted from the hub change feed.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum JournalEventKind {
    Create,
    Modify,
    Delete,
    Rename,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalEvent {
    pub kind: JournalEventKind,
    pub artifact: ArtifactId,
    /// Optional rename target.
    pub to: Option<ArtifactId>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    pub id: ContentHash,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DirtyReport {
    /// CacheZero store schema major (refuse on skew vs reader).
    pub schema_major: u32,
    /// CacheZero store schema minor (older-minor degrades gracefully).
    pub schema_minor: u32,
    /// Producer identity, e.g. `graphzero-core@0.1.0`.
    pub writer_version: String,
    pub since: Bookmark,
    pub journal_changed: BTreeSet<ArtifactId>,
    pub dirty_artifacts: BTreeSet<ArtifactId>,
    pub certificate: InvalidationCertificate,
}

impl DirtyReport {
    /// Schema stamp carried on this dirty-set output.
    #[must_use]
    pub fn schema_stamp(&self) -> SchemaVersionStamp {
        SchemaVersionStamp {
            schema_major: self.schema_major,
            schema_minor: self.schema_minor,
            writer_version: self.writer_version.clone(),
        }
    }

    /// Admit this dirty-set segment against the current GraphZero schema.
    pub fn admit_schema(
        &self,
    ) -> Result<graphzero_types::AdmitOutcome, graphzero_types::SchemaVersionError> {
        admit_current(StoreSegmentKind::DirtySet, &self.schema_stamp())
    }
}

fn dirty_set_writer_version() -> String {
    format!("graphzero-core@{}", env!("CARGO_PKG_VERSION"))
}

fn stamp_dirty_report(
    since: Bookmark,
    journal_changed: BTreeSet<ArtifactId>,
    dirty_artifacts: BTreeSet<ArtifactId>,
    certificate: InvalidationCertificate,
) -> DirtyReport {
    let stamp = SchemaVersionStamp::current(dirty_set_writer_version());
    debug_assert_eq!(stamp.schema_major, GRAPHZERO_STORE_SCHEMA_MAJOR);
    debug_assert_eq!(stamp.schema_minor, GRAPHZERO_STORE_SCHEMA_MINOR);
    DirtyReport {
        schema_major: stamp.schema_major,
        schema_minor: stamp.schema_minor,
        writer_version: stamp.writer_version,
        since,
        journal_changed,
        dirty_artifacts,
        certificate,
    }
}

/// Collect changed artifact ids from journal events after a bookmark.
#[must_use]
pub fn journal_changed_set(events: &[JournalEvent]) -> BTreeSet<ArtifactId> {
    let mut out = BTreeSet::new();
    for e in events {
        out.insert(e.artifact);
        if let Some(to) = e.to {
            out.insert(to);
        }
    }
    out
}

/// Compute dirty derived-artifact set since `bookmark` from journal events and
/// recorded dependency closures. Sound overapprox via influence upward closure.
pub fn dirty_since(
    since: Bookmark,
    events: &[JournalEvent],
    closures: &[DependencyClosureRecord],
    influence: &DependencyGraph,
) -> DirtyReport {
    let journal_changed = journal_changed_set(events);
    let mut dirty = dirty_from_closure(closures, &journal_changed);
    // Also treat direct journal paths as dirty seeds.
    for c in &journal_changed {
        dirty.insert(*c);
    }
    let certificate = influence.certify_invalidation(&dirty);
    let dirty_artifacts = certificate.invalidated.clone();
    stamp_dirty_report(since, journal_changed, dirty_artifacts, certificate)
}

/// Convenience: empty influence graph still reports closure-intersect dirty set.
#[must_use]
pub fn dirty_since_closures_only(
    since: Bookmark,
    events: &[JournalEvent],
    closures: &[DependencyClosureRecord],
) -> DirtyReport {
    let mut g = DependencyGraph::new(InfluenceClass::SoundOverapproximation);
    for rec in closures {
        g.ensure_node(rec.artifact);
        for c in &rec.consulted {
            g.add_dependency(*c, rec.artifact);
        }
    }
    dirty_since(since, events, closures, &g)
}
