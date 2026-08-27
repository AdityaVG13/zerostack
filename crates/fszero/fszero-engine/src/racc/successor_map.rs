//! Old-ref → successor-ref validity map (fszero-09kg).

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuccessorMapError {
    Ambiguous { old_ref: String },
    UnknownFate(String),
}

impl std::fmt::Display for SuccessorMapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ambiguous { old_ref } => write!(f, "ambiguous successor for {old_ref}"),
            Self::UnknownFate(s) => write!(f, "unknown fate: {s}"),
        }
    }
}
impl std::error::Error for SuccessorMapError {}

/// Fate of a pre-transition ref after a project mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefFate {
    Unchanged {
        ref_id: String,
    },
    Moved {
        from: String,
        to: String,
    },
    Modified {
        from: String,
        to: String,
    },
    Removed {
        from: String,
    },
    Ambiguous {
        from: String,
        candidates: Vec<String>,
    },
}

/// Successor identity map: feeds cache invalidation, not semantic graph claims.
#[derive(Debug, Clone, Default)]
pub struct SuccessorMap {
    by_old: BTreeMap<String, RefFate>,
}

impl SuccessorMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, fate: RefFate) -> Result<(), SuccessorMapError> {
        let key = match &fate {
            RefFate::Unchanged { ref_id } => ref_id.clone(),
            RefFate::Moved { from, .. }
            | RefFate::Modified { from, .. }
            | RefFate::Removed { from }
            | RefFate::Ambiguous { from, .. } => from.clone(),
        };
        if let Some(RefFate::Ambiguous { .. }) = self.by_old.get(&key) {
            return Err(SuccessorMapError::Ambiguous { old_ref: key });
        }
        if self.by_old.contains_key(&key) {
            if let Some(prev) = self.by_old.remove(&key) {
                let candidates = match prev {
                    RefFate::Moved { to, .. } | RefFate::Modified { to, .. } => vec![to],
                    RefFate::Unchanged { ref_id } => vec![ref_id],
                    RefFate::Removed { .. } => vec![],
                    RefFate::Ambiguous { candidates, .. } => candidates,
                };
                let mut candidates = candidates;
                match &fate {
                    RefFate::Moved { to, .. } | RefFate::Modified { to, .. } => {
                        candidates.push(to.clone())
                    }
                    RefFate::Unchanged { ref_id } => candidates.push(ref_id.clone()),
                    _ => {}
                }
                self.by_old.insert(
                    key.clone(),
                    RefFate::Ambiguous {
                        from: key,
                        candidates,
                    },
                );
                return Ok(());
            }
        }
        self.by_old.insert(key, fate);
        Ok(())
    }

    pub fn lookup(&self, old_ref: &str) -> Option<&RefFate> {
        self.by_old.get(old_ref)
    }

    pub fn is_valid_successor(&self, old_ref: &str, new_ref: &str) -> bool {
        match self.by_old.get(old_ref) {
            Some(RefFate::Unchanged { ref_id }) => ref_id == new_ref,
            Some(RefFate::Moved { to, .. }) | Some(RefFate::Modified { to, .. }) => to == new_ref,
            Some(RefFate::Removed { .. }) => false,
            Some(RefFate::Ambiguous { .. }) | None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.by_old.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_old.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &RefFate)> {
        self.by_old.iter()
    }

    /// Deterministic wire form for durable storage (JSON object map).
    pub fn to_wire_json(&self) -> String {
        let mut map = serde_json::Map::new();
        for (old, fate) in &self.by_old {
            let v = match fate {
                RefFate::Unchanged { ref_id } => {
                    serde_json::json!({"kind": "unchanged", "to": ref_id})
                }
                RefFate::Moved { to, .. } => serde_json::json!({"kind": "moved", "to": to}),
                RefFate::Modified { to, .. } => {
                    serde_json::json!({"kind": "modified", "to": to})
                }
                RefFate::Removed { .. } => serde_json::json!({"kind": "removed"}),
                RefFate::Ambiguous { candidates, .. } => {
                    serde_json::json!({"kind": "ambiguous", "candidates": candidates})
                }
            };
            map.insert(old.clone(), v);
        }
        serde_json::Value::Object(map).to_string()
    }

    /// Parse wire form produced by [`to_wire_json`].
    pub fn from_wire_json(s: &str) -> Result<Self, SuccessorMapError> {
        let v: serde_json::Value = serde_json::from_str(s)
            .map_err(|e| SuccessorMapError::UnknownFate(format!("wire json: {e}")))?;
        let obj = v
            .as_object()
            .ok_or_else(|| SuccessorMapError::UnknownFate("wire root must be object".into()))?;
        let mut m = Self::new();
        for (old, fate_v) in obj {
            let kind = fate_v
                .get("kind")
                .and_then(|k| k.as_str())
                .ok_or_else(|| SuccessorMapError::UnknownFate("missing kind".into()))?;
            let fate = match kind {
                "unchanged" => RefFate::Unchanged {
                    ref_id: fate_v
                        .get("to")
                        .and_then(|t| t.as_str())
                        .unwrap_or(old)
                        .to_string(),
                },
                "moved" => RefFate::Moved {
                    from: old.clone(),
                    to: fate_v
                        .get("to")
                        .and_then(|t| t.as_str())
                        .ok_or_else(|| SuccessorMapError::UnknownFate("moved needs to".into()))?
                        .to_string(),
                },
                "modified" => RefFate::Modified {
                    from: old.clone(),
                    to: fate_v
                        .get("to")
                        .and_then(|t| t.as_str())
                        .ok_or_else(|| SuccessorMapError::UnknownFate("modified needs to".into()))?
                        .to_string(),
                },
                "removed" => RefFate::Removed { from: old.clone() },
                "ambiguous" => {
                    let candidates = fate_v
                        .get("candidates")
                        .and_then(|c| c.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    RefFate::Ambiguous {
                        from: old.clone(),
                        candidates,
                    }
                }
                other => return Err(SuccessorMapError::UnknownFate(other.to_string())),
            };
            m.by_old.insert(old.clone(), fate);
        }
        Ok(m)
    }
}
