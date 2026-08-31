//! Memory schema types, index, and export/input envelopes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::store::query::{Snapshot, path_record_for_rel};

use super::anchors::{anchor_path_drifted_now, path_drifted};
use super::persistence::{index_fact, mem_dir, mem_ref};

pub const MAX_FACT_TEXT: usize = 500;
pub const MAX_ANCHORS: usize = 16;
pub const MEMORY_EXPORT_SCHEMA: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Decision,
    Invariant,
    Gotcha,
    Note,
}

impl MemoryKind {
    pub fn parse(s: Option<&str>) -> Self {
        match s.map(str::trim).map(str::to_lowercase).as_deref() {
            Some("decision") => Self::Decision,
            Some("invariant") => Self::Invariant,
            Some("gotcha") => Self::Gotcha,
            _ => Self::Note,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Invariant => "invariant",
            Self::Gotcha => "gotcha",
            Self::Note => "note",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnchorResolution {
    pub anchor: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_ref: Option<String>,
    pub drifted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryFact {
    pub id: String,
    pub ts: u64,
    pub kind: MemoryKind,
    pub text: String,
    pub anchors: Vec<String>,
    pub anchor_resolutions: Vec<AnchorResolution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supersedes: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct MemoryIndex {
    by_path: HashMap<String, Vec<String>>,
    by_symbol: HashMap<String, Vec<String>>,
    by_id: BTreeMap<String, MemoryFact>,
    superseded: HashSet<String>,
}

impl MemoryIndex {
    pub fn load(store_root: &Path) -> Result<Self> {
        let dir = mem_dir(store_root);
        let mut by_id = BTreeMap::new();
        let mut by_path = HashMap::new();
        let mut by_symbol = HashMap::new();
        if !dir.is_dir() {
            return Ok(Self {
                by_path,
                by_symbol,
                by_id,
                superseded: HashSet::new(),
            });
        }
        let mut superseded = HashSet::new();
        for entry in fs::read_dir(&dir).context("read mem dir")? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let bytes = fs::read(&path)?;
            let fact: MemoryFact = serde_json::from_slice(&bytes).context("parse mem fact")?;
            superseded.extend(fact.supersedes.iter().cloned());
            index_fact(&mut by_path, &mut by_symbol, &fact);
            by_id.insert(fact.id.clone(), fact);
        }
        Ok(Self {
            by_path,
            by_symbol,
            by_id,
            superseded,
        })
    }

    pub fn facts_for_target(&self, target: &str) -> Vec<&MemoryFact> {
        let mut ids = HashSet::new();
        for id in self.by_path.get(target).into_iter().flatten() {
            ids.insert(id.clone());
        }
        for id in self.by_symbol.get(target).into_iter().flatten() {
            ids.insert(id.clone());
        }
        for fact in self.by_id.values() {
            if fact.anchors.iter().any(|a| a == target) {
                ids.insert(fact.id.clone());
            }
        }
        let mut out: Vec<&MemoryFact> = ids
            .iter()
            .filter(|id| !self.superseded.contains(*id))
            .filter_map(|id| self.by_id.get(id))
            .collect();
        out.sort_by_key(|f| std::cmp::Reverse(f.ts));
        out
    }

    pub fn all_facts(&self) -> Vec<&MemoryFact> {
        self.by_id.values().collect()
    }

    pub fn active_facts(&self) -> Vec<&MemoryFact> {
        self.by_id
            .iter()
            .filter(|(id, _)| !self.superseded.contains(*id))
            .map(|(_, fact)| fact)
            .collect()
    }

    pub fn is_superseded(&self, id: &str) -> bool {
        self.superseded.contains(id)
    }

    pub fn facts_for_target_including_superseded(&self, target: &str) -> Vec<&MemoryFact> {
        let mut ids = HashSet::new();
        for id in self.by_path.get(target).into_iter().flatten() {
            ids.insert(id.clone());
        }
        for id in self.by_symbol.get(target).into_iter().flatten() {
            ids.insert(id.clone());
        }
        for fact in self.by_id.values() {
            if fact.anchors.iter().any(|a| a == target) {
                ids.insert(fact.id.clone());
            }
        }
        let mut out: Vec<&MemoryFact> = ids.iter().filter_map(|id| self.by_id.get(id)).collect();
        out.sort_by_key(|f| std::cmp::Reverse(f.ts));
        out
    }

    pub fn hints_for_path(&self, snapshot: &Snapshot, rel: &str, limit: usize) -> Vec<MemoryHint> {
        self.facts_for_target(rel)
            .into_iter()
            .take(limit)
            .map(|f| MemoryHint::from_fact(snapshot, f, rel))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryHint {
    pub line: String,
}

impl MemoryHint {
    fn from_fact(snapshot: &Snapshot, fact: &MemoryFact, path: &str) -> Self {
        let drifted =
            fact.anchor_resolutions.iter().any(|r| {
                r.path.as_deref() == Some(path) && anchor_path_drifted_now(snapshot, path, r)
            }) || fact.anchors.iter().any(|a| {
                a == path
                    && path_record_for_rel(snapshot, path)
                        .is_some_and(|(h, rec)| path_drifted(snapshot, path, &h, rec))
            });
        let preview: String = fact.text.chars().take(80).collect();
        let suffix = if drifted { " (anchor drifted)" } else { "" };
        Self {
            line: format!(
                "mem: {}: {} ({}){}",
                fact.kind.as_str(),
                preview,
                mem_ref(&fact.id),
                suffix
            ),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct RememberInput {
    pub text: String,
    #[serde(default)]
    pub anchors: Vec<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub supersedes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryExport {
    pub schema: u32,
    pub active_only: bool,
    pub facts: Vec<MemoryFact>,
}
