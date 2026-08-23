use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};

use parking_lot::Mutex;

use fszero_kernel::ZeroFileEngine;
use serde_json::{Value, json};
use tempfile::tempdir;
use tokenzero_kernel::ZeroTokenEngine;
use zero_abi::{
    AsgrepOptions, CompressionRequest, CompressionResult, EngineError, EngineErrorKind,
    EngineInvocation, FileEffectKind, FileEffectReceipt, FileEffectRequest, FileEngine, FileLease,
    FileReadRequest, FileSnapshot, KernelBudget, KernelContext, LookupOptions, ProjectionRequest,
    ProjectionResult, ReadOptions, ShellOptions, StructuralEngine, StructuralHit, StructuralQuery,
    StructuralResult, TokenAccounting, TokenEngine, ZeroHandle,
};
use zero_kernel::{AtomicCancellation, HostError, ShellCommand, TransactionError, ZeroKernel};

fn handle(bytes: &[u8]) -> ZeroHandle {
    ZeroHandle::from_digest(blake3::hash(bytes).to_hex().as_str()).unwrap()
}

struct MockLease;
impl FileLease for MockLease {}

#[derive(Default)]
struct Files(Mutex<BTreeMap<PathBuf, Vec<u8>>>);

impl FileEngine for Files {
    fn lease(&self, _invocation: &EngineInvocation) -> Result<Box<dyn FileLease>, EngineError> {
        Ok(Box::new(MockLease))
    }

    fn read(
        &self,
        _invocation: &EngineInvocation,
        request: FileReadRequest,
    ) -> Result<FileSnapshot, EngineError> {
        let files = self.0.lock();
        let bytes = files
            .get(&request.path)
            .ok_or_else(|| EngineError::new(EngineErrorKind::NotFound, "missing", false))?;
        // Files named "outline-only.*" simulate oversized sources whose bytes
        // exceed the inline envelope: engine returns outline + digest only.
        let (inline_utf8, outline) = if request
            .path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("outline-only."))
        {
            (None, Some("L7: pub struct OutlineOnly;\n".to_string()))
        } else {
            (Some(String::from_utf8(bytes.clone()).unwrap()), None)
        };
        Ok(FileSnapshot {
            path: request.path,
            content: handle(bytes),
            byte_len: bytes.len() as u64,
            modified_unix_ns: 0,
            mode: 0,
            inline_utf8,
            outline,
        })
    }

    fn lookup(
        &self,
        _invocation: &EngineInvocation,
        _root: PathBuf,
        _options: LookupOptions,
    ) -> Result<Vec<PathBuf>, EngineError> {
        Ok(self.0.lock().keys().cloned().collect())
    }

    fn apply(
        &self,
        _invocation: &EngineInvocation,
        request: FileEffectRequest,
    ) -> Result<FileEffectReceipt, EngineError> {
        let mut files = self.0.lock();
        let before = files.get(&request.path).map(|bytes| handle(bytes));
        let after = match request.kind {
            FileEffectKind::Remove => {
                // Mirror real engines: removing an absent path is a NotFound.
                if files.remove(&request.path).is_none() {
                    return Err(EngineError::new(
                        EngineErrorKind::NotFound,
                        "remove target does not exist",
                        false,
                    ));
                }
                None
            }
            _ => {
                let bytes = request.content.unwrap();
                let handle = handle(&bytes);
                files.insert(request.path.clone(), bytes);
                Some(handle)
            }
        };
        Ok(FileEffectReceipt {
            kind: request.kind,
            path: request.path,
            before,
            after,
            before_metadata: None,
            journal: handle(b"journal"),
        })
    }

    fn restore(
        &self,
        _invocation: &EngineInvocation,
        receipt: &FileEffectReceipt,
    ) -> Result<(), EngineError> {
        let mut files = self.0.lock();
        match receipt.before.as_ref() {
            Some(_) => {
                // Tests only exercise rollback of newly created files.
                return Err(EngineError::new(
                    EngineErrorKind::Unsupported,
                    "mock cannot resolve prior bytes",
                    false,
                ));
            }
            None => {
                files.remove(&receipt.path);
            }
        }
        Ok(())
    }

    fn reconcile(&self, _invocation: &EngineInvocation) -> Result<Vec<ZeroHandle>, EngineError> {
        Ok(Vec::new())
    }
}

struct SlowFiles {
    inner: Files,
    barrier: Barrier,
    inflight: AtomicUsize,
    peak: AtomicUsize,
}

