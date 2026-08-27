use super::config::{ServeFlight, new_session_id};
use super::session_persist::{SessionPersistSnapshot, SessionPersistence};
use super::*;

impl TokenZeroEngine {
    pub fn new(config: EngineConfig) -> Self {
        Self::new_with_response_ledger(config, true)
    }

    /// Build a one-shot CLI engine.
    ///
    /// The response ledger writes the first record directly without starting the
    /// background flush scheduler, preserving accounting with no idle worker.
    pub fn new_cli(config: EngineConfig) -> Self {
        Self::new_with_response_ledger(config, true)
    }

    fn new_with_response_ledger(config: EngineConfig, response_ledger: bool) -> Self {
        // Self-cleaning storage: every engine reclaims abandoned temp files and
        // aged spills, so users never have to run cache maintenance by hand.
        // Coalesced so concurrent constructors do not multiply scan/sort work.
        let _ = cache_maintenance_coalesced(&config.cache_path, false);
        let metrics = metrics::ToolMetrics::new(&config.cache_path);
        let session_id = new_session_id();
        let ledger = response_ledger.then(|| {
            let repo = config
                .allowed_roots
                .first()
                .map(PathBuf::as_path)
                .or_else(|| config.cache_path.parent())
                .unwrap_or_else(|| Path::new("."))
                .to_string_lossy()
                .into_owned();
            let optimization_tags = vec![
                format!(
                    "session_dedup:{}",
                    if config.session_dedup { "on" } else { "off" }
                ),
                format!(
                    "diff_reads:{}",
                    if config.diff_reads { "on" } else { "off" }
                ),
                format!("tool_surface:{}", config.tool_surface),
                format!("ratc:{}", crate::config::RATC_STATUS_ADVISORY),
            ];
            crate::ledger::LedgerWriter::new(
                &config.cache_path,
                session_id.clone(),
                repo,
                optimization_tags,
                config.ratc,
            )
        });
        let session_persist =
            SessionPersistence::for_cache(&config.cache_path, config.session_dedup);
        // Persisted session records are a demand-paged working set. Loading them
        // (and session_boot) here would make cold CLI boot proportional to prior
        // session size; both are opened on first use instead.
        let cache_path = config.cache_path.clone();
        let exposure_scope = crate::session_persist::session_scope_id(&config.cache_path);
        Self {
            config,
            rg_binary: OnceLock::new(),
            session: Mutex::new(None),
            working_set: Mutex::new(tokenzero_recovery::working_set::WorkingSet::new(
                tokenzero_recovery::working_set::DEFAULT_WORKING_SET_TOKENS,
            )),
            recovery_store: response_ledger
                .then(|| Mutex::new(Some(RecoveryStore::new(Some(cache_path))))),
            in_flight: (Mutex::new(HashSet::new()), Condvar::new()),
            session_id,
            ledger,
            metrics,
            session_persist,
            session_boot: OnceLock::new(),
            surface_health: std::sync::Arc::new(crate::surface_health::SurfaceHealth::new()),
            exposure: crate::exposure::session_exposure_ledger(&exposure_scope),
            lifecycle: Mutex::new(InitializeState::Uninitialized),
        }
    }

    /// The session exposure ledger for this engine's scope (vz89.10).
    pub fn session_exposure(
        &self,
    ) -> std::sync::Arc<Mutex<crate::exposure::SessionExposureLedger>> {
        std::sync::Arc::clone(&self.exposure)
    }

    /// Record a re-expansion of a session-known object; returns the running
    /// replay count when the ref was previously exposed to this session.
    pub fn record_session_reexpansion(&self, ref_id: &str) -> Option<u64> {
        self.exposure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .record_reexpansion(ref_id, None)
    }

    /// Mark the MCP initialize lifecycle Ready for unit tests that exercise
    /// tools without replaying the full initialize handshake.
    pub fn mark_lifecycle_ready_for_tests(&self) {
        if let Ok(mut state) = self.lifecycle.lock() {
            *state = InitializeState::Ready;
        }
    }

