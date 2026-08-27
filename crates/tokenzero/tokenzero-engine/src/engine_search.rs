use super::*;

/// Emission-path crossover labels (ZS-CACHE-006): the policy names the
/// deterministic grouped rewrite of search/tree result bodies.
const EMISSION_CROSSOVER_POLICY_ID: &str = "tokenzero-engine.emission.search-output/v1";
const EMISSION_CROSSOVER_TOKEN_UNIT_ID: &str = "estimator:tokenzero-count-tokens/v1";
const EMISSION_GROUPED_ADMISSION_ID: &str = "tokenzero-engine.grouped-path-output.v1";

struct SearchBackendRun {
    matches: Vec<SearchMatch>,
    stats: SearchStats,
    backend: &'static str,
    fallback_reason: Option<String>,
}

/// Empty-scan note grammar shared by search/glob/tree: suffix, guidance,
/// budget signal, and exhausted-miss diagnostic. Non-empty truncated hints
/// stay at the call site -- only search applies them.
fn apply_empty_discovery_notes(
    response: &mut ToolResponse,
    mode: Mode,
    max_visible_tokens: usize,
    tool: &str,
    query: &str,
    headline: String,
    truncated: bool,
    unreadable: bool,
) {
    let suffix = match (truncated, unreadable) {
        (true, true) => " (scan truncated, incomplete)",
        (true, false) => " (scan truncated)",
        (false, true) => " (scan incomplete)",
        (false, false) => "",
    };
    apply_zero_hit_note(
        response,
        mode,
        with_guidance(
            format!("{headline}{suffix}"),
            tool,
            query,
            truncated,
            unreadable,
        ),
    );
    attach_budget_signal(response, max_visible_tokens, truncated);
    if unreadable {
        mark_unreadable_miss(response);
    }
    if truncated {
        mark_budget_exhausted_miss(response);
    }
}

