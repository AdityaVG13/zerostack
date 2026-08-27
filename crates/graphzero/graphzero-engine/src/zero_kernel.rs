//! Typed GraphZero implementation consumed directly by ZeroKernel.
//!
//! `z.find` is the typed structural entry point. Natural/pattern/semantic modes execute
//! ast-sgrep in-process; graph relationship modes execute GraphZero's typed
//! query router. Index freshness is automatic and no command registry, MCP
//! tool, raw worker, or subprocess search participates.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ast_sgrep_core::{IndexOptions, IndexStats, Indexer, SearchOptions, Searcher, search_pattern};
use graphzero_core::atlas::LocusRank;
use graphzero_core::cognitive_work::{
    MechanicalGraphVerdict, MechanicalRegionInput, ProofSupportHyperedge, TypedObligation,
    TypedObligationKind, classify_mechanical_region,
};
use graphzero_core::decision::{ClosureClass, DecisionGap, EvidenceKind};
use graphzero_core::graph::NodeId;
use graphzero_core::truth::TruthClass;
use graphzero_core::world_fiber::FiberClass;
use graphzero_store::store::csr::{CsrAdjacency, edge_kind};
use graphzero_store::store::query::QueryEngine;
use graphzero_store::store::refs::blob_span_ref;
use graphzero_store::store::symbol_table::SymbolTable;
use graphzero_types::ContentHash;
use zero_abi::{
    AsgrepMode, EngineError, EngineErrorKind, EngineInvocation, SafetyVerdict, StructuralAbsence,
    StructuralBudget, StructuralCoverage, StructuralEngine, StructuralHit, StructuralQuery,
    StructuralResult, TASK_LENS_CONTRACT_VERSION, TaskLensCompilerImpact, TaskLensRequest,
    TaskLensResult, ZeroHandle,
};
use zero_store::{StoreLock, ZeroCas};

use crate::query_surface::{QuerySurfaceRequest, QuerySurfaceResponse, QuerySurfaceRouter};

pub struct ZeroStructuralEngine {
    repo_root: PathBuf,
    graph_store_root: PathBuf,
    cas: ZeroCas,
    ast_query_lock: Arc<Mutex<()>>,
    graph_index_lock: Arc<Mutex<()>>,
    graph_pool: Arc<OnceLock<Result<rayon::ThreadPool, String>>>,
}

impl Clone for ZeroStructuralEngine {
    fn clone(&self) -> Self {
        Self {
            repo_root: self.repo_root.clone(),
            graph_store_root: self.graph_store_root.clone(),
            cas: self.cas.clone(),
            ast_query_lock: Arc::clone(&self.ast_query_lock),
            graph_index_lock: Arc::clone(&self.graph_index_lock),
            graph_pool: Arc::clone(&self.graph_pool),
        }
    }
}

impl std::fmt::Debug for ZeroStructuralEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZeroStructuralEngine")
            .field("repo_root", &self.repo_root)
            .field("graph_store_root", &self.graph_store_root)
            .field("ast_query_lock", &self.ast_query_lock)
            .field("graph_index_lock", &self.graph_index_lock)
            .field("graph_pool_initialized", &self.graph_pool.get().is_some())
            .finish_non_exhaustive()
    }
}

impl ZeroStructuralEngine {
    /// Authoritative ZeroKernel index worker limit. Both AST and graph
    /// indexing through this adapter are capped to one worker to avoid
    /// ~500% CPU amplification on harness activation / first-use Zero.
    pub const INDEX_WORKERS: usize = 1;
    /// Bounded reverse-impact closure edge budget for task-lens verdicts.
    /// Exceeding the bound degrades the closure to incomplete (Unknown).
    pub const TASK_LENS_IMPACT_BOUND: usize = 512;

    fn ensure_graph_pool(&self) -> Result<&rayon::ThreadPool, EngineError> {
        self.graph_pool
            .get_or_init(|| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(Self::INDEX_WORKERS)
                    .build()
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|error| {
                EngineError::new(
                    EngineErrorKind::Internal,
                    format!("build GraphZero index pool: {error}"),
                    false,
                )
            })
    }

    pub fn open(
        repo_root: impl Into<PathBuf>,
        graph_store_root: impl Into<PathBuf>,
        zero_store_root: impl Into<PathBuf>,
    ) -> Result<Self, EngineError> {
        let repo_root = std::fs::canonicalize(repo_root.into()).map_err(|error| {
            EngineError::new(
                EngineErrorKind::InvalidInput,
                format!("canonicalize GraphZero root: {error}"),
                false,
            )
        })?;
        Ok(Self {
            repo_root,
            graph_store_root: graph_store_root.into(),
            cas: ZeroCas::open(zero_store_root),
            ast_query_lock: Arc::new(Mutex::new(())),
            graph_index_lock: Arc::new(Mutex::new(())),
            graph_pool: Arc::new(OnceLock::new()),
        })
    }

    fn cancelled(invocation: &EngineInvocation) -> Result<(), EngineError> {
        if invocation.cancellation.is_cancelled() {
            return Err(EngineError::new(
                EngineErrorKind::Cancelled,
                "GraphZero query cancelled",
                false,
            ));
        }
        if now_ms() >= invocation.context.deadline_unix_ms {
            return Err(EngineError::new(
                EngineErrorKind::Deadline,
                "GraphZero query deadline exceeded",
                true,
            ));
        }
        Ok(())
    }

    fn deadline(invocation: &EngineInvocation) -> Option<Instant> {
        let remaining = invocation.context.deadline_unix_ms.saturating_sub(now_ms());
        Some(Instant::now() + Duration::from_millis(remaining))
    }

    fn ast_index_path(&self, scope: Option<&str>) -> Result<PathBuf, EngineError> {
        let directory = self.graph_store_root.join("ast-sgrep");
        std::fs::create_dir_all(&directory).map_err(ast_error)?;
        match scope {
            Some(scope) => {
                let directory = directory.join("scopes");
                std::fs::create_dir_all(&directory).map_err(ast_error)?;
                let digest = blake3::hash(scope.as_bytes()).to_hex();
                Ok(directory.join(format!("{digest}.db")))
            }
            None => Ok(directory.join("index.db")),
        }
    }

