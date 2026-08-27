use super::*;
use std::io::Write;

impl TokenZeroEngine {
    pub fn mem(&self) -> ToolResponse {
        let store = self.recovery_store();
        let mut status = store.export_status();
        if let Some(object) = status.as_object_mut() {
            object.insert("session_dedup".to_string(), self.session_rollup());
        }
        let text = serde_json::to_string_pretty(&status).unwrap_or_else(|_| "{}".to_string());
        let tokens = count_tokens(&text);
        success_response(
            "mem",
            Mode::Hybrid,
            text,
            Vec::new(),
            (tokens, tokens, 0, Some(0)),
        )
    }

    pub fn cache_pack(&self, scope: &str) -> ToolResponse {
        let root = self
            .config
            .allowed_roots
            .first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));
        let mut stable_sections = Vec::new();
        let mut source_paths = cache_pack_sources(&root, scope);
        source_paths.sort();
        source_paths.dedup();
        for path in &source_paths {
            if let Ok(text) = fs::read_to_string(path) {
                stable_sections.push(format!(
                    "## {}\n{}",
                    path.strip_prefix(&root).unwrap_or(path).display(),
                    text.trim_end()
                ));
            }
        }
        let mut repo_rows = Vec::new();
        let mut unreadable = 0usize;
        collect_tree(
            &root,
            &root,
            3,
            false,
            500,
            0,
            &mut repo_rows,
            &mut unreadable,
        );
        repo_rows.retain(|row| {
            !row.rel.contains("recovery-cache")
                && !row.rel.contains("cache.json")
                && !row.rel.contains("cache-packs")
                && !row.rel.ends_with(".lock")
                && !row.rel.starts_with("ledger.jsonl")
                && !row.rel.starts_with("maintenance.")
                && !row.rel.starts_with("gc.")
                && !row.rel.starts_with(".tokenzero")
        });
        let mut repo_map = repo_rows
            .iter()
            .map(|row| row.rel.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if unreadable > 0 {
            repo_map.push_str(&format!(
                "\n# scan incomplete: {unreadable} unreadable paths"
            ));
        }
        stable_sections.push(format!("## repo-map\n{repo_map}"));
        let operation_contract =
            serde_json::to_string_pretty(&tokenzero_core::operation_abi::contract_manifest())
                .unwrap_or_default();
        stable_sections.push(format!("## operation-abi\n{operation_contract}"));
        let stable_text = stable_sections.join("\n\n");
        let volatile_text = format!(
            "volatile_tail:\nroot: {}\nmanifest: {}\nexpand refs for exact current source bytes\n",
            root.display(),
            cache_pack_manifest_path(&self.config.cache_path, scope).display()
        );
        let content_digest = sha256_hex(&stable_text);
        let cache_key = format!(
            "tz-cache-pack-v1:{}:{}",
            scope,
            content_digest.chars().take(16).collect::<String>()
        );
        let manifest_path = cache_pack_manifest_path(&self.config.cache_path, scope);
        let invalidation_reason = previous_cache_digest(&manifest_path)
            .map_or("new_pack", |previous| {
                if previous == content_digest {
                    "unchanged"
                } else {
                    "sources_changed"
                }
            })
            .to_string();
        let mut store = self.recovery_store();
        let stable_stored = store.store_payload_deferred(
            &stable_text,
            ContentType::Markdown,
            Some(Path::new("cache-pack:stable-prefix")),
            None,
            None,
        );
        let volatile_stored = store.store_payload_deferred(
            &volatile_text,
            ContentType::Markdown,
            Some(Path::new("cache-pack:volatile-tail")),
            None,
            None,
        );
        if let Err(err) = store.persist_pending() {
            return failure_response(
                "cache-pack",
                "cache_write_failed",
                err.to_string(),
                Some("fix recovery cache permissions"),
            );
        }
        // The manifest embeds these refs; if eviction dropped either during
        // the persist, fail loud instead of publishing dead handles.
        if !store.has_ref(&stable_stored.blob_ref) || !store.has_ref(&volatile_stored.blob_ref) {
            return failure_response(
                "cache-pack",
                "cache_evicted",
                "cache pack payload was evicted from the recovery cache before it could be advertised",
                Some("increase recovery cache max_bytes or reduce the pack scope"),
            );
        }
        let cacheable_tokens = count_tokens(&stable_text);
        let volatile_tokens = count_tokens(&volatile_text);
        let invalidation_count = if invalidation_reason == "unchanged" {
            0
        } else {
            1
        };
        let manifest = json!({
            "schema_version": "tokenzero.cache-pack.v1",
            "status": "ok",
            "scope": scope,
            "cache_key": cache_key,
            "content_digest": content_digest,
            "cacheable_tokens": cacheable_tokens,
            "stable_prefix_tokens": cacheable_tokens,
            "volatile_tokens": volatile_tokens,
            "estimated_cached_tokens": cacheable_tokens,
            "estimated_cached_token_savings": cacheable_tokens.saturating_sub(count_tokens(&stable_stored.blob_ref)),
            "prefix_stability_ratio": if cacheable_tokens + volatile_tokens == 0 { 0.0 } else { cacheable_tokens as f64 / (cacheable_tokens + volatile_tokens) as f64 },
            "invalidation_reason": invalidation_reason,
            "invalidation_count": invalidation_count,
            "daemon_required": false,
            "source_count": source_paths.len(),
            "source_paths": source_paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "source_refs": [stable_stored.blob_ref.clone()],
            "volatile_refs": [volatile_stored.blob_ref.clone()],
            "host_hints": {
                "stable_prefix_first": true,
                "volatile_tail_last": true,
                "expand_before_sensitive_action": true
            },
            "manifest_path": manifest_path.display().to_string()
        });
        let visible = match serde_json::to_string_pretty(&manifest) {
            Ok(visible) => visible,
            Err(err) => {
                return failure_response(
                    "cache-pack",
                    "manifest_serialize_failed",
                    err.to_string(),
                    Some("report this TokenZero serialization defect"),
                );
            }
        };
        let manifest_body = format!("{visible}\n");
        if let Err(err) = write_cache_pack_manifest(&manifest_path, manifest_body.as_bytes()) {
            return failure_response(
                "cache-pack",
                "manifest_write_failed",
                format!("could not publish {}: {err}", manifest_path.display()),
                Some("fix cache-pack directory permissions and retry"),
            );
        }
        let refs = vec![
            ref_record("stable_prefix", stable_stored.blob_ref, stable_text.len()),
            ref_record(
                "volatile_tail",
                volatile_stored.blob_ref,
                volatile_text.len(),
            ),
        ];
        let exact_ref_tokens = exact_ref_token_count(&refs);
        let mut response = success_response(
            "cache-pack",
            Mode::Structured,
            visible.clone(),
            refs,
            (
                cacheable_tokens + volatile_tokens,
                count_tokens(&visible),
                store.recovery_tokens,
                Some(exact_ref_tokens),
            ),
        );
        response.content_type = Some(ContentType::JsonConfig.to_string());
        response.telemetry = Some(json!({
            "cache_key": manifest["cache_key"],
            "content_digest": manifest["content_digest"],
            "cacheable_tokens": cacheable_tokens,
            "volatile_tokens": volatile_tokens,
            "invalidation_reason": manifest["invalidation_reason"],
            "daemon_required": false
        }));
        response
    }

    pub fn path_allowed(&self, path: &Path) -> bool {
        let abs = if path.is_absolute() {
            comparable_path(path)
        } else {
            comparable_path(&self.config.call_root.join(path))
        };
        // canonicalize_existing_prefix can only resolve `..` while the prefix
        // exists on disk; a `..` left behind a nonexistent component would
        // defeat the component-wise root check below, so fail closed. The
        // filesystem rejects such paths anyway (every component before `..`
        // must exist), so nothing readable is lost.
        if abs
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return false;
        }
        self.config.allowed_roots.iter().any(|root| {
            let root = comparable_path(root);
            abs.starts_with(root)
        })
    }

    /// Resolve a caller-supplied path against `call_root`: relative paths
    /// bind to the configured root instead of the process working directory,
    /// and absolute paths pass through unchanged (`Path::join` semantics).
    /// Filesystem operations must use the resolved path so the allowlist
    /// check and the actual I/O agree on the same file.
    pub(crate) fn resolve_call_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.config.call_root.join(path)
        }
    }
}

fn write_cache_pack_manifest(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|err| err.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
