//! Exact-payload session deduplication, delta telemetry, and persisted rollup state.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum ServeKey {
    File {
        path: PathBuf,
        start: Option<usize>,
        end: Option<usize>,
    },
    Output {
        tool: String,
        query: String,
        roots: Vec<PathBuf>,
    },
    Expand {
        ref_id: String,
        start_line: Option<usize>,
        end_line: Option<usize>,
        selector_norm: String,
        symbol_norm: String,
        anchor_kind_norm: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServedRecord {
    pub content_sha256: String,
    pub blob_ref: String,
    #[allow(dead_code)]
    pub file_ref: String,
    #[allow(dead_code)]
    pub raw_tokens: usize,
    pub line_count: usize,
    pub byte_len: usize,
    #[allow(dead_code)]
    #[serde(
        rename = "served_at_unix_secs",
        default = "SystemTime::now",
        serialize_with = "serialize_served_at",
        deserialize_with = "deserialize_served_at"
    )]
    pub served_at: SystemTime,
    pub serve_count: usize,
}

fn serialize_served_at<S: serde::Serializer>(
    time: &SystemTime,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
        .serialize(serializer)
}

fn deserialize_served_at<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<SystemTime, D::Error> {
    Ok(Option::<u64>::deserialize(deserializer)?
        .and_then(|secs| SystemTime::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(secs)))
        .unwrap_or_else(SystemTime::now))
}

#[derive(Debug, Clone)]
pub(crate) enum SeenState {
    Miss,
    Unchanged {
        serve_count: usize,
        cross_session: bool,
    },
    Changed {
        previous: ServedRecord,
    },
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(crate) struct SessionRollup {
    pub(crate) dedup_hits: usize,
    pub(crate) diff_hits: usize,
    pub(crate) visible_tokens_saved: usize,
    pub(crate) diff_tokens_saved: usize,
    #[serde(default)]
    pub(crate) full_bytes: usize,
    #[serde(default)]
    pub(crate) delta_bytes: usize,
}

#[derive(Debug, Default)]
pub struct SessionMemory {
    records: HashMap<ServeKey, ServedRecord>,
    restored_content_hashes: HashSet<String>,
    rollup: SessionRollup,
    session_hwm: u64,
}

impl SessionMemory {
    pub(crate) fn lookup(&self, key: &ServeKey, content_sha256: &str) -> SeenState {
        let cross_session = self.restored_content_hashes.contains(content_sha256);
        match self.records.get(key) {
            Some(record) if record.content_sha256 == content_sha256 => SeenState::Unchanged {
                serve_count: record.serve_count,
                cross_session,
            },
            Some(record) => SeenState::Changed {
                previous: record.clone(),
            },
            None => self
                .records
                .values()
                .filter(|record| record.content_sha256 == content_sha256)
                .map(|record| record.serve_count)
                .max()
                .map(|serve_count| SeenState::Unchanged {
                    serve_count,
                    cross_session,
                })
                .unwrap_or(SeenState::Miss),
        }
    }

    pub(crate) fn record(&mut self, key: ServeKey, mut record: ServedRecord) {
        if let Some(existing) = self.records.get(&key) {
            record.serve_count = existing.serve_count + 1;
        }
        self.records.insert(key, record);
    }

    pub(crate) fn absorb(&mut self, summary: &SessionSummary) {
        self.rollup.dedup_hits += summary.dedup_notes;
        self.rollup.diff_hits += summary.diff_serves;
        self.rollup.visible_tokens_saved += summary.visible_saved;
        self.rollup.diff_tokens_saved += summary.diff_saved;
    }

    pub(crate) fn restore_from_persist(
        &mut self,
        records: HashMap<ServeKey, ServedRecord>,
        rollup: SessionRollup,
        session_hwm: u64,
    ) {
        self.restored_content_hashes = records
            .values()
            .map(|record| record.content_sha256.clone())
            .collect();
        self.records = records;
        self.rollup = rollup;
        self.session_hwm = session_hwm;
    }

    pub(crate) fn records_snapshot(&self) -> &HashMap<ServeKey, ServedRecord> {
        &self.records
    }

    pub(crate) fn persisted_rollup(&self) -> SessionRollup {
        self.rollup.clone()
    }

    pub(crate) fn session_hwm(&self) -> u64 {
        self.session_hwm
    }

    pub(crate) fn advance_hwm(&mut self) -> (u64, u64) {
        let from = self.session_hwm;
        self.session_hwm = self.session_hwm.saturating_add(1);
        (from, self.session_hwm)
    }