impl SlowFiles {
    fn new() -> Self {
        Self {
            inner: Files::default(),
            barrier: Barrier::new(2),
            inflight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    fn observe_peak(&self, value: usize) {
        let mut seen = self.peak.load(Ordering::Relaxed);
        while value > seen {
            match self
                .peak
                .compare_exchange_weak(seen, value, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(actual) => seen = actual,
            }
        }
    }
}

impl FileEngine for SlowFiles {
    fn lease(&self, invocation: &EngineInvocation) -> Result<Box<dyn FileLease>, EngineError> {
        self.inner.lease(invocation)
    }

    fn read(
        &self,
        invocation: &EngineInvocation,
        request: FileReadRequest,
    ) -> Result<FileSnapshot, EngineError> {
        let active = self.inflight.fetch_add(1, Ordering::AcqRel) + 1;
        self.observe_peak(active);
        self.barrier.wait();
        std::thread::sleep(std::time::Duration::from_millis(25));
        let result = self.inner.read(invocation, request);
        self.inflight.fetch_sub(1, Ordering::AcqRel);
        result
    }

    fn lookup(
        &self,
        invocation: &EngineInvocation,
        root: PathBuf,
        options: LookupOptions,
    ) -> Result<Vec<PathBuf>, EngineError> {
        self.inner.lookup(invocation, root, options)
    }

    fn apply(
        &self,
        invocation: &EngineInvocation,
        request: FileEffectRequest,
    ) -> Result<FileEffectReceipt, EngineError> {
        self.inner.apply(invocation, request)
    }

    fn restore(
        &self,
        invocation: &EngineInvocation,
        receipt: &FileEffectReceipt,
    ) -> Result<(), EngineError> {
        self.inner.restore(invocation, receipt)
    }

    fn reconcile(&self, invocation: &EngineInvocation) -> Result<Vec<ZeroHandle>, EngineError> {
        self.inner.reconcile(invocation)
    }
}

struct Graph;
impl StructuralEngine for Graph {
    fn query(
        &self,
        _invocation: &EngineInvocation,
        query: StructuralQuery,
    ) -> Result<StructuralResult, EngineError> {
        let hit = StructuralHit {
            path: PathBuf::from("src/lib.rs"),
            symbol: Some(query.query.clone()),
            line_start: Some(1),
            line_end: Some(1),
            preview: Some("hit".into()),
            evidence: None,
            source: Some(handle(b"pub fn alpha() {}\n")),
            score: 1.0,
        };
        let hits = if query.query.contains("ambiguous") {
            vec![hit.clone(), hit]
        } else {
            vec![hit]
        };
        Ok(StructuralResult {
            hits,
            index_digest: "index".into(),
            complete: true,
            coverage: None,
            absence: None,
            budget: None,
            diagnostic: None,
            continuation: None,
        })
    }
}

struct Tokens;
impl TokenEngine for Tokens {
    fn measure(
        &self,
        _invocation: &EngineInvocation,
        bytes: &[u8],
    ) -> Result<TokenAccounting, EngineError> {
        Ok(TokenAccounting {
            tokenizer: "bytes".into(),
            billed: bytes.len() as u64,
            visible: bytes.len() as u64,
            cached: 0,
            certified: true,
        })
    }

    fn certify(
        &self,
        _invocation: &EngineInvocation,
        bytes: &[u8],
        claimed: &TokenAccounting,
    ) -> Result<zero_abi::CertifyResult, EngineError> {
        let recomputed = TokenAccounting {
            tokenizer: "bytes".into(),
            billed: bytes.len() as u64,
            visible: bytes.len() as u64,
            cached: 0,
            certified: true,
        };
        Ok(zero_abi::CertifyResult {
            matches: recomputed == *claimed,
            recomputed,
        })
    }

    fn project(
        &self,
        invocation: &EngineInvocation,
        request: ProjectionRequest,
    ) -> Result<ProjectionResult, EngineError> {
        if request
            .bytes
            .windows(b"force projection failure".len())
            .any(|window| window == b"force projection failure")
        {
            return Err(EngineError::new(
                EngineErrorKind::Budget,
                "forced projection failure",
                false,
            ));
        }
        let accounting = self.measure(invocation, &request.bytes)?;
        let visible_source_bytes = request.bytes.len() as u64;
        Ok(ProjectionResult {
            visible: String::from_utf8(request.bytes).unwrap(),
            visible_source_bytes,
            exact: None,
            accounting,
        })
    }

    fn compress(
        &self,
        invocation: &EngineInvocation,
        request: CompressionRequest,
    ) -> Result<CompressionResult, EngineError> {
        let accounting = self.measure(invocation, &request.bytes)?;
        Ok(CompressionResult {
            visible: String::from_utf8(request.bytes.clone()).unwrap(),
            exact: handle(&request.bytes),
            truncated: false,
            omitted_tokens: 0,
            accounting,
        })
    }

    fn expand(
        &self,
        _invocation: &EngineInvocation,
        _handle: &ZeroHandle,
        _options: zero_abi::ExpandOptions,
    ) -> Result<Vec<u8>, EngineError> {
        Err(EngineError::new(
            EngineErrorKind::NotFound,
            "missing",
            false,
        ))
    }
}

fn kernel(root: &std::path::Path, files: Arc<dyn FileEngine>) -> ZeroKernel {
    ZeroKernel::new(
        KernelContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "session".into(),
            expected_state_root: None,
            contract_digest: "contract".into(),
        },
        KernelBudget {
            wall_ms: 1_000,
            cpu_ms: 1_000,
            memory_bytes: 64 * 1024 * 1024,
            call_limit: 64,
            task_limit: 8,
            output_byte_limit: 64 * 1024,
        },
        files,
        Arc::new(Graph),
        Arc::new(Tokens),
        root.join(".zerostack"),
    )
    .unwrap()
}

fn production_kernel(root: &std::path::Path) -> ZeroKernel {
    let store_root = root.join(".zerostack");
    let files = Arc::new(
        ZeroFileEngine::open(root, &store_root, "contract").expect("open production file engine"),
    );
    let tokens = Arc::new(ZeroTokenEngine::open(&store_root, None));
    ZeroKernel::new(
        KernelContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "session".into(),
            expected_state_root: None,
            contract_digest: "contract".into(),
        },
        KernelBudget {
            wall_ms: 1_000,
            cpu_ms: 1_000,
            memory_bytes: 64 * 1024 * 1024,
            call_limit: 64,
            task_limit: 8,
            output_byte_limit: 64 * 1024,
        },
        files,
        Arc::new(Graph),
        tokens,
        store_root,
    )
    .unwrap()
}
/// production_kernel with a relaxed wall budget: full-suite parallel runs
/// contend for CPU and can exceed the default 1s deadline mid-cell.
fn production_kernel_relaxed(root: &std::path::Path) -> ZeroKernel {
    let store_root = root.join(".zerostack");
    let files = Arc::new(
        ZeroFileEngine::open(root, &store_root, "contract").expect("open production file engine"),
    );
    ZeroKernel::new(
        KernelContext {
            workspace_root: root.to_path_buf(),
            project_root: root.to_path_buf(),
            session_id: "session".into(),
            expected_state_root: None,
            contract_digest: "contract".into(),
        },
        KernelBudget {
            wall_ms: 20_000,
            cpu_ms: 1_000,
            memory_bytes: 64 * 1024 * 1024,
            call_limit: 64,
            task_limit: 8,
            output_byte_limit: 64 * 1024,
        },
        files,
        Arc::new(Graph),
        Arc::new(Tokens),
        root.join(".zerostack"),
    )
    .unwrap()
}

fn model_json(response: &zero_abi::ZeroKernelResponse) -> Value {
    let value = response.value.as_ref().expect("model-visible value");
    match value {
        Value::String(text) => serde_json::from_str(text).expect("model-visible JSON"),
        value => value.clone(),
    }
}

fn write_fixture(root: &std::path::Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("fixture parent")).unwrap();
    fs::write(path, content).unwrap();
}

#[test]
fn direct_methods_and_state_finalize_through_event_log() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files
        .0
        .lock()
        .insert(PathBuf::from("src/lib.rs"), b"content".to_vec());
    let kernel = kernel(root.path(), files);
    let mut cell = kernel.begin_cell("return z.read('src/lib.rs')").unwrap();
    assert_eq!(
        cell.read("src/lib.rs", Default::default()).unwrap(),
        "content"
    );
    assert_eq!(
        cell.asgrep(
            "symbol",
            AsgrepOptions {
                mode: zero_abi::AsgrepMode::Symbols,
                path: None,
                language: None,
                source: None,
                sink: None,
                limit: Some(10),
                budget_tokens: None,
            }
        )
        .unwrap()
        .hits
        .len(),
        1
    );
    cell.state_set("answer", json!(42));
    let response = cell.finish(json!({"ok": true})).unwrap();
    assert_eq!(response.protocol, "ZeroKernel");
    assert!(!response.state.unchanged);
    assert_eq!(kernel.live_frames(), 0);

    let cell = kernel.begin_cell("return z.state.get('answer')").unwrap();
    assert_eq!(cell.state_get("answer"), Some(&json!(42)));
    drop(cell);
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn read_labels_nul_content_as_non_text() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files
        .0
        .lock()
        .insert(PathBuf::from("binary.dat"), b"prefix\0suffix".to_vec());
    let kernel = kernel(root.path(), files);
    let mut cell = kernel.begin_cell("return z.read('binary.dat')").unwrap();
    let visible = cell.read("binary.dat", Default::default()).unwrap();
    assert!(visible.contains("READ OUTLINE"), "{visible:?}");
    assert!(visible.contains("exact=z://blob/"), "{visible:?}");
    assert!(
        !visible.contains('\0'),
        "NUL bytes must not reach model text"
    );
}

#[test]
fn transaction_rolls_back_created_file() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let mut cell = kernel.begin_cell("transaction").unwrap();
    cell.begin_transaction().unwrap();
    cell.write("new.txt", b"new".to_vec(), None).unwrap();
    cell.rollback_transaction().unwrap();
    assert!(!files.0.lock().contains_key(&PathBuf::from("new.txt")));
    drop(cell);
    assert_eq!(kernel.live_frames(), 0);
}

#[cfg(unix)]
#[test]
fn outline_read_is_explicitly_labeled() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let mut cell = kernel.begin_cell("outline-read").unwrap();
    files.0.lock().insert(
        PathBuf::from("crates/outline-only.rs"),
        b"pub struct OutlineOnly;\n".to_vec(),
    );
    let text = cell
        .read("crates/outline-only.rs", ReadOptions::default())
        .unwrap();
    assert!(
        text.starts_with("[ZeroStack READ OUTLINE - not file content"),
        "outline must carry the unambiguous header: {text}"
    );
    assert!(text.contains("path=crates/outline-only.rs"), "{text}");
    assert!(text.contains("exact="), "{text}");
    drop(cell);
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn remove_missing_path_keeps_transaction_alive() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let mut cell = kernel.begin_cell("transaction-remove").unwrap();
    cell.begin_transaction().unwrap();
    cell.write("kept.txt", b"kept".to_vec(), None).unwrap();
    let error = cell
        .remove("missing.txt", None)
        .expect_err("remove of missing path must fail");
    let not_found = match &error {
        HostError::Engine(engine_error) => engine_error.kind == EngineErrorKind::NotFound,
        HostError::Transaction(transaction_error) => matches!(
            transaction_error,
            TransactionError::Engine(engine_error)
                if engine_error.kind == EngineErrorKind::NotFound
        ),
        _ => false,
    };
    assert!(not_found, "expected engine NotFound, got {error:?}");
    // The earlier write must still commit; one failed effect cannot poison
    // the whole transaction (pc_2ed8bb7745f4).
    cell.commit_transaction().unwrap();
    let files = files.0.lock();
    assert_eq!(
        files.get(&PathBuf::from("kept.txt")).map(Vec::as_slice),
        Some(&b"kept"[..])
    );
    drop(cell);
    assert_eq!(kernel.live_frames(), 0);
}

