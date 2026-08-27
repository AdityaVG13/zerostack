//! Candidate overlays, journals, atomic publication + effect realization (fszero-g9oz).

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::exact_snapshot::{ExactSnapshot, FileRecord, SnapshotError, normalize_path};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverlayError {
    Snapshot(SnapshotError),
    WrongBase { expected: String, actual: String },
    Stale,
    UnexpectedExisting(String),
    InvalidPath(String),
    Journal(String),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Snapshot(e) => write!(f, "snapshot: {e}"),
            Self::WrongBase { expected, actual } => {
                write!(f, "wrong base: expected {expected}, actual {actual}")
            }
            Self::Stale => write!(f, "overlay base is stale"),
            Self::UnexpectedExisting(p) => write!(f, "unexpected existing path: {p}"),
            Self::InvalidPath(p) => write!(f, "invalid path: {p}"),
            Self::Journal(s) => write!(f, "journal: {s}"),
        }
    }
}
impl std::error::Error for OverlayError {}

impl From<SnapshotError> for OverlayError {
    fn from(e: SnapshotError) -> Self {
        Self::Snapshot(e)
    }
}

/// Typed file mutation realized against an overlay (deterministic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectMutation {
    Put { path: String, bytes: Vec<u8> },
    Delete { path: String },
}

/// Candidate overlay: isolated expected-root effects on a base snapshot.
#[derive(Debug, Clone)]
pub struct Overlay {
    base_root: String,
    /// path → Some(bytes) put, None = deleted
    deltas: BTreeMap<String, Option<Vec<u8>>>,
}

impl Overlay {
    pub fn on_base(base: &ExactSnapshot) -> Self {
        Self {
            base_root: base.root_digest_hex().to_string(),
            deltas: BTreeMap::new(),
        }
    }

    pub fn base_root(&self) -> &str {
        &self.base_root
    }

    pub fn apply_mutation(&mut self, m: EffectMutation) -> Result<(), OverlayError> {
        match m {
            EffectMutation::Put { path, bytes } => {
                let path = normalize_path(&path).map_err(OverlayError::from)?;
                self.deltas.insert(path, Some(bytes));
            }
            EffectMutation::Delete { path } => {
                let path = normalize_path(&path).map_err(OverlayError::from)?;
                self.deltas.insert(path, None);
            }
        }
        Ok(())
    }

    /// Materialize overlay onto base files (base path → bytes).
    pub fn materialize(
        &self,
        base_files: &BTreeMap<String, Vec<u8>>,
        expected_base_root: &str,
    ) -> Result<BTreeMap<String, Vec<u8>>, OverlayError> {
        if expected_base_root != self.base_root {
            return Err(OverlayError::WrongBase {
                expected: self.base_root.clone(),
                actual: expected_base_root.to_string(),
            });
        }
        let mut out = base_files.clone();
        for (path, delta) in &self.deltas {
            match delta {
                Some(bytes) => {
                    out.insert(path.clone(), bytes.clone());
                }
                None => {
                    out.remove(path);
                }
            }
        }
        Ok(out)
    }
}