    fn ast_file_filter(&self, query: &StructuralQuery) -> Result<Option<String>, EngineError> {
        let Some(path) = query.options.path.as_ref() else {
            return Ok(None);
        };
        if path.as_os_str().is_empty() || path == Path::new(".") {
            return Ok(None);
        }
        if path.is_absolute() {
            return Err(EngineError::new(
                EngineErrorKind::OutsideWorkspace,
                "z.find is confined to the kernel root; absolute paths are byte operations only (z.read/z.edit)",
                false,
            ));
        }
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(EngineError::new(
                EngineErrorKind::OutsideWorkspace,
                "z.find path must be workspace-relative",
                false,
            ));
        }
        Ok(Some(path.to_string_lossy().into_owned()))
    }

    fn is_glob_filter(filter: &str) -> bool {
        filter.contains('*') || filter.contains('?') || filter.contains('[')
    }

    fn file_matches_prefix(file: &str, prefix: &str) -> bool {
        let file = file.trim_start_matches("./");
        let prefix = prefix.trim_end_matches('/');
        if prefix.is_empty() {
            return true;
        }
        file == prefix || file.starts_with(&format!("{prefix}/"))
    }

    fn ast_scope(&self, filter: Option<&str>) -> Option<(PathBuf, String, Option<String>)> {
        let filter = filter?;
        if Self::is_glob_filter(filter) {
            return None;
        }
        let prefix = filter.trim_end_matches('/');
        let relative = Path::new(prefix);
        let target = self.repo_root.join(relative);
        if target.is_dir() {
            return Some((target, prefix.to_owned(), None));
        }
        if !target.is_file() {
            return None;
        }
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        let filename = relative.file_name()?.to_string_lossy().into_owned();
        Some((
            self.repo_root.join(parent),
            parent.to_string_lossy().into_owned(),
            Some(filename),
        ))
    }

    fn ensure_ast_index(
        &self,
        invocation: &EngineInvocation,
        query: &StructuralQuery,
        index_root: &Path,
        index_path: &Path,
    ) -> Result<IndexStats, EngineError> {
        Self::cancelled(invocation)?;
        let remaining_ms = invocation.context.deadline_unix_ms.saturating_sub(now_ms());
        let lock_root = self.graph_store_root.join("ast-sgrep-lock");
        let _process_guard =
            StoreLock::sweep(&lock_root, Duration::from_millis(remaining_ms.max(1))).map_err(
                |error| {
                    let deadline = error.kind() == std::io::ErrorKind::WouldBlock;
                    let kind = if deadline {
                        EngineErrorKind::Deadline
                    } else {
                        EngineErrorKind::Io
                    };
                    EngineError::new(
                        kind,
                        format!("acquire cross-process ast-sgrep index lock: {error}"),
                        deadline,
                    )
                },
            )?;
        Self::cancelled(invocation)?;

        let semantic = matches!(query.options.mode, AsgrepMode::Semantic);
        let mut indexer = Indexer::new(IndexOptions {
            root: index_root.to_path_buf(),
            index_path: Some(index_path.to_path_buf()),
            lang_filter: query.options.language.clone(),
            embed_semantic: semantic,
            ..IndexOptions::default()
        })
        .map_err(|error| {
            EngineError::new(
                EngineErrorKind::Internal,
                format!(
                    "ast-sgrep open scoped index root={} index={}: {error}",
                    index_root.display(),
                    index_path.display()
                ),
                true,
            )
        })?;
        let cancel = invocation
            .cancellation
            .atomic_flag()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        indexer.set_cancel(Arc::clone(&cancel));
        indexer.set_thread_limit(Self::INDEX_WORKERS);
        let finished = Arc::new(AtomicBool::new(false));
        let watcher_finished = Arc::clone(&finished);
        let watcher_cancel = Arc::clone(&cancel);
        let caller_cancel = Arc::clone(&invocation.cancellation);
        let deadline_unix_ms = invocation.context.deadline_unix_ms;
        let watcher = thread::Builder::new()
            .name("graphzero-index-deadline".into())
            .spawn(move || {
                while !watcher_finished.load(Ordering::Acquire) {
                    if caller_cancel.is_cancelled() || now_ms() >= deadline_unix_ms {
                        watcher_cancel.store(true, Ordering::Release);
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            })
            .map_err(|error| {
                EngineError::new(
                    EngineErrorKind::Internal,
                    format!("spawn ast-sgrep deadline watcher: {error}"),
                    false,
                )
            })?;
        let indexed = indexer
            .index_all()
            .map_err(|error| ast_stage_error("refresh scoped index", error));
        finished.store(true, Ordering::Release);
        let _ = watcher.join();
        let stats = indexed?;
        Self::cancelled(invocation)?;
        Ok(stats)
    }

    fn ensure_graph_index(&self, invocation: &EngineInvocation) -> Result<(), EngineError> {
        Self::cancelled(invocation)?;
        let manifest = self.graph_store_root.join(".manifest");
        if manifest.exists() {
            return Ok(());
        }
        let _guard = self.graph_index_lock.lock().map_err(|_| {
            EngineError::new(
                EngineErrorKind::Internal,
                "GraphZero cold-index lock is poisoned",
                false,
            )
        })?;
        Self::cancelled(invocation)?;
        if manifest.exists() {
            return Ok(());
        }
        let pool = self.ensure_graph_pool()?;
        pool.install(|| {
            graphzero_store::store::indexer::index_repo_in_process(
                &self.repo_root,
                &self.graph_store_root,
                Self::deadline(invocation),
                invocation.cancellation.atomic_flag(),
            )
        })
        .map_err(graph_error)?;
        Self::cancelled(invocation)
    }

    fn refresh_graph_index(
        &self,
        invocation: &EngineInvocation,
    ) -> Result<Arc<graphzero_store::Snapshot>, EngineError> {
        let pool = self.ensure_graph_pool()?;
        pool.install(|| {
            graphzero_store::store::indexer::repair_repo_in_process(
                &self.repo_root,
                &self.graph_store_root,
                Self::deadline(invocation),
                invocation.cancellation.atomic_flag(),
            )
        })
        .map_err(graph_error)?;
        graphzero_store::Snapshot::open(&self.graph_store_root, Some(&self.repo_root))
            .map(Arc::new)
            .map_err(graph_error)
    }
    fn ast_query(
        &self,
        invocation: &EngineInvocation,
        query: &StructuralQuery,
    ) -> Result<StructuralResult, EngineError> {
        let _guard = self.ast_query_lock.lock().map_err(|_| {
            EngineError::new(
                EngineErrorKind::Internal,
                "GraphZero ast-index lock is poisoned",
                false,
            )
        })?;
        Self::cancelled(invocation)?;
        let raw_filter = self.ast_file_filter(query)?;
        let scoped = self.ast_scope(raw_filter.as_deref());
        let (index_root, scope_prefix, scoped_file) = scoped
            .map(|(root, prefix, file)| (root, Some(prefix), file))
            .unwrap_or_else(|| (self.repo_root.clone(), None, None));
        let index_path = self.ast_index_path(scope_prefix.as_deref())?;
        let index_stats = self.ensure_ast_index(invocation, query, &index_root, &index_path)?;
        let coverage = ast_structural_coverage(&index_stats);
        let limit = query.options.limit.unwrap_or(16).clamp(1, 100) as usize;
        let semantic = matches!(query.options.mode, AsgrepMode::Semantic);
        let pattern_mode = matches!(query.options.mode, AsgrepMode::Pattern);
        let (core_filter, manual_prefix) = if let Some(file) = scoped_file {
            (Some(file), None)
        } else if scope_prefix.is_some() {
            (None, None)
        } else {
            match &raw_filter {
                Some(filter) if pattern_mode => {
                    if Self::is_glob_filter(filter) {
                        return Err(EngineError::new(
                            EngineErrorKind::InvalidInput,
                            "pattern mode path must name a file or directory, not a glob",
                            false,
                        ));
                    }
                    (None, Some(filter.clone()))
                }
                Some(filter) if Self::is_glob_filter(filter) => (Some(filter.clone()), None),
                Some(filter) => {
                    let prefix = filter.trim_end_matches('/');
                    let filter = if self.repo_root.join(prefix).is_dir() {
                        format!("{prefix}/**")
                    } else {
                        prefix.to_owned()
                    };
                    (Some(filter), None)
                }
                None => (None, None),
            }
        };
        // Exact search passes cap their SQL candidates before finish_response
        // applies file_filter. Bounded overfetch prevents unrelated global hits
        // from starving a scoped result; the public limit is reapplied below.
        let search_limit = if core_filter.is_some() { 100 } else { limit };
        let options = SearchOptions {
            root: index_root.clone(),
            index_path: Some(index_path),
            limit: search_limit,
            lang_filter: query.options.language.clone(),
            file_filter: core_filter,
            use_embed: semantic,
            use_semantic_only: semantic,
            use_repository_vocabulary: semantic,
            ..SearchOptions::default()
        };
        let searcher =
            Searcher::new(options).map_err(|error| ast_stage_error("open scoped search", error))?;
        let mut hits = if matches!(query.options.mode, AsgrepMode::Pattern) {
            search_pattern(
                &query.query,
                searcher.store(),
                &index_root,
                query.options.language.as_deref(),
            )
            .map_err(|error| ast_stage_error("pattern search", error))?
        } else {
            let indexed_query = match query.options.mode {
                AsgrepMode::Word => format!("word:{}", query.query),
                AsgrepMode::Literal => format!("literal:{}", query.query),
                AsgrepMode::Regex => format!("regex:{}", query.query),
                AsgrepMode::Imports => format!("imports:{}", query.query),
                AsgrepMode::Definition => format!("defs:{}", query.query),
                AsgrepMode::Callers => format!("callers:{}", query.query),
                _ => query.query.clone(),
            };
            searcher
                .search(&indexed_query)
                .map_err(|error| ast_stage_error("indexed search", error))?
                .hits
        };
        if let Some(prefix) = manual_prefix.as_deref() {
            hits.retain(|hit| Self::file_matches_prefix(&hit.file, prefix));
        }
        // A non-empty hit carries source/evidence. A clean indexing pass certifies
        // the selected source scope was refreshed, but ranked absence remains unknown.
        let complete = !hits.is_empty() && coverage.freshness_verified;
        let serialized = serde_json::to_vec(&hits).map_err(|error| {
            EngineError::new(EngineErrorKind::Internal, error.to_string(), false)
        })?;
        let evidence = self.cas.put(&serialized).map_err(cas_error)?;
        let mut sources = BTreeMap::new();
        let hits: Vec<StructuralHit> = hits
            .into_iter()
            .take(limit)
            .map(|hit| {
                let path = PathBuf::from(hit.file);
                let path = if path.is_absolute() {
                    path.strip_prefix(&self.repo_root)
                        .unwrap_or(&path)
                        .to_path_buf()
                } else if let Some(prefix) = scope_prefix.as_deref() {
                    PathBuf::from(prefix).join(path)
                } else {
                    path
                };
                let source = sources
                    .entry(path.clone())
                    .or_insert_with(|| source_handle(&self.cas, &self.repo_root, &path))
                    .clone();
                StructuralHit {
                    path,
                    symbol: hit.symbol.or(hit.caller).or(hit.callee),
                    line_start: Some(hit.line_start),
                    line_end: Some(hit.line_end),
                    preview: Some(hit.excerpt),
                    evidence: Some(evidence.clone()),
                    source,
                    score: finite_score(hit.score),
                }
            })
            .collect();
        let absence = hits.is_empty().then(|| StructuralAbsence {
            class: "unknown".into(),
            reason: format!(
                "no ranked hits for {:?}; indexed absence is not certified",
                query.options.mode
            ),
            coverage: Some(coverage.clone()),
            suggestion: "retry with literal, word, or definition mode and narrow the path".into(),
        });
        let diagnostic = if coverage.freshness_verified {
            absence.as_ref().map(|absence| absence.reason.clone())
        } else {
            Some("ast-sgrep indexing was incomplete; results may omit source files".into())
        };
        Ok(StructuralResult {
            hits,
            index_digest: blake3::hash(&serialized).to_hex().to_string(),
            complete,
            coverage: Some(coverage),
            absence,
            budget: None,
            diagnostic,
            continuation: (!complete).then_some(evidence),
        })
    }

    fn intent_anchor_query(
        &self,
        invocation: &EngineInvocation,
        query: &StructuralQuery,
    ) -> Result<Option<StructuralResult>, EngineError> {
        self.ensure_graph_index(invocation)?;
        let snapshot =
            graphzero_store::Snapshot::open(&self.graph_store_root, Some(&self.repo_root))
                .map_err(graph_error)?;
        let resolved = match graphzero_store::store::query::snap_to_edit(&snapshot, &query.query) {
            Ok(resolved) => resolved,
            Err(_) => return Ok(None),
        };
        let path_filter = self.ast_file_filter(query)?;
        let mut anchors = Vec::with_capacity(1 + resolved.alternates.len());
        anchors.push(resolved.best);
        anchors.extend(resolved.alternates);
        if let Some(filter) = path_filter.as_deref() {
            anchors.retain(|anchor| Self::file_matches_prefix(&anchor.path, filter));
        }
        if anchors.is_empty() {
            return Ok(None);
        }
        let serialized = serde_json::to_vec(&anchors).map_err(|error| {
            EngineError::new(EngineErrorKind::Internal, error.to_string(), false)
        })?;
        let evidence = self.cas.put(&serialized).map_err(cas_error)?;
        let limit = query.options.limit.unwrap_or(16).clamp(1, 100) as usize;
        let hits = anchors
            .into_iter()
            .take(limit)
            .map(|anchor| {
                let path = PathBuf::from(anchor.path);
                let source = source_handle(&self.cas, &self.repo_root, &path);
                StructuralHit {
                    path,
                    symbol: Some(anchor.symbol),
                    line_start: Some(anchor.line),
                    line_end: Some(anchor.line),
                    preview: Some(format!(
                        "{} at bytes {}..{}",
                        anchor.definition_kind, anchor.byte_span.start, anchor.byte_span.end
                    )),
                    evidence: Some(evidence.clone()),
                    source,
                    score: finite_score(anchor.confidence),
                }
            })
            .collect::<Vec<_>>();
        let complete = hits
            .first()
            .is_some_and(|hit| hit.score >= 0.8 && hit.source.is_some());
        Ok(Some(StructuralResult {
            hits,
            index_digest: evidence.digest().to_string(),
            complete,
            coverage: Some(StructuralCoverage {
                tier_a_pct: 100.0,
                tier_b_pct: 0.0,
                tier_c_pct: 0.0,
                freshness_verified: true,
                snapshot_id: 0,
            }),
            absence: None,
            budget: None,
            diagnostic: (!complete)
                .then(|| "intent anchor confidence is below the exact threshold".into()),
            continuation: (!complete).then_some(evidence),
        }))
    }

    fn graph_query(
        &self,
        invocation: &EngineInvocation,
        query: &StructuralQuery,
    ) -> Result<StructuralResult, EngineError> {
        self.ensure_graph_index(invocation)?;
        let surface = match query.options.mode {
            AsgrepMode::Symbols | AsgrepMode::Definition => "symbol",
            AsgrepMode::References => "context",
            AsgrepMode::Callers => "callers",
            AsgrepMode::Callees => "deps",
            AsgrepMode::CallPath => "callpath",
            _ => "search",
        };
        let mut snapshot =
            graphzero_store::Snapshot::open_cached(&self.graph_store_root, Some(&self.repo_root))
                .map_err(graph_error)?;
        // Surface-aware request shape: symbol expects `name`, context/references expects
        // `query`; setting both to the same raw string can mismatch surface validation
        // and drops rows via convert_graph_response. See pc_e89d88c3c2a8.
        let (name, query_str) = match surface {
            "symbol" => (Some(query.query.clone()), None),
            "context" => (None, Some(query.query.clone())),
            "callers" | "deps" => (Some(query.query.clone()), None),
            "callpath" => (
                query
                    .options
                    .source
                    .clone()
                    .or_else(|| Some(query.query.clone())),
                query
                    .options
                    .sink
                    .clone()
                    .or_else(|| Some(query.query.clone())),
            ),
            _ => (
                query
                    .options
                    .source
                    .clone()
                    .or_else(|| Some(query.query.clone())),
                query
                    .options
                    .sink
                    .clone()
                    .or_else(|| Some(query.query.clone())),
            ),
        };
        let requested_budget = query.options.budget_tokens.unwrap_or(512).clamp(8, 8_192) as usize;
        let request = QuerySurfaceRequest {
            surface: surface.into(),
            name,
            query: query_str,
            path: query
                .options
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            budget: Some(requested_budget),
            session: Some(invocation.context.session_id.clone()),
            cursor: None,
        };
        let mut response = match QuerySurfaceRouter::execute(&snapshot, &request) {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = %error, "GraphZero query failed; rebuilding before retry");
                snapshot = self.refresh_graph_index(invocation)?;
                QuerySurfaceRouter::execute(&snapshot, &request).map_err(graph_error)?
            }
        };
        if !response.coverage.freshness_verified {
            snapshot = self.refresh_graph_index(invocation)?;
            response = QuerySurfaceRouter::execute(&snapshot, &request).map_err(graph_error)?;
        }
        Self::cancelled(invocation)?;
        self.convert_graph_response(
            response,
            query.options.limit.unwrap_or(16) as usize,
            requested_budget,
            query.options.path.as_deref(),
        )
    }

    fn convert_graph_response(
        &self,
        response: QuerySurfaceResponse,
        limit: usize,
        requested_budget: usize,
        requested_path: Option<&Path>,
    ) -> Result<StructuralResult, EngineError> {
        let serialized = serde_json::to_vec(&response).map_err(|error| {
            EngineError::new(EngineErrorKind::Internal, error.to_string(), false)
        })?;
        let evidence = self.cas.put(&serialized).map_err(cas_error)?;
        let mut hits = Vec::new();
        for hit in &response.hits {
            let path = path_from_label(&hit.label);
            let source = source_handle(&self.cas, &self.repo_root, &path);
            hits.push(StructuralHit {
                path,
                symbol: Some(hit.label.clone()),
                line_start: None,
                line_end: None,
                preview: Some(hit.snippet.clone()),
                evidence: Some(evidence.clone()),
                source,
                score: 1.0,
            });
        }
        for item in &response.outline {
            let path = PathBuf::from(&item.source);
            let source = source_handle(&self.cas, &self.repo_root, &path);
            hits.push(StructuralHit {
                path,
                symbol: Some(item.name.clone()),
                line_start: item.start_line,
                line_end: item.end_line,
                preview: Some(item.kind.clone()),
                evidence: Some(evidence.clone()),
                source,
                score: 1.0,
            });
        }
        for edge in &response.edges {
            hits.push(StructuralHit {
                path: PathBuf::from("."),
                symbol: Some(match edge.from.as_deref() {
                    Some(from) => format!("{from} -> {}", edge.to),
                    None => edge.to.clone(),
                }),
                line_start: None,
                line_end: None,
                preview: Some(edge.kind.clone()),
                evidence: Some(evidence.clone()),
                source: None,
                score: finite_score(edge.confidence),
            });
        }
        for row in &response.rows {
            let path = row
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            let source = source_handle(&self.cas, &self.repo_root, &path);
            hits.push(StructuralHit {
                path,
                symbol: row
                    .get("symbol")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                line_start: row
                    .get("line_start")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                line_end: row
                    .get("line_end")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok()),
                preview: Some(row.to_string()),
                evidence: Some(evidence.clone()),
                source,
                score: 1.0,
            });
        }
        // Symbol/context surfaces populate decl_ref/capsule rather than
        // hits/rows/edges. Without this synthesis `z.find(..., {mode: "symbols"})`
        // and `references` return 0 hits with complete:false for existing
        // symbols (pc_e89d88c3c2a8). Synthesize definition/usage rows here.
        if hits.is_empty() {
            if let Some(decl) = response.decl_ref.as_deref() {
                let symbol = response.symbol.clone().or_else(|| Some(decl.to_string()));
                let path = requested_path
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("."));
                let source = source_handle(&self.cas, &self.repo_root, &path);
                hits.push(StructuralHit {
                    path,
                    symbol,
                    line_start: None,
                    line_end: None,
                    preview: Some(decl.to_string()),
                    evidence: Some(evidence.clone()),
                    source,
                    score: 1.0,
                });
            }
            if let Some(capsule) = response.capsule.as_ref() {
                if let Some(dests) = capsule
                    .get("destinations")
                    .and_then(|value| value.as_array())
                {
                    for dest in dests {
                        let label = dest
                            .get("label")
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        let path_str = dest
                            .get("path")
                            .and_then(|value| value.as_str())
                            .or_else(|| dest.get("label").and_then(|value| value.as_str()))
                            .unwrap_or(".");
                        let path_clean = path_str.split('#').next().unwrap_or(path_str);
                        let path = if path_clean.is_empty() {
                            PathBuf::from(".")
                        } else {
                            PathBuf::from(path_clean)
                        };
                        let source = source_handle(&self.cas, &self.repo_root, &path);
                        let symbol = dest
                            .get("sym")
                            .and_then(|value| value.as_str())
                            .map(|value| value.to_string())
                            .or_else(|| {
                                if label.is_empty() {
                                    None
                                } else {
                                    Some(label.to_string())
                                }
                            });
                        let preview = dest
                            .get("content")
                            .and_then(|value| value.as_str())
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| dest.to_string());
                        hits.push(StructuralHit {
                            path,
                            symbol,
                            line_start: None,
                            line_end: None,
                            preview: Some(preview),
                            evidence: Some(evidence.clone()),
                            source,
                            score: 1.0,
                        });
                    }
                }
                if hits.len() <= 1 {
                    // Fallback to refs_footer when destinations are empty or synthesized hit was just decl_ref.
                    // For context, refs_footer carries evidence refs for usage rows.
                    if !response.refs_footer.is_empty() {
                        let base_len = if response.decl_ref.is_some() { 1 } else { 0 };
                        if hits.len() == base_len {
                            for reference in &response.refs_footer {
                                hits.push(StructuralHit {
                                    path: PathBuf::from("."),
                                    symbol: Some(reference.clone()),
                                    line_start: None,
                                    line_end: None,
                                    preview: Some(reference.clone()),
                                    evidence: Some(evidence.clone()),
                                    source: None,
                                    score: 1.0,
                                });
                            }
                        }
                    }
                }
            } else if !response.refs_footer.is_empty() {
                for reference in &response.refs_footer {
                    hits.push(StructuralHit {
                        path: PathBuf::from("."),
                        symbol: Some(reference.clone()),
                        line_start: None,
                        line_end: None,
                        preview: Some(reference.clone()),
                        evidence: Some(evidence.clone()),
                        source: None,
                        score: 1.0,
                    });
                }
            }
        }
        hits.truncate(limit.clamp(1, 100));
        let coverage = structural_coverage(&response);
        let budget = structural_budget(&response).or_else(|| {
            let requested = u32::try_from(requested_budget).ok()?;
            let actual_used = u32::try_from(serialized.len().div_ceil(4)).unwrap_or(u32::MAX);
            let used = actual_used.min(requested);
            Some(StructuralBudget {
                requested,
                used,
                actual_used,
                remaining: requested.saturating_sub(used),
                exceeded: actual_used > requested,
                truncated: actual_used > requested,
            })
        });
        let complete = coverage.freshness_verified
            && coverage.tier_a_pct >= 99.0
            && response.truncated != Some(true)
            && response.error.is_none();
        let absence = hits
            .is_empty()
            .then(|| structural_absence(&response, &coverage));
        let diagnostic = (!complete).then(|| {
            absence
                .as_ref()
                .map(|absence| absence.reason.clone())
                .or_else(|| response.error.clone())
                .unwrap_or_else(|| {
                    "GraphZero coverage is partial or stale; absence is not certified".into()
                })
        });
        Ok(StructuralResult {
            hits,
            index_digest: blake3::hash(&serialized).to_hex().to_string(),
            complete,
            coverage: Some(coverage),
            absence,
            budget,
            diagnostic,
            continuation: (!complete).then_some(evidence),
        })
    }
}