    pub(crate) fn note_bytes(&mut self, full: usize, delta: usize) {
        self.rollup.full_bytes = self.rollup.full_bytes.saturating_add(full);
        self.rollup.delta_bytes = self.rollup.delta_bytes.saturating_add(delta);
    }

    pub fn rollup(&self) -> Value {
        json!({
            "records": self.records.len(),
            "dedup_hits": self.rollup.dedup_hits,
            "diff_hits": self.rollup.diff_hits,
            "visible_tokens_saved": self.rollup.visible_tokens_saved,
            "diff_tokens_saved": self.rollup.diff_tokens_saved,
            "session_hwm": self.session_hwm,
            "full_bytes": self.rollup.full_bytes,
            "delta_bytes": self.rollup.delta_bytes
        })
    }
}

#[derive(Debug, Default)]
pub(crate) struct SessionSummary {
    pub dedup_notes: usize,
    pub diff_serves: usize,
    pub visible_saved: usize,
    pub diff_saved: usize,
    pub serve_count: usize,
    pub cross_session_hits: usize,
    pub diff: Option<DiffTelemetry>,
    pub full_bytes: Option<usize>,
    pub delta_bytes: Option<usize>,
    pub from_hwm: u64,
    pub to_hwm: u64,
    /// Spans masked by the expand secret gate (yevj); 0 = no masking.
    pub secret_masked: usize,
}

#[derive(Debug, Clone)]
pub struct DiffTelemetry {
    pub hunks: usize,
    pub plus: usize,
    pub minus: usize,
    pub base_ref: String,
}

impl SessionSummary {
    pub fn note_dedup(&mut self, serve_count: usize, saved: usize, cross_session: bool) {
        self.dedup_notes += 1;
        self.serve_count = serve_count;
        self.visible_saved += saved;
        self.cross_session_hits += usize::from(cross_session);
    }

    pub fn note_diff(&mut self, info: DiffTelemetry, saved: usize) {
        self.diff_serves += 1;
        self.diff_saved += saved;
        self.diff = Some(info);
    }

    pub fn note_wire_bytes(&mut self, full_bytes: usize, delta_bytes: usize) {
        self.full_bytes = Some(full_bytes);
        self.delta_bytes = Some(delta_bytes);
    }

    pub fn set_watermark(&mut self, from_hwm: u64, to_hwm: u64) {
        self.from_hwm = from_hwm;
        self.to_hwm = to_hwm;
    }

    pub fn note_secret_masking(&mut self, masked_spans: usize) {
        self.secret_masked += masked_spans;
    }

    pub fn secret_masked_count(&self) -> usize {
        self.secret_masked
    }

    pub fn telemetry(&self) -> Option<Value> {
        let strategy = match (self.dedup_notes > 0, self.diff_serves > 0) {
            (true, true) => "seen_set_dedup+diff_since_served",
            (true, false) => "seen_set_dedup",
            (false, true) => "diff_since_served",
            (false, false) if self.full_bytes.is_some() => "full",
            (false, false) if self.secret_masked > 0 => "exact_masked",
            (false, false) => return None,
        };
        let mut value = json!({
            "output_strategy": strategy,
            "cache_hit": self.dedup_notes > 0 || self.diff_serves > 0
        });
        if self.secret_masked > 0 {
            // Loud receipt for the yevj secret gate: how many spans were
            // masked and that stored bytes are untouched.
            value["secret_masking"] = json!({
                "masked_spans": self.secret_masked,
                "stored_bytes_modified": false
            });
        }
        if let (Some(full_bytes), Some(delta_bytes)) = (self.full_bytes, self.delta_bytes) {
            value["session_delta"] = json!({
                "from_hwm": self.from_hwm,
                "to_hwm": self.to_hwm,
                "full_bytes": full_bytes,
                "delta_bytes": delta_bytes,
                "saved_bytes": full_bytes.saturating_sub(delta_bytes)
            });
        }
        if self.dedup_notes > 0 {
            let cross_session_bytes_saved = if self.cross_session_hits > 0 {
                self.full_bytes
                    .zip(self.delta_bytes)
                    .map(|(full, delta)| full.saturating_sub(delta))
                    .unwrap_or(0)
            } else {
                0
            };
            value["dedup"] = json!({
                "hits": self.dedup_notes,
                "serve_count": self.serve_count,
                "visible_tokens_saved": self.visible_saved,
                "cross_session_hits": self.cross_session_hits,
                "cross_session_bytes_saved": cross_session_bytes_saved
            });
        }
        if let Some(diff) = &self.diff {
            value["diff"] = json!({ "hunks": diff.hunks, "plus": diff.plus, "minus": diff.minus, "base_ref": diff.base_ref, "visible_tokens_saved": self.diff_saved });
        }
        Some(value)
    }
}