impl TokenZeroEngine {
    /// Select rg vs internal and run the scan. Eligibility-vs-hit stays here:
    /// explicit rg+grep is an error; Auto falls back; find stays in-process.
    fn run_search_backend(
        &self,
        tool: &str,
        query: &str,
        roots: &[PathBuf],
        max_files: usize,
        max_visited_files: usize,
    ) -> Result<SearchBackendRun, ToolResponse> {
        let run_internal = |stats: &mut SearchStats, matches: &mut Vec<SearchMatch>| {
            stats.search_threads = 1;
            for root in roots {
                collect_search(
                    root,
                    root,
                    query,
                    max_files,
                    max_visited_files,
                    MAX_WALK_DEPTH,
                    stats,
                    matches,
                );
                if stats.truncated_by_results || stats.truncated_by_visit || stats.truncated_by_wall
                {
                    break;
                }
            }
        };
        let mut matches: Vec<SearchMatch> = Vec::new();
        let mut stats = SearchStats::default();
        let mut backend = "internal";
        let mut fallback_reason: Option<String> = None;
        let explicit_rg = matches!(self.config.search_backend, SearchBackend::Rg);
        let backend_unavailable = |reason: &str| {
            failure_response(
                tool,
                "backend_unavailable",
                format!("TOKENZERO_SEARCH_BACKEND=rg but ripgrep is unusable: {reason}"),
                Some(
                    "install ripgrep, set TOKENZERO_RG_PATH, or use auto/internal \
                  (internal matches literal substrings, not regex)",
                ),
            )
        };
        let rg = match self.config.search_backend {
            SearchBackend::Internal => None,
            SearchBackend::Auto if tool == "find" => {
                fallback_reason = Some("in_process_find_literal".to_owned());
                None
            }
            SearchBackend::Rg | SearchBackend::Auto => {
                let resolved = self.rg_binary();
                if resolved.is_none() {
                    if explicit_rg && tool == "grep" {
                        return Err(backend_unavailable("rg_not_found"));
                    }
                    fallback_reason = Some("rg_not_found".to_string());
                }
                resolved
            }
        };
        match rg {
            Some(rg_path) => match rg_search(rg_path, tool, query, roots, max_files) {
                Ok((rg_matches, rg_stats)) => {
                    matches = rg_matches;
                    stats = rg_stats;
                    backend = "rg";
                }
                Err(RgFailure::InvalidPattern(message)) => {
                    return Err(failure_response(
                        tool,
                        "invalid_pattern",
                        message,
                        Some("fix the regex, or use tz_find for literal substring search"),
                    ));
                }
                Err(RgFailure::Unavailable(reason)) => {
                    if explicit_rg && tool == "grep" {
                        return Err(backend_unavailable(&reason));
                    }
                    fallback_reason = Some(reason);
                    run_internal(&mut stats, &mut matches);
                }
            },
            None => run_internal(&mut stats, &mut matches),
        }
        Ok(SearchBackendRun {
            matches,
            stats,
            backend,
            fallback_reason,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn search(
        &self,
        tool: &str,
        query: &str,
        roots: &[PathBuf],
        mode: Mode,
        max_files: usize,
        max_visible_tokens: usize,
        options: ServeOptions,
    ) -> ToolResponse {
        for root in roots {
            if !self.path_allowed(root) {
                return path_not_allowed(tool, root, &self.config.allowed_roots);
            }
        }
        // Single-flight identical searches so a second pipelined call dedups
        // against the first's recorded serve instead of racing it. Same key
        // the session block uses below; held until after session_apply.
        let _flight = if self.config.session_dedup {
            let mut canonical_roots: Vec<PathBuf> =
                roots.iter().map(|root| comparable_path(root)).collect();
            canonical_roots.sort();
            self.begin_serve_flight(vec![ServeKey::Output {
                tool: format!("{tool}:{:?}", self.config.search_backend),
                query: query.to_string(),
                roots: canonical_roots,
            }])
        } else {
            self.begin_serve_flight(Vec::new())
        };
        let max_visited_files = max_search_visited_files(max_files);
        // With the EXPLICIT rg backend, grep's pattern is a regex by
        // contract; silently degrading to the internal substring scanner
        // would change result semantics, so unavailability is an error for
        // grep (find keeps identical substring semantics either way and may
        // fall back). Auto mode always falls back.
        //
        // Auto+find always stays in-process: find is fixed-string isomorphic
        // with collect_search, and parent spawn/wait on rg dominates agent
        // finds (S3_find). Internal DFS can also early-exit on max_results;
        // rg_search must wait for the full child before sorting. Explicit
        // TOKENZERO_SEARCH_BACKEND=rg still forces the subprocess path.
        // Auto+grep still prefers rg for true regex semantics.
        let SearchBackendRun {
            mut matches,
            stats,
            backend,
            fallback_reason,
        } = match self.run_search_backend(tool, query, roots, max_files, max_visited_files) {
            Ok(run) => run,
            Err(response) => return response,
        };
        if stats.truncated_by_wall {
            let message = crate::wall::check_active_wall_deadline()
                .map(|(message, _)| message)
                .unwrap_or_else(|| "runtime: hard_max_wall_ms exceeded".to_string());
            return failure_response(tool, "hard_max_wall_ms", message, None);
        }
        {
            let store = self.recovery_store();
            matches.sort_by(|left, right| {
                store
                    .frecency_for_path(Path::new(&left.path))
                    .total_cmp(&store.frecency_for_path(Path::new(&right.path)))
                    .reverse()
                    .then(left.path.cmp(&right.path))
                    .then(left.line.cmp(&right.line))
            });
        }
        // Canonical recoverable payload keeps the grep-compatible flat format
        // for byte-stable replay. The visible rendering uses the FSZero
        // snap-to-file hit grammar (FSZero docs/design/target-ref-grammar.md):
        // one HIT record per match with a `#L<start>-L<end>` target ref and an
        // inlined context window, so discovery results are one-call actionable.
        let output = flat_search_output(&matches);
        let hit_kind = match (tool, backend) {
            ("grep", "rg") => "regex",
            _ => "literal",
        };
        let visible_source = hit_search_output(&matches, hit_kind);
        let grouped = false;
        let mut store = self.recovery_store();
        let search_refs = store.store_search_output_deferred(&output, Some(query));
        let stored =
            store.store_payload_deferred(&output, ContentType::SearchResult, None, None, None);
        let mut refs = Vec::with_capacity(2 + search_refs.len());
        push_payload_refs(&mut refs, &stored, output.len());
        refs.extend(
            search_refs
                .into_iter()
                .map(|id| ref_record("search", id, 0)),
        );
        let persisted = persist_refs(&mut store, &mut refs);
        let refs_complete = persisted.refs_complete;
        let storage_error = persisted.error;
        let exact_ref_tokens = exact_ref_token_count(&refs);
        let exact_refs_available = !refs.is_empty();
        let capsule = match recoverable_capsule(
            &visible_source,
            &output,
            stored.raw_tokens,
            mode,
            max_visible_tokens,
            &format!("{tool} {query}"),
            Some(&stored.blob_ref),
            refs_complete,
        ) {
            Ok(capsule) => capsule,
            Err(error) => return capsule_error_response(tool, error),
        };
        let full_bytes = capsule.text.len();
        let mut visible_text = capsule.text;
        let mut final_visible_tokens = capsule.visible_tokens;
        let mut summary = SessionSummary::default();
        let mut pending: Vec<(ServeKey, ServedRecord)> = Vec::new();
        // Session redundancy layer (docs/codemode.md §5a): identical flat
        // output already served this session collapses to a note. Zero-hit
        // notes below stay untouched (empty output skips the layer; notes
        // are never deduped), and changed output gets a full serve — search
        // results are never diffed. Skipped entirely when this call's refs
        // failed to persist: a note must never advertise unrecoverable refs,
        // and a serve whose refs died must not become a dedup base.
        if self.config.session_dedup
            && !output.is_empty()
            && storage_error.is_none()
            && refs_complete
        {
            let mut canonical_roots: Vec<PathBuf> =
                roots.iter().map(|root| comparable_path(root)).collect();
            canonical_roots.sort();
            let key = ServeKey::Output {
                tool: format!("{tool}:{:?}", self.config.search_backend),
                query: query.to_string(),
                roots: canonical_roots,
            };
            let content_sha256 = sha256_hex(&output);
            let bypass = matches!(mode, Mode::Passthrough) || options.fresh;
            if let SeenState::Unchanged {
                serve_count,
                cross_session,
            } = self.session_lookup(&key, &content_sha256)
            {
                if let Some((message, _)) = crate::wall::check_active_wall_deadline() {
                    return failure_response(tool, "hard_max_wall_ms", message, None);
                }
                if !bypass {
                    let note = unchanged_search_note(tool, query, &output, &stored);
                    let note_tokens = count_tokens(&note);
                    // ROI guard: emit only when strictly cheaper than the
                    // full render.
                    if note_tokens < final_visible_tokens {
                        summary.note_dedup(
                            serve_count + 1,
                            final_visible_tokens - note_tokens,
                            cross_session,
                        );
                        visible_text = note;
                        final_visible_tokens = note_tokens;
                    }
                }
            }
            pending.push((key, served_record(&output, &stored)));
        }
        let mut response = success_response(
            tool,
            mode,
            visible_text,
            refs,
            (
                capsule.raw_tokens,
                final_visible_tokens,
                0,
                Some(exact_ref_tokens),
            ),
        );
        response.content_type = Some(ContentType::SearchResult.to_string());
        if storage_error.is_some() {
            response.diagnostic = Some(cache_write_diagnostic(format!(
                "could not persist recovery cache for {tool} output"
            )));
        }
        let mut telemetry = json!({
            "query": query,
            "roots": roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "visited_files": stats.visited_files,
            "matched_files": stats.matched_files,
            "matches": stats.matched_lines,
            "result_limit": max_files,
            "visit_limit": max_visited_files,
            "truncated_by_results": stats.truncated_by_results,
            "truncated_by_visit": stats.truncated_by_visit,
            "search_backend": backend,
            "search_threads": stats.search_threads,
            "concurrent_search": stats.search_threads > 1,
            "output_strategy": if grouped { "grouped_by_file" } else { "hit_target" },
            "unreadable_entries": stats.unreadable_entries,
            "transport_status": if storage_error.is_some() || stats.unreadable_entries > 0 {
                "degraded"
            } else {
                "ok"
            },
            "degraded": storage_error.is_some() || stats.unreadable_entries > 0,
            "storage_error": storage_error,
            "exact_refs_available": exact_refs_available
        });
        if let Some(reason) = &fallback_reason {
            telemetry["fallback_reason"] = json!(reason);
        }
        if stats.unparsed_rows > 0 {
            telemetry["rg_unparsed_rows"] = json!(stats.unparsed_rows);
        }
        response.telemetry = Some(telemetry);
        if self.config.session_dedup {
            let delta_bytes = response
                .visible
                .as_ref()
                .map_or(0, |visible| visible.text.len());
            summary.note_wire_bytes(full_bytes, delta_bytes);
        }
        let (from_hwm, to_hwm) = match self.session_apply(pending, &summary) {
            Ok(hwm) => hwm,
            Err(err) => return session_persist_failure(tool, &err),
        };
        summary.set_watermark(from_hwm, to_hwm);
        // Merge — never overwrite — so backend/storage telemetry survives a
        // dedup serve in the same response.
        if let Some(extra) = summary.telemetry() {
            merge_telemetry(&mut response, extra);
        }
        if matches.is_empty() {
            apply_empty_discovery_notes(
                &mut response,
                mode,
                max_visible_tokens,
                tool,
                query,
                format!("# {tool} {} — 0 matches", zero_hit_label(query)),
                stats.truncated_by_results || stats.truncated_by_visit,
                stats.unreadable_entries > 0,
            );
        } else if stats.truncated_by_results || stats.truncated_by_visit {
            apply_truncated_hint(&mut response, mode);
            attach_budget_signal(&mut response, max_visible_tokens, true);
            if stats.unreadable_entries > 0 {
                mark_unreadable_miss(&mut response);
            }
        } else {
            attach_budget_signal(&mut response, max_visible_tokens, false);
            if stats.unreadable_entries > 0 {
                mark_unreadable_miss(&mut response);
            }
        }
        response
    }

    pub fn glob(
        &self,
        pattern: &str,
        roots: &[PathBuf],
        include_hidden: bool,
        mode: Mode,
        max_files: usize,
        max_visible_tokens: usize,
    ) -> ToolResponse {
        let matcher = match GlobBuilder::new(pattern).literal_separator(false).build() {
            Ok(glob) => glob.compile_matcher(),
            Err(err) => {
                return failure_response(
                    "glob",
                    "invalid_glob",
                    err.to_string(),
                    Some("check glob syntax"),
                );
            }
        };
        let mut paths: Vec<PathBuf> = Vec::new();
        let mut unreadable = 0usize;
        for root in roots {
            if !self.path_allowed(root) {
                return path_not_allowed("glob", root, &self.config.allowed_roots);
            }
            collect_glob(
                root,
                root,
                &matcher,
                pattern.contains('/'),
                include_hidden,
                max_files,
                MAX_WALK_DEPTH,
                &mut paths,
                &mut unreadable,
            );
        }
        paths.sort();
        paths.dedup();
        let rows = paths.iter().map(|p| display_path(p)).collect::<Vec<_>>();
        let output = rows.join("\n");
        let compact = grouped_path_output(&paths, roots);
        // Emission-path cache crossover (ZS-CACHE-006): the flat listing is
        // the stable cacheable artifact, the grouped listing the compact
        // rewrite. Defaults reproduce the historical pick_cheaper choice
        // exactly; configured cache economics can prefer the cached stable
        // form over per-call compaction.
        let crossover = self.decide_emission_crossover(&output, &compact);
        let (visible_source, grouped) = match crossover.action {
            CacheCrossoverAction::Compress => (&compact, true),
            _ => (&output, false),
        };
        let mut response = self.search_result_response(
            "glob",
            pattern,
            &output,
            Some(visible_source),
            mode,
            max_visible_tokens,
        );
        // search_result_response records degraded cache-persist markers in
        // telemetry; fold them into glob's object instead of clobbering them.
        let prior = response.telemetry.take().unwrap_or_default();
        let degraded = prior["degraded"].as_bool().unwrap_or(false) || unreadable > 0;
        response.telemetry = Some(json!({
            "pattern": pattern,
            "roots": roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "matches": rows.len(),
            "include_hidden": include_hidden,
            "unreadable_entries": unreadable,
            "output_strategy": if grouped { "grouped_by_root" } else { "exact_first_glob" },
            "crossover_action": json!(crossover.action),
            "crossover_reason": json!(crossover.reason),
            "transport_status": if degraded { "degraded" } else { "ok" },
            "degraded": degraded,
            "storage_error": prior.get("storage_error").cloned().unwrap_or(Value::Null),
            "exact_refs_available": prior["exact_refs_available"].as_bool()
                .unwrap_or(!response.refs.is_empty())
        }));
        if rows.is_empty() {
            // max_files == 0 stops collect_glob before it scans anything, so
            // an unqualified "0 matches" would be a false affirmative.
            apply_empty_discovery_notes(
                &mut response,
                mode,
                max_visible_tokens,
                "glob",
                pattern,
                format!("# glob {} — 0 matches", zero_hit_label(pattern)),
                max_files == 0,
                unreadable > 0,
            );
        } else {
            let exhausted = max_files > 0 && rows.len() >= max_files;
            attach_budget_signal(&mut response, max_visible_tokens, exhausted);
            if unreadable > 0 {
                mark_unreadable_miss(&mut response);
            }
        }
        response
    }

    pub fn tree(
        &self,
        roots: &[PathBuf],
        depth: usize,
        include_hidden: bool,
        mode: Mode,
        max_files: usize,
        max_visible_tokens: usize,
    ) -> ToolResponse {
        let mut entries: Vec<TreeEntry> = Vec::new();
        let mut spans: Vec<(String, usize)> = Vec::new();
        let mut unreadable = 0usize;
        for root in roots {
            if !self.path_allowed(root) {
                return path_not_allowed("tree", root, &self.config.allowed_roots);
            }
            spans.push((root.display().to_string(), entries.len()));
            collect_tree(
                root,
                root,
                depth,
                include_hidden,
                max_files,
                0,
                &mut entries,
                &mut unreadable,
            );
        }
        let output = entries
            .iter()
            .map(|entry| entry.rel.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let compact = grouped_tree_output(&entries, &spans, roots.len() > 1);
        // Same emission-path crossover as glob: the flat tree is the stable
        // cacheable artifact, the grouped tree the compact rewrite.
        let crossover = self.decide_emission_crossover(&output, &compact);
        let visible_source = match crossover.action {
            CacheCrossoverAction::Compress => compact.as_str(),
            _ => output.as_str(),
        };
        let mut response = self.ingest_with_tool(
            "tree",
            &output,
            visible_source,
            ContentType::Tree,
            mode,
            "tree",
            max_visible_tokens,
        );
        if entries.is_empty() {
            // depth == 0 or max_files == 0 stops collect_tree before it scans
            // anything, so an unqualified "0 entries" would be a false
            // affirmative on a populated root.
            apply_empty_discovery_notes(
                &mut response,
                mode,
                max_visible_tokens,
                "tree",
                "",
                "# tree — 0 entries".to_string(),
                max_files == 0 || depth == 0,
                unreadable > 0,
            );
        } else {
            let exhausted = max_files > 0 && entries.len() >= max_files;
            attach_budget_signal(&mut response, max_visible_tokens, exhausted);
            if unreadable > 0 {
                mark_unreadable_miss(&mut response);
            }
        }
        response
    }

    /// Emission-path cache crossover for search/tree result bodies
    /// (ZS-CACHE-006). The flat form is the stable cacheable artifact; the
    /// grouped form is the compact rewrite -- a deterministic transform of
    /// the same content, so quality admission is by construction (named by
    /// the constant admission id). Default `EmissionCrossoverConfig` values
    /// (d = 1.0, horizon = 1) reproduce the historical `pick_cheaper`
    /// emission byte-for-byte; configured cache economics change the choice
    /// deliberately. Empty bodies stay inline (KeepInline), matching the
    /// historical pick_cheaper for zero-result listings.
    fn decide_emission_crossover(&self, flat: &str, compact: &str) -> CacheCrossoverReceipt {
        let cfg = self.config.emission_crossover;
        let flat_tokens = count_tokens(flat) as u64;
        let compact_tokens = count_tokens(compact) as u64;
        if flat_tokens == 0 {
            return CacheCrossoverReceipt {
                schema: CACHE_CROSSOVER_SCHEMA,
                provider: CacheProvider::Anthropic,
                policy_id: EMISSION_CROSSOVER_POLICY_ID.to_owned(),
                token_unit_id: EMISSION_CROSSOVER_TOKEN_UNIT_ID.to_owned(),
                content_class: CacheContentClass::Stable,
                original_tokens: 0,
                compressed_tokens: compact_tokens,
                compression_admission_id: None,
                common_overhead_tokens: 0,
                cached_read_multiplier_ppm: cfg.cached_read_multiplier_ppm,
                min_cacheable_tokens: cfg.min_cacheable_tokens,
                action: CacheCrossoverAction::KeepInline,
                reason: CacheCrossoverReason::BelowCacheableFloor,
                cache_eligible: false,
                suffix_size_tokens: 0,
                compaction_cost_tokens: 0,
                remaining_reuse_horizon: cfg.remaining_reuse_horizon,
                inline_total_token_cost_ppm: 0,
                compressed_total_token_cost_ppm: 0,
                cached_total_token_cost_ppm: 0,
                inline_projected_token_cost_ppm: 0,
                compressed_projected_token_cost_ppm: 0,
                cached_projected_token_cost_ppm: 0,
            };
        }
        decide_cache_crossover(&CacheCrossoverInput {
            provider: CacheProvider::Anthropic,
            policy_id: EMISSION_CROSSOVER_POLICY_ID.to_owned(),
            token_unit_id: EMISSION_CROSSOVER_TOKEN_UNIT_ID.to_owned(),
            content_class: CacheContentClass::Stable,
            original_tokens: flat_tokens,
            compressed_tokens: compact_tokens,
            compression_admission_id: Some(EMISSION_GROUPED_ADMISSION_ID.to_owned()),
            common_overhead_tokens: 0,
            cached_read_multiplier_ppm: cfg.cached_read_multiplier_ppm,
            min_cacheable_tokens: cfg.min_cacheable_tokens,
            suffix_size_tokens: 0,
            compaction_cost_tokens: 0,
            remaining_reuse_horizon: cfg.remaining_reuse_horizon,
        })
        .expect("emission crossover inputs are validated constants")
    }

    pub(crate) fn search_result_response(
        &self,
        tool: &str,
        key: &str,
        output: &str,
        rendered: Option<&str>,
        mode: Mode,
        max_visible_tokens: usize,
    ) -> ToolResponse {
        let mut store = self.recovery_store();
        let search_refs = store.store_search_output_deferred(output, Some(key));
        let stored =
            store.store_payload_deferred(output, ContentType::SearchResult, None, None, None);
        let mut refs = Vec::with_capacity(2 + search_refs.len());
        push_payload_refs(&mut refs, &stored, output.len());
        refs.extend(
            search_refs
                .into_iter()
                .map(|id| ref_record("search", id, 0)),
        );
        let persisted = persist_refs(&mut store, &mut refs);
        let exact_ref_tokens = exact_ref_token_count(&refs);
        let capsule = match recoverable_capsule(
            rendered.unwrap_or(output),
            output,
            stored.raw_tokens,
            mode,
            max_visible_tokens,
            &format!("{tool} {key}"),
            Some(&stored.blob_ref),
            persisted.refs_complete,
        ) {
            Ok(capsule) => capsule,
            Err(error) => return capsule_error_response(tool, error),
        };
        let mut response = success_response(
            tool,
            mode,
            capsule.text,
            refs,
            (
                capsule.raw_tokens,
                capsule.visible_tokens,
                store.recovery_tokens,
                Some(exact_ref_tokens),
            ),
        );
        response.content_type = Some(ContentType::SearchResult.to_string());
        if let Some(error) = persisted.error {
            response.diagnostic = Some(cache_write_diagnostic(format!(
                "could not persist recovery cache for {tool} output"
            )));
            response.telemetry = Some(json!({
                "transport_status": "degraded",
                "degraded": true,
                "storage_error": error,
                "exact_refs_available": false
            }));
        }
        response
    }
}