#[cfg(unix)]
#[test]
fn shell_argv_is_call_scoped_and_reaped() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), files);
    let response = kernel
        .execute_cell(r#"return await z.shell(["printf", "hello"]);"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let visible = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(visible.contains("hello"), "{visible}");
    assert_eq!(kernel.live_processes(), 0);
    assert_eq!(kernel.live_frames(), 0);
}

#[cfg(unix)]
#[test]
fn shell_large_output_returns_bounded_head_tail_previews() {
    let root = tempdir().unwrap();
    let kernel = production_kernel(root.path());
    let mut cell = kernel.begin_cell("large shell output").unwrap();
    let result = cell
        .shell(
            ShellCommand::Script(
                "i=0; while [ $i -lt 5000 ]; do printf 'stdout-%04d\\n' $i; printf 'stderr-%04d\\n' $i >&2; i=$((i+1)); done"
                    .into(),
            ),
            ShellOptions {
                max_visible_bytes: Some(4096),
                ..ShellOptions::default()
            },
        )
        .unwrap();
    assert!(result.stdout.len() <= 2048, "{}", result.stdout.len());
    assert!(result.stderr.len() <= 2048, "{}", result.stderr.len());
    assert!(result.stdout.starts_with("stdout-0000"));
    assert!(result.stdout.ends_with("stdout-4999\n"));
    assert!(result.stderr.starts_with("stderr-0000"));
    assert!(result.stderr.ends_with("stderr-4999\n"));
    assert!(result.stdout.contains("... output omitted ..."));
    assert!(result.stderr.contains("... output omitted ..."));
    assert!(result.exact.is_some());
    drop(cell);
    assert_eq!(kernel.live_processes(), 0);
    assert_eq!(kernel.live_frames(), 0);
}

#[cfg(unix)]
#[test]
fn shell_timeout_kills_exact_tree() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), files);
    let mut cell = kernel
        .begin_cell("return await z.shell('sleep 5')")
        .unwrap();
    let error = cell
        .shell(
            ShellCommand::Script("sleep 5".into()),
            ShellOptions {
                timeout_ms: Some(50),
                ..ShellOptions::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("deadline"), "{error}");
    assert_eq!(kernel.live_processes(), 0);
    drop(cell);
}

#[test]
fn typescript_cell_uses_only_direct_z_methods() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files
        .0
        .lock()
        .insert(PathBuf::from("src/lib.rs"), b"content".to_vec());
    let kernel = kernel(root.path(), files);
    let response = kernel
        .execute_cell(
            r#"
            const path: string = "src/lib.rs";
            const text = await z.read(path);
            const help = await z.help();
            return { text, help };
            "#,
        )
        .unwrap();
    let visible = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(visible.contains("content"), "{visible}");
    assert!(visible.contains("read"), "{visible}");
    assert!(visible.contains("apply_atomic"), "{visible}");
    assert!(visible.contains("find_callers"), "{visible}");
    assert!(visible.contains("compatibilityAliases"), "{visible}");
    assert!(!visible.contains("invoke"), "{visible}");
    assert!(!visible.contains("zero.fs"), "{visible}");
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn direct_parallel_and_pipeline_accept_thunks() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files.0.lock().insert(PathBuf::from("a.txt"), b"a".to_vec());
    files.0.lock().insert(PathBuf::from("b.txt"), b"b".to_vec());
    let kernel = kernel(root.path(), files);
    let response = kernel
        .execute_cell(
            r#"
            const pair = await z.parallel([
              () => z.read("a.txt"),
              () => z.read("b.txt"),
            ]);
            const upper = await z.pipeline(pair, async value => value + "!");
            return upper;
            "#,
        )
        .unwrap();
    let visible = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(visible.contains("a!"), "{visible}");
    assert!(visible.contains("b!"), "{visible}");
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn parallel_preserves_destructured_callback_captures() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files.0.lock().insert(PathBuf::from("a.txt"), b"a".to_vec());
    files.0.lock().insert(PathBuf::from("b.txt"), b"b".to_vec());
    let kernel = kernel(root.path(), files);
    let response = kernel
        .execute_cell(
            r#"
            const cases = [["left", "a.txt"], ["right", "b.txt"]];
            return await z.parallel(cases.map(([name, path]) => async () => {
              const text = await z.read(path);
              return name + ":" + text;
            }));
            "#,
        )
        .unwrap();
    assert_eq!(response.operations.len(), 2);
    assert!(!response.operations_truncated);
    assert!(response.operations.iter().all(|operation| {
        operation.method == "read"
            && operation.status == zero_abi::ZeroOperationStatus::Completed
            && operation.parallel_group == Some(1)
            && operation.duration_ns > 0
    }));
    let visible = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(visible.contains("left:a"), "{visible}");
    assert!(visible.contains("right:b"), "{visible}");
    assert_eq!(kernel.live_tasks(), 0);
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn failed_transaction_restores_created_file() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let response = kernel
        .execute_cell(
            r#"
            return await z.transact(async () => {
              await z.write("created.txt", "new");
              throw new Error("stop");
            });
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(!files.0.lock().contains_key(&PathBuf::from("created.txt")));
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn parallel_reads_overlap_in_real_time() {
    let root = tempdir().unwrap();
    let files = Arc::new(SlowFiles::new());
    files
        .inner
        .0
        .lock()
        .insert(PathBuf::from("a.txt"), b"a".to_vec());
    files
        .inner
        .0
        .lock()
        .insert(PathBuf::from("b.txt"), b"b".to_vec());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let response = kernel
        .execute_cell(
            r#"
            return await z.parallel([
              () => z.read("a.txt"),
              () => z.read("b.txt"),
            ]);
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert!(files.peak.load(Ordering::Acquire) >= 2);
    assert_eq!(kernel.live_tasks(), 0);
}

#[test]
fn direct_token_methods_are_bound_on_z() {
    let root = tempdir().unwrap();
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let response = kernel
        .execute_cell(
            r#"
            return await z.parallel([
              () => z.measure("alpha beta"),
              () => z.project("alpha beta", {visibleBytes: 128}),
              () => z.compress("alpha beta", {maxTokens: 32, mode: "structured"}),
            ]);
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let visible = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(visible.contains("tokenizer"), "{visible}");
    assert!(visible.contains("visible"), "{visible}");
    assert!(visible.contains("exact"), "{visible}");
    assert_eq!(kernel.live_tasks(), 0);
}

#[test]
fn cancelled_frame_returns_cancelled_terminal() {
    let root = tempdir().unwrap();
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let cancellation = AtomicCancellation::new();
    cancellation.cancel();
    let response = kernel
        .execute_cell_with_cancellation("return 1;", cancellation)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Cancelled);
    assert_eq!(
        response.error.as_ref().map(|error| &error.kind),
        Some(&EngineErrorKind::Cancelled)
    );
    assert!(response.state.unchanged);
    assert_eq!(kernel.live_frames(), 0);
    assert_eq!(kernel.live_tasks(), 0);
}

#[test]
fn failed_cell_rolls_back_unscoped_write_and_logs_once() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let response = kernel
        .execute_cell(
            r#"
            await z.write("unscoped.txt", "temporary");
            throw new Error("stop after write");
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(!files.0.lock().contains_key(&PathBuf::from("unscoped.txt")));
    let records = zero_store::EventLog::open(root.path().join(".zerostack"))
        .records("session")
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event, response.event);
    let visible = response.error.as_ref().unwrap().detail.as_bytes();
    assert_eq!(
        records[0].model_visible_digest,
        blake3::hash(visible).to_hex().to_string()
    );
    assert_eq!(kernel.live_frames(), 0);
    assert_eq!(kernel.live_tasks(), 0);
}

#[test]
fn projection_failure_rolls_back_file_and_state() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let response = kernel
        .execute_cell(
            r#"
            await z.write("projection.txt", "temporary");
            z.state.set("staged", true);
            return "force projection failure";
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        response.error.as_ref().map(|error| &error.kind),
        Some(&EngineErrorKind::Budget)
    );
    assert!(
        !files
            .0
            .lock()
            .contains_key(&PathBuf::from("projection.txt"))
    );
    assert!(response.state.unchanged);
    let state = kernel
        .execute_cell("return z.state.has('staged');")
        .unwrap();
    assert_eq!(state.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(state.value, Some(json!("false")));
}

#[test]
fn fresh_kernel_continues_transaction_cell_sequence() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let first = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let response = first
        .execute_cell(r#"return await z.write("first.txt", "first");"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    drop(first);

    let second = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let response = second
        .execute_cell(r#"return await z.write("second.txt", "second");"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let files = files.0.lock();
    assert_eq!(files.get(&PathBuf::from("first.txt")).unwrap(), b"first");
    assert_eq!(files.get(&PathBuf::from("second.txt")).unwrap(), b"second");
}

#[cfg(unix)]
#[test]
fn cancelled_shell_frame_drains_before_response() {
    let root = tempdir().unwrap();
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let cancellation = AtomicCancellation::new();
    let trigger = cancellation.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        trigger.cancel();
    });
    let response = kernel
        .execute_cell_with_cancellation("return await z.shell('sleep 5');", cancellation)
        .unwrap();
    canceller.join().unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Cancelled);
    assert_eq!(kernel.live_frames(), 0);
    assert_eq!(kernel.live_tasks(), 0);
    assert_eq!(kernel.live_processes(), 0);
}