/// One reverse-closure edge with its content-addressed span evidence.
#[derive(Clone, Debug, serde::Serialize)]
struct LensClosureEdge {
    from: String,
    to: String,
    kind: &'static str,
    confidence: f64,
    evidence_ref: String,
}

impl ZeroStructuralEngine {
    /// Task-lens verdict over one rooted definition/symbol query.
    ///
    /// Executes the existing GraphZero query path (cold/warm graph index,
    /// symbol surface, capsule extraction), then compiles the live evidence
    /// into a [`MechanicalRegionInput`] and lets
    /// [`classify_mechanical_region`] decide the trivalent verdict. Every
    /// fail-closed gate mirrors a classifier law so reasons stay specific:
    /// `Safe` requires a unique rooted compiler-semantic definition locus, a
    /// fresh >=99% tier-A index, a bounded exact reverse (caller) closure
    /// with edge evidence, and every requested capsule/snapshot root
    /// honored. Any missing, stale, ambiguous, or incomplete condition
    /// degrades to `Unknown`; only explicit semantic decision gaps surface
    /// as `Unsafe`.
    fn lens_symbol(
        &self,
        invocation: &EngineInvocation,
        request: &TaskLensRequest,
    ) -> Result<TaskLensResult, EngineError> {
        // Existing graph query path: cold index, warm snapshot, symbol surface.
        self.ensure_graph_index(invocation)?;
        let mut snapshot =
            graphzero_store::Snapshot::open_cached(&self.graph_store_root, Some(&self.repo_root))
                .map_err(graph_error)?;
        let requested_budget =
            request.options.budget_tokens.unwrap_or(512).clamp(8, 8_192) as usize;
        let surface_request = QuerySurfaceRequest {
            surface: "symbol".into(),
            name: Some(request.query.clone()),
            query: None,
            path: request
                .options
                .path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            budget: Some(requested_budget),
            session: Some(invocation.context.session_id.clone()),
            cursor: None,
        };
        let mut response = match QuerySurfaceRouter::execute(&snapshot, &surface_request) {
            Ok(response) => response,
            Err(error) => {
                tracing::warn!(error = %error, "GraphZero lens query failed; rebuilding before retry");
                snapshot = self.refresh_graph_index(invocation)?;
                QuerySurfaceRouter::execute(&snapshot, &surface_request).map_err(graph_error)?
            }
        };
        if !response.coverage.freshness_verified {
            snapshot = self.refresh_graph_index(invocation)?;
            response =
                QuerySurfaceRouter::execute(&snapshot, &surface_request).map_err(graph_error)?;
        }
        Self::cancelled(invocation)?;
        // Live capsule: the same primitive the symbol surface executes.
        let capsule =
            QueryEngine::warm(&snapshot, &request.query, requested_budget).map_err(graph_error)?;

        let snapshot_root = format!(
            "graphzero:{}:{:016x}",
            snapshot.entry.snapshot_id, snapshot.entry.global_hash
        );
        let index_digest = blake3::hash(snapshot_root.as_bytes()).to_hex().to_string();
        let coverage = StructuralCoverage {
            tier_a_pct: response.coverage.tier_a * 100.0,
            tier_b_pct: response.coverage.tier_b * 100.0,
            tier_c_pct: response.coverage.tier_c * 100.0,
            freshness_verified: response.coverage.freshness_verified,
            snapshot_id: response.coverage.snapshot_id,
        };

        // Explicit semantic decision gaps dominate every other condition.
        let gaps = lens_decision_gaps(&response);
        if !gaps.is_empty() {
            let reasons: Vec<String> = gaps
                .iter()
                .map(|gap| format!("semantic decision gap: {}", gap.reason))
                .collect();
            let result = TaskLensResult {
                verdict: SafetyVerdict::Unsafe {
                    reasons: reasons.clone(),
                },
                locus: None,
                impact: TaskLensCompilerImpact {
                    complete: false,
                    edge_roots: Vec::new(),
                    reverse_roots: Vec::new(),
                },
                proof_support: Vec::new(),
                evidence_roots: Vec::new(),
                coverage: Some(coverage),
                index_digest: index_digest.clone(),
                reasons,
            };
            return Ok(Self::lens_checked(request, result));
        }

        // Requested roots must be honored before any verdict can be Safe.
        let mut evidence_roots: Vec<ZeroHandle> = Vec::new();
        if let Some(capsule_root) = request.capsule_root.as_ref() {
            if self.cas.contains(capsule_root) {
                evidence_roots.push(capsule_root.clone());
            } else {
                return Ok(Self::lens_unknown(
                    request,
                    &index_digest,
                    Some(coverage),
                    "requested capsule root is not present in the store",
                ));
            }
        }
        if let Some(required_snapshot) = request.required_snapshot.as_ref() {
            if required_snapshot.digest() == index_digest {
                evidence_roots.push(required_snapshot.clone());
            } else {
                return Ok(Self::lens_unknown(
                    request,
                    &index_digest,
                    Some(coverage),
                    "required snapshot does not match the live index digest",
                ));
            }
        }

        if let Some(error) = response.error.as_deref() {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                format!("no compiler-resolved definition: {error}"),
            ));
        }
        if capsule.matches.is_empty() {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "no compiler-resolved definition found",
            ));
        }
        if !coverage.freshness_verified {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "graph index freshness is not verified",
            ));
        }
        if coverage.tier_a_pct < 99.0 {
            let reason = format!(
                "tier A coverage is {:.2}%; the task lens requires at least 99%",
                coverage.tier_a_pct
            );
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                reason,
            ));
        }
        if response.truncated == Some(true) {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "query response is truncated",
            ));
        }

        // Unique rooted locus.
        let matches = &capsule.matches;
        if matches.len() != 1 || matches[0].defs.len() != 1 {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "query matches multiple candidate definitions; exactly one rooted locus is required",
            ));
        }
        let definition = &matches[0].defs[0];
        if definition.stale {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "the definition locus is stale",
            ));
        }
        let locus_path = definition
            .path
            .as_deref()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                let hash_hex = blob_hash_from_evidence_ref(&definition.evidence_ref)?;
                snapshot
                    .path_for_blob(&hash_hex)
                    .map(|record| PathBuf::from(&record.path))
            });
        let Some(locus_path) = locus_path else {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "the definition locus is not rooted to a path",
            ));
        };
        let locus_path = if locus_path.is_absolute() {
            locus_path
                .strip_prefix(&self.repo_root)
                .unwrap_or(&locus_path)
                .to_path_buf()
        } else {
            locus_path
        };

        // Bounded exact reverse (caller) closure over the live graph.
        let Some((reverse_edges, bounded)) =
            self.lens_reverse_closure(&snapshot, &matches[0].name, Self::TASK_LENS_IMPACT_BOUND)?
        else {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "the reverse impact closure is unavailable for the locus",
            ));
        };
        if !bounded {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "the reverse impact closure is truncated or incomplete",
            ));
        }
        if reverse_edges.is_empty() {
            return Ok(Self::lens_unknown(
                request,
                &index_digest,
                Some(coverage),
                "the reverse impact closure has no edge evidence",
            ));
        }

        // Content-addressed evidence roots: forward edges, reverse closure,
        // and the composed lens payload.
        let forward_edges: Vec<serde_json::Value> = matches[0]
            .edges
            .iter()
            .map(|edge| {
                serde_json::json!({
                    "kind": edge.kind,
                    "to": edge.to,
                    "confidence": edge.confidence,
                    "evidence_ref": edge.evidence_ref,
                    "source": edge.source,
                })
            })
            .collect();
        let edge_root = self
            .cas
            .put(&serde_json::to_vec(&forward_edges).map_err(|error| {
                EngineError::new(EngineErrorKind::Internal, error.to_string(), false)
            })?)
            .map_err(cas_error)?;
        let reverse_root = self
            .cas
            .put(&serde_json::to_vec(&reverse_edges).map_err(|error| {
                EngineError::new(EngineErrorKind::Internal, error.to_string(), false)
            })?)
            .map_err(cas_error)?;
        let lens_payload = serde_json::json!({
            "contract": "graphzero-task-lens",
            "contract_version": TASK_LENS_CONTRACT_VERSION,
            "surface": "symbol",
            "query": request.query,
            "snapshot_root": snapshot_root,
            "snapshot_id": response.coverage.snapshot_id,
            "coverage": {
                "tier_a": response.coverage.tier_a,
                "tier_b": response.coverage.tier_b,
                "tier_c": response.coverage.tier_c,
                "freshness_verified": response.coverage.freshness_verified,
                "snapshot_id": response.coverage.snapshot_id,
            },
            "locus": {
                "symbol": matches[0].name,
                "def": definition.evidence_ref,
                "path": locus_path,
            },
            "forward_edges": forward_edges,
            "reverse_closure": reverse_edges,
        });
        let evidence = self
            .cas
            .put(&serde_json::to_vec(&lens_payload).map_err(|error| {
                EngineError::new(EngineErrorKind::Internal, error.to_string(), false)
            })?)
            .map_err(cas_error)?;
        evidence_roots.push(evidence.clone());
        evidence_roots.push(edge_root.clone());
        evidence_roots.push(reverse_root.clone());
        let proof_support = evidence_roots.clone();

        // Obligations discharged by fresh proof support bound to the live
        // snapshot root (the requested snapshot identity when it matched).
        let obligations = vec![
            TypedObligation {
                id: "rooted-locus".into(),
                kind: TypedObligationKind::Verification,
                protected_scope_root: index_digest.clone(),
                required_evidence_kinds: vec!["definition".into()],
            },
            TypedObligation {
                id: "reverse-impact-closure".into(),
                kind: TypedObligationKind::Verification,
                protected_scope_root: index_digest.clone(),
                required_evidence_kinds: vec!["compiler_edge".into()],
            },
        ];
        let mut supports = vec![ProofSupportHyperedge {
            id: "rooted-locus-support".into(),
            obligation_id: "rooted-locus".into(),
            sources: vec![definition.evidence_ref.clone()],
            target: matches[0].name.clone(),
            proof_root: index_digest.clone(),
            verifier_contract_root: format!("graphzero-task-lens/{TASK_LENS_CONTRACT_VERSION}"),
            snapshot_root: index_digest.clone(),
            provenance_root: snapshot_root.clone(),
            valid_from_epoch: 0,
            valid_to_epoch: None,
        }];
        supports.push(ProofSupportHyperedge {
            id: "reverse-impact-closure-support".into(),
            obligation_id: "reverse-impact-closure".into(),
            sources: reverse_edges
                .iter()
                .map(|edge| edge.evidence_ref.clone())
                .collect(),
            target: matches[0].name.clone(),
            proof_root: index_digest.clone(),
            verifier_contract_root: format!("graphzero-task-lens/{TASK_LENS_CONTRACT_VERSION}"),
            snapshot_root: index_digest.clone(),
            provenance_root: snapshot_root.clone(),
            valid_from_epoch: 0,
            valid_to_epoch: None,
        });

        let input = MechanicalRegionInput {
            truth: TruthClass::CompilerExact,
            fiber: FiberClass::Exact,
            gaps: Vec::new(),
            independently_verified: true,
            loci: vec![LocusRank {
                node: NodeId(ContentHash::of(matches[0].name.as_bytes())),
                score: 1,
                truth: TruthClass::CompilerExact,
                premises: vec![definition.evidence_ref.clone()],
            }],
            impact_closure: ClosureClass::Exact,
            obligations,
            supports,
            snapshot_root: index_digest.clone(),
            epoch: now_ms(),
        };
        let verdict = match classify_mechanical_region(&input) {
            MechanicalGraphVerdict::Safe => SafetyVerdict::Safe,
            // Gaps were handled above; a classifier disagreement is a
            // contract failure and degrades fail-closed, never a panic.
            MechanicalGraphVerdict::Unsafe => {
                return Ok(Self::lens_unknown(
                    request,
                    &index_digest,
                    Some(coverage),
                    "task lens classifier returned unsafe without decision gap evidence",
                ));
            }
            MechanicalGraphVerdict::Unknown => {
                return Ok(Self::lens_unknown(
                    request,
                    &index_digest,
                    Some(coverage),
                    "mechanical region classification did not certify the region as safe",
                ));
            }
        };
        let locus_source = source_handle(&self.cas, &self.repo_root, &locus_path);
        let result = TaskLensResult {
            verdict,
            locus: Some(StructuralHit {
                path: locus_path,
                symbol: Some(matches[0].name.clone()),
                line_start: None,
                line_end: None,
                preview: Some(format!(
                    "{} at {}",
                    matches[0].name, definition.evidence_ref
                )),
                evidence: Some(evidence),
                source: locus_source,
                score: 1.0,
            }),
            impact: TaskLensCompilerImpact {
                complete: true,
                edge_roots: vec![edge_root],
                reverse_roots: vec![reverse_root],
            },
            proof_support,
            evidence_roots,
            coverage: Some(coverage),
            index_digest,
            reasons: Vec::new(),
        };
        Ok(Self::lens_checked(request, result))
    }

    /// Normalize and validate a lens result against the request. A contract
    /// violation degrades to `Unknown` with a canonical reason — a corrupt
    /// lens result is never returned.
    fn lens_checked(request: &TaskLensRequest, result: TaskLensResult) -> TaskLensResult {
        let mut result = result.normalize();
        if let Err(error) = result.validate(request) {
            tracing::warn!(error = %error, "task lens contract violation; degrading to Unknown");
            result = Self::lens_unknown(
                request,
                &result.index_digest,
                result.coverage.clone(),
                format!("task lens contract violation: {error}"),
            );
        }
        result
    }

    /// Fail-closed `Unknown` result: no locus, no roots, one canonical reason.
    fn lens_unknown(
        request: &TaskLensRequest,
        index_digest: &str,
        coverage: Option<StructuralCoverage>,
        reason: impl Into<String>,
    ) -> TaskLensResult {
        let reasons = vec![reason.into()];
        let mut result = TaskLensResult {
            verdict: SafetyVerdict::Unknown {
                reasons: reasons.clone(),
            },
            locus: None,
            impact: TaskLensCompilerImpact {
                complete: false,
                edge_roots: Vec::new(),
                reverse_roots: Vec::new(),
            },
            proof_support: Vec::new(),
            evidence_roots: Vec::new(),
            coverage,
            index_digest: index_digest.to_string(),
            reasons,
        };
        result = result.normalize();
        debug_assert!(result.validate(request).is_ok());
        result
    }

    /// Non-lens modes: execute the existing query path first, then degrade
    /// to `Unknown` — AST or non-definition graph evidence can never be
    /// `Safe`.
    fn lens_degraded(
        &self,
        invocation: &EngineInvocation,
        request: &TaskLensRequest,
        reason: &str,
    ) -> Result<TaskLensResult, EngineError> {
        let structural = self.query(
            invocation,
            StructuralQuery {
                query: request.query.clone(),
                options: request.options.clone(),
            },
        )?;
        let reasons = vec![reason.to_string()];
        let mut result = TaskLensResult {
            verdict: SafetyVerdict::Unknown {
                reasons: reasons.clone(),
            },
            locus: None,
            impact: TaskLensCompilerImpact {
                complete: false,
                edge_roots: Vec::new(),
                reverse_roots: Vec::new(),
            },
            proof_support: Vec::new(),
            evidence_roots: Vec::new(),
            coverage: structural.coverage,
            index_digest: structural.index_digest,
            reasons,
        };
        result = result.normalize();
        if let Err(error) = result.validate(request) {
            tracing::warn!(error = %error, "task lens contract violation; degrading to Unknown");
            return Ok(Self::lens_unknown(
                request,
                &result.index_digest,
                result.coverage.clone(),
                format!("task lens contract violation: {error}"),
            ));
        }
        Ok(result)
    }

    /// Bounded reverse (caller) closure of one locus symbol over the live
    /// graph, walking the same reverse-index evidence the callers surface
    /// uses. Returns the closure edges and whether the walk stayed within
    /// the bound; `None` when the symbol is absent from the symbol table.
    fn lens_reverse_closure(
        &self,
        snapshot: &graphzero_store::Snapshot,
        symbol: &str,
        bound: usize,
    ) -> Result<Option<(Vec<LensClosureEdge>, bool)>, EngineError> {
        let view = snapshot.global_view().map_err(graph_error)?;
        let table = SymbolTable::from_view(&view).map_err(graph_error)?;
        let Some(target_id) = table.get(symbol) else {
            return Ok(None);
        };
        let csr = CsrAdjacency::new(view.edges().map_err(graph_error)?);
        let evidence = view.edge_evidence().map_err(graph_error)?;
        let blob_hashes = view.coverage().map_err(graph_error)?.blob_hashes;
        let reverse = snapshot.calls_reverse_index().map_err(graph_error)?;
        let mut edges = Vec::new();
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();
        visited.insert(target_id);
        queue.push_back(target_id);
        let mut bounded = true;
        while let Some(target) = queue.pop_front() {
            for &(src, edge_index) in reverse.callers(target) {
                if edges.len() >= bound {
                    bounded = false;
                    break;
                }
                let edge_index = edge_index as usize;
                let Some(edge) = csr
                    .edges(src)
                    .nth(edge_index.saturating_sub(csr.edge_base(src)))
                    .filter(|edge| edge.target == target && edge.kind == edge_kind::CALLS)
                else {
                    continue;
                };
                let span = evidence.get(edge_index).copied().unwrap_or_default();
                let hash_hex = graphzero_store::hex_blob_hash(blob_hashes, span.blob_idx)
                    .map_err(graph_error)?;
                let from = table.name(src).unwrap_or("").to_string();
                let to = table.name(target).unwrap_or("").to_string();
                edges.push(LensClosureEdge {
                    from,
                    to,
                    kind: "calls",
                    confidence: f64::from(edge.confidence) / 255.0,
                    evidence_ref: blob_span_ref(&hash_hex, span.start, span.end),
                });
                if visited.insert(src) {
                    queue.push_back(src);
                }
            }
            if !bounded {
                break;
            }
        }
        Ok(Some((edges, bounded)))
    }
}

