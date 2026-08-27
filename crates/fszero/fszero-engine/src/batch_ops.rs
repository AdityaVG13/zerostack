use super::access_log::{content_hash_from_ref, rel_path_for_log_with_canon};
use super::batch_evidence::{
    PassEvidence, cache_status_of, operator_for, pass_cache_status, short_digest, snapshot_digest,
};
use super::list_ops::format_stat_manifest;
use super::read_ops::parse_read_arg;
use super::{DomainError, FSZeroSession, OpCode, visible_ack};
use memchr::memmem;
use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

pub struct BatchItemOutcome {
    pub ok: bool,
    pub ack: String,
    pub detail: Option<String>,
    pub source_ref: Option<String>,
    pub payload: Vec<u8>,
    pub error: Option<DomainError>,
    pub fields: Map<String, Value>,
}

impl BatchItemOutcome {
    fn failure(_operation: OpCode, error: DomainError, mut fields: Map<String, Value>) -> Self {
        fields.insert("timing_ns".into(), json!(0));
        fields.insert("timing_scope".into(), json!("shared_batch"));
        Self {
            ok: false,
            ack: "X0".into(),
            detail: Some(error.message.clone()),
            source_ref: None,
            payload: Vec::new(),
            error: Some(error),
            fields,
        }
    }

    fn success(
        operation: OpCode,
        detail: String,
        source_ref: String,
        payload: Vec<u8>,
        mut fields: Map<String, Value>,
    ) -> Self {
        // Fused work has no honest per-row wall attribution.
        fields.insert("timing_ns".into(), json!(0));
        fields.insert("timing_scope".into(), json!("shared_batch"));
        Self {
            ok: true,
            ack: visible_ack(operation, None),
            detail: Some(detail),
            source_ref: Some(source_ref),
            payload,
            error: None,
            fields,
        }
    }
}

pub struct BatchKernelResult {
    pub rows: Vec<BatchItemOutcome>,
    pub physical_passes: usize,
    pub unique_inputs: usize,
    pub visited_files: usize,
    /// Cost-based schedule choice (fszero-h46w).
    pub exec_shape: &'static str,
}

#[derive(Clone)]
struct ReadRequest {
    path: String,
    byte_range: Option<(usize, usize)>,
    line_range: Option<(usize, usize)>,
    max_bytes: Option<usize>,
}