#[test]
fn snap_full_view_expands_exact_source_and_is_listed() {
    let root = tempdir().unwrap();
    let source = "pub fn alpha() -> u32 {\n    42\n}\n";
    write_fixture(root.path(), "src/lib.rs", source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const help = await z.help();
            const snap = await z.snap({
              target: {path: "src/lib.rs"},
              view: {mode: "full"},
            });
            const expanded = await z.expand(snap);
            return {methods: help.methods, aliases: help.compatibilityAliases, snap, expanded};
            "#,
        )
        .unwrap();

    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "error={:?}",
        response.error
    );
    let value = model_json(&response);
    assert_eq!(
        value["methods"],
        json!(["read", "find", "edit", "apply", "run", "state"])
    );
    let aliases = value["aliases"]
        .as_array()
        .expect("compatibilityAliases array");
    assert!(aliases.iter().any(|method| method == "snap"));
    assert_eq!(value["snap"]["schema"], "zerostack.snap.workspace");
    assert_eq!(value["snap"]["path"], "src/lib.rs");
    assert_eq!(value["snap"]["view"]["mode"], "full");
    assert_eq!(value["snap"]["view"]["text"], source);
    assert_eq!(value["snap"]["view"]["fullFileVisible"], true);
    assert_eq!(value["snap"]["accounting"]["omittedTokens"], 0);
    assert_eq!(value["snap"]["accounting"]["savedTokensNow"], 0);
    assert_eq!(
        value["snap"]["source"]["contentDigest"],
        handle(source.as_bytes()).digest()
    );
    assert_eq!(value["snap"]["source"]["newline"], "lf");
    assert_eq!(value["snap"]["source"]["bom"], false);
    assert!(value["snap"]["source"]["modifiedUnixNs"].is_string());
    assert_eq!(value["snap"]["view"]["visibleRanges"][0]["byteStart"], 0);
    assert_eq!(
        value["snap"]["view"]["visibleRanges"][0]["byteEnd"],
        source.len()
    );
    assert_eq!(value["snap"]["recovery"]["retained"], false);
    assert_eq!(
        value["snap"]["recovery"]["retentionPolicy"],
        "call_output_handles"
    );
    assert_eq!(
        value["snap"]["source"]["exact"],
        value["snap"]["recovery"]["exact"]
    );
    assert_eq!(value["snap"]["recovery"]["complete"], true);
    assert_ne!(
        value["snap"]["recovery"]["manifest"],
        value["snap"]["snapshot"]
    );
    assert!(
        value["snap"]["recovery"]["manifest"]
            .as_str()
            .unwrap()
            .starts_with("z://blob/")
    );
    assert_eq!(value["snap"]["recovery"]["unrecoverableBytes"], 0);
    assert_eq!(value["expanded"]["schema"], "zerostack.expand");
    assert_eq!(value["expanded"]["text"], source);
    assert_eq!(value["expanded"]["complete"], true);
    let records = zero_store::EventLog::open(root.path().join(".zerostack"))
        .records("session")
        .unwrap();
    let record = records.last().unwrap();
    assert_eq!(record.event, response.event);
    assert_eq!(
        record.model_visible_digest,
        blake3::hash(
            response
                .value
                .as_ref()
                .unwrap()
                .as_str()
                .unwrap()
                .as_bytes()
        )
        .to_hex()
        .to_string()
    );
    assert!(value["expanded"]["accounting"]["billed"].as_u64().unwrap() > 0);
    assert_eq!(
        value["expanded"]["accounting"]["billed"],
        value["expanded"]["accounting"]["visible"]
    );
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn snap_decision_view_retains_large_exact_source() {
    let root = tempdir().unwrap();
    let source = "0123456789abcdef\n".repeat(2_500);
    write_fixture(root.path(), "large.txt", &source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(r#"return await z.snap("large.txt");"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let snap = model_json(&response);
    assert_eq!(snap["source"]["byteLength"], source.len());
    assert_eq!(snap["view"]["fullFileVisible"], false);
    assert!(snap["view"]["omittedBytes"].as_u64().unwrap() > 0);
    assert!(snap["view"]["omittedBytes"].as_u64().unwrap() < source.len() as u64);
    assert!(snap["accounting"]["omittedTokens"].as_u64().unwrap() > 0);
    assert_eq!(
        snap["accounting"]["savedTokensNow"],
        snap["accounting"]["omittedTokens"]
    );
    assert_eq!(snap["recovery"]["complete"], true);
    assert_eq!(snap["recovery"]["recoverableBytes"], source.len());
    assert_eq!(snap["recovery"]["unrecoverableBytes"], 0);

    let exact = serde_json::to_string(snap["source"]["exact"].as_str().unwrap()).unwrap();
    let response = kernel
        .execute_cell(&format!("return await z.expand({exact});"))
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let expanded = model_json(&response);
    assert_eq!(expanded["text"], source);
    assert_eq!(expanded["complete"], true);
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn snap_search_uses_structural_engine_and_exact_file_snapshot() {
    let root = tempdir().unwrap();
    let source = "pub fn alpha() {}\n";
    write_fixture(root.path(), "src/lib.rs", source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.snap({
              target: {
                search: {
                  query: "alpha",
                  under: "src",
                  language: "rust",
                  mode: "natural",
                },
              },
              cardinality: "exactly_one",
              view: {mode: "structure"},
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let snap = model_json(&response);
    assert_eq!(snap["path"], "src/lib.rs");
    assert_eq!(snap["selection"]["kind"], "lines");
    assert_eq!(snap["structural"]["indexDigest"], "index");
    assert_eq!(snap["structural"]["complete"], true);
    assert_eq!(snap["structural"]["source"], snap["source"]["exact"]);
    assert_eq!(snap["recovery"]["complete"], true);
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn snap_and_edit_commit_exactly_once_in_one_cell() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "edit.ts", "const before = 1;\n");
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap({
              target: {path: "edit.ts"},
              selection: {exactText: "before"},
            });
            const receipt = await z.edit(snap, {find: "before", replace: "after"});
            const after = await z.read("edit.ts");
            return {receipt, after};
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let value = model_json(&response);
    assert_eq!(value["after"], "const after = 1;\n");
    assert!(
        value["receipt"]["before"]
            .as_str()
            .unwrap()
            .starts_with("z://blob/")
    );
    assert!(
        value["receipt"]["after"]
            .as_str()
            .unwrap()
            .starts_with("z://blob/")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("edit.ts")).unwrap(),
        "const after = 1;\n"
    );
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn snap_aware_edit_rejects_stale_preimage_without_mutation() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "stale.ts", "const before = 1;\n");
    let kernel = production_kernel(root.path());
    let response = kernel
        .execute_cell(r#"return await z.snap("stale.ts");"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let snap = model_json(&response);

    let concurrent = "// concurrent\nconst before = 1;\n";
    fs::write(root.path().join("stale.ts"), concurrent).unwrap();
    let snap = serde_json::to_string(&snap).unwrap();
    let response = kernel
        .execute_cell(&format!(
            "const snap = {snap}; return await z.edit(snap, {{find: \"before\", replace: \"after\"}});"
        ))
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(response.error.as_ref().unwrap().detail.contains("preimage"));
    assert_eq!(
        fs::read_to_string(root.path().join("stale.ts")).unwrap(),
        concurrent
    );
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn failed_cell_restores_snap_aware_edit() {
    let root = tempdir().unwrap();
    let original = "const before = 1;\n";
    write_fixture(root.path(), "rollback.ts", original);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap("rollback.ts");
            await z.edit(snap, {find: "before", replace: "after"});
            throw new Error("stop after snap-aware edit");
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        fs::read_to_string(root.path().join("rollback.ts")).unwrap(),
        original,
        "error={:?}",
        response.error
    );
    assert_eq!(kernel.live_frames(), 0);
    assert_eq!(kernel.live_tasks(), 0);
    assert_eq!(kernel.live_processes(), 0);
}

#[test]
fn snap_shorthand_preserves_selection_and_view() {
    let root = tempdir().unwrap();
    let source = "pub fn alpha() {}\n";
    write_fixture(root.path(), "src/lib.rs", source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.snap({
              path: "src/lib.rs",
              selection: {exactText: "alpha"},
              view: {mode: "full"},
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let snap = model_json(&response);
    assert_eq!(snap["selection"]["kind"], "exact_text");
    assert_eq!(snap["view"]["mode"], "full");
    assert_eq!(snap["view"]["text"], source);
}

#[test]
fn expand_accepts_next_as_its_byte_cursor() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "cursor.txt", "abcdefghij");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap("cursor.txt");
            return await z.expand(snap, {next: 2, limit: 3});
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let expanded = model_json(&response);
    assert_eq!(expanded["text"], "cde");
    assert_eq!(expanded["byteStart"], 2);
    assert_eq!(expanded["byteEnd"], 5);
    assert_eq!(expanded["next"], 5);
    assert_eq!(expanded["complete"], false);
}

#[test]
fn snap_symbol_selection_uses_in_process_structural_search() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/lib.rs", "pub fn alpha() {}\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.snap({
              path: "src/lib.rs",
              selection: {symbol: "alpha"},
              view: {mode: "structure"},
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let snap = model_json(&response);
    assert_eq!(snap["selection"]["kind"], "symbol");
    assert_eq!(snap["selection"]["lineStart"], 1);
    assert_eq!(snap["structural"]["complete"], true);
}

#[test]
fn snap_aware_edit_rejects_patch_outside_selection() {
    let root = tempdir().unwrap();
    let original = "const before = 1; const elsewhere = 2;\n";
    write_fixture(root.path(), "scope.ts", original);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap({
              path: "scope.ts",
              selection: {exactText: "before"},
            });
            return await z.edit(snap, {find: "elsewhere", replace: "changed"});
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        response.error.as_ref().map(|error| &error.kind),
        Some(&EngineErrorKind::Conflict)
    );
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .detail
            .contains("selection_scope_mismatch")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("scope.ts")).unwrap(),
        original
    );
}

#[test]
fn effect_creates_file_and_updates_module_index_atomically() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/mod.rs", "mod old;\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.effect({
              targets: {
                newModule: {path: "src/new.rs", expect: "absent"},
                moduleIndex: {path: "src/mod.rs", expect: "exists"},
              },
              changes: [
                {
                  target: "newModule",
                  kind: "create_file",
                  content: "pub fn created() {}\n",
                },
                {
                  target: "moduleIndex",
                  kind: "insert_after",
                  anchor: {exactText: "mod old;"},
                  content: "mod new;",
                },
              ],
              verify: {changedTargetsOnly: true},
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let effect = model_json(&response);
    assert_eq!(effect["schema"], "zerostack.effect");
    assert_eq!(effect["outcome"], "staged");
    assert_eq!(effect["changedFiles"], 2);
    assert!(effect["delta"].as_str().unwrap().starts_with("z://blob/"));
    let delta = serde_json::to_string(effect["delta"].as_str().unwrap()).unwrap();
    let expanded = kernel
        .execute_cell(&format!("return await z.expand({delta});"))
        .unwrap();
    assert_eq!(
        model_json(&expanded)["text"]
            .as_str()
            .map(|text| text.contains("newModule")),
        Some(true)
    );
    assert_eq!(effect["verification"]["changedTargetsOnly"], true);
    let targets = effect["targets"].as_array().unwrap();
    let new_module = targets
        .iter()
        .find(|target| target["name"] == "newModule")
        .unwrap();
    let new_after = serde_json::to_string(new_module["after"].as_str().unwrap()).unwrap();
    let expanded = kernel
        .execute_cell(&format!("return await z.expand({new_after});"))
        .unwrap();
    assert_eq!(model_json(&expanded)["text"], "pub fn created() {}\n");
    let module_index = targets
        .iter()
        .find(|target| target["name"] == "moduleIndex")
        .unwrap();
    let index_before = serde_json::to_string(module_index["before"].as_str().unwrap()).unwrap();
    let expanded = kernel
        .execute_cell(&format!("return await z.expand({index_before});"))
        .unwrap();
    assert_eq!(model_json(&expanded)["text"], "mod old;\n");
    assert_eq!(
        fs::read_to_string(root.path().join("src/new.rs")).unwrap(),
        "pub fn created() {}\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("src/mod.rs")).unwrap(),
        "mod old;\nmod new;\n"
    );
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn effect_rejects_unconfined_verification_command_without_mutation() {
    let root = tempdir().unwrap();
    let first = "const first = 1;\n";
    let second = "const second = 2;\n";
    write_fixture(root.path(), "first.ts", first);
    write_fixture(root.path(), "second.ts", second);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.effect({
              targets: {
                first: {path: "first.ts"},
                second: {path: "second.ts"},
              },
              changes: [
                {
                  target: "first",
                  kind: "replace_exact",
                  old: "first",
                  replacement: "changedFirst",
                  expectedCount: 1,
                },
                {
                  target: "second",
                  kind: "replace_exact",
                  old: "second",
                  replacement: "changedSecond",
                  expectedCount: 1,
                },
              ],
              verify: {
                changedTargetsOnly: true,
                command: {argv: ["sh", "-c", "exit 7"], timeoutMs: 1000},
              },
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .detail
            .contains("verification_unavailable")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("first.ts")).unwrap(),
        first
    );
    assert_eq!(
        fs::read_to_string(root.path().join("second.ts")).unwrap(),
        second
    );
    assert_eq!(kernel.live_frames(), 0);
    assert_eq!(kernel.live_tasks(), 0);
    assert_eq!(kernel.live_processes(), 0);
}

#[test]
fn snap_rejects_structural_evidence_for_another_source() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/lib.rs", "pub fn beta() {}\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.snap({
              target: {search: {query: "alpha", under: "src", mode: "natural"}},
              cardinality: "exactly_one",
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        response.error.as_ref().map(|error| &error.kind),
        Some(&EngineErrorKind::Conflict)
    );
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .detail
            .contains("stale structural source")
    );
}

#[test]
fn expand_accepts_nested_byte_line_and_all_selectors() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "selectors.txt", "alpha\nbeta\ngamma\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap("selectors.txt");
            const bytes = await z.expand(snap, {bytes: {start: 6, end: 10}});
            const lines = await z.expand(snap, {lines: {start: 2, end: 2}});
            const all = await z.expand(snap, {all: true});
            return {bytes, lines, all};
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let expanded = model_json(&response);
    assert_eq!(expanded["bytes"]["text"], "beta");
    assert_eq!(expanded["bytes"]["byteStart"], 6);
    assert_eq!(expanded["bytes"]["byteEnd"], 10);
    assert_eq!(expanded["bytes"]["exactDigest"], handle(b"beta").digest());
    assert!(expanded["bytes"]["recoveredTokens"].as_u64().unwrap() > 0);
    assert_eq!(expanded["lines"]["text"], "beta\n");
    assert_eq!(expanded["all"]["text"], "alpha\nbeta\ngamma\n");
    assert_eq!(expanded["all"]["complete"], true);
}

#[test]
fn snap_binary_file_returns_exact_recovery_without_text_claims() {
    let root = tempdir().unwrap();
    let binary = [0, 0xff, b'\r', b'\n', b'\n'];
    fs::write(root.path().join("binary.bin"), binary).unwrap();
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap("binary.bin");
            const expanded = await z.expand(snap, {all: true});
            return {snap, expanded};
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let value = model_json(&response);
    let snap = &value["snap"];
    assert_eq!(snap["source"]["encoding"], "binary");
    assert_eq!(snap["source"]["newline"], "none");
    assert_eq!(snap["source"]["byteLength"], 5);
    assert!(snap["view"].get("text").is_none());
    assert_eq!(snap["view"]["visibleRanges"], json!([]));
    assert_eq!(snap["recovery"]["recoverableBytes"], 5);
    assert_eq!(snap["recovery"]["unrecoverableBytes"], 0);
    assert_eq!(snap["recovery"]["exact"], snap["source"]["exact"]);
    assert_eq!(value["expanded"]["encoding"], "hex");
    assert_eq!(value["expanded"]["bytes"], "00ff0d0a0a");
    assert!(value["expanded"].get("text").is_none());
    assert_eq!(
        value["expanded"]["exactDigest"],
        blake3::hash(&binary).to_hex().to_string()
    );
}

#[test]
fn snap_reports_bom_and_mixed_newlines() {
    let root = tempdir().unwrap();
    let source = "\u{feff}alpha\r\nbeta\n";
    write_fixture(root.path(), "mixed.txt", source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(r#"return await z.snap({target:{path:"mixed.txt"},view:{mode:"full"}});"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let snap = model_json(&response);
    assert_eq!(snap["source"]["bom"], true);
    assert_eq!(snap["source"]["newline"], "mixed");
    assert_eq!(snap["view"]["text"], source);
}

#[test]
fn snap_edit_supports_line_replacement_and_anchored_insertion() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "typed.txt", "alpha\nbeta\ngamma\n");
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const line = await z.snap({
              path: "typed.txt",
              selection: {lines: {start: 2, end: 2}},
            });
            await z.edit(line, {kind: "replace_lines", content: "BETA\n"});
            const anchor = await z.snap({
              path: "typed.txt",
              selection: {exactText: "gamma"},
            });
            await z.edit(anchor, {kind: "insert_before", content: "before-"});
            return await z.read("typed.txt");
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(model_json(&response), "alpha\nBETA\nbefore-gamma\n");
    assert_eq!(
        fs::read_to_string(root.path().join("typed.txt")).unwrap(),
        "alpha\nBETA\nbefore-gamma\n"
    );
}

#[test]
fn snap_edit_replace_file_requires_unselected_snap() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "whole.txt", "old\n");
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap("whole.txt");
            await z.edit(snap, {kind: "replace_file", content: "new\n"});
            return await z.read("whole.txt");
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(model_json(&response), "new\n");
}

#[test]
fn snap_edit_replace_exact_requires_explicit_single_match() {
    let root = tempdir().unwrap();
    let original = "const alpha = 1;\n";
    write_fixture(root.path(), "count.ts", original);
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap({
              path: "count.ts",
              selection: {exactText: "alpha"},
            });
            return await z.edit(snap, {
              kind: "replace_exact",
              old: "alpha",
              replacement: "beta",
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .detail
            .contains("expectedCount: 1")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("count.ts")).unwrap(),
        original
    );
}

#[test]
fn effect_existing_absence_target_rolls_back_related_edit() {
    let root = tempdir().unwrap();
    let index = "mod old;\n";
    write_fixture(root.path(), "src/mod.rs", index);
    write_fixture(root.path(), "src/new.rs", "already here\n");
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.effect({
              targets: {
                moduleIndex: {path: "src/mod.rs", expect: "exists"},
                newModule: {path: "src/new.rs", expect: "absent"},
              },
              changes: [
                {
                  target: "moduleIndex",
                  kind: "insert_after",
                  anchor: {exactText: "mod old;"},
                  content: "\nmod new;",
                },
                {
                  target: "newModule",
                  kind: "create_file",
                  content: "pub fn created() {}\n",
                },
              ],
              verify: {changedTargetsOnly: true},
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        fs::read_to_string(root.path().join("src/mod.rs")).unwrap(),
        index
    );
    assert_eq!(
        fs::read_to_string(root.path().join("src/new.rs")).unwrap(),
        "already here\n"
    );
}

#[cfg(unix)]
#[test]
fn effect_refuses_symlink_substituted_parent() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.effect({
              targets: {target: {path: "escape/new.rs", expect: "absent"}},
              changes: [{
                target: "target",
                kind: "create_file",
                content: "pub fn escaped() {}\n",
              }],
              verify: {changedTargetsOnly: true},
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(!outside.path().join("new.rs").exists());
}

#[test]
fn full_snap_fails_typed_instead_of_truncating() {
    let root = tempdir().unwrap();
    let source = "0123456789abcdef\n".repeat(2_500);
    write_fixture(root.path(), "large-full.txt", &source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"return await z.snap({target:{path:"large-full.txt"},view:{mode:"full"}});"#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        response.error.as_ref().map(|error| &error.kind),
        Some(&EngineErrorKind::Budget)
    );
    let detail = &response.error.as_ref().unwrap().detail;
    assert!(detail.contains("full_view_unavailable"), "{detail}");
    assert!(detail.contains("exact=z://blob/"), "{detail}");
    assert!(detail.contains("recovery=z://blob/"), "{detail}");
}

#[test]
fn snap_search_refuses_ambiguous_exact_target() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/lib.rs", "pub fn alpha() {}\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.snap({
              target: {search: {query: "ambiguous", under: "src", mode: "natural"}},
              cardinality: "exactly_one",
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        response.error.as_ref().map(|error| &error.kind),
        Some(&EngineErrorKind::Conflict)
    );
    let detail = &response.error.as_ref().unwrap().detail;
    assert!(detail.contains("ambiguous"), "{detail}");
    assert!(detail.contains("src/lib.rs:1-1"), "{detail}");
}

#[test]
fn effect_rejects_replace_file_after_prior_change() {
    let root = tempdir().unwrap();
    let original = "const alpha = 1;\n";
    write_fixture(root.path(), "overwrite.ts", original);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.effect({
              targets: {target: {path: "overwrite.ts"}},
              changes: [
                {
                  target: "target",
                  kind: "replace_exact",
                  old: "alpha",
                  replacement: "beta",
                  expectedCount: 1,
                },
                {
                  target: "target",
                  kind: "replace_file",
                  content: "silently replaced\n",
                },
              ],
              verify: {changedTargetsOnly: true},
            });
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(
        response
            .error
            .as_ref()
            .unwrap()
            .detail
            .contains("replace_file must be the target's only change")
    );
    assert_eq!(
        fs::read_to_string(root.path().join("overwrite.ts")).unwrap(),
        original
    );
}

#[test]
fn repeated_expand_cursors_reconstruct_exact_source() {
    let root = tempdir().unwrap();
    let source = "0123456789".repeat(25);
    write_fixture(root.path(), "paged.txt", &source);
    let kernel = production_kernel(root.path());
    let response = kernel
        .execute_cell(r#"return await z.snap("paged.txt");"#)
        .unwrap();
    let snap = model_json(&response);
    let exact = snap["source"]["exact"].as_str().unwrap();
    let mut next = 0_u64;
    let mut reconstructed = String::new();
    loop {
        let response = kernel
            .execute_cell(&format!(
                "return await z.expand({:?}, {{next: {}, limit: 17}});",
                exact, next
            ))
            .unwrap();
        assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
        let page = model_json(&response);
        reconstructed.push_str(page["text"].as_str().unwrap());
        let page_digest = blake3::hash(page["text"].as_str().unwrap().as_bytes())
            .to_hex()
            .to_string();
        assert_eq!(page["exactDigest"], page_digest);
        match page["next"].as_u64() {
            Some(cursor) => next = cursor,
            None => break,
        }
    }
    assert_eq!(reconstructed, source);
    assert_eq!(
        blake3::hash(reconstructed.as_bytes()).to_hex().to_string(),
        handle(source.as_bytes()).digest()
    );
}

#[test]
fn canonical_kernel_binds_graph_hits_without_repository_indexes() {
    let root = tempdir().unwrap();
    let source = "pub fn alpha() {}\n";
    write_fixture(root.path(), "src/lib.rs", source);
    let store = root.path().join(".state");
    let kernel = ZeroKernel::canonical(
        root.path(),
        &store,
        "canonical-graph",
        KernelBudget {
            wall_ms: 30_000,
            cpu_ms: 30_000,
            memory_bytes: 256 * 1024 * 1024,
            call_limit: 64,
            task_limit: 4,
            output_byte_limit: 64 * 1024,
        },
    )
    .unwrap();

    let response = kernel
        .execute_cell(
            r#"
            const ast = await z.parallel([
              async () => await z.asgrep("alpha", {
                mode: "natural",
                path: "src/lib.rs",
                language: "rust",
                limit: 2,
              }),
              async () => await z.asgrep("alpha", {
                mode: "natural",
                path: "src/lib.rs",
                language: "rust",
                limit: 2,
              }),
            ]);
            const hits = ast[0];
            const graph = await z.parallel([
              async () => await z.asgrep("alpha", {
                mode: "symbols",
                path: "src/lib.rs",
                language: "rust",
                limit: 2,
              }),
              async () => await z.asgrep("alpha", {
                mode: "symbols",
                path: "src/lib.rs",
                language: "rust",
                limit: 2,
              }),
            ]);
            const snap = await z.snap({
              target: {search: {query: "alpha", under: "src/lib.rs", mode: "natural"}},
              cardinality: "exactly_one",
            });
            const symbol = await z.snap({
              target: {path: "src/lib.rs"},
              cardinality: "exactly_one",
              selection: {symbol: "alpha"},
            });
            return {hits, snap, symbol, graph, astCount: ast.length};
            "#,
        )
        .unwrap();

    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "error={:?}",
        response.error
    );
    let value = model_json(&response);
    assert_eq!(value["graph"].as_array().map(Vec::len), Some(2));
    assert_eq!(value["astCount"], 2);
    assert_eq!(value["hits"]["hits"][0]["path"], "src/lib.rs");
    assert_eq!(
        value["hits"]["hits"][0]["source"],
        value["snap"]["source"]["exact"]
    );
    assert_eq!(
        value["snap"]["structural"]["source"],
        value["snap"]["source"]["exact"]
    );
    assert_eq!(value["symbol"]["selection"]["kind"], "symbol");
    assert_eq!(value["symbol"]["selection"]["lineStart"], 1);
    assert_eq!(value["symbol"]["selection"]["lineEnd"], 1);
    assert_eq!(
        value["symbol"]["structural"]["source"],
        value["symbol"]["source"]["exact"]
    );
    assert!(!root.path().join(".asgrep").exists());
    assert!(!root.path().join("src/.asgrep").exists());
    assert!(store.join("graph/ast-sgrep/index.db").is_file());
}

#[test]
fn optional_connector_properties_follow_javascript_undefined_semantics() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("binary.bin"), [0, 0xff]).unwrap();
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.snap("binary.bin");
            const expanded = await z.expand(snap, {all: true});
            return {
              snapTextMissing: snap.view.text === undefined,
              expandedTextMissing: expanded.text === undefined,
              expandedBytes: expanded.bytes,
            };
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let value = model_json(&response);
    assert_eq!(value["snapTextMissing"], true);
    assert_eq!(value["expandedTextMissing"], true);
    assert_eq!(value["expandedBytes"], "00ff");
}

#[test]
fn edit_path_rejects_whole_file_replacement_string() {
    let root = tempdir().unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            r#"
            const p = 'guard-edit.txt';
            await z.write(p, 'v1\nv2\nv3\n');
            let caught = null;
            try { await z.edit(p, 'WHOLE-FILE-REPLACEMENT'); }
            catch (e) { caught = String(e.message || e); }
            const after = await z.read(p);
            return [caught, after];
        "#,
        )
        .unwrap();
    if response.outcome != zero_abi::ZeroKernelOutcome::Completed || response.value.is_none() {
        panic!("response: {response:?}");
    }
    // Cell returns serialize to a JSON-encoded string in the response value.
    let raw = response.value.unwrap();
    let raw = raw
        .as_str()
        .expect("cell return must serialize to a string");
    let pair: Value = serde_json::from_str(raw).unwrap();
    let caught = pair
        .get(0)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    assert!(
        caught.contains("refuses a bare replacement string"),
        "{caught}"
    );
    assert_eq!(
        pair.get(1).and_then(Value::as_str),
        Some("v1\nv2\nv3\n"),
        "file must be untouched"
    );
}