impl StructuralEngine for ZeroStructuralEngine {
    fn query(
        &self,
        invocation: &EngineInvocation,
        query: StructuralQuery,
    ) -> Result<StructuralResult, EngineError> {
        Self::cancelled(invocation)?;
        if query.query.trim().is_empty() {
            return Err(EngineError::new(
                EngineErrorKind::InvalidInput,
                "z.find query must not be empty",
                false,
            ));
        }
        if matches!(query.options.mode, AsgrepMode::Natural)
            && let Some(result) = self.intent_anchor_query(invocation, &query)?
        {
            return Ok(result);
        }
        match query.options.mode {
            AsgrepMode::Natural
            | AsgrepMode::Pattern
            | AsgrepMode::Word
            | AsgrepMode::Literal
            | AsgrepMode::Regex
            | AsgrepMode::Imports
            | AsgrepMode::Semantic
            | AsgrepMode::Definition
            | AsgrepMode::Callers => self.ast_query(invocation, &query),
            _ => self.graph_query(invocation, &query),
        }
    }

    fn task_lens(
        &self,
        invocation: &EngineInvocation,
        request: TaskLensRequest,
    ) -> Result<TaskLensResult, EngineError> {
        Self::cancelled(invocation)?;
        if let Err(error) = request.validate() {
            return Ok(Self::lens_unknown(
                &request,
                "",
                None,
                format!("invalid task lens request: {error}"),
            ));
        }
        match request.options.mode {
            AsgrepMode::Definition | AsgrepMode::Symbols => self.lens_symbol(invocation, &request),
            AsgrepMode::Natural
            | AsgrepMode::Pattern
            | AsgrepMode::Word
            | AsgrepMode::Literal
            | AsgrepMode::Regex
            | AsgrepMode::Imports
            | AsgrepMode::Semantic => {
                self.lens_degraded(invocation, &request, "compiler_semantics_required")
            }
            _ => self.lens_degraded(
                invocation,
                &request,
                "task lens requires definition or symbols mode",
            ),
        }
    }
}