#[derive(Clone)]
enum FileCapture {
    Ok {
        path: PathBuf,
        bytes: Arc<Vec<u8>>,
        content_ref: Option<Arc<str>>,
        cache_status: &'static str,
        consistency: &'static str,
    },
    Err(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileMetadataSignature {
    len: u64,
    modified: SystemTime,
}

fn metadata_signature(path: &Path) -> Result<FileMetadataSignature, String> {
    let metadata = fs::metadata(path).map_err(|error| format!("metadata failed: {error}"))?;
    let modified = metadata
        .modified()
        .map_err(|error| format!("modified time failed: {error}"))?;
    Ok(FileMetadataSignature {
        len: metadata.len(),
        modified,
    })
}

fn capture_file(
    path: &Path,
) -> Result<(Arc<Vec<u8>>, Option<FileMetadataSignature>, &'static str), String> {
    // Metadata only: fs::read on a FIFO/socket blocks the worker.
    crate::path::refuse_non_regular_file(path)?;
    let before = metadata_signature(path).ok();
    let content = fs::read(path).map_err(|error| format!("read failed: {error}"))?;
    let after = metadata_signature(path).ok();
    match (before, after) {
        (Some(before), Some(after)) if before == after => {
            Ok((Arc::new(content), Some(after), "per_file_metadata_stable"))
        }
        (Some(_), Some(_)) => Err("file changed during capture".into()),
        _ => Ok((Arc::new(content), None, "per_file_unverified")),
    }
}

fn reusable_capture_ref(
    content_ref: Option<&Arc<str>>,
    full_len: usize,
    start: usize,
    end: usize,
    truncated: bool,
) -> Option<String> {
    if !truncated && start == 0 && end == full_len {
        content_ref.map(|reference| reference.to_string())
    } else {
        None
    }
}

impl FSZeroSession {
    /// Execute one vectorized physical batch. Watch reconciliation, per-file
    /// stable capture, index preparation, and traversal happen once for the call.
    /// Captures are not an atomic cross-file filesystem snapshot.
    pub fn execute_batch_kernel(
        &mut self,
        op_id: &str,
        items: &[Value],
        args: &Value,
    ) -> BatchKernelResult {
        self.record_internal_op();
        self.drain_watch_events();
        let budget = super::budget::BatchBudget::from_args(args);
        match op_id {
            "fs.multiRead" => self.multi_read_kernel(items, budget),
            "fs.multiStat" => self.multi_stat_kernel(items),
            "fs.multiSearch" => self.multi_search_kernel(items, args, budget),
            "fs.multiList" => self.multi_list_kernel(items, budget),
            #[cfg(feature = "fszero-ast-sgrep")]
            "fs.multiAstSearch" => self.multi_ast_search_kernel(items),
            _ => BatchKernelResult {
                rows: Vec::new(),
                physical_passes: 0,
                unique_inputs: 0,
                visited_files: 0,
                exec_shape: "inline",
            },
        }
    }

    fn multi_read_kernel(
        &mut self,
        items: &[Value],
        budget: super::budget::BatchBudget,
    ) -> BatchKernelResult {
        let parsed: Vec<Result<ReadRequest, DomainError>> =
            items.iter().map(parse_read_request).collect();
        let root = self.root.clone();
        let mut captures: HashMap<String, FileCapture> = HashMap::new();
        for request in parsed.iter().filter_map(|r| r.as_ref().ok()) {
            if captures.contains_key(&request.path) {
                continue;
            }
            let captured = match self.resolve_existing_path_cached(root.as_deref(), &request.path) {
                Ok(path) => {
                    let cached = self
                        .caches
                        .content
                        .get(&path)
                        .filter(|entry| super::read_ops::content_cache_fresh(&path, entry))
                        .map(|entry| (Arc::clone(&entry.bytes), Arc::clone(&entry.content_ref)));
                    if let Some((bytes, content_ref)) = cached {
                        FileCapture::Ok {
                            path,
                            bytes,
                            content_ref: Some(content_ref),
                            cache_status: "hit",
                            consistency: "cached_metadata_match",
                        }
                    } else {
                        match capture_file(&path) {
                            Ok((bytes, stable_signature, consistency)) => {
                                let (cache_status, content_ref) = if let Some(mtime) =
                                    stable_signature.map(|signature| signature.modified)
                                {
                                    let content_ref: Arc<str> =
                                        Arc::from(self.recovery.put_content_ref(bytes.as_slice()));
                                    self.caches.content.insert(
                                        path.clone(),
                                        super::ReadCacheEntry {
                                            bytes: Arc::clone(&bytes),
                                            mtime,
                                            content_ref: Arc::clone(&content_ref),
                                        },
                                    );
                                    ("miss", Some(content_ref))
                                } else {
                                    ("uncached", None)
                                };
                                FileCapture::Ok {
                                    path,
                                    bytes,
                                    content_ref,
                                    cache_status,
                                    consistency,
                                }
                            }
                            Err(error) => FileCapture::Err(error),
                        }
                    }
                }
                Err(e) => FileCapture::Err(format!("bad path: {e}")),
            };
            captures.insert(request.path.clone(), captured);
        }
        let unique_inputs = captures.len();
        let mut pass = PassEvidence::new(operator_for("fs.multiRead"));
        let mut inputs = BTreeMap::new();
        let (mut cached, mut read) = (0usize, 0usize);
        for (path, capture) in &captures {
            if let FileCapture::Ok {
                bytes,
                cache_status,
                ..
            } = capture
            {
                inputs.insert(path.clone(), short_digest(bytes.as_slice()));
                if *cache_status == "hit" {
                    cached += 1;
                } else {
                    read += 1;
                }
            }
        }
        pass.snapshot = snapshot_digest(&inputs);
        pass.cache_status = pass_cache_status(cached, read);
        let mut tracker = super::budget::BatchBudgetTracker::start(budget);
        let mut rows = Vec::with_capacity(items.len());
        for (index, request) in parsed.into_iter().enumerate() {
            let row_start = Instant::now();
            let mut fields = Map::new();
            fields.insert("index".into(), json!(index));
            let request = match request {
                Ok(request) => request,
                Err(error) => {
                    pass.attach_error(&mut fields, &json!({ "index": index }), row_start);
                    rows.push(BatchItemOutcome::failure(OpCode::Read, error, fields));
                    continue;
                }
            };
            fields.insert("path".into(), json!(request.path));
            let params = json!({
                "path": request.path, "byte_range": request.byte_range,
                "line_range": request.line_range, "max_bytes": request.max_bytes,
            });
            // Cooperative budget: after a hit, remaining ok rows are empty truncated prefixes.
            if tracker.should_stop() {
                fields.insert("truncated".into(), json!(true));
                fields.insert(
                    "budget_hit".into(),
                    json!(tracker.hit_kind.unwrap_or("budget")),
                );
                fields.insert("span".into(), json!({"start_line": 0, "end_line": 0}));
                fields.insert("payload_len".into(), json!(0));
                let source_ref = self.recovery.put_content_ref(b"");
                fields.insert("content_ref".into(), json!(source_ref));
                pass.attach(&mut fields, &params, row_start, json!([0, 0]), true, "cold");
                rows.push(BatchItemOutcome::success(
                    OpCode::Read,
                    format!("read:budget-truncated ref={source_ref}"),
                    source_ref,
                    Vec::new(),
                    fields,
                ));
                continue;
            }
            let capture = captures.get(&request.path).expect("parsed path captured");
            let (full_path, bytes, content_ref, row_cache_status) = match capture {
                FileCapture::Ok {
                    path,
                    bytes,
                    content_ref,
                    cache_status,
                    consistency,
                } => {
                    fields.insert("cache_status".into(), json!(cache_status));
                    fields.insert("consistency".into(), json!(consistency));
                    (path, bytes, content_ref, cache_status_of(cache_status))
                }
                FileCapture::Err(message) => {
                    pass.attach_error(&mut fields, &params, row_start);
                    rows.push(BatchItemOutcome::failure(
                        OpCode::Read,
                        DomainError::from_detail(message),
                        fields,
                    ));
                    continue;
                }
            };
            let (start, end, span) =
                project_read_span(bytes, request.byte_range, request.line_range);
            let selected = &bytes[start..end];
            let (payload, truncated) = tracker.take_bytes(selected, request.max_bytes);
            let take = payload.len();
            let source_ref =
                reusable_capture_ref(content_ref.as_ref(), bytes.len(), start, end, truncated)
                    .unwrap_or_else(|| self.recovery.put_content_ref(&payload));
            fields.insert("span".into(), span);
            fields.insert("truncated".into(), json!(truncated));
            if truncated {
                fields.insert(
                    "budget_hit".into(),
                    json!(tracker.hit_kind.unwrap_or("bytes")),
                );
            }
            fields.insert("content_ref".into(), json!(source_ref));
            fields.insert("payload_len".into(), json!(payload.len()));
            pass.attach(
                &mut fields,
                &params,
                row_start,
                json!([start, start + take]),
                truncated,
                row_cache_status,
            );
            let rel =
                rel_path_for_log_with_canon(root.as_deref(), self.root_canon.as_deref(), full_path);
            self.record_access("read", &rel, content_hash_from_ref(&source_ref));
            let detail = format!("read:{} bytes ref={source_ref}", payload.len());
            rows.push(BatchItemOutcome::success(
                OpCode::Read,
                detail,
                source_ref,
                payload,
                fields,
            ));
        }
        let shape = super::batch_cse::choose_exec_shape(
            items.len(),
            unique_inputs,
            cached > 0 && read == 0,
        );
        BatchKernelResult {
            rows,
            physical_passes: usize::from(!captures.is_empty()),
            unique_inputs,
            visited_files: 0,
            exec_shape: shape.as_str(),
        }
    }

    fn multi_stat_kernel(&mut self, items: &[Value]) -> BatchKernelResult {
        let parsed: Vec<Result<String, DomainError>> =
            items.iter().map(parse_path_request).collect();
        let root = self.root.clone();
        let mut captures: HashMap<String, Result<(PathBuf, String), String>> = HashMap::new();
        for path_arg in parsed.iter().filter_map(|r| r.as_ref().ok()) {
            if captures.contains_key(path_arg) {
                continue;
            }
            let captured = match self.resolve_existing_path_cached(root.as_deref(), path_arg) {
                Ok(path) => match fs::symlink_metadata(&path) {
                    Ok(meta) => Ok((path.clone(), format_stat_manifest(&path, &meta))),
                    Err(e) => Err(format!("metadata failed: {e}")),
                },
                Err(e) => Err(format!("bad path: {e}")),
            };
            captures.insert(path_arg.clone(), captured);
        }
        let unique_inputs = captures.len();
        let mut pass = PassEvidence::new(operator_for("fs.multiStat"));
        let mut inputs = BTreeMap::new();
        for (path, capture) in &captures {
            if let Ok((_, manifest)) = capture {
                inputs.insert(path.clone(), short_digest(manifest.as_bytes()));
            }
        }
        // Stat reads live directory metadata every call; nothing is memoized.
        pass.snapshot = snapshot_digest(&inputs);
        pass.cache_status = pass_cache_status(0, inputs.len());
        let mut rows = Vec::with_capacity(items.len());
        for (index, path) in parsed.into_iter().enumerate() {
            let row_start = Instant::now();
            let mut fields = Map::new();
            fields.insert("index".into(), json!(index));
            let path = match path {
                Ok(path) => path,
                Err(error) => {
                    pass.attach_error(&mut fields, &json!({ "index": index }), row_start);
                    rows.push(BatchItemOutcome::failure(OpCode::Stat, error, fields));
                    continue;
                }
            };
            fields.insert("path".into(), json!(path));
            let params = json!({ "path": path });
            match captures.get(&path).expect("parsed path captured") {
                Ok((_full_path, manifest)) => {
                    let payload = manifest.as_bytes().to_vec();
                    let source_ref = self.recovery.put_content_ref(&payload);
                    fields.insert("payload_len".into(), json!(payload.len()));
                    pass.attach(
                        &mut fields,
                        &params,
                        row_start,
                        json!([0, payload.len()]),
                        false,
                        "miss",
                    );
                    let detail = format!("stat:{} bytes ref={source_ref}", payload.len());
                    rows.push(BatchItemOutcome::success(
                        OpCode::Stat,
                        detail,
                        source_ref,
                        payload,
                        fields,
                    ));
                }
                Err(message) => {
                    pass.attach_error(&mut fields, &params, row_start);
                    rows.push(BatchItemOutcome::failure(
                        OpCode::Stat,
                        DomainError::from_detail(message),
                        fields,
                    ));
                }
            }
        }
        let shape = super::batch_cse::choose_exec_shape(items.len(), unique_inputs, false);
        BatchKernelResult {
            rows,
            physical_passes: usize::from(!captures.is_empty()),
            unique_inputs,
            visited_files: 0,
            exec_shape: shape.as_str(),
        }
    }

    fn multi_list_kernel(
        &mut self,
        items: &[Value],
        _budget: super::budget::BatchBudget,
    ) -> BatchKernelResult {
        let parsed: Vec<Result<super::multi_list::ListManyItem, DomainError>> = items
            .iter()
            .map(|item| {
                if let Some(path) = item.as_str() {
                    Ok(super::multi_list::ListManyItem::new(path))
                } else {
                    serde_json::from_value::<super::multi_list::ListManyItem>(item.clone()).map_err(
                        |e| DomainError::invalid_argument(format!("multi_list item parse: {e}")),
                    )
                }
            })
            .collect();
        let valid_items: Vec<super::multi_list::ListManyItem> = parsed
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .cloned()
            .collect();
        let results = self.multi_list(&valid_items);
        let mut pass = PassEvidence::new(operator_for("fs.multiList"));
        let mut inputs = BTreeMap::new();
        let (mut cached, mut read) = (0usize, 0usize);
        for result in &results {
            inputs.insert(
                result.path.clone(),
                short_digest(&serde_json::to_vec(&result.entries).unwrap_or_default()),
            );
            match result.cache_status {
                super::multi_list::ListCacheStatus::Warm => cached += 1,
                super::multi_list::ListCacheStatus::Cold => read += 1,
            }
        }
        pass.snapshot = snapshot_digest(&inputs);
        pass.cache_status = pass_cache_status(cached, read);
        let mut rows = Vec::with_capacity(items.len());
        let mut valid_iter = results.into_iter();
        for (index, parse_result) in parsed.into_iter().enumerate() {
            let row_start = Instant::now();
            let mut fields = Map::new();
            fields.insert("index".into(), json!(index));
            match parse_result {
                Ok(item) => {
                    let result = valid_iter.next().expect("result per valid item");
                    let params = json!({ "path": item.path, "depth": item.depth, "include_hidden": item.include_hidden });
                    fields.insert("path".into(), json!(result.path));
                    let label = match result.cache_status {
                        super::multi_list::ListCacheStatus::Warm => "warm",
                        super::multi_list::ListCacheStatus::Cold => "cold",
                    };
                    fields.insert("cache_status".into(), json!(label));
                    if let Some(error) = &result.error {
                        pass.attach_error(&mut fields, &params, row_start);
                        rows.push(BatchItemOutcome::failure(
                            OpCode::Ls,
                            DomainError::from_detail(error),
                            fields,
                        ));
                    } else {
                        let payload = serde_json::to_vec(&result.entries).unwrap_or_default();
                        let source_ref = self.recovery.put_content_ref(&payload);
                        fields.insert("entry_count".into(), json!(result.entries.len()));
                        pass.attach(
                            &mut fields,
                            &params,
                            row_start,
                            json!([0, payload.len()]),
                            false,
                            cache_status_of(label),
                        );
                        let detail =
                            format!("list:{} entries ref={source_ref}", result.entries.len());
                        rows.push(BatchItemOutcome::success(
                            OpCode::Ls,
                            detail,
                            source_ref,
                            payload,
                            fields,
                        ));
                    }
                }
                Err(error) => {
                    pass.attach_error(&mut fields, &json!({ "index": index }), row_start);
                    rows.push(BatchItemOutcome::failure(OpCode::Ls, error, fields));
                }
            }
        }
        let shape = super::batch_cse::choose_exec_shape(items.len(), valid_items.len(), false);
        BatchKernelResult {
            rows,
            physical_passes: 1,
            unique_inputs: valid_items.len(),
            visited_files: 0,
            exec_shape: shape.as_str(),
        }
    }

    /// One multi-pattern AST walk: every item is evaluated during ONE parse of
    /// each interested file, and the session syntax forest keeps unchanged files
    /// from being reparsed across calls.
    #[cfg(feature = "fszero-ast-sgrep")]
    fn multi_ast_search_kernel(&mut self, items: &[Value]) -> BatchKernelResult {
        use super::multi_ast_search::{AstFile, AstItem, multi_ast_search};

        let parsed: Vec<Result<AstItem, DomainError>> =
            items.iter().map(parse_ast_request).collect();
        let root = self.root_canon.clone().or_else(|| self.root.clone());
        let mut traversal_error = None;
        if root.is_none() {
            traversal_error = Some(DomainError::invalid_argument("no root"));
        }
        if traversal_error.is_none() {
            if let Err(e) = self.prepare_index_or_busy(root.as_deref()) {
                traversal_error = Some(DomainError::from_detail(&e));
            }
        }

        let valid: Vec<AstItem> = parsed
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .map(|item| {
                AstItem::new(
                    &item.language,
                    &item.pattern,
                    item.paths.clone(),
                    item.limit,
                )
            })
            .collect();

        // Read every indexed structural file once; the walk then parses each at
        // most once for ALL patterns of its language.
        let mut texts: Vec<(String, String)> = Vec::new();
        let mut visited_files = 0usize;
        let mut inputs = BTreeMap::new();
        let (mut cached_files, mut read_files) = (0usize, 0usize);
        if traversal_error.is_none() && !valid.is_empty() {
            let root = root.as_deref().expect("root checked");
            let mut keys: Vec<&str> = self
                .index
                .indexed_file_keys
                .iter()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            for key in keys {
                if !super::ast::is_structural_file_key(key) {
                    continue;
                }
                let path = root.join(key);
                let cached = self
                    .caches
                    .content
                    .get(&path)
                    .filter(|entry| super::read_ops::content_cache_fresh(&path, entry))
                    .map(|entry| Arc::clone(&entry.bytes));
                let bytes = match cached {
                    Some(bytes) => {
                        cached_files += 1;
                        bytes
                    }
                    None => match capture_file(&path) {
                        Ok((bytes, stable_signature, _)) => {
                            read_files += 1;
                            if let Some(mtime) =
                                stable_signature.map(|signature| signature.modified)
                            {
                                let content_ref =
                                    Arc::from(self.recovery.put_content_ref(bytes.as_slice()));
                                self.caches.content.insert(
                                    path.clone(),
                                    super::ReadCacheEntry {
                                        bytes: Arc::clone(&bytes),
                                        mtime,
                                        content_ref,
                                    },
                                );
                            }
                            bytes
                        }
                        Err(_) => continue,
                    },
                };
                let Ok(text) = std::str::from_utf8(bytes.as_slice()) else {
                    continue;
                };
                visited_files += 1;
                inputs.insert(key.to_string(), short_digest(bytes.as_slice()));
                texts.push((key.to_string(), text.to_string()));
            }
        }

        let files: Vec<AstFile<'_>> = texts
            .iter()
            .map(|(key, text)| AstFile {
                file_key: key,
                text,
            })
            .collect();
        let mut results = if traversal_error.is_none() && !valid.is_empty() {
            multi_ast_search(&mut self.caches.ast_forest, &valid, &files).into_iter()
        } else {
            Vec::new().into_iter()
        };

        let mut pass = PassEvidence::new(operator_for("fs.multiAstSearch"));
        pass.snapshot = snapshot_digest(&inputs);
        pass.cache_status = pass_cache_status(cached_files, read_files);
        let mut rows = Vec::with_capacity(items.len());
        for (index, parse_result) in parsed.into_iter().enumerate() {
            let row_start = Instant::now();
            let mut fields = Map::new();
            fields.insert("index".into(), json!(index));
            let item = match parse_result {
                Ok(item) => item,
                Err(error) => {
                    pass.attach_error(&mut fields, &json!({ "index": index }), row_start);
                    rows.push(BatchItemOutcome::failure(OpCode::Search, error, fields));
                    continue;
                }
            };
            fields.insert("language".into(), json!(item.language));
            fields.insert("pattern".into(), json!(item.pattern));
            let params = json!({
                "language": item.language, "pattern": item.pattern,
                "paths": item.paths, "limit": item.limit,
            });
            if let Some(error) = traversal_error.clone() {
                pass.attach_error(&mut fields, &params, row_start);
                rows.push(BatchItemOutcome::failure(OpCode::Search, error, fields));
                continue;
            }
            let result = results.next().expect("result per valid item");
            if let Some(error) = &result.error {
                pass.attach_error(&mut fields, &params, row_start);
                rows.push(BatchItemOutcome::failure(
                    OpCode::Search,
                    DomainError::invalid_argument(error.clone()),
                    fields,
                ));
                continue;
            }
            let hits: Vec<Value> = result
                .hits
                .iter()
                .map(|hit| {
                    json!({
                        "path": hit.path, "span": hit.span, "preview": hit.preview,
                    })
                })
                .collect();
            fields.insert("match_count".into(), json!(hits.len()));
            fields.insert("truncated".into(), json!(result.truncated));
            fields.insert("visited_files".into(), json!(visited_files));
            let payload = serde_json::to_vec(&hits).unwrap_or_default();
            let source_ref = self.recovery.put_content_ref(&payload);
            pass.attach(
                &mut fields,
                &params,
                row_start,
                json!([0, payload.len()]),
                result.truncated,
                pass.cache_status,
            );
            let detail = format!("ast-search:{} hits ref={source_ref}", hits.len());
            rows.push(BatchItemOutcome::success(
                OpCode::Search,
                detail,
                source_ref,
                payload,
                fields,
            ));
        }
        let shape = super::batch_cse::choose_exec_shape(items.len(), visited_files.max(1), false);
        BatchKernelResult {
            rows,
            physical_passes: usize::from(traversal_error.is_none() && !valid.is_empty()),
            unique_inputs: items.len(),
            visited_files,
            exec_shape: shape.as_str(),
        }
    }

    fn multi_search_kernel(
        &mut self,
        items: &[Value],
        args: &Value,
        budget: super::budget::BatchBudget,
    ) -> BatchKernelResult {
        let mut requests: Vec<SearchRequest> = Vec::with_capacity(items.len());
        let mut parse_errors: Vec<Option<DomainError>> = Vec::with_capacity(items.len());
        for (index, item) in items.iter().enumerate() {
            match SearchRequest::parse(index, item) {
                Ok(mut request) => {
                    // Per-item limit composes with batch max_matches_per_query (min wins).
                    request.limit = budget.match_cap(Some(request.limit));
                    requests.push(request);
                    parse_errors.push(None);
                }
                Err(error) => {
                    requests.push(SearchRequest::invalid(index));
                    parse_errors.push(Some(error));
                }
            }
        }
        let has_valid_query = parse_errors.iter().any(Option::is_none);
        // Use root_canon so cache keys match resolve_existing_path_cached,
        // which canonicalizes paths before inserting into self.caches.content.
        let root = self.root_canon.clone().or_else(|| self.root.clone());
        let mut traversal_error = None;
        if root.is_none() {
            traversal_error = Some(DomainError::invalid_argument("no root"));
        }
        if traversal_error.is_none() {
            if let Err(e) = self.prepare_index_or_busy(root.as_deref()) {
                traversal_error = Some(DomainError::from_detail(&e));
            }
        }
        if traversal_error.is_none() {
            if let Some(message) = super::search::files_budget_message(self.indexed_file_count()) {
                traversal_error = Some(DomainError::from_detail(&message));
            }
        }

        let global_limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(usize::MAX);
        let mut tracker = super::budget::BatchBudgetTracker::start(budget);
        // CSE / fusion: path filters across queries share one visit set (multi_search is already one-pass).
        let path_filters: Vec<Vec<String>> = requests.iter().map(|r| r.paths.clone()).collect();
        let fusion = super::batch_cse::plan_search_ast_fusion(&path_filters, &[]);
        let mut visited_files = 0usize;
        let mut total_hits = 0usize;
        let mut inputs = BTreeMap::new();
        let (mut cached_files, mut read_files) = (0usize, 0usize);
        if traversal_error.is_none() {
            let root = root.as_deref().expect("root checked");
            let mut keys: Vec<&str> = self
                .index
                .indexed_file_keys
                .iter()
                .map(String::as_str)
                .collect();
            keys.sort_unstable();
            for request in &mut requests {
                request.retry_without_unmatched_path_constraints(&keys);
            }
            for key in keys {
                if tracker.should_stop() {
                    for request in &mut requests {
                        if !request.complete() {
                            request.truncated = true;
                        }
                    }
                    break;
                }
                if requests.iter().all(SearchRequest::complete) {
                    break;
                }
                let interested: Vec<usize> = requests
                    .iter()
                    .enumerate()
                    .filter_map(|(index, request)| {
                        (!request.complete() && request.matches_path(key)).then_some(index)
                    })
                    .collect();
                if interested.is_empty() {
                    continue;
                }
                let needles: Option<Vec<&[u8]>> = interested
                    .iter()
                    .map(|index| requests[*index].prefilter_needle())
                    .collect();
                if needles.as_ref().is_some_and(|needles| {
                    self.lazy_bigrams.may_contain_stable(root, key, needles) == Some(false)
                }) {
                    continue;
                }
                let path = root.join(key);
                let cached = self
                    .caches
                    .content
                    .get(&path)
                    .filter(|entry| super::read_ops::content_cache_fresh(&path, entry))
                    .map(|entry| {
                        let signature = FileMetadataSignature {
                            len: entry.bytes.len() as u64,
                            modified: entry.mtime,
                        };
                        (Arc::clone(&entry.bytes), Some(signature))
                    });
                let captured = if let Some(cached) = cached {
                    cached_files += 1;
                    Ok(cached)
                } else {
                    read_files += 1;
                    capture_file(&path).or_else(|_| capture_file(&path)).map(
                        |(bytes, signature, _)| {
                            if let Some(signature) = signature {
                                let content_ref =
                                    Arc::from(self.recovery.put_content_ref(bytes.as_slice()));
                                self.caches.content.insert(
                                    path.clone(),
                                    super::ReadCacheEntry {
                                        bytes: Arc::clone(&bytes),
                                        mtime: signature.modified,
                                        content_ref,
                                    },
                                );
                            }
                            (bytes, signature)
                        },
                    )
                };
                let (bytes, signature) = match captured {
                    Ok(captured) => captured,
                    Err(_) => {
                        for index in interested {
                            requests[index].incomplete = true;
                        }
                        continue;
                    }
                };
                if let Some(signature) =
                    signature.filter(|signature| metadata_signature(&path).ok() == Some(*signature))
                {
                    self.lazy_bigrams.upsert_stable(
                        key,
                        bytes.as_slice(),
                        signature.modified,
                        signature.len,
                    );
                }
                let Ok(text) = std::str::from_utf8(bytes.as_slice()) else {
                    for index in interested {
                        requests[index].incomplete = true;
                    }
                    continue;
                };
                visited_files += 1;
                inputs.insert(key.to_string(), short_digest(bytes.as_slice()));
                let mut byte_base = 0usize;
                for (line_index, raw_line) in text.split_inclusive('\n').enumerate() {
                    let without_newline = raw_line.strip_suffix('\n').unwrap_or(raw_line);
                    let line = without_newline
                        .strip_suffix('\r')
                        .unwrap_or(without_newline);
                    for request in &mut requests {
                        if request.complete() || !request.matches_path(key) {
                            continue;
                        }
                        let Some((start, end)) = request.find(line) else {
                            continue;
                        };
                        if request.hits.len() >= request.limit || total_hits >= global_limit {
                            request.truncated = true;
                            continue;
                        }
                        request.hits.push(json!({
                            "query_id": request.query_id, "path": key,
                            "line_range": [line_index + 1, line_index + 1],
                            "byte_span": [byte_base + start, byte_base + end], "preview": line,
                        }));
                        total_hits += 1;
                    }
                    byte_base += raw_line.len();
                }
            }
        }

        let mut pass = PassEvidence::new(operator_for("fs.multiSearch"));
        pass.snapshot = snapshot_digest(&inputs);
        pass.cache_status = pass_cache_status(cached_files, read_files);
        let mut rows = Vec::with_capacity(items.len());
        for (index, request) in requests.into_iter().enumerate() {
            let row_start = Instant::now();
            let mut fields = Map::new();
            fields.insert("index".into(), json!(index));
            fields.insert("query_id".into(), request.query_id.clone());
            fields.insert("query".into(), json!(request.query));
            let params = json!({
                "query_id": request.query_id, "query": request.query,
                "paths": request.paths, "limit": request.limit,
            });
            if let Some(error) = parse_errors[index]
                .take()
                .or_else(|| traversal_error.clone())
            {
                pass.attach_error(&mut fields, &params, row_start);
                rows.push(BatchItemOutcome::failure(OpCode::Search, error, fields));
                continue;
            }
            fields.insert("match_count".into(), json!(request.hits.len()));
            let truncated = request.truncated || request.incomplete || tracker.hit;
            fields.insert("truncated".into(), json!(truncated));
            if tracker.hit {
                fields.insert(
                    "budget_hit".into(),
                    json!(tracker.hit_kind.unwrap_or("budget")),
                );
            }
            fields.insert("visited_files".into(), json!(visited_files));
            fields.insert("path_union_len".into(), json!(fusion.path_union.len()));
            if request.constraint_fallback {
                fields.insert("constraint_fallback".into(), json!("paths_relaxed"));
            }
            let payload = serde_json::to_vec(&request.hits).unwrap_or_default();
            tracker.record_bytes(payload.len());
            let source_ref = self.recovery.put_content_ref(&payload);
            pass.attach(
                &mut fields,
                &params,
                row_start,
                json!([0, payload.len()]),
                truncated,
                pass.cache_status,
            );
            let fallback = if request.constraint_fallback {
                " constraint_fallback=paths_relaxed"
            } else {
                ""
            };
            let detail = format!(
                "search:{} hits ref={source_ref}{fallback}",
                request.hits.len()
            );
            rows.push(BatchItemOutcome::success(
                OpCode::Search,
                detail,
                source_ref,
                payload,
                fields,
            ));
        }
        let shape = super::batch_cse::choose_exec_shape(items.len(), visited_files.max(1), false);
        BatchKernelResult {
            rows,
            physical_passes: usize::from(traversal_error.is_none() && has_valid_query),
            unique_inputs: items.len(),
            visited_files,
            exec_shape: shape.as_str(),
        }
    }
}

/// Parse one multiAstSearch item: {language, pattern, paths?, limit?}.
/// Pattern compile errors are reported per row by the walk, not here.
#[cfg(feature = "fszero-ast-sgrep")]
fn parse_ast_request(value: &Value) -> Result<super::multi_ast_search::AstItem, DomainError> {
    let object = value
        .as_object()
        .ok_or_else(|| DomainError::invalid_argument("ast item must be an object"))?;
    let language = object
        .get("language")
        .and_then(Value::as_str)
        .ok_or_else(|| DomainError::invalid_argument("ast item missing language"))?;
    let pattern = object
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| DomainError::invalid_argument("ast item missing pattern"))?;
    let paths = match object.get("paths") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(path)) => vec![path.clone()],
        Some(Value::Array(paths)) => paths
            .iter()
            .map(|p| {
                p.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| DomainError::invalid_argument("paths entries must be strings"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(DomainError::invalid_argument(
                "paths must be a string or string array",
            ));
        }
    };
    let limit = match object.get("limit") {
        None | Some(Value::Null) => usize::MAX,
        Some(limit) => usize::try_from(limit.as_u64().ok_or_else(|| {
            DomainError::invalid_argument("limit must be a non-negative integer")
        })?)
        .map_err(|_| DomainError::invalid_argument("limit exceeds platform limits"))?,
    };
    Ok(super::multi_ast_search::AstItem::new(
        language, pattern, paths, limit,
    ))
}

fn parse_path_request(value: &Value) -> Result<String, DomainError> {
    if let Some(path) = value.as_str() {
        return Ok(path.to_string());
    }
    value
        .get("path")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            DomainError::invalid_argument("path item must be a string or object with path")
        })
}