#[test]
fn edit_path_find_replacement_substitutes_first_unique_match() {
    let root = tempdir().unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            r#"
            const p = 'guard-edit-sub.txt';
            await z.write(p, 'v1\nv2\nv3\n');
            await z.edit(p, { find: 'v2', replacement: 'V2-DONE' });
            return await z.read(p);
        "#,
        )
        .unwrap();
    if response.outcome != zero_abi::ZeroKernelOutcome::Completed {
        panic!("cell failed under load: {response:?}");
    }
    let returned = response.value.unwrap();
    assert_eq!(
        returned.as_str(),
        // execute_cell returns the TokenZero projection of the cell value:
        // a plain string return arrives JSON-quoted.
        Some("\"v1\\nV2-DONE\\nv3\\n\""),
        "substituted content must round-trip"
    );
}

#[test]
fn edit_path_mismatch_reports_context_and_recovery() {
    let root = tempdir().unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            r#"
            const p = 'guard-edit-mismatch.txt';
            await z.write(p, 'alpha\nbeta\ngamma\n');
            return await z.edit(p, { find: 'delta', replacement: 'DELTA' });
        "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    let detail = &response.error.as_ref().unwrap().detail;
    assert!(detail.contains("mismatch=not_found"), "{detail}");
    assert!(detail.contains("alpha\\nbeta\\ngamma"), "{detail}");
    assert!(detail.contains("re-read with z.read(path)"), "{detail}");
}