    /// Build an engine that shares crash-only health with a parent session.
    pub fn with_shared_surface_health(
        config: EngineConfig,
        surface_health: std::sync::Arc<crate::surface_health::SurfaceHealth>,
    ) -> Self {
        let mut engine = Self::new(config);
        engine.surface_health = surface_health;
        engine
    }

    /// Stable, bounded boot capsule and exact attribution buckets.
    pub fn session_boot_snapshot(&self) -> Value {
        let working_set_loaded = self
            .session
            .lock()
            .map(|slot| slot.is_some())
            .unwrap_or(false);
        let boot = self.session_boot.get_or_init(|| {
            let boot_root = self
                .config
                .allowed_roots
                .first()
                .cloned()
                .unwrap_or_else(|| {
                    self.config
                        .cache_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .to_path_buf()
                });
            tokenzero_recovery::boot::open_session_boot(
                &self.config.cache_path,
                &boot_root,
                &self.config.allowed_roots,
            )
            .ok()
        });
        let mut snapshot = match boot {
            Some(boot) => serde_json::to_value(boot).unwrap_or_else(|_| json!({})),
            None => {
                let total = count_tokens("TZ/1 fallback=metadata_unavailable");
                json!({
                    "schema": "tokenzero.session-boot.v1",
                    "mode": "legacy_fallback",
                    "status": "metadata_unavailable",
                    "wire": "TZ/1 fallback=metadata_unavailable",
                    "telemetry": {
                        "manifest": 0,
                        "delta": 0,
                        "toc_working_set": 0,
                        "other": total,
                        "total": total
                    }
                })
            }
        };
        if let Some(object) = snapshot.as_object_mut() {
            object.insert(
                "demand_paging".to_string(),
                json!({"working_set_loaded": working_set_loaded}),
            );
        }
        snapshot
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Crash-only recovery health for CodeMode expand/read (wqw.9).
    pub fn surface_health(&self) -> &crate::surface_health::SurfaceHealth {
        &self.surface_health
    }

    pub fn surface_health_handle(&self) -> std::sync::Arc<crate::surface_health::SurfaceHealth> {
        std::sync::Arc::clone(&self.surface_health)
    }

    /// Record one tool-call outcome for observability. Fail-open.
    pub fn record_tool_call(&self, tool: &str, elapsed: std::time::Duration, is_error: bool) {
        self.metrics.record(tool, elapsed, is_error);
    }

    pub fn record_tool_attribution(
        &self,
        tool: &str,
        engine: std::time::Duration,
        persist: std::time::Duration,
    ) {
        self.metrics.record_attribution(tool, engine, persist);
    }

    /// Append one response to the session ledger when this engine owns
    /// accounting. Persistence is fail-closed: a served accounting block
    /// without a durable JSONL line is a lie to `tokenzero ledger`.
    pub fn record_ledger_response(
        &self,
        tool: &str,
        response: &ToolResponse,
    ) -> std::io::Result<()> {
        match &self.ledger {
            Some(ledger) => ledger.record_response(tool, response),
            None => Ok(()),
        }
    }

    /// Persist one Pulse event when this response claims token accounting.
    ///
    /// Fail-closed: a served accounting block without a durable Pulse write, or
    /// a Pulse row stamped with a different tokenizer than the accounting, is a
    /// lie. No-op when the response has no accounting.
    pub fn record_tool_pulse(
        &self,
        tool: &str,
        response: &ToolResponse,
        call_id: Option<String>,
        extra_ref_ids: Vec<String>,
    ) -> std::io::Result<()> {
        let Some(accounting) = response.accounting.as_ref() else {
            return Ok(());
        };
        let root = self
            .config
            .allowed_roots
            .first()
            .map(PathBuf::as_path)
            .or_else(|| self.config.cache_path.parent())
            .unwrap_or_else(|| Path::new("."));
        let mut ref_ids: Vec<String> = response
            .refs
            .iter()
            .map(|record| record.ref_id.clone())
            .collect();
        ref_ids.extend(extra_ref_ids);
        let latency_ms = response
            .telemetry
            .as_ref()
            .and_then(|value| value.get("latency_ms"))
            .and_then(|value| value.as_u64())
            .unwrap_or(0) as u128;
        let event = tokenzero_pulse::PulseEvent::tool_call(
            tool,
            response.mode.as_deref().unwrap_or("hybrid"),
            accounting.raw_tokens,
            accounting.visible_tokens,
            accounting.recovery_tokens,
            response.refs.len(),
            latency_ms,
            None,
        )
        .with_attribution(Some(self.session_id().to_string()), call_id, ref_ids);
        let mut event = event
            .with_tokenizer_id(&accounting.tokenizer_id)
            .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
        event.failure = response.error.is_some();
        event.task_lossless = tokenzero_pulse::pulse_task_lossless(
            accounting.raw_tokens,
            accounting.visible_tokens,
            accounting.recovery_tokens,
        ) && !event.failure;
        tokenzero_pulse::record_event(&tokenzero_pulse::default_ledger_path(root), &event)
    }

    /// Snapshot served by `resource://tokenzero/metrics`.
    pub fn tool_metrics_snapshot(&self) -> Value {
        let mut snap = self.metrics.snapshot();
        if let Some(obj) = snap.as_object_mut() {
            obj.insert("session_boot".to_string(), self.session_boot_snapshot());
            obj.insert(
                "surface_health".to_string(),
                self.surface_health.telemetry(),
            );
            obj.insert(
                "worker_process_observation".to_string(),
                crate::shell_hooks::process_observation_snapshot(),
            );
            let working_set = self
                .working_set
                .lock()
                .map(|state| state.telemetry())
                .unwrap_or_default();
            obj.insert(
                "working_set".to_string(),
                serde_json::to_value(working_set).unwrap_or_else(|_| json!({})),
            );
        }
        snap
    }

    /// Fail-open lookup: a poisoned session mutex reads as a miss (full
    /// serve, nothing recorded) instead of failing the call.
    pub(crate) fn session_lookup(&self, key: &ServeKey, content_sha256: &str) -> SeenState {
        self.with_session_memory(
            || SeenState::Miss,
            |memory| memory.lookup(key, content_sha256),
        )
    }

    /// Write-back of this call's serve records and rollup counters.
    ///
    /// Persist is fail-closed when session persistence is enabled: a served
    /// seen-set without a journal/snapshot write is a lie on resume.
    pub(crate) fn session_apply(
        &self,
        pending: Vec<(ServeKey, ServedRecord)>,
        summary: &SessionSummary,
    ) -> std::io::Result<(u64, u64)> {
        let persist_enabled = self.session_persist.is_some();
        let (watermark, snapshot) = self.with_session_memory(
            || ((0, 0), None),
            |memory| {
                let changed_keys: Vec<_> = pending
                    .into_iter()
                    .map(|(key, record)| {
                        memory.record(key.clone(), record);
                        key
                    })
                    .collect();
                memory.absorb(summary);
                if let (Some(full), Some(delta)) = (summary.full_bytes, summary.delta_bytes) {
                    memory.note_bytes(full, delta);
                }
                let watermark = memory.advance_hwm();
                let snapshot = persist_enabled
                    .then(|| SessionPersistSnapshot::from_memory(memory, &changed_keys));
                (watermark, snapshot)
            },
        );
        if let (Some(persist), Some(snapshot)) = (self.session_persist.as_ref(), snapshot.as_ref())
        {
            persist.persist(snapshot)?;
        }
        Ok(watermark)
    }

    /// Claim a set of ServeKeys for single-flight serving. Blocks until none
    /// of `keys` is already in flight, then marks them all in flight and
    /// returns a guard that releases them (and wakes waiters) on drop. An
    /// empty key set (dedup off, or nothing dedupable) is a no-op.
    pub(crate) fn begin_serve_flight(&self, keys: Vec<ServeKey>) -> ServeFlight<'_> {
        if !keys.is_empty() {
            let (lock, cvar) = &self.in_flight;
            let mut set = lock.lock().unwrap_or_else(|p| p.into_inner());
            // Wait until every requested key is free, then claim them all at
            // once. Claiming atomically avoids a livelock between two calls
            // whose key sets overlap in opposite order.
            while keys.iter().any(|key| set.contains(key)) {
                set = cvar.wait(set).unwrap_or_else(|p| p.into_inner());
            }
            for key in &keys {
                set.insert(key.clone());
            }
        }
        ServeFlight { engine: self, keys }
    }