fn parse_read_request(value: &Value) -> Result<ReadRequest, DomainError> {
    if let Some(raw) = value.as_str() {
        let (path, byte_range) = parse_read_arg(raw).map_err(DomainError::invalid_argument)?;
        let byte_range = byte_range
            .map(|range| {
                let start = usize::try_from(range.start).map_err(|_| {
                    DomainError::invalid_argument("range start exceeds platform limits")
                })?;
                let end = usize::try_from(range.end).map_err(|_| {
                    DomainError::invalid_argument("range end exceeds platform limits")
                })?;
                Ok::<_, DomainError>((start, end))
            })
            .transpose()?;
        return Ok(ReadRequest {
            path: path.to_string(),
            byte_range,
            line_range: None,
            max_bytes: None,
        });
    }
    let object = value
        .as_object()
        .ok_or_else(|| DomainError::invalid_argument("read item must be a string or object"))?;
    let path = object
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| DomainError::invalid_argument("read item missing path"))?;
    let line_range = match object.get("range") {
        None => None,
        Some(Value::Array(range)) if range.len() == 2 => {
            let start = usize::try_from(range[0].as_u64().ok_or_else(|| {
                DomainError::invalid_argument("range start must be a positive line number")
            })?)
            .map_err(|_| DomainError::invalid_argument("range start exceeds platform limits"))?;
            let end = usize::try_from(range[1].as_u64().ok_or_else(|| {
                DomainError::invalid_argument("range end must be a positive line number")
            })?)
            .map_err(|_| DomainError::invalid_argument("range end exceeds platform limits"))?;
            if start == 0 || end < start {
                return Err(DomainError::invalid_argument(
                    "range must be [start_line,end_line] with 1 <= start <= end",
                ));
            }
            Some((start, end))
        }
        Some(_) => {
            return Err(DomainError::invalid_argument(
                "range must be [start_line,end_line]",
            ));
        }
    };
    let max_bytes = match object.get("max_bytes") {
        None => None,
        Some(value) => Some(
            usize::try_from(value.as_u64().ok_or_else(|| {
                DomainError::invalid_argument("max_bytes must be a non-negative integer")
            })?)
            .map_err(|_| DomainError::invalid_argument("max_bytes exceeds platform limits"))?,
        ),
    };
    Ok(ReadRequest {
        path: path.to_string(),
        byte_range: None,
        line_range,
        max_bytes,
    })
}