#[test]
fn edit_path_replace_file_kind_replaces_deliberately() {
    let root = tempdir().unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            r#"
            const p = 'guard-edit-rf.txt';
            await z.write(p, 'old content\n');
            await z.edit(p, { kind: 'replace_file', content: 'fresh\n' });
            return await z.read(p);
        "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let returned = response.value.unwrap();
    assert_eq!(
        returned.as_str(),
        Some("\"fresh\\n\""),
        "deliberate replace_file content must round-trip"
    );
}

#[test]
fn snap_directory_error_guides_to_lookup() {
    let root = tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("crates")).unwrap();
    let kernel = production_kernel(root.path());
    let response = kernel
        .execute_cell(
            r#"
            try { await z.snap({ path: 'crates' }); return 'NO-ERROR'; }
            catch (e) { return String(e.message || e); }
        "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let message = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(
        message.contains("is a directory") && message.contains("z.lookup"),
        "{message}"
    );
}

#[test]
fn asgrep_accepts_single_object_form() {
    let root = tempdir().unwrap();
    std::fs::write(
        root.path().join("probe.rs"),
        "pub struct AsgrepProbeMarker;\n",
    )
    .unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), files);
    let object_form = kernel
        .execute_cell(
            r#"
            const r = await z.asgrep({ query: "AsgrepProbeMarker", mode: "natural" });
            return r.hits.length;
        "#,
        )
        .unwrap();
    assert_eq!(object_form.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let positional = kernel
        .execute_cell(
            r#"
            const r = await z.asgrep("AsgrepProbeMarker", { mode: "natural" });
            return r.hits.length;
        "#,
        )
        .unwrap();
    assert_eq!(positional.outcome, zero_abi::ZeroKernelOutcome::Completed);
}