    /// Run `f` on live session memory. Cold load is a local `SessionMemory`
    /// (flock/disk without `self.session`); the mutex inserts only if the
    /// slot is still `None`, then runs `f`.
    fn with_session_memory<R>(
        &self,
        on_poison: impl FnOnce() -> R,
        f: impl FnOnce(&mut SessionMemory) -> R,
    ) -> R {
        match self.session.lock() {
            Ok(mut slot) => {
                if let Some(memory) = slot.as_mut() {
                    return f(memory);
                }
            }
            Err(_) => return on_poison(),
        }
        let loaded = Self::session_memory_from_disk(self.session_persist.as_ref());
        match self.session.lock() {
            Ok(mut slot) => f(slot.get_or_insert(loaded)),
            Err(_) => on_poison(),
        }
    }

    fn session_memory_from_disk(persistence: Option<&SessionPersistence>) -> SessionMemory {
        let mut memory = SessionMemory::default();
        if let Some(persist) = persistence {
            persist.load_into(&mut memory);
        }
        memory
    }

    pub(crate) fn admit_working_set_response(
        &self,
        store: &mut RecoveryStore,
        response: &mut ToolResponse,
        anchor: tokenzero_recovery::working_set::SpanAnchor,
    ) -> bool {
        let Some(text) = response
            .visible
            .as_ref()
            .map(|visible| visible.text.clone())
        else {
            return false;
        };
        if text.is_empty() {
            return false;
        }
        let Ok(mut working_set) = self.working_set.lock() else {
            return false;
        };
        let Ok(admission) = working_set.rewrite_render(store, text, anchor) else {
            return false;
        };
        let replaced = admission.replacement.is_some();
        if let Some(replacement) = admission.replacement {
            if let Some(visible) = response.visible.as_mut() {
                visible.text = replacement;
            }
            if let Some(accounting) = response.accounting.as_mut() {
                accounting.visible_tokens = response
                    .visible
                    .as_ref()
                    .map(|visible| count_tokens(&visible.text))
                    .unwrap_or(0);
            }
        }
        if !admission.evicted.is_empty() {
            for eviction in &admission.evicted {
                if !response
                    .refs
                    .iter()
                    .any(|record| record.ref_id == eviction.ref_id)
                {
                    response.refs.push(ref_record(
                        "blob",
                        eviction.ref_id.clone(),
                        eviction.bytes_evicted,
                    ));
                }
            }
            merge_telemetry(
                response,
                json!({
                    "working_set_eviction": {
                        "replacements": admission.evicted.iter().map(|entry| &entry.replacement).collect::<Vec<_>>(),
                        "amortized": working_set.telemetry().eviction_accounting
                    }
                }),
            );
        }
        replaced
    }

    pub fn session_rollup(&self) -> Value {
        self.with_session_memory(
            || {
                json!({
                    "records": 0,
                    "dedup_hits": 0,
                    "diff_hits": 0,
                    "visible_tokens_saved": 0,
                    "diff_tokens_saved": 0,
                    "poisoned": true
                })
            },
            |memory| memory.rollup(),
        )
    }

    pub(crate) fn rg_binary(&self) -> Option<&Path> {
        self.rg_binary
            .get_or_init(|| {
                // Prefer engine config override, else portable resolver
                // (env TOKENZERO_RG_PATH → PATH → well-known).
                match &self.config.rg_path_override {
                    Some(path) if path.is_file() => Some(path.clone()),
                    Some(_) => None,
                    None => find_rg_in_path(),
                }
            })
            .as_deref()
    }
}