fn project_read_span(
    bytes: &[u8],
    byte_range: Option<(usize, usize)>,
    line_range: Option<(usize, usize)>,
) -> (usize, usize, Value) {
    if let Some((range_start, range_end)) = byte_range {
        let start = range_start.min(bytes.len());
        let end = range_end.min(bytes.len()).max(start);
        return (start, end, json!({"start_byte": start, "end_byte": end}));
    }
    if let Some((start_line, end_line)) = line_range {
        let mut line = 1usize;
        let mut start = if start_line == 1 { Some(0) } else { None };
        let mut end = None;
        for (index, byte) in bytes.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            if line == end_line {
                end = Some(index + 1);
                break;
            }
            line += 1;
            if line == start_line {
                start = Some(index + 1);
            }
        }
        let start = start.unwrap_or(bytes.len());
        let end = end.unwrap_or(bytes.len()).max(start);
        let actual_end = if start == bytes.len() && start_line > 1 {
            start_line.saturating_sub(1)
        } else {
            end_line.min(
                bytes[..end].iter().filter(|b| **b == b'\n').count()
                    + usize::from(end > 0 && bytes[end - 1] != b'\n'),
            )
        };
        return (
            start,
            end,
            json!({"start_line": start_line, "end_line": actual_end}),
        );
    }
    (
        0,
        bytes.len(),
        json!({"start_byte": 0, "end_byte": bytes.len()}),
    )
}