#[test]
fn state_namespace_call_carries_guidance() {
    let root = tempdir().unwrap();
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let response = kernel
        .execute_cell(
            r#"
            try { z.state(); return 'NO-ERROR'; }
            catch (e) { return String(e.message || e); }
        "#,
        )
        .unwrap();
    let message = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(message.contains("z.state.get"), "{message}");
}

#[test]
fn shell_output_accounting_is_truthful_against_visible_bytes() {
    // RACC truthfulness at the shell boundary: reported visible tokens must
    // equal a fresh measurement over the exact stdout bytes the model saw.
    let root = tempdir().unwrap();
    // The byte-faithful token engine makes the truthfulness property exact.
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), files);
    let payload = "SHELL_OK accounting probe";
    let response = kernel
        .execute_cell(&format!("return await z.shell([\"printf\", {payload:?}]);"))
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let raw = response.value.unwrap();
    let raw = raw.as_str().expect("shell result serializes to a string");
    let shell: Value = serde_json::from_str(raw).unwrap();
    let stdout = shell["stdout"].as_str().unwrap();
    assert_eq!(stdout, payload);
    let accounting = &shell["accounting"];
    let visible = accounting["visible"].as_u64().unwrap();
    let billed = accounting["billed"].as_u64().unwrap();
    // Byte-faithful estimator: visible tokens must track the exact stdout
    // byte length the model received (production_kernel counts bytes).
    assert_eq!(visible as usize, stdout.len(), "visible must match bytes");
    assert_eq!(billed as usize, stdout.len(), "billed must match bytes");
}

