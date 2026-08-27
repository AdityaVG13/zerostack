use super::*;

/// Accumulators filled by one successful path read.
struct ReadPathAcc<'a> {
    visible_parts: &'a mut Vec<String>,
    raw_visible_parts: &'a mut Vec<String>,
    refs: &'a mut Vec<tokenzero_core::RefRecord>,
    /// Formation-receipt refs (ZS-VIEW-002). Kept out of the prune gate:
    /// `tz://capsule/` refs are digest receipts, not expand targets, so
    /// `RecoveryStore::has_ref` cannot verify them and they must not decide
    /// `refs_complete` (V6-T3 defect: every formed read degraded to a raw
    /// full-text serve because the capsule ref flipped the prune verdict).
    capsule_refs: &'a mut Vec<tokenzero_core::RefRecord>,
    raw_tokens: &'a mut usize,
    visible_tokens: &'a mut usize,
    content_types: &'a mut Vec<ContentType>,
    bytes_read: &'a mut usize,
    working_set_anchor: &'a mut Option<tokenzero_recovery::working_set::SpanAnchor>,
    pending: &'a mut Vec<(ServeKey, ServedRecord)>,
    substitutions: &'a mut Vec<PendingSubstitution>,
    stale_store_hit: &'a mut bool,
}

impl TokenZeroEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn read(
        &self,
        paths: &[PathBuf],
        mode: Mode,
        start_line: Option<usize>,
        end_line: Option<usize>,
        raw: bool,
        max_files: usize,
        max_visible_tokens: usize,
    ) -> ToolResponse {
        self.read_with_options(
            paths,
            mode,
            start_line,
            end_line,
            raw,
            max_files,
            max_visible_tokens,
            ServeOptions::default(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn read_with_options(
        &self,
        paths: &[PathBuf],
        mode: Mode,
        start_line: Option<usize>,
        end_line: Option<usize>,
        raw: bool,
        max_files: usize,
        max_visible_tokens: usize,
        options: ServeOptions,
    ) -> ToolResponse {
        let response = crate::perf_profile::_profile_read_inner(|| {
            self.read_with_options_inner(
                paths,
                mode,
                start_line,
                end_line,
                raw,
                max_files,
                max_visible_tokens,
                options,
            )
        });
        let ok = response.error.is_none();
        let code = response.error.as_ref().map(|err| err.code.as_str());
        self.surface_health().record_read_outcome(ok, code);
        response
    }

    #[allow(clippy::too_many_arguments)]
    fn read_with_options_inner(
        &self,
        paths: &[PathBuf],
        mode: Mode,
        start_line: Option<usize>,
        end_line: Option<usize>,
        raw: bool,
        max_files: usize,
        max_visible_tokens: usize,
        options: ServeOptions,
    ) -> ToolResponse {
        // Bind caller-supplied paths to the configured root so relative
        // arguments target `call_root`, never the process working directory.
        let paths: Vec<PathBuf> = paths
            .iter()
            .map(|path| self.resolve_call_path(path))
            .collect();
        // Single-flight the serve so a second pipelined identical read waits
        // for this one to record its serve before it looks up the seen-set
        // (otherwise both miss and both serve full). Keyed per path+range, so
        // disjoint reads still run fully concurrently. Held until after
        // session_apply via the guard's lifetime.
        let _flight = if self.config.session_dedup {
            let keys = paths
                .iter()
                .take(max_files)
                .map(|path| ServeKey::File {
                    path: comparable_path(path),
                    start: start_line,
                    end: end_line,
                })
                .collect();
            self.begin_serve_flight(keys)
        } else {
            self.begin_serve_flight(Vec::new())
        };
        let mut store = self.recovery_store();
        let mut visible_parts = Vec::new();
        let mut raw_visible_parts = Vec::new();
        let mut refs = Vec::new();
        let mut capsule_refs = Vec::new();
        let mut raw_tokens = 0usize;
        let mut visible_tokens = 0usize;
        let mut storage_errors = Vec::new();
        let mut content_types = Vec::new();
        let mut bytes_read = 0usize;
        let mut summary = SessionSummary::default();
        let mut working_set_anchor = None;
        // Serve records are applied only after every path succeeded: an
        // error response serves nothing, so nothing may be marked as seen.
        let mut pending: Vec<(ServeKey, ServedRecord)> = Vec::new();
        // Dedup/diff substitutions are buffered and applied only after this
        // call's refs persist: a note replaces content with refs, which is
        // only safe when the refs are actually recoverable.
        let mut substitutions: Vec<PendingSubstitution> = Vec::new();
        let mut stale_store_hit = false;
        // Append-never-rewrite slots for capsules formed by this call
        // (ZS-VIEW-002): the same causal key may only ever hold one payload.
        let mut capsule_slots = tokenzero_core::model_artifacts::AppendOnlyCapsuleSlots::new();
        let path_count = paths.len();
        for path in paths.iter().take(max_files) {
            let mut acc = ReadPathAcc {
                visible_parts: &mut visible_parts,
                raw_visible_parts: &mut raw_visible_parts,
                refs: &mut refs,
                capsule_refs: &mut capsule_refs,
                raw_tokens: &mut raw_tokens,
                visible_tokens: &mut visible_tokens,
                content_types: &mut content_types,
                bytes_read: &mut bytes_read,
                working_set_anchor: &mut working_set_anchor,
                pending: &mut pending,
                substitutions: &mut substitutions,
                stale_store_hit: &mut stale_store_hit,
            };
            if let Err(response) = self.read_one_path(
                path,
                path_count,
                mode,
                start_line,
                end_line,
                raw,
                max_visible_tokens,
                &options,
                &mut store,
                &mut capsule_slots,
                &mut acc,
            ) {
                return response;
            }
        }
        let persisted = persist_refs(&mut store, &mut refs);
        if let Some(error) = persisted.error {
            storage_errors.push(error);
        }
        let refs_complete = persisted.refs_complete;
        if !refs_complete && !raw {
            visible_parts = raw_visible_parts;
            visible_tokens = raw_tokens;
        }
        // Formation receipts are advertised after the prune gate: they are
        // digest receipts, not recoverable refs, so they must not affect
        // `refs_complete` (see ReadPathAcc::capsule_refs).
        refs.extend(capsule_refs);
        let full_bytes = joined_bytes(&visible_parts);
        // Dedup/diff notes advertise refs in place of content: apply them
        // only when persistence succeeded AND every ref survived eviction.
        // Degraded storage always serves full — the bytes are in the text,
        // which is unconditionally safe.
        if storage_errors.is_empty() && refs_complete {
            for substitution in substitutions {
                match substitution {
                    PendingSubstitution::Dedup {
                        idx,
                        note,
                        note_tokens,
                        full_tokens,
                        serve_count,
                        cross_session,
                    } => {
                        summary.note_dedup(serve_count, full_tokens - note_tokens, cross_session);
                        visible_tokens -= full_tokens - note_tokens;
                        visible_parts[idx] = note;
                    }
                    PendingSubstitution::Diff {
                        idx,
                        text,
                        diff_tokens,
                        full_tokens,
                        telemetry,
                    } => {
                        summary.note_diff(telemetry, full_tokens - diff_tokens);
                        visible_tokens -= full_tokens - diff_tokens;
                        visible_parts[idx] = text;
                    }
                }
            }
        }
        if self.config.session_dedup {
            let delta_bytes = joined_bytes(&visible_parts);
            summary.note_wire_bytes(full_bytes, delta_bytes);
        }
        let exact_refs_available = !refs.is_empty();
        let exact_ref_tokens = exact_ref_token_count(&refs);
        let mut response = success_response(
            "read",
            mode,
            visible_parts.join("\n\n"),
            refs,
            (
                raw_tokens,
                visible_tokens,
                store.recovery_tokens,
                Some(exact_ref_tokens),
            ),
        );
        response.content_type = Some(common_content_type(&content_types).to_string());
        if !storage_errors.is_empty() {
            response.diagnostic = Some(cache_write_diagnostic(
                "could not persist recovery cache for one or more read paths",
            ));
            response.telemetry = Some(json!({
                "transport_status": "degraded",
                "degraded": true,
                "storage_errors": storage_errors,
                "exact_refs_available": exact_refs_available
            }));
        }
        let working_set_replaced = !raw
            && !matches!(mode, Mode::Passthrough)
            && working_set_anchor.is_some_and(|anchor| {
                self.admit_working_set_response(&mut store, &mut response, anchor)
            });
        // A serve whose refs failed to persist, or whose visible bytes were
        // replaced by working-set eviction, must not become a dedup base.
        if storage_errors.is_empty() && refs_complete && !working_set_replaced {
            match self.session_apply(pending, &summary) {
                Ok((from_hwm, to_hwm)) => summary.set_watermark(from_hwm, to_hwm),
                Err(err) => return session_persist_failure("read", &err),
            }
        }
        // Merge — never overwrite — so degraded-storage markers survive a
        // dedup/diff serve in the same response.
        if let Some(extra) = summary.telemetry() {
            merge_telemetry(&mut response, extra);
        }
        if stale_store_hit {
            merge_telemetry(
                &mut response,
                json!({
                    "stale": true,
                    "stale_reason": "store_hash_mismatch_disk"
                }),
            );
        }
        // Raw reads keep the verbatim slice contract even when it is empty;
        // raw=true does not imply Mode::Passthrough, so guard it explicitly.
        if !raw && bytes_read == 0 {
            let label = zero_hit_label(
                &paths
                    .iter()
                    .take(max_files)
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            apply_zero_hit_note(&mut response, mode, format!("# read {label} — 0 bytes"));
        }
        response
    }

    /// One allowed path: load, store, capsule, session substitution. Stale-hash
    /// and working-set admit stay visible to the parent via the accumulator.
    #[allow(clippy::too_many_arguments)]
    fn read_one_path(
        &self,
        path: &Path,
        path_count: usize,
        mode: Mode,
        start_line: Option<usize>,
        end_line: Option<usize>,
        raw: bool,
        max_visible_tokens: usize,
        options: &ServeOptions,
        store: &mut RecoveryStore,
        capsule_slots: &mut tokenzero_core::model_artifacts::AppendOnlyCapsuleSlots,
        acc: &mut ReadPathAcc<'_>,
    ) -> Result<(), ToolResponse> {
        if !self.path_allowed(path) {
            return Err(path_not_allowed("read", path, &self.config.allowed_roots));
        }
        let source_start = start_line;
        let source_end = end_line;
        let text_result = if let Some(start) = start_line {
            read_line_range_from_file(path, start, end_line.unwrap_or(start))
        } else {
            fs::read_to_string(path)
        };
        let mut text = match text_result {
            Ok(text) => text,
            Err(err) => {
                // "could not read X (read_failed)" with no cause stranded
                // live sessions guessing between missing file, directory,
                // and permissions. Name the reason and the obvious next op.
                let hint = if path.is_dir() {
                    " (path is a directory - use tree)"
                } else if !path.exists() {
                    " (no such file)"
                } else {
                    ""
                };
                return Err(ToolResponse::error(
                    "read",
                    "read_failed",
                    format!("could not read {}: {err}{hint}", path.display()),
                    None,
                ));
            }
        };
        *acc.bytes_read += text.len();
        let line_count = text.lines().count();
        if path_count == 1 {
            let anchor_start = source_start.unwrap_or(1);
            let anchor_end = source_end
                .unwrap_or_else(|| anchor_start.saturating_add(line_count.saturating_sub(1)));
            *acc.working_set_anchor = Some(tokenzero_recovery::working_set::SpanAnchor {
                path: path.to_path_buf(),
                symbol: None,
                start_line: anchor_start,
                end_line: anchor_end,
            });
        }
        let ctype = detect_content_type(&text, Some(path));
        acc.content_types.push(ctype);
        let stored = if path_count == 1
            && source_start.is_none()
            && source_end.is_none()
            && text.len() >= 64 * 1024
        {
            store.store_source_backed_payload_deferred_batch(&text, ctype, path)
        } else {
            store.store_payload_deferred_batch(&text, ctype, Some(path), source_start, source_end)
        };
        acc.refs
            .push(ref_record("blob", stored.blob_ref.clone(), text.len()));
        acc.refs
            .push(ref_record("file", stored.file_ref.clone(), text.len()));
        let capsule_result = if raw {
            Ok(tokenzero_core::Capsule {
                text: text.clone(),
                raw_tokens: stored.raw_tokens,
                visible_tokens: stored.raw_tokens,
                omitted_lines: 0,
                mode,
                protected_anchors: Vec::new(),
                exact_refs: Vec::new(),
                lossy_spans: Vec::new(),
                lossy_policy_id: None,
            })
        } else {
            // Capsule admission (ZS-VIEW-006): the default policy keeps the
            // legacy fixed byte threshold byte-for-byte; HorizonCost requires
            // labeled expansion/horizon and refuses when those are missing.
            let capsule_mode = match self.read_payload_admission(
                text.len(),
                mode,
                exact_ref_token_count(&acc.refs),
            ) {
                Ok(LocalPayloadPolicy::Inline) => mode,
                Ok(LocalPayloadPolicy::ExactRef) => Mode::Exact,
                Err(response) => return Err(response),
            };
            tokenzero_core::make_capsule_with_recovery_ref(
                &text,
                stored.raw_tokens,
                capsule_mode,
                max_visible_tokens,
                Some(&path.display().to_string()),
                Some(&stored.file_ref),
            )
        };
        let capsule = match capsule_result {
            Ok(capsule) => capsule,
            Err(error) => return Err(capsule_error_response("read", error)),
        };
        let part_text = capsule.text;
        let part_tokens = capsule.visible_tokens;
        // Production formation (ZS-VIEW-002): form the real ModelCapsule
        // artifact with its formation receipt. The plain Capsule above stays
        // authoritative for the visible render; the formed capsule is the
        // production artifact binding constructor, contract root, dependency
        // roots (blob/file store refs), payload root, and epoch, exposed to
        // the caller as a content-addressed ref. Formation failures that do
        // not violate the append-never-rewrite policy degrade to no capsule
        // ref (e.g. passthrough payloads beyond the capsule render bound), so
        // reads never fail because an optional artifact could not be formed.
        // A same-key different-bytes re-formation fails loud: that is the
        // append-never-rewrite violation.
        if let Ok(formed) = Self::form_read_model_capsule(
            path,
            source_start,
            source_end,
            line_count,
            &stored,
            &part_text,
            part_tokens,
        ) {
            if let Err(error) = capsule_slots.record(&formed) {
                return Err(capsule_error_response("read", error.to_string()));
            }
            acc.capsule_refs.push(ref_record(
                "capsule",
                format!("tz://capsule/{}", formed.digest().to_hex()),
                part_text.len(),
            ));
        }
        // Session redundancy layer (docs/codemode.md §5). Zero-payload
        // notes are cheap and stay untouched: empty payloads skip the
        // layer entirely (notes are never deduped).
        if self.config.session_dedup && !text.is_empty() {
            let key = ServeKey::File {
                path: comparable_path(path),
                start: source_start,
                end: source_end,
            };
            let disk_sha = sha256_hex(&text);
            let content_sha256 = stored
                .blob_ref
                .strip_prefix("tz://blob/")
                .filter(|digest| digest.len() == 64)
                .map(str::to_owned)
                .unwrap_or_else(|| disk_sha.clone());
            // Store hash must match the bytes just read from disk.
            // A drifted pin (tokenzero-oquc) must never collapse to
            // "unchanged" or an expand of superseded refs.
            let stale_store = content_sha256 != disk_sha;
            *acc.stale_store_hit |= stale_store;
            // raw keeps the verbatim-slice contract, passthrough keeps
            // its verbatim-payload contract, and fresh is the per-call
            // opt-out; all three bypass the replacement render but still
            // record the serve below so later calls can dedup.
            let bypass = raw || matches!(mode, Mode::Passthrough) || options.fresh || stale_store;
            match self.session_lookup(&key, &content_sha256) {
                SeenState::Unchanged {
                    serve_count,
                    cross_session,
                } if !bypass => {
                    let note = unchanged_read_note(path, &text, &stored, cross_session);
                    let note_tokens = count_tokens(&note);
                    // ROI guard: a note that costs as much as the full
                    // render is never emitted.
                    if note_tokens < part_tokens {
                        acc.substitutions.push(PendingSubstitution::Dedup {
                            idx: acc.visible_parts.len(),
                            note,
                            note_tokens,
                            full_tokens: part_tokens,
                            serve_count: serve_count + 1,
                            cross_session,
                        });
                    }
                }
                SeenState::Changed { previous } if !bypass && self.config.diff_reads => {
                    if let Some((diff_text, diff_tokens, telemetry)) =
                        diff_since_served(store, path, &text, &previous, &stored, part_tokens)
                    {
                        acc.substitutions.push(PendingSubstitution::Diff {
                            idx: acc.visible_parts.len(),
                            text: diff_text,
                            diff_tokens,
                            full_tokens: part_tokens,
                            telemetry,
                        });
                    }
                }
                _ => {}
            }
            acc.pending.push((
                key,
                served_record_with_metadata(content_sha256, text.len(), line_count, &stored),
            ));
        }
        *acc.raw_tokens += capsule.raw_tokens;
        *acc.visible_tokens += part_tokens;
        if !raw {
            let trimmed_len = text.trim_end().len();
            text.truncate(trimmed_len);
            acc.raw_visible_parts.push(text);
        }
        acc.visible_parts.push(part_text);
        Ok(())
    }

    /// Form the production ModelCapsule artifact for one read path
    /// (ZS-VIEW-002). The causal key is the path+range slot, so a changed
    /// payload under the same key is a rewrite, never a silent replacement.
    /// The engine links no provider tokenizer yet (ZS-VIEW-008): profile and
    /// tokenizer identities are the canonical absent markers, map digests are
    /// byte-identity digests, and token counts are the visible-token counts.
    /// The receipt binds constructor, key contract root, store refs, and
    /// payload root; `from_formed` refuses a receipt that does not bind the
    /// actual payload.
    pub fn form_read_model_capsule(
        path: &Path,
        source_start: Option<usize>,
        source_end: Option<usize>,
        line_count: usize,
        stored: &StoredPayload,
        payload: &str,
        payload_tokens: usize,
    ) -> Result<
        tokenzero_core::model_artifacts::ModelCapsule,
        tokenzero_core::model_artifacts::ModelArtifactError,
    > {
        use tokenzero_core::model_artifacts::{
            CapsuleCausalKey, ModelArtifactError, ModelCapsule, ModelCapsuleFormationReceipt,
        };
        let blob_hex = stored
            .blob_ref
            .strip_prefix("tz://blob/")
            .filter(|digest| digest.len() == 64)
            .ok_or_else(|| ModelArtifactError::InvalidBlobRef(stored.blob_ref.clone()))?;
        let source_root = zero_abi::Sha256Digest::from_hex(blob_hex)
            .map_err(|_| ModelArtifactError::InvalidBlobRef(stored.blob_ref.clone()))?;
        let causal_key = CapsuleCausalKey::new(format!(
            "{}:{}..{}",
            path.display(),
            source_start.unwrap_or(1),
            source_end.unwrap_or(line_count),
        ))?;
        let contract_root = causal_key.contract_root()?;
        let payload_root = ModelCapsule::payload_digest(payload.as_bytes());
        // Evidence refs must be portable ZeroRefs (blob kind); the store's
        // engine-owned file ref is carried in the receipt dependency roots.
        let blob_ref = stored.blob_ref.clone();
        let file_ref = stored.file_ref.clone();
        let receipt = ModelCapsuleFormationReceipt::new(
            "tokenzero-engine.read-one-path.v1",
            contract_root,
            vec![file_ref.clone(), blob_ref.clone()],
            payload_root,
            0,
        )?;
        let payload_tokens =
            u64::try_from(payload_tokens).map_err(|_| ModelArtifactError::LengthOverflow)?;
        let formed = ModelCapsule::from_formed(
            causal_key,
            receipt,
            source_root,
            ModelCapsule::absent_model_profile_digest(),
            ModelCapsule::absent_tokenizer_digest(),
            vec![blob_ref],
            Vec::new(),
            payload.as_bytes(),
            payload_root,
            payload_tokens,
            &[],
            ModelCapsule::payload_digest(&[]),
            0,
        )?;
        crate::perf_profile::note_hot_path_capsule();
        Ok(formed)
    }
}

impl TokenZeroEngine {
    /// Capsule admission for one read payload (ZS-VIEW-006). The default
    /// `ByteThreshold` policy is the legacy fixed-threshold rule, unchanged.
    /// `HorizonCost` consults the estimator only with per-call or
    /// replay-derived expansion probability and horizon. This read path has
    /// neither (`ServeOptions` carries only `fresh`; no replay estimator is
    /// wired), so Auto-mode HorizonCost is refused instead of unlabeled
    /// `AdmissionEstimator` defaults. Explicit modes stay inline without
    /// consulting the estimator.
    fn read_payload_admission(
        &self,
        payload_bytes: usize,
        mode: Mode,
        _handling_cost_tokens: usize,
    ) -> Result<LocalPayloadPolicy, ToolResponse> {
        match self.config.admission_policy {
            AdmissionPolicy::ByteThreshold => Ok(local_payload_policy(
                payload_bytes,
                self.config.capsule_exact_ref_threshold_bytes,
                mode,
                true,
            )),
            AdmissionPolicy::HorizonCost if mode != Mode::Auto => Ok(LocalPayloadPolicy::Inline),
            AdmissionPolicy::HorizonCost => Err(failure_response(
                "read",
                "horizon_cost_refused",
                "HorizonCost admission refused: expansion probability and \
                 horizon estimates are missing",
                Some("use ByteThreshold until labeled expansion/horizon exist"),
            )),
        }
    }
}