struct SearchRequest {
    query_id: Value,
    query: String,
    literal: String,
    matcher: Option<Regex>,
    paths: Vec<String>,
    limit: usize,
    hits: Vec<Value>,
    truncated: bool,
    incomplete: bool,
    invalid: bool,
    constraint_fallback: bool,
}

impl SearchRequest {
    fn parse(index: usize, value: &Value) -> Result<Self, DomainError> {
        let (query, query_id, paths, limit, case_sensitive, regex) =
            if let Some(query) = value.as_str() {
                (
                    query.to_string(),
                    json!(index),
                    Vec::new(),
                    16usize,
                    true,
                    false,
                )
            } else {
                let object = value.as_object().ok_or_else(|| {
                    DomainError::invalid_argument("search item must be a string or object")
                })?;
                let query = object
                    .get("query")
                    .and_then(Value::as_str)
                    .ok_or_else(|| DomainError::invalid_argument("search item missing query"))?
                    .to_string();
                let query_id = object
                    .get("query_id")
                    .or_else(|| object.get("id"))
                    .cloned()
                    .unwrap_or_else(|| json!(index));
                let paths = match object.get("paths") {
                    None => Vec::new(),
                    Some(Value::String(path)) => vec![path.clone()],
                    Some(Value::Array(paths)) => paths
                        .iter()
                        .map(|p| {
                            p.as_str().map(str::to_string).ok_or_else(|| {
                                DomainError::invalid_argument("paths entries must be strings")
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    Some(_) => {
                        return Err(DomainError::invalid_argument(
                            "paths must be a string or string array",
                        ));
                    }
                };
                let limit = object
                    .get("limit")
                    .and_then(Value::as_u64)
                    .map(|n| n as usize)
                    .unwrap_or(16);
                let case_sensitive = object.get("case").and_then(Value::as_bool).unwrap_or(true);
                let regex = object
                    .get("regex")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                (query, query_id, paths, limit, case_sensitive, regex)
            };
        if query.is_empty() {
            return Err(DomainError::invalid_argument(
                "search query must not be empty",
            ));
        }
        let matcher_pattern = if regex {
            Some(query.clone())
        } else if !case_sensitive {
            Some(regex::escape(&query))
        } else {
            None
        };
        let matcher = matcher_pattern
            .as_deref()
            .map(|pattern| {
                RegexBuilder::new(pattern)
                    .case_insensitive(!case_sensitive)
                    .build()
                    .map_err(|e| DomainError::invalid_argument(format!("invalid regex: {e}")))
            })
            .transpose()?;
        let literal = query.clone();
        Ok(Self {
            query_id,
            query,
            literal,
            matcher,
            paths,
            limit,
            hits: Vec::new(),
            truncated: false,
            incomplete: false,
            invalid: false,
            constraint_fallback: false,
        })
    }

    fn invalid(index: usize) -> Self {
        Self {
            query_id: json!(index),
            query: String::new(),
            literal: String::new(),
            matcher: None,
            paths: Vec::new(),
            limit: 0,
            hits: Vec::new(),
            truncated: false,
            incomplete: false,
            invalid: true,
            constraint_fallback: false,
        }
    }

    fn retry_without_unmatched_path_constraints(&mut self, candidates: &[&str]) {
        if !self.paths.is_empty() && !candidates.iter().any(|path| self.matches_path(path)) {
            self.constraint_fallback = true;
        }
    }

    fn complete(&self) -> bool {
        self.invalid || self.truncated
    }

    fn prefilter_needle(&self) -> Option<&[u8]> {
        self.matcher.is_none().then_some(self.literal.as_bytes())
    }

    fn matches_path(&self, path: &str) -> bool {
        self.constraint_fallback
            || self.paths.is_empty()
            || self.paths.iter().any(|pattern| glob_matches(pattern, path))
    }

    fn find(&self, line: &str) -> Option<(usize, usize)> {
        if let Some(regex) = &self.matcher {
            return regex.find(line).map(|m| (m.start(), m.end()));
        }
        memmem::find(line.as_bytes(), self.literal.as_bytes())
            .map(|start| (start, start + self.literal.len()))
    }
}

fn glob_matches(pattern: &str, text: &str) -> bool {
    // A path with no glob metacharacters is a directory or file prefix.
    // `crates/` and `crates` must match `crates/foo.rs` so compound
    // search path= does not fall back to the whole repo.
    if !pattern.as_bytes().iter().any(|b| *b == b'*' || *b == b'?') {
        let prefix = pattern.trim_end_matches('/');
        return text == prefix || text.starts_with(&format!("{prefix}/"));
    }
    let (pattern, text) = (pattern.as_bytes(), text.as_bytes());
    let (mut p, mut t, mut star, mut mark) = (0usize, 0usize, None, 0usize);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            mark = t;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}