#[test]
fn final_surface_read_lists_directories() {
    let root = tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/a.rs"), "a").unwrap();
    std::fs::write(root.path().join("src/b.rs"), "b").unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel.execute_cell("return await z.read('src');").unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let raw = response.value.unwrap();
    let listing: Value = serde_json::from_str(raw.as_str().unwrap()).unwrap();
    let entries = listing.as_array().unwrap();
    assert_eq!(entries.len(), 2, "directory read must list both entries");
    assert!(
        entries
            .iter()
            .any(|v| v.as_str().unwrap().ends_with("a.rs"))
    );
}

#[test]
fn final_surface_read_expands_exact_handles_to_text() {
    let root = tempdir().unwrap();
    std::fs::write(root.path().join("blob.txt"), "handle payload\n").unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            "const snap = await z.snap('blob.txt'); return await z.read(snap.source.exact);",
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(
        response.value.unwrap().as_str(),
        Some("\"handle payload\\n\"")
    );
}

#[test]
fn final_surface_edit_creates_and_removes() {
    let root = tempdir().unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let created = kernel
        .execute_cell("return await z.edit('created.txt', {create:'payload'});")
        .unwrap();
    assert_eq!(created.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(
        std::fs::read_to_string(root.path().join("created.txt")).unwrap(),
        "payload"
    );

    let duplicate = kernel
        .execute_cell("return await z.edit('created.txt', {create:'overwrite'});")
        .unwrap();
    assert_eq!(duplicate.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        std::fs::read_to_string(root.path().join("created.txt")).unwrap(),
        "payload"
    );

    let removed = kernel
        .execute_cell("return await z.edit('created.txt', {remove:true});")
        .unwrap();
    assert_eq!(removed.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert!(!root.path().join("created.txt").exists());
}

#[test]
fn caught_create_conflict_keeps_cell_committable() {
    let root = tempdir().unwrap();
    std::fs::write(root.path().join("existing.txt"), "original").unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            r#"
            try {
                await z.edit('existing.txt', {create:'overwrite'});
                return 'MISSED';
            } catch (error) {
                return String(error.message || error);
            }
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(
        std::fs::read_to_string(root.path().join("existing.txt")).unwrap(),
        "original"
    );
}

#[test]
fn final_surface_apply_is_atomic_and_flat() {
    let root = tempdir().unwrap();
    std::fs::write(root.path().join("a.txt"), "old").unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            r#"return await z.apply([
                {path:'a.txt', edit:{find:'old', replacement:'new'}},
                {path:'b.txt', create:'created'}
            ]);"#,
        )
        .unwrap();
    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "{response:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
        "new"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("b.txt")).unwrap(),
        "created"
    );
}

#[test]
fn final_surface_apply_sequences_multiple_edits_on_one_file() {
    let root = tempdir().unwrap();
    std::fs::write(root.path().join("same.txt"), "alpha beta gamma").unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            r#"return await z.apply([
                {path:'same.txt', edit:{find:'alpha', replacement:'ALPHA'}},
                {path:'same.txt', edit:{find:'gamma', replacement:'GAMMA'}}
            ]);"#,
        )
        .unwrap();
    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "{response:?}"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("same.txt")).unwrap(),
        "ALPHA beta GAMMA"
    );

    let rejected = kernel
        .execute_cell(
            "return await z.apply([{path:'same.txt', edit:{find:'ALPHA', replacement:'alpha'}, remove:true}]);",
        )
        .unwrap();
    assert_eq!(rejected.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(
        rejected
            .error
            .as_ref()
            .unwrap()
            .detail
            .contains("exactly one action")
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("same.txt")).unwrap(),
        "ALPHA beta GAMMA"
    );
}

#[test]
fn final_surface_find_and_run_aliases_work() {
    let root = tempdir().unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let find = kernel
        .execute_cell(
            "const out = [(await z.find({query:'alpha'})).hits.length]; for (const mode of ['natural','word','literal','regex','imports','defs']) { const r = await z.find({query:'alpha', mode}); out.push(r.hits.length); } return out;",
        )
        .unwrap();
    assert_eq!(
        find.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "{find:?}"
    );

    let run = kernel
        .execute_cell("const r = await z.run(['printf','RUN_OK']); return r.stdout;")
        .unwrap();
    assert_eq!(
        run.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "{run:?}"
    );
    assert_eq!(run.value.unwrap().as_str(), Some("\"RUN_OK\""));
}

#[test]
fn final_surface_help_teaches_only_six_operations() {
    let root = tempdir().unwrap();
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let response = kernel.execute_cell("return await z.help();").unwrap();
    let raw = response.value.unwrap();
    let help: Value = serde_json::from_str(raw.as_str().unwrap()).unwrap();
    assert_eq!(
        help["methods"],
        json!(["read", "find", "edit", "apply", "run", "state"])
    );
    assert!(
        help["examples"]["apply_atomic"]
            .as_str()
            .unwrap()
            .contains("z.apply")
    );
}