fn ast_structural_coverage(stats: &IndexStats) -> StructuralCoverage {
    let attempted = stats
        .files_indexed
        .saturating_add(stats.files_skipped)
        .saturating_add(stats.files_failed);
    let covered = attempted.saturating_sub(stats.files_failed);
    let tier_a_pct = if attempted == 0 {
        if stats.walk_errors { 0.0 } else { 100.0 }
    } else {
        covered as f64 * 100.0 / attempted as f64
    };
    StructuralCoverage {
        tier_a_pct,
        tier_b_pct: 0.0,
        tier_c_pct: 0.0,
        freshness_verified: !stats.walk_errors && stats.files_failed == 0,
        // The embedded AST index has no GraphZero snapshot generation.
        snapshot_id: 0,
    }
}

fn structural_coverage(response: &QuerySurfaceResponse) -> StructuralCoverage {
    StructuralCoverage {
        tier_a_pct: response.coverage.tier_a * 100.0,
        tier_b_pct: response.coverage.tier_b * 100.0,
        tier_c_pct: response.coverage.tier_c * 100.0,
        freshness_verified: response.coverage.freshness_verified,
        snapshot_id: response.coverage.snapshot_id,
    }
}

fn response_ledger(
    response: &QuerySurfaceResponse,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    response
        .capsule
        .as_ref()
        .and_then(|value| value.get("ledger"))
        .or_else(|| {
            response
                .absence_certificate
                .as_ref()
                .and_then(|value| value.get("ledger"))
        })
        .and_then(serde_json::Value::as_object)
}