/// Apply a hub-validated Effect IR program deterministically (same effect ⇒ same bytes).
pub fn realize_effects(
    base: &ExactSnapshot,
    base_files: &BTreeMap<String, Vec<u8>>,
    effects: &[EffectMutation],
) -> Result<(Overlay, BTreeMap<String, Vec<u8>>, String), OverlayError> {
    let mut overlay = Overlay::on_base(base);
    for e in effects {
        overlay.apply_mutation(e.clone())?;
    }
    let materialized = overlay.materialize(base_files, base.root_digest_hex())?;
    let snap = ExactSnapshot::from_files(materialized.clone().into_iter())?;
    Ok((overlay, materialized, snap.root_digest_hex().to_string()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicationStage {
    Prepared,
    Verified,
    Published,
    Durable,
    Restored,
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashPoint {
    None,
    AfterPrepare,
    AfterVerify,
    AfterPublish,
    BeforeBarrier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalRecord {
    pub stage: PublicationStage,
    pub base_root: String,
    pub candidate_root: String,
    pub intent_digest: String,
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn intent_digest(base_root: &str, candidate_root: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"FSZERO-PUBLISH-INTENT-V1\0");
    h.update(base_root.as_bytes());
    h.update(&[0]);
    h.update(candidate_root.as_bytes());
    hex_encode(h.finalize().as_slice())
}

/// In-memory atomic root publication with crash injection points.
#[derive(Debug, Clone)]
pub struct AtomicPublication {
    pub published_root: Option<String>,
    pub journal: Vec<JournalRecord>,
}

impl AtomicPublication {
    pub fn new() -> Self {
        Self {
            published_root: None,
            journal: Vec::new(),
        }
    }

    /// Publish candidate root. On crash points, returns Err with stage left in journal.
    pub fn publish_with_fault(
        &mut self,
        base_root: &str,
        candidate_root: &str,
        crash: CrashPoint,
    ) -> Result<String, OverlayError> {
        let intent = intent_digest(base_root, candidate_root);
        self.journal.push(JournalRecord {
            stage: PublicationStage::Prepared,
            base_root: base_root.to_string(),
            candidate_root: candidate_root.to_string(),
            intent_digest: intent.clone(),
        });
        if crash == CrashPoint::AfterPrepare {
            return Err(OverlayError::Journal("crash after prepare".into()));
        }
        self.journal.push(JournalRecord {
            stage: PublicationStage::Verified,
            base_root: base_root.to_string(),
            candidate_root: candidate_root.to_string(),
            intent_digest: intent.clone(),
        });
        if crash == CrashPoint::AfterVerify {
            return Err(OverlayError::Journal("crash after verify".into()));
        }
        if crash == CrashPoint::BeforeBarrier {
            return Err(OverlayError::Journal("crash before barrier".into()));
        }
        self.published_root = Some(candidate_root.to_string());
        self.journal.push(JournalRecord {
            stage: PublicationStage::Published,
            base_root: base_root.to_string(),
            candidate_root: candidate_root.to_string(),
            intent_digest: intent.clone(),
        });
        if crash == CrashPoint::AfterPublish {
            return Err(OverlayError::Journal("crash after publish".into()));
        }
        self.journal.push(JournalRecord {
            stage: PublicationStage::Durable,
            base_root: base_root.to_string(),
            candidate_root: candidate_root.to_string(),
            intent_digest: intent,
        });
        Ok(candidate_root.to_string())
    }

    /// Recover: if Durable or Published present, root is candidate; else abort to base.
    pub fn recover(&mut self, base_root: &str) -> String {
        let mut best = PublicationStage::Aborted;
        let mut root = base_root.to_string();
        for rec in &self.journal {
            match rec.stage {
                PublicationStage::Published | PublicationStage::Durable => {
                    best = rec.stage;
                    root = rec.candidate_root.clone();
                }
                PublicationStage::Prepared | PublicationStage::Verified => {
                    // incomplete — keep base unless later published
                }
                PublicationStage::Restored | PublicationStage::Aborted => {}
            }
        }
        if matches!(
            best,
            PublicationStage::Published | PublicationStage::Durable
        ) {
            self.published_root = Some(root.clone());
            root
        } else {
            self.published_root = Some(base_root.to_string());
            self.journal.push(JournalRecord {
                stage: PublicationStage::Aborted,
                base_root: base_root.to_string(),
                candidate_root: base_root.to_string(),
                intent_digest: intent_digest(base_root, base_root),
            });
            base_root.to_string()
        }
    }
}

impl Default for AtomicPublication {
    fn default() -> Self {
        Self::new()
    }
}

// silence unused FileRecord warning in this module
#[allow(dead_code)]
fn _file_record_link(_: &FileRecord) {}
