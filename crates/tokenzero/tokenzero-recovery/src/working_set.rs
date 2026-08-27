//! Prompt-resident working-set eviction backed by durable recovery refs.

use crate::{RecoveryError, RecoveryStore};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokenzero_core::{ContentType, count_tokens};

pub const DEFAULT_WORKING_SET_TOKENS: usize = 8192;
pub const EVICTION_REF_LINE_PREFIX: &str = "TZ-EVICT/1";
/// Portable one-token response emitted when requested bytes are already resident.
/// `0` is verified as one token by every tokenizer in tokenzero-core's tests/fixtures/one-token-atoms.json.
pub const ALREADY_RESIDENT_ATOM: &str = "0";
/// Alarm threshold for pathological fault/re-eviction cycles.
pub const THRASH_ALARM_FAULT_RATE: f64 = 0.5;
const MAX_PREFETCH_HINTS_PER_FAULT: usize = 1;
/// Cap queued hints so a caller that never drains cannot grow the set without bound.
const MAX_QUEUED_PREFETCH_HINTS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanAnchor {
    pub path: PathBuf,
    pub symbol: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct RehydrationLatencyTelemetry {
    pub samples: u64,
    pub min_us: u64,
    pub mean_us: f64,
    pub max_us: u64,
}

/// Amortized eviction cost. Worst case is one largest-span fault per access.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct EvictionAccounting {
    pub p_fault: f64,
    pub expected_rehydration_tokens: f64,
    pub amortized_tokens_per_access: f64,
    pub actual_rehydration_tokens: u64,
    pub thrash_worst_case_tokens: u64,
    pub alarm: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkingSetTelemetry {
    pub admissions: u64,
    pub evictions: u64,
    pub bytes_evicted: u64,
    pub refs_created: u64,
    pub lookups: u64,
    pub faults: u64,
    pub fault_rate: f64,
    pub rehydrations: u64,
    pub resident_hits: u64,
    pub delta_renders: u64,
    pub dedup_tokens_saved: u64,
    pub churn: u64,
    pub render_rewrites: u64,
    pub fault_hook_calls: u64,
    pub context_edits: u64,
    pub eviction_accounting: EvictionAccounting,
    pub rehydration_latency: RehydrationLatencyTelemetry,
    #[serde(skip)]
    rehydration_latency_total_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvictedSpan {
    pub id: u64,
    pub ref_id: String,
    pub replacement: String,
    pub bytes_evicted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub id: u64,
    pub replacement: Option<String>,
    pub evicted: Vec<EvictedSpan>,
    pub response: WorkingSetResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkingSetResponse {
    Full,
    AlreadyResident,
    Delta {
        acknowledgement: String,
        delta: WorkingSetDelta,
    },
}

impl WorkingSetResponse {
    pub fn visible_text(&self) -> Option<&str> {
        match self {
            Self::Full => None,
            Self::AlreadyResident => Some(ALREADY_RESIDENT_ATOM),
            Self::Delta {
                acknowledgement, ..
            } => Some(acknowledgement),
        }
    }
}

/// A single exact line hunk. Stored line chunks retain their terminators so
/// integration is byte-identical, including a missing final newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingSetDelta {
    pub start_line: usize,
    pub removed: Vec<String>,
    pub inserted: Vec<String>,
}

impl WorkingSetDelta {
    pub fn acknowledgement(&self) -> String {
        let mut output = format!(
            "@@ -{},{} +{},{} @@\n",
            self.start_line,
            self.removed.len(),
            self.start_line,
            self.inserted.len()
        );
        for line in &self.removed {
            output.push('-');
            output.push_str(line);
            if !line.ends_with('\n') {
                output.push('\n');
            }
        }
        for line in &self.inserted {
            output.push('+');
            output.push_str(line);
            if !line.ends_with('\n') {
                output.push('\n');
            }
        }
        output
    }
}

pub fn integrate_delta(base: &str, delta: &WorkingSetDelta) -> Option<String> {
    let mut lines = split_exact_lines(base);
    let start = delta.start_line.checked_sub(1)?;
    let end = start.checked_add(delta.removed.len())?;
    if end > lines.len() || lines[start..end] != delta.removed {
        return None;
    }
    lines.splice(start..end, delta.inserted.iter().cloned());
    Some(lines.concat())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rehydration {
    pub id: u64,
    pub anchor: SpanAnchor,
    pub partial: bool,
    pub evicted: Vec<EvictedSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefetchCandidate {
    pub id: u64,
    pub ref_id: String,
    pub anchor: SpanAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefetchHint {
    pub ref_id: String,
    pub anchor: SpanAnchor,
}

pub trait PrefetchHook: std::fmt::Debug + Send + Sync {
    fn hints(&self, fault: &SpanAnchor, candidates: &[PrefetchCandidate]) -> Vec<PrefetchHint>;
}

#[derive(Debug, Default)]
pub struct NoopPrefetchHook;

impl PrefetchHook for NoopPrefetchHook {
    fn hints(&self, _: &SpanAnchor, _: &[PrefetchCandidate]) -> Vec<PrefetchHint> {
        Vec::new()
    }
}

/// Conservative opt-in policy: queue the nearest evicted span from the same
/// file. The working set only exposes the hint; callers decide whether and
/// when to perform I/O, so the default fault path never triggers speculative I/O.
#[derive(Debug, Default)]
pub struct SameFileNeighborPrefetch;

impl PrefetchHook for SameFileNeighborPrefetch {
    fn hints(&self, fault: &SpanAnchor, candidates: &[PrefetchCandidate]) -> Vec<PrefetchHint> {
        candidates
            .iter()
            .filter(|candidate| candidate.anchor.path == fault.path)
            .min_by_key(|candidate| {
                let distance = candidate
                    .anchor
                    .start_line
                    .saturating_sub(fault.end_line)
                    .max(fault.start_line.saturating_sub(candidate.anchor.end_line));
                (distance, candidate.id)
            })
            .map(|candidate| PrefetchHint {
                ref_id: candidate.ref_id.clone(),
                anchor: candidate.anchor.clone(),
            })
            .into_iter()
            .collect()
    }
}

#[derive(Debug)]
struct ResidentSpan {
    id: u64,
    last_touched: u64,
    anchor: SpanAnchor,
    body: SpanBody,
}

#[derive(Debug)]
enum SpanBody {
    Resident(String),
    Evicted { ref_id: String, replacement: String },
}

impl ResidentSpan {
    fn visible_text(&self) -> &str {
        match &self.body {
            SpanBody::Resident(text) => text,
            SpanBody::Evicted { replacement, .. } => replacement,
        }
    }

    fn visible_tokens(&self) -> usize {
        count_tokens(self.visible_text())
    }
}

/// Session-local prompt working set. Bodies are replaced only after their
/// exact bytes have been persisted through RecoveryStore's blob/CAS path.
#[derive(Debug)]
pub struct WorkingSet {
    budget_tokens: usize,
    sequence: u64,
    spans: Vec<ResidentSpan>,
    evicted_refs: HashMap<String, Vec<u64>>,
    telemetry: WorkingSetTelemetry,
    prefetch_hook: Box<dyn PrefetchHook>,
    prefetch_hints: VecDeque<PrefetchHint>,
    evicted_tokens_total: u64,
    max_evicted_tokens: u64,
}

impl WorkingSet {
    pub fn new(budget_tokens: usize) -> Self {
        Self {
            budget_tokens,
            sequence: 0,
            spans: Vec::new(),
            evicted_refs: HashMap::new(),
            telemetry: WorkingSetTelemetry::default(),
            prefetch_hook: Box::<NoopPrefetchHook>::default(),
            prefetch_hints: VecDeque::new(),
            evicted_tokens_total: 0,
            max_evicted_tokens: 0,
        }
    }

    pub fn register_prefetch_hook(&mut self, hook: Box<dyn PrefetchHook>) {
        self.prefetch_hook = hook;
    }

    pub fn enable_same_file_neighbor_prefetch(&mut self, enabled: bool) {
        self.prefetch_hook = if enabled {
            Box::<SameFileNeighborPrefetch>::default()
        } else {
            Box::<NoopPrefetchHook>::default()
        };
    }

    pub fn take_prefetch_hints(&mut self) -> Vec<PrefetchHint> {
        self.prefetch_hints.drain(..).collect()
    }

    pub fn rewrite_render(
        &mut self,
        store: &mut RecoveryStore,
        text: String,
        anchor: SpanAnchor,
    ) -> Result<Admission, RecoveryError> {
        self.telemetry.render_rewrites = self.telemetry.render_rewrites.saturating_add(1);
        self.admit(store, text, anchor)
    }

    pub fn apply_context_edit(
        &mut self,
        store: &mut RecoveryStore,
        text: String,
        anchor: SpanAnchor,
    ) -> Result<Admission, RecoveryError> {
        self.telemetry.context_edits = self.telemetry.context_edits.saturating_add(1);
        self.admit(store, text, anchor)
    }

    pub fn admit(
        &mut self,
        store: &mut RecoveryStore,
        text: String,
        anchor: SpanAnchor,
    ) -> Result<Admission, RecoveryError> {
        if anchor.start_line == 0 || anchor.start_line > anchor.end_line {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "invalid span line window {}..={}",
                    anchor.start_line, anchor.end_line
                ),
            )
            .into());
        }
        if let Some(index) = self.spans.iter().position(|span| span.anchor == anchor) {
            if let SpanBody::Resident(previous) = &self.spans[index].body {
                if previous == &text {
                    self.sequence = self.sequence.saturating_add(1);
                    self.spans[index].last_touched = self.sequence;
                    self.telemetry.resident_hits = self.telemetry.resident_hits.saturating_add(1);
                    let saved =
                        count_tokens(&text).saturating_sub(count_tokens(ALREADY_RESIDENT_ATOM));
                    self.telemetry.dedup_tokens_saved = self
                        .telemetry
                        .dedup_tokens_saved
                        .saturating_add(saved as u64);
                    return Ok(Admission {
                        id: self.spans[index].id,
                        replacement: None,
                        evicted: Vec::new(),
                        response: WorkingSetResponse::AlreadyResident,
                    });
                }
                let delta = delta_between(previous, &text);
                let acknowledgement = delta.acknowledgement();
                if count_tokens(&acknowledgement) < count_tokens(&text) {
                    let id = self.spans[index].id;
                    let saved = count_tokens(&text).saturating_sub(count_tokens(&acknowledgement));
                    self.sequence = self.sequence.saturating_add(1);
                    self.spans[index].last_touched = self.sequence;
                    self.spans[index].body = SpanBody::Resident(text);
                    self.telemetry.delta_renders = self.telemetry.delta_renders.saturating_add(1);
                    self.telemetry.dedup_tokens_saved = self
                        .telemetry
                        .dedup_tokens_saved
                        .saturating_add(saved as u64);
                    let evicted = self.enforce_budget(store)?;
                    return Ok(Admission {
                        id,
                        replacement: None,
                        evicted,
                        response: WorkingSetResponse::Delta {
                            acknowledgement,
                            delta,
                        },
                    });
                }
            }
            // A changed evicted span or a non-beneficial delta becomes the new
            // baseline; remove the stale anchor before normal admission.
            let stale = self.spans.remove(index);
            if let SpanBody::Evicted { .. } = stale.body {
                self.remove_evicted_ref(store, stale.id);
            }
        }

        let id = self.push_resident(text, anchor);
        let evicted = match self.enforce_budget(store) {
            Ok(evicted) => evicted,
            Err(error) => {
                self.spans.retain(|span| span.id != id);
                return Err(error);
            }
        };
        let replacement = self
            .spans
            .iter()
            .find(|span| span.id == id)
            .and_then(|span| match &span.body {
                SpanBody::Evicted { replacement, .. } => Some(replacement.clone()),
                SpanBody::Resident(_) => None,
            });
        Ok(Admission {
            id,
            replacement,
            evicted,
            response: WorkingSetResponse::Full,
        })
    }

    pub fn handle_fault_hook(
        &mut self,
        store: &mut RecoveryStore,
        ref_id: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
    ) -> Result<Option<Rehydration>, RecoveryError> {
        self.telemetry.fault_hook_calls = self.telemetry.fault_hook_calls.saturating_add(1);
        self.rehydrate_ref(store, ref_id, start_line, end_line)
    }

    /// Demand-page an evicted ref. A ref not owned by this working set costs
    /// one hash-map lookup and returns immediately without touching the store.
    pub fn rehydrate_ref(
        &mut self,
        store: &mut RecoveryStore,
        ref_id: &str,
        start_line: Option<usize>,
        end_line: Option<usize>,
    ) -> Result<Option<Rehydration>, RecoveryError> {
        self.telemetry.lookups = self.telemetry.lookups.saturating_add(1);
        self.refresh_rates();
        let (lookup_ref, fragment_window) = match ref_id.split_once('#') {
            Some((base, fragment)) => {
                let Some(window) = parse_line_fragment(fragment) else {
                    return Ok(None);
                };
                (base, Some(window))
            }
            None => (ref_id, None),
        };
        let Some(id) = self
            .evicted_refs
            .get(lookup_ref)
            .and_then(|ids| ids.iter().copied().min())
        else {
            return Ok(None);
        };

        self.telemetry.faults = self.telemetry.faults.saturating_add(1);
        let started = Instant::now();
        // Expand's fragment on the ref wins over start_line/end_line args.
        // Mirror that here so the advertised absolute window matches the
        // bytes actually returned (not the caller's overridden range).
        let effective_start = fragment_window.map(|window| window.0).or(start_line);
        let effective_end = fragment_window.map(|window| window.1).or(end_line);
        let partial = effective_start.is_some() || effective_end.is_some();
        // Expand the span's canonical blob ref. `lookup_ref` may be a
        // link_refs alias; portable expand rejects many alias spellings
        // before the store alias table is consulted.
        let (source_anchor, canonical_ref, expand_ref) = {
            let span = self
                .spans
                .iter()
                .find(|span| span.id == id)
                .expect("evicted ref index must point at a span");
            let SpanBody::Evicted {
                ref_id: canonical, ..
            } = &span.body
            else {
                panic!("evicted ref index must point at an evicted span");
            };
            let expand_ref = match ref_id.split_once('#') {
                Some((_, fragment)) => format!("{canonical}#{fragment}"),
                None => canonical.clone(),
            };
            (span.anchor.clone(), canonical.clone(), expand_ref)
        };
        let result = store.expand(
            &expand_ref,
            Some("raw"),
            effective_start,
            effective_end,
            None,
            None,
        );
        let elapsed_us = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
        self.record_rehydration_latency(elapsed_us);
        self.refresh_rates();
        if !result.found {
            return Err(std::io::Error::other(format!(
                "working-set rehydrate of owned ref {ref_id} failed: {}",
                result.reason
            ))
            .into());
        }
        let rehydrated_tokens = count_tokens(&result.content) as u64;

        let (resident_id, resident_anchor) = if partial {
            let relative_start = effective_start.unwrap_or(1).max(1);
            let returned_lines = result.content.lines().count().max(1);
            let requested_end = effective_end
                .unwrap_or_else(|| relative_start.saturating_add(returned_lines.saturating_sub(1)));
            let relative_end = requested_end.max(relative_start);
            let absolute_start = source_anchor
                .start_line
                .saturating_add(relative_start.saturating_sub(1));
            let absolute_end = source_anchor
                .start_line
                .saturating_add(relative_end.saturating_sub(1))
                .min(source_anchor.end_line);
            let narrowed = SpanAnchor {
                path: source_anchor.path.clone(),
                symbol: source_anchor.symbol.clone(),
                start_line: absolute_start,
                end_line: absolute_end,
            };
            let resident_id = self.push_resident(result.content, narrowed.clone());
            (resident_id, narrowed)
        } else {
            self.sequence = self.sequence.saturating_add(1);
            let span = self
                .spans
                .iter_mut()
                .find(|span| span.id == id)
                .expect("evicted ref index must point at a span");
            span.body = SpanBody::Resident(result.content);
            span.last_touched = self.sequence;
            self.remove_evicted_ref(store, id);
            self.note_admission();
            (id, source_anchor)
        };

        self.telemetry.rehydrations = self.telemetry.rehydrations.saturating_add(1);
        self.telemetry.eviction_accounting.actual_rehydration_tokens = self
            .telemetry
            .eviction_accounting
            .actual_rehydration_tokens
            .saturating_add(rehydrated_tokens);
        self.refresh_rates();
        self.queue_prefetch_hints(&resident_anchor, resident_id, &canonical_ref);
        let evicted = self.enforce_budget(store)?;
        Ok(Some(Rehydration {
            id: resident_id,
            anchor: resident_anchor,
            partial,
            evicted,
        }))
    }

    pub fn touch(&mut self, id: u64) -> bool {
        let Some(span) = self.spans.iter_mut().find(|span| span.id == id) else {
            return false;
        };
        self.sequence = self.sequence.saturating_add(1);
        span.last_touched = self.sequence;
        true
    }

    /// Page out one resident span: drop visible text, keep the exact blob ref.
    /// Returns `None` when `id` is missing or already evicted (no mutation).
    pub fn evict(
        &mut self,
        store: &mut RecoveryStore,
        id: u64,
    ) -> Result<Option<EvictedSpan>, RecoveryError> {
        let Some(victim) = self
            .spans
            .iter()
            .position(|span| span.id == id && matches!(span.body, SpanBody::Resident(_)))
        else {
            return Ok(None);
        };
        self.page_out_at(store, victim).map(Some)
    }

    pub fn evicted_refs(&self) -> &HashMap<String, Vec<u64>> {
        &self.evicted_refs
    }

    pub fn anchor_for_path(&self, path: &Path) -> Option<SpanAnchor> {
        self.spans
            .iter()
            .find(|span| span.anchor.path == path)
            .map(|span| span.anchor.clone())
    }

    /// Record that `alias` recovers the same spans as `source`.
    /// Persists `alias -> source` so `RecoveryStore::expand` can resolve it
    /// after restart. Returns `Ok(false)` when `source` is unknown, equal to
    /// `alias`, or the map is unchanged. Persist failure is `Err`, never a
    /// successful unpersisted serve.
    pub fn link_refs(
        &mut self,
        store: &mut RecoveryStore,
        source: &str,
        alias: &str,
    ) -> Result<bool, RecoveryError> {
        if source.is_empty() || alias.is_empty() || source == alias {
            return Ok(false);
        }
        let Some(ids) = self.evicted_refs.get(source).cloned() else {
            return Ok(false);
        };
        if ids.is_empty() {
            return Ok(false);
        }
        let already = self
            .evicted_refs
            .get(alias)
            .is_some_and(|entry| ids.iter().all(|id| entry.contains(id)));
        if already {
            return Ok(false);
        }
        store.store_alias(alias, source)?;
        let entry = self.evicted_refs.entry(alias.to_string()).or_default();
        for id in ids {
            if !entry.contains(&id) {
                entry.push(id);
            }
        }
        Ok(true)
    }

    pub fn used_tokens(&self) -> usize {
        self.spans.iter().map(ResidentSpan::visible_tokens).sum()
    }

    pub fn visible_lines(&self) -> Vec<&str> {
        self.spans.iter().map(ResidentSpan::visible_text).collect()
    }

    pub fn telemetry(&self) -> WorkingSetTelemetry {
        self.telemetry
    }

    fn push_resident(&mut self, text: String, anchor: SpanAnchor) -> u64 {
        self.sequence = self.sequence.saturating_add(1);
        let id = self.sequence;
        self.spans.push(ResidentSpan {
            id,
            last_touched: self.sequence,
            anchor,
            body: SpanBody::Resident(text),
        });
        self.note_admission();
        id
    }

    fn note_admission(&mut self) {
        self.telemetry.admissions = self.telemetry.admissions.saturating_add(1);
        self.telemetry.churn = self.telemetry.churn.saturating_add(1);
    }

    fn record_rehydration_latency(&mut self, elapsed_us: u64) {
        let latency = &mut self.telemetry.rehydration_latency;
        latency.samples = latency.samples.saturating_add(1);
        latency.min_us = if latency.samples == 1 {
            elapsed_us
        } else {
            latency.min_us.min(elapsed_us)
        };
        latency.max_us = latency.max_us.max(elapsed_us);
        self.telemetry.rehydration_latency_total_us = self
            .telemetry
            .rehydration_latency_total_us
            .saturating_add(elapsed_us);
        latency.mean_us =
            self.telemetry.rehydration_latency_total_us as f64 / latency.samples as f64;
    }

    fn refresh_rates(&mut self) {
        self.telemetry.fault_rate = if self.telemetry.lookups != 0 {
            self.telemetry.faults as f64 / self.telemetry.lookups as f64
        } else {
            0.0
        };
        let accounting = &mut self.telemetry.eviction_accounting;
        accounting.p_fault = self.telemetry.fault_rate;
        accounting.expected_rehydration_tokens = if self.telemetry.evictions == 0 {
            0.0
        } else {
            self.evicted_tokens_total as f64 / self.telemetry.evictions as f64
        };
        accounting.amortized_tokens_per_access =
            accounting.p_fault * accounting.expected_rehydration_tokens;
        accounting.thrash_worst_case_tokens = self.max_evicted_tokens;
        accounting.alarm = accounting.p_fault >= THRASH_ALARM_FAULT_RATE
            && accounting.amortized_tokens_per_access >= 1.0;
    }

    fn queue_prefetch_hints(&mut self, fault: &SpanAnchor, fault_id: u64, fault_ref: &str) {
        let candidates = self
            .spans
            .iter()
            .filter_map(|span| match &span.body {
                SpanBody::Evicted { ref_id, .. } if span.id != fault_id && ref_id != fault_ref => {
                    Some(PrefetchCandidate {
                        id: span.id,
                        ref_id: ref_id.clone(),
                        anchor: span.anchor.clone(),
                    })
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        self.prefetch_hints.extend(
            self.prefetch_hook
                .hints(fault, &candidates)
                .into_iter()
                .take(MAX_PREFETCH_HINTS_PER_FAULT),
        );
        while self.prefetch_hints.len() > MAX_QUEUED_PREFETCH_HINTS {
            self.prefetch_hints.pop_front();
        }
    }

    fn remove_evicted_ref(&mut self, store: &mut RecoveryStore, id: u64) {
        let mut empty_keys = Vec::new();
        for (key, ids) in &mut self.evicted_refs {
            ids.retain(|candidate| *candidate != id);
            if ids.is_empty() {
                empty_keys.push(key.clone());
            }
        }
        for key in empty_keys {
            self.evicted_refs.remove(&key);
            if store.alias_target(&key).is_some() {
                store.remove_alias(&key);
            }
        }
    }

    fn enforce_budget(
        &mut self,
        store: &mut RecoveryStore,
    ) -> Result<Vec<EvictedSpan>, RecoveryError> {
        let mut evicted = Vec::new();
        while self.used_tokens() > self.budget_tokens {
            let victim = self
                .spans
                .iter()
                .enumerate()
                .filter_map(|(index, span)| match &span.body {
                    SpanBody::Resident(text) => {
                        let replacement_floor = format_ref_line(
                            "tz://blob/".to_string() + &"0".repeat(64),
                            &span.anchor,
                        );
                        let floor_tokens = count_tokens(&replacement_floor);
                        (count_tokens(text) > floor_tokens).then_some((
                            index,
                            span.last_touched,
                            count_tokens(text),
                            span.id,
                        ))
                    }
                    SpanBody::Evicted { .. } => None,
                })
                .min_by(|a, b| {
                    a.1.cmp(&b.1)
                        .then_with(|| b.2.cmp(&a.2))
                        .then_with(|| a.3.cmp(&b.3))
                })
                .map(|candidate| candidate.0);
            let Some(victim) = victim else { break };
            evicted.push(self.page_out_at(store, victim)?);
        }
        // Best-effort: spans already reduced to their eviction markers cannot
        // shrink further, so a budget below the marker floor is served at the
        // floor rather than failing the admission (which would strand the full
        // inline text at the caller - the exact opposite of paging out).
        Ok(evicted)
    }

    fn page_out_at(
        &mut self,
        store: &mut RecoveryStore,
        victim: usize,
    ) -> Result<EvictedSpan, RecoveryError> {
        let (bytes, anchor, id) = {
            let span = &self.spans[victim];
            let SpanBody::Resident(text) = &span.body else {
                unreachable!("page_out_at requires a resident span")
            };
            (text, &span.anchor, span.id)
        };
        let ref_id = store.store_blob(bytes, ContentType::Unknown)?;
        let replacement = format_ref_line(ref_id.clone(), anchor);
        let bytes_evicted = bytes.len();
        let tokens_evicted = count_tokens(bytes) as u64;
        self.evicted_tokens_total = self.evicted_tokens_total.saturating_add(tokens_evicted);
        self.max_evicted_tokens = self.max_evicted_tokens.max(tokens_evicted);
        self.spans[victim].body = SpanBody::Evicted {
            ref_id: ref_id.clone(),
            replacement: replacement.clone(),
        };
        self.evicted_refs
            .entry(ref_id.clone())
            .or_default()
            .push(id);
        self.telemetry.evictions = self.telemetry.evictions.saturating_add(1);
        self.telemetry.churn = self.telemetry.churn.saturating_add(1);
        self.telemetry.bytes_evicted = self
            .telemetry
            .bytes_evicted
            .saturating_add(bytes_evicted as u64);
        self.telemetry.refs_created = self.telemetry.refs_created.saturating_add(1);
        self.refresh_rates();
        Ok(EvictedSpan {
            id,
            ref_id,
            replacement,
            bytes_evicted,
        })
    }
}

fn split_exact_lines(text: &str) -> Vec<String> {
    // Match RecoveryStore::content_line_count: empty/0-byte text is 0 lines.
    // `split_inclusive` yields one empty remainder, which made empty-to-empty
    // deltas start at line 2 and treat a 0-byte body as line 1.
    if text.is_empty() {
        Vec::new()
    } else {
        text.split_inclusive('\n').map(str::to_owned).collect()
    }
}

fn delta_between(old: &str, new: &str) -> WorkingSetDelta {
    let old_lines = split_exact_lines(old);
    let new_lines = split_exact_lines(new);
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(left, right)| left == right)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    WorkingSetDelta {
        start_line: prefix + 1,
        removed: old_lines[prefix..old_lines.len().saturating_sub(suffix)].to_vec(),
        inserted: new_lines[prefix..new_lines.len().saturating_sub(suffix)].to_vec(),
    }
}

pub fn format_ref_line(ref_id: String, anchor: &SpanAnchor) -> String {
    let path =
        serde_json::to_string(&normalize_path(&anchor.path)).unwrap_or_else(|_| r#""#.to_string());
    let mut line = format!("{EVICTION_REF_LINE_PREFIX} ref={ref_id} path={path}");
    if let Some(symbol) = anchor.symbol.as_deref() {
        let symbol = serde_json::to_string(symbol).unwrap_or_else(|_| r#""#.to_string());
        line.push_str(" symbol=");
        line.push_str(&symbol);
    }
    line.push_str(&format!(" lines={}-{}", anchor.start_line, anchor.end_line));
    line
}

fn parse_line_fragment(fragment: &str) -> Option<(usize, usize)> {
    let range = fragment.strip_prefix('L')?;
    let (start, end) = match range.split_once('-') {
        Some((start, end)) => (start.parse().ok()?, end.parse().ok()?),
        None => {
            let line = range.parse().ok()?;
            (line, line)
        }
    };
    (start > 0 && start <= end).then_some((start, end))
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