fn ledger_u32(ledger: &serde_json::Map<String, serde_json::Value>, field: &str) -> Option<u32> {
    ledger
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn structural_budget(response: &QuerySurfaceResponse) -> Option<StructuralBudget> {
    let ledger = response_ledger(response)?;
    let requested = ledger_u32(ledger, "requested_budget")?;
    let actual_used =
        ledger_u32(ledger, "actual_used_budget").or_else(|| ledger_u32(ledger, "used_budget"))?;
    let used = ledger_u32(ledger, "used_budget")
        .unwrap_or(actual_used)
        .min(requested);
    Some(StructuralBudget {
        requested,
        used,
        actual_used,
        remaining: ledger_u32(ledger, "remaining_budget")
            .unwrap_or_else(|| requested.saturating_sub(used))
            .min(requested),
        exceeded: ledger
            .get("budget_exceeded")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(actual_used > requested),
        truncated: ledger
            .get("truncated")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

fn structural_absence(
    response: &QuerySurfaceResponse,
    coverage: &StructuralCoverage,
) -> StructuralAbsence {
    let class = if !coverage.freshness_verified {
        "stale_index"
    } else if coverage.tier_a_pct < 99.0 {
        "low_coverage"
    } else if response.absence_certificate.is_some() {
        "verified_empty"
    } else {
        "unknown"
    };
    let certificate_reason = response
        .absence_certificate
        .as_ref()
        .and_then(|certificate| {
            certificate
                .get("reason")
                .or_else(|| certificate.get("code"))
                .or_else(|| certificate.get("class"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    let reason = response
        .error
        .clone()
        .or(certificate_reason)
        .unwrap_or_else(|| match class {
            "stale_index" => "the GraphZero snapshot is stale".into(),
            "low_coverage" => format!(
                "Tier A coverage is {:.2}%; absence is not certified",
                coverage.tier_a_pct
            ),
            "verified_empty" => "fresh covered index certifies no matching structure".into(),
            _ => "no structural hits and no complete absence certificate".into(),
        });
    let suggestion = match class {
        "stale_index" => "retry once; GraphZero will rebuild the stale snapshot",
        "low_coverage" => "narrow the path or use literal/word mode while coverage is repaired",
        "verified_empty" => "change the query or inspect a broader scope",
        _ => "retry with literal, word, or definition mode",
    };
    StructuralAbsence {
        class: class.into(),
        reason,
        coverage: Some(coverage.clone()),
        suggestion: suggestion.into(),
    }
}

/// Explicit semantic decision-gap evidence carried by the graph response.
///
/// Only marked gaps count: `decision_gaps` entries in the capsule/absence
/// ledger, or an error prefixed `SEMANTIC_DECISION_GAP:`. Absence
/// certificates and ordinary index errors are not decision gaps.
fn lens_decision_gaps(response: &QuerySurfaceResponse) -> Vec<DecisionGap> {
    let mut gaps = Vec::new();
    for payload in [
        response.capsule.as_ref(),
        response.absence_certificate.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(entries) = payload
            .pointer("/ledger/decision_gaps")
            .and_then(serde_json::Value::as_array)
        {
            for entry in entries {
                let kind = entry.get("kind").and_then(serde_json::Value::as_str);
                let reason = entry.get("reason").and_then(serde_json::Value::as_str);
                if let (Some(kind), Some(reason)) = (kind, reason) {
                    gaps.push(DecisionGap {
                        kind: EvidenceKind::UnresolvedGap,
                        reason: format!("{kind}: {reason}"),
                    });
                }
            }
        }
    }
    if let Some(error) = response.error.as_deref()
        && let Some(reason) = error.strip_prefix("SEMANTIC_DECISION_GAP:")
    {
        gaps.push(DecisionGap {
            kind: EvidenceKind::UnresolvedGap,
            reason: reason.trim().to_string(),
        });
    }
    gaps
}

/// The blob hash from a `gz://blob/<hex>#B<start>-<end>` span ref.
fn blob_hash_from_evidence_ref(evidence_ref: &str) -> Option<String> {
    let hex = evidence_ref.strip_prefix("gz://blob/")?.split('#').next()?;
    (hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| hex.to_string())
}

fn source_handle(cas: &ZeroCas, repo_root: &Path, path: &Path) -> Option<ZeroHandle> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    let canonical = std::fs::canonicalize(candidate).ok()?;
    if !canonical.starts_with(repo_root) || !canonical.is_file() {
        return None;
    }
    let bytes = std::fs::read(canonical).ok()?;
    cas.put(&bytes).ok()
}

fn path_from_label(label: &str) -> PathBuf {
    let candidate = label.split(':').next().unwrap_or(label);
    if candidate.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(candidate)
    }
}

fn finite_score(score: f64) -> f64 {
    if score.is_finite() { score } else { 0.0 }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn ast_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::new(
        EngineErrorKind::Internal,
        format!("ast-sgrep: {error}"),
        true,
    )
}

fn ast_stage_error(stage: &str, error: impl std::fmt::Display) -> EngineError {
    EngineError::new(
        EngineErrorKind::Internal,
        format!("ast-sgrep {stage}: {error}"),
        true,
    )
}

fn graph_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::new(
        EngineErrorKind::Corrupt,
        format!("GraphZero: {error}"),
        true,
    )
}

fn cas_error(error: impl std::fmt::Display) -> EngineError {
    EngineError::new(
        EngineErrorKind::Corrupt,
        format!("ZeroHandle CAS: {error}"),
        false,
    )
}

#[cfg(test)]
mod exact_scope_tests {
    use super::ZeroStructuralEngine;
    use std::path::Path;

    #[test]
    fn exact_file_scope_indexes_only_its_parent() {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(
            repo.join("src/lib.rs"),
            "pub struct Needle;
",
        )
        .unwrap();
        let engine =
            ZeroStructuralEngine::open(&repo, temp.path().join("graph"), temp.path().join("zero"))
                .unwrap();

        let (index_root, prefix, file) = engine.ast_scope(Some("src/lib.rs")).unwrap();
        assert_eq!(index_root, std::fs::canonicalize(repo.join("src")).unwrap());
        assert_eq!(prefix, "src");
        assert_eq!(file.as_deref(), Some("lib.rs"));

        let (index_root, prefix, file) = engine.ast_scope(Some("src")).unwrap();
        assert_eq!(index_root, std::fs::canonicalize(repo.join("src")).unwrap());
        assert_eq!(prefix, "src");
        assert_eq!(file, None);
        assert_eq!(engine.ast_scope(Some("src/*.rs")), None);
        assert_eq!(engine.ast_scope(Some("missing.rs")), None);
        assert!(Path::new(&prefix).is_relative());
    }
}

#[allow(dead_code)]
fn _path_inside(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}
