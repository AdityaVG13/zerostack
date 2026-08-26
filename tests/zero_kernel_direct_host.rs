use std::collections::{BTreeMap, BTreeSet};
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
    AsgrepOptions, CapsulePublication, CompressionRequest, CompressionResult, EngineError,
    EngineErrorKind, EngineInvocation, FileEffectKind, FileEffectReceipt, FileEffectRequest,
    FileEngine, FileLease, FileReadRequest, FileSnapshot, KernelBudget, KernelContext,
    LookupOptions, ProjectionRequest, ProjectionResult, ReadOptions, SafetyVerdict, ShellOptions,
    SpeculationBinding, StructuralCoverage, StructuralEngine, StructuralHit, StructuralQuery,
    StructuralResult, TaskLensCompilerImpact, TaskLensRequest, TaskLensResult, TokenAccounting,
    TokenEngine, ZeroHandle, ZeroKernelEvent, ZeroOperationStatus, ZeroOperationTrace, sha256_hex,
};
use zero_kernel::{
    AtomicCancellation, CellPreparation, HostError, PreparedCell, ShellCommand, TransactionError,
    ZeroKernel,
};

fn handle(bytes: &[u8]) -> ZeroHandle {
    ZeroHandle::from_digest(blake3::hash(bytes).to_hex().as_str()).unwrap()
}

struct MockLease;
impl FileLease for MockLease {}

/// In-memory file engine whose capsule store is content-addressed by the
/// canonical capsule root: `put_capsule` and `get_capsule` serve capsules
/// exactly in memory.
#[derive(Default)]
struct Files(
    Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    Mutex<BTreeMap<String, zero_abi::WorkCapsule>>,
);

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

    fn put_capsule(
        &self,
        _invocation: &EngineInvocation,
        capsule: &zero_abi::WorkCapsule,
    ) -> Result<zero_abi::CapsulePublication, EngineError> {
        let capsule_root = capsule
            .root()
            .map_err(|detail| EngineError::new(EngineErrorKind::InvalidInput, detail, false))?;
        let object_digest = capsule_object_digest(capsule);
        self.1.lock().insert(object_digest.clone(), capsule.clone());
        let object = ZeroHandle::from_digest(&object_digest).map_err(|error| {
            EngineError::new(EngineErrorKind::InvalidInput, error.to_string(), false)
        })?;
        Ok(zero_abi::CapsulePublication {
            capsule_root,
            object,
            created: true,
        })
    }

    fn get_capsule(
        &self,
        _invocation: &EngineInvocation,
        publication: &zero_abi::CapsulePublication,
    ) -> Result<zero_abi::WorkCapsule, EngineError> {
        // Recovery is object-addressed, exactly like the real capsule store:
        // a valid root with a wrong object handle must not recover anything.
        let capsule = self
            .1
            .lock()
            .get(publication.object.digest())
            .cloned()
            .ok_or_else(|| {
                EngineError::new(
                    EngineErrorKind::NotFound,
                    "capsule object is not published",
                    false,
                )
            })?;
        let capsule_root = capsule
            .root()
            .map_err(|detail| EngineError::new(EngineErrorKind::InvalidInput, detail, false))?;
        if capsule_root != publication.capsule_root {
            return Err(EngineError::new(
                EngineErrorKind::Corrupt,
                "capsule root does not match its publication",
                false,
            ));
        }
        Ok(capsule)
    }
}

/// Distinct deterministic object digest over the canonical capsule envelope
/// bytes: sha256 of the canonical JSON of a typed envelope wrapping the
/// capsule. Never equal to the capsule root itself.
fn capsule_object_digest(capsule: &zero_abi::WorkCapsule) -> String {
    let value = serde_json::to_value(capsule).expect("capsule JSON");
    let envelope = serde_json::json!({
        "schema": "zerostack.capsule.object.v1",
        "capsule": value,
    });
    sha256_hex(zero_abi::canonical_json(&envelope).as_bytes())
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

    fn put_capsule(
        &self,
        invocation: &EngineInvocation,
        capsule: &zero_abi::WorkCapsule,
    ) -> Result<zero_abi::CapsulePublication, EngineError> {
        self.inner.put_capsule(invocation, capsule)
    }

    fn get_capsule(
        &self,
        invocation: &EngineInvocation,
        publication: &zero_abi::CapsulePublication,
    ) -> Result<zero_abi::WorkCapsule, EngineError> {
        self.inner.get_capsule(invocation, publication)
    }
}

/// FileEngine whose capsule publication always fails: every cell launch must
/// fail closed before any guest work or event exists.
struct NoCapsuleFiles(Files);

impl FileEngine for NoCapsuleFiles {
    fn lease(&self, invocation: &EngineInvocation) -> Result<Box<dyn FileLease>, EngineError> {
        self.0.lease(invocation)
    }

    fn read(
        &self,
        invocation: &EngineInvocation,
        request: FileReadRequest,
    ) -> Result<FileSnapshot, EngineError> {
        self.0.read(invocation, request)
    }

    fn lookup(
        &self,
        invocation: &EngineInvocation,
        root: PathBuf,
        options: LookupOptions,
    ) -> Result<Vec<PathBuf>, EngineError> {
        self.0.lookup(invocation, root, options)
    }

    fn apply(
        &self,
        invocation: &EngineInvocation,
        request: FileEffectRequest,
    ) -> Result<FileEffectReceipt, EngineError> {
        self.0.apply(invocation, request)
    }

    fn restore(
        &self,
        invocation: &EngineInvocation,
        receipt: &FileEffectReceipt,
    ) -> Result<(), EngineError> {
        self.0.restore(invocation, receipt)
    }

    fn reconcile(&self, invocation: &EngineInvocation) -> Result<Vec<ZeroHandle>, EngineError> {
        self.0.reconcile(invocation)
    }

    fn put_capsule(
        &self,
        _invocation: &EngineInvocation,
        _capsule: &zero_abi::WorkCapsule,
    ) -> Result<zero_abi::CapsulePublication, EngineError> {
        Err(EngineError::new(
            EngineErrorKind::Unsupported,
            "capsule publication is disabled",
            false,
        ))
    }
    // get_capsule intentionally not implemented (trait default Unsupported).
}

/// FileEngine that publishes capsules but cannot recover them: prepared
/// launches must fail closed at the recovery roundtrip.
struct PutOnlyFiles(Files);

impl FileEngine for PutOnlyFiles {
    fn lease(&self, invocation: &EngineInvocation) -> Result<Box<dyn FileLease>, EngineError> {
        self.0.lease(invocation)
    }

    fn read(
        &self,
        invocation: &EngineInvocation,
        request: FileReadRequest,
    ) -> Result<FileSnapshot, EngineError> {
        self.0.read(invocation, request)
    }

    fn lookup(
        &self,
        invocation: &EngineInvocation,
        root: PathBuf,
        options: LookupOptions,
    ) -> Result<Vec<PathBuf>, EngineError> {
        self.0.lookup(invocation, root, options)
    }

    fn apply(
        &self,
        invocation: &EngineInvocation,
        request: FileEffectRequest,
    ) -> Result<FileEffectReceipt, EngineError> {
        self.0.apply(invocation, request)
    }

    fn restore(
        &self,
        invocation: &EngineInvocation,
        receipt: &FileEffectReceipt,
    ) -> Result<(), EngineError> {
        self.0.restore(invocation, receipt)
    }

    fn reconcile(&self, invocation: &EngineInvocation) -> Result<Vec<ZeroHandle>, EngineError> {
        self.0.reconcile(invocation)
    }

    fn put_capsule(
        &self,
        invocation: &EngineInvocation,
        capsule: &zero_abi::WorkCapsule,
    ) -> Result<zero_abi::CapsulePublication, EngineError> {
        self.0.put_capsule(invocation, capsule)
    }
    // get_capsule intentionally not implemented (trait default Unsupported).
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

/// A complete, valid task-lens Safe result honoring the requested roots.
fn complete_lens_result(request: &TaskLensRequest) -> TaskLensResult {
    let mut evidence_roots = Vec::new();
    if let Some(root) = &request.capsule_root {
        evidence_roots.push(root.clone());
    }
    if let Some(root) = &request.required_snapshot {
        evidence_roots.push(root.clone());
    }
    TaskLensResult {
        verdict: SafetyVerdict::Safe,
        locus: Some(StructuralHit {
            path: PathBuf::from("src/lib.rs"),
            symbol: Some(request.query.clone()),
            line_start: Some(1),
            line_end: Some(1),
            preview: Some("pub fn alpha() {}".into()),
            evidence: Some(handle(b"lens-locus")),
            source: None,
            score: 1.0,
        }),
        impact: TaskLensCompilerImpact {
            complete: true,
            edge_roots: vec![handle(b"lens-edge")],
            reverse_roots: vec![handle(b"lens-reverse")],
        },
        proof_support: vec![handle(b"lens-proof")],
        evidence_roots,
        coverage: Some(StructuralCoverage {
            tier_a_pct: 99.5,
            tier_b_pct: 40.0,
            tier_c_pct: 10.0,
            freshness_verified: true,
            snapshot_id: 7,
        }),
        index_digest: "a".repeat(64),
        reasons: Vec::new(),
    }
}

/// Structural engine whose task lens returns a complete Safe result that
/// satisfies every Safe law against the request.
struct SafeLensGraph;
impl StructuralEngine for SafeLensGraph {
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
        Ok(StructuralResult {
            hits: vec![hit],
            index_digest: "index".into(),
            complete: true,
            coverage: None,
            absence: None,
            budget: None,
            diagnostic: None,
            continuation: None,
        })
    }

    fn task_lens(
        &self,
        _invocation: &EngineInvocation,
        request: TaskLensRequest,
    ) -> Result<TaskLensResult, EngineError> {
        Ok(complete_lens_result(&request))
    }
}

/// Structural engine whose task lens claims Safe while violating a Safe law;
/// the kernel must degrade the invalid would-be Safe to Unknown.
struct BrokenLensGraph;
impl StructuralEngine for BrokenLensGraph {
    fn query(
        &self,
        invocation: &EngineInvocation,
        query: StructuralQuery,
    ) -> Result<StructuralResult, EngineError> {
        SafeLensGraph.query(invocation, query)
    }

    fn task_lens(
        &self,
        _invocation: &EngineInvocation,
        request: TaskLensRequest,
    ) -> Result<TaskLensResult, EngineError> {
        let mut result = complete_lens_result(&request);
        result.impact.complete = false;
        Ok(result)
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

struct FailingProjectionTokens;
impl TokenEngine for FailingProjectionTokens {
    fn measure(
        &self,
        invocation: &EngineInvocation,
        bytes: &[u8],
    ) -> Result<TokenAccounting, EngineError> {
        (Tokens).measure(invocation, bytes)
    }
    fn certify(
        &self,
        invocation: &EngineInvocation,
        bytes: &[u8],
        claimed: &TokenAccounting,
    ) -> Result<zero_abi::CertifyResult, EngineError> {
        (Tokens).certify(invocation, bytes, claimed)
    }
    fn project(
        &self,
        _invocation: &EngineInvocation,
        _request: ProjectionRequest,
    ) -> Result<ProjectionResult, EngineError> {
        Err(EngineError::new(
            EngineErrorKind::Budget,
            "injected projection budget exceeded",
            false,
        ))
    }
    fn compress(
        &self,
        invocation: &EngineInvocation,
        request: CompressionRequest,
    ) -> Result<CompressionResult, EngineError> {
        (Tokens).compress(invocation, request)
    }
    fn expand(
        &self,
        invocation: &EngineInvocation,
        handle: &ZeroHandle,
        options: zero_abi::ExpandOptions,
    ) -> Result<Vec<u8>, EngineError> {
        (Tokens).expand(invocation, handle, options)
    }
}

fn kernel(root: &std::path::Path, files: Arc<dyn FileEngine>) -> ZeroKernel {
    kernel_with_structural(root, files, Arc::new(Graph))
}

fn kernel_with_structural(
    root: &std::path::Path,
    files: Arc<dyn FileEngine>,
    structural: Arc<dyn StructuralEngine>,
) -> ZeroKernel {
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
        structural,
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

fn production_kernel_relaxed_with_graph(root: &std::path::Path) -> ZeroKernel {
    let store_root = root.join(".zerostack");
    let files = Arc::new(
        ZeroFileEngine::open(root, &store_root, "contract").expect("open production file engine"),
    );
    let graph = Arc::new(
        graphzero_kernel::ZeroStructuralEngine::open(root, store_root.join("graph"), &store_root)
            .expect("open production structural engine"),
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
            cpu_ms: 20_000,
            memory_bytes: 128 * 1024 * 1024,
            call_limit: 64,
            task_limit: 8,
            output_byte_limit: 64 * 1024,
        },
        files,
        graph,
        Arc::new(Tokens),
        store_root,
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
fn run_argv_is_call_scoped_and_reaped() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), files);
    let response = kernel
        .execute_cell(r#"return await z.run(["printf", "hello"]);"#)
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
                "i=0; while [ $i -lt 5000 ]; do printf 'stdout-%04d\n' $i; printf 'stderr-%04d\n' $i >&2; i=$((i+1)); done"
                    .into(),
            ),
            ShellOptions {
                max_visible_bytes: Some(4096),
                ..ShellOptions::default()
            },
        )
        .unwrap();
    assert!(
        result.stdout.len() <= 4096,
        "stdout {} > 4096",
        result.stdout.len()
    );
    assert!(
        result.stderr.len() <= 4096,
        "stderr {} > 4096",
        result.stderr.len()
    );
    assert!(
        result.stdout.len() + result.stderr.len() <= 4096,
        "combined {} > 4096",
        result.stdout.len() + result.stderr.len()
    );
    assert!(result.stdout.starts_with("stdout-0000"));
    assert!(result.stdout.ends_with(
        "stdout-4999
"
    ));
    assert!(result.stderr.starts_with("stderr-0000"));
    assert!(result.stderr.ends_with(
        "stderr-4999
"
    ));
    assert!(result.stdout.contains("... output omitted ..."));
    assert!(result.stderr.contains("... output omitted ..."));
    assert!(result.exact.is_some());
    let exact = result.exact.unwrap();
    let cas = zero_store::ZeroCas::open(root.path().join(".zerostack"));
    let bytes = cas.get(&exact).expect("exact handle");
    let blob: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let full_stdout = blob["stdout"].as_str().unwrap();
    let full_stderr = blob["stderr"].as_str().unwrap();
    assert!(full_stdout.contains("stdout-0000"));
    assert!(full_stdout.contains(
        "stdout-4999
"
    ));
    assert!(!full_stdout.contains("output omitted"));
    assert!(full_stderr.contains("stderr-0000"));
    assert!(full_stderr.contains(
        "stderr-4999
"
    ));
    assert!(!full_stderr.contains("output omitted"));
    assert!(full_stdout.lines().count() >= 5000);
    assert!(full_stderr.lines().count() >= 5000);
    drop(cell);
    assert_eq!(kernel.live_processes(), 0);
    assert_eq!(kernel.live_frames(), 0);
}

#[cfg(unix)]
#[test]
fn run_timeout_kills_exact_tree() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), files);
    let mut cell = kernel.begin_cell("tree deadline").unwrap();
    let error = cell
        .shell(
            ShellCommand::Script("sh -c 'sleep 10 & sleep 10 & wait'".into()),
            ShellOptions {
                timeout_ms: Some(80),
                ..ShellOptions::default()
            },
        )
        .unwrap_err();
    assert!(
        matches!(
            &error,
            HostError::Engine(EngineError {
                kind: EngineErrorKind::Deadline,
                detail,
                ..
            }) if detail.contains("deadline")
        ),
        "expected Deadline, got {error:?}"
    );
    assert_eq!(kernel.live_processes(), 0);
    drop(cell);
    assert_eq!(kernel.live_processes(), 0);
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
            return await z.read(path);
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(response.value, Some(json!("\"content\"")));
    assert!(response.operations.iter().any(|op| op.method == "read"));
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn promise_all_and_array_map_replace_legacy_orchestration() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files.0.lock().insert(PathBuf::from("a.txt"), b"a".to_vec());
    files.0.lock().insert(PathBuf::from("b.txt"), b"b".to_vec());
    let kernel = kernel(root.path(), files);
    let response = kernel
        .execute_cell(
            r#"
            const pair = await Promise.all([
              z.read("a.txt"),
              z.read("b.txt"),
            ]);
            return pair.map(value => value + "!");
            "#,
        )
        .unwrap();
    let visible = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(visible.contains("a!"), "{visible}");
    assert!(visible.contains("b!"), "{visible}");
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn promise_all_preserves_destructured_callback_captures() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files.0.lock().insert(PathBuf::from("a.txt"), b"a".to_vec());
    files.0.lock().insert(PathBuf::from("b.txt"), b"b".to_vec());
    let kernel = kernel(root.path(), files);
    let response = kernel
        .execute_cell(
            r#"
            const cases = [["left", "a.txt"], ["right", "b.txt"]];
            return await Promise.all(cases.map(async ([name, path]) => {
              const text = await z.read(path);
              return name + ":" + text;
            }));
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let visible = response.value.unwrap().as_str().unwrap().to_owned();
    assert!(visible.contains("left:a"), "{visible}");
    assert!(visible.contains("right:b"), "{visible}");
    assert_eq!(kernel.live_tasks(), 0);
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn parallel_reads_overlap_in_real_time() {
    let root = tempdir().unwrap();
    let files = Arc::new(SlowFiles::new());
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt"] {
        files
            .inner
            .0
            .lock()
            .insert(PathBuf::from(name), b"content".to_vec());
    }
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let response = kernel
        .execute_cell(
            r#"
            return await Promise.all([
              z.read("a.txt"),
              z.read("b.txt"),
              z.read("c.txt"),
              z.read("d.txt"),
              z.read("e.txt"),
              z.read("f.txt"),
            ]);
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    // The six reads must run in three overlapping pairs.
    let peak = files.peak.load(Ordering::Acquire);
    assert_eq!(peak, 2, "peak must be exactly 2, got {peak}");
    // Settlement counters return to zero after quiescence.
    assert_eq!(
        files.inflight.load(Ordering::Acquire),
        0,
        "SlowFiles inflight must settle to zero"
    );
    assert_eq!(kernel.live_tasks(), 0);
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn token_projection_is_automatic_at_the_cell_boundary() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files
        .0
        .lock()
        .insert(PathBuf::from("token.txt"), b"alpha beta".to_vec());
    let kernel = kernel(root.path(), files);
    let response = kernel
        .execute_cell(r#"return await z.read("token.txt");"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert!(
        response
            .value
            .as_ref()
            .is_some_and(|value| value == "\"alpha beta\"")
    );
    assert!(response.ledger.bytes_visible > 0);
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
    let baseline = zero_store::EventLog::open(root.path().join(".zerostack"))
        .records("session")
        .unwrap()
        .len();
    let response = kernel
        .execute_cell(
            r#"
            await z.edit("unscoped.txt", {create: "temporary"});
            throw new Error("stop after write");
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(!files.0.lock().contains_key(&PathBuf::from("unscoped.txt")));
    let detail = response.error.as_ref().unwrap().detail.clone();
    assert!(detail.contains("stop after write"), "{detail}");
    let records = zero_store::EventLog::open(root.path().join(".zerostack"))
        .records("session")
        .unwrap();
    assert_eq!(records.len(), baseline + 1, "exactly one terminal event");
    assert_eq!(records[baseline].event, response.event);
    assert_eq!(
        records[baseline].model_visible_digest,
        blake3::hash(detail.as_bytes()).to_hex().to_string()
    );
    assert_eq!(kernel.live_frames(), 0);
    assert_eq!(kernel.live_tasks(), 0);
}

#[test]
fn projection_failure_rolls_back_file_and_state() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let store_root = root.path().join(".zerostack");
    let kernel = ZeroKernel::new(
        KernelContext {
            workspace_root: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            session_id: "session".into(),
            expected_state_root: None,
            contract_digest: "contract".into(),
        },
        KernelBudget {
            wall_ms: 20_000,
            cpu_ms: 20_000,
            memory_bytes: 64 * 1024 * 1024,
            call_limit: 64,
            task_limit: 8,
            output_byte_limit: 64 * 1024,
        },
        Arc::clone(&files) as Arc<dyn FileEngine>,
        Arc::new(Graph),
        Arc::new(FailingProjectionTokens),
        store_root,
    )
    .unwrap();
    let response = kernel
        .execute_cell(
            r#"
            await z.edit("projection.txt", {create: "temporary"});
            z.state.set("staged", true);
            return "any value triggers injected budget failure";
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
    let store_root = root.path().join(".zerostack");
    let first = ZeroKernel::new(
        KernelContext {
            workspace_root: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            session_id: "session".into(),
            expected_state_root: None,
            contract_digest: "contract".into(),
        },
        KernelBudget {
            wall_ms: 20_000,
            cpu_ms: 20_000,
            memory_bytes: 64 * 1024 * 1024,
            call_limit: 64,
            task_limit: 8,
            output_byte_limit: 64 * 1024,
        },
        Arc::clone(&files) as Arc<dyn FileEngine>,
        Arc::new(Graph),
        Arc::new(Tokens),
        store_root.clone(),
    )
    .unwrap();
    let response = first
        .execute_cell(r#"return await z.edit("first.txt", {create: "first"});"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let first_event = response.event.clone();
    let first_seq = response.turn.as_ref().unwrap().sequence;
    let records_after_first = zero_store::EventLog::open(root.path().join(".zerostack"))
        .records("session")
        .unwrap();
    assert_eq!(records_after_first.len(), 1);
    assert_eq!(records_after_first[0].event, first_event);
    drop(first);

    let second = ZeroKernel::new(
        KernelContext {
            workspace_root: root.path().to_path_buf(),
            project_root: root.path().to_path_buf(),
            session_id: "session".into(),
            expected_state_root: None,
            contract_digest: "contract".into(),
        },
        KernelBudget {
            wall_ms: 20_000,
            cpu_ms: 20_000,
            memory_bytes: 64 * 1024 * 1024,
            call_limit: 64,
            task_limit: 8,
            output_byte_limit: 64 * 1024,
        },
        Arc::clone(&files) as Arc<dyn FileEngine>,
        Arc::new(Graph),
        Arc::new(Tokens),
        store_root,
    )
    .unwrap();
    let response = second
        .execute_cell(r#"return await z.edit("second.txt", {create: "second"});"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert!(
        response.turn.as_ref().unwrap().sequence > first_seq,
        "sequence must advance monotonically"
    );
    let records = zero_store::EventLog::open(root.path().join(".zerostack"))
        .records("session")
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1].event, response.event);
    let files = files.0.lock();
    assert_eq!(files.get(&PathBuf::from("first.txt")).unwrap(), b"first");
    assert_eq!(files.get(&PathBuf::from("second.txt")).unwrap(), b"second");
}

#[cfg(unix)]
#[test]
fn cancelled_run_frame_drains_before_response() {
    let root = tempdir().unwrap();
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let cancellation = AtomicCancellation::new();
    let trigger = cancellation.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(50));
        trigger.cancel();
    });
    let response = kernel
        .execute_cell_with_cancellation("return await z.run('sleep 5');", cancellation)
        .unwrap();
    canceller.join().unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Cancelled);
    assert_eq!(kernel.live_frames(), 0);
    assert_eq!(kernel.live_tasks(), 0);
    assert_eq!(kernel.live_processes(), 0);
}

#[test]
fn read_accepts_parent_relative_constellation_path() {
    let constellation = tempdir().unwrap();
    let workspace = constellation.path().join("ZeroStack");
    let sibling = constellation.path().join("TokenZero");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&sibling).unwrap();
    fs::write(sibling.join("contract.txt"), "sibling contract").unwrap();
    let kernel = production_kernel(&workspace);

    let response = kernel
        .execute_cell(r#"return await z.read("../TokenZero/contract.txt");"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(model_json(&response), json!("sibling contract"));
}

#[test]
fn read_full_view_expands_exact_source() {
    let root = tempdir().unwrap();
    let source = "pub fn alpha() -> u32 {\n    42\n}\n";
    write_fixture(root.path(), "src/lib.rs", source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({
              target: {path: "src/lib.rs"},
              view: {mode: "full"},
            });
            const expanded = await z.read(snap, {all: true});
            return {snap, expanded};
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
fn read_decision_view_retains_large_exact_source() {
    let root = tempdir().unwrap();
    let source = "0123456789abcdef\n".repeat(2_500);
    write_fixture(root.path(), "large.txt", &source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(r#"return await z.read({path: "large.txt"});"#)
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
        .execute_cell(&format!("return await z.read({exact}, {{all: true}});"))
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let expanded = model_json(&response);
    assert_eq!(expanded["text"], source);
    assert_eq!(expanded["complete"], true);
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn read_search_uses_structural_engine_and_exact_file_snapshot() {
    let root = tempdir().unwrap();
    let source = "pub fn alpha() {}\n";
    write_fixture(root.path(), "src/lib.rs", source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.read({
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
fn read_snapshot_and_edit_commit_exactly_once_in_one_cell() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "edit.ts", "const before = 1;\n");
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({
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
fn one_cell_find_snap_edit_verify_returns_only_final_ack() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/needle.rs", "pub const BEFORE: u32 = 1;\n");
    let kernel = production_kernel_relaxed_with_graph(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({
              target: {
                search: {
                  query: "BEFORE",
                  under: "src",
                  language: "rust",
                  mode: "literal",
                },
              },
              cardinality: "exactly_one",
              selection: {exactText: "pub const BEFORE: u32 = 1;"},
            });
            await z.edit(snap, {
              find: "pub const BEFORE: u32 = 1;",
              replace: "pub const AFTER: u32 = 1;",
            });
            return await z.read("src/needle.rs");
            "#,
        )
        .unwrap();

    assert_eq!(
        fs::read_to_string(root.path().join("src/needle.rs")).unwrap(),
        "pub const AFTER: u32 = 1;\n"
    );
    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "error={:?}",
        response.error
    );
    assert_eq!(model_json(&response), json!("pub const AFTER: u32 = 1;\n"));
    assert_eq!(
        response
            .operations
            .iter()
            .map(|operation| operation.method.as_str())
            .collect::<Vec<_>>(),
        vec!["read", "edit", "read"]
    );
}

#[test]
fn read_snapshot_edit_rejects_stale_preimage_without_mutation() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "stale.ts", "const before = 1;\n");
    let kernel = production_kernel(root.path());
    let response = kernel
        .execute_cell(r#"return await z.read({path: "stale.ts"});"#)
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
fn failed_cell_restores_snapshot_aware_edit() {
    let root = tempdir().unwrap();
    let original = "const before = 1;\n";
    write_fixture(root.path(), "rollback.ts", original);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({path: "rollback.ts"});
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
fn read_shorthand_preserves_selection_and_view() {
    let root = tempdir().unwrap();
    let source = "pub fn alpha() {}\n";
    write_fixture(root.path(), "src/lib.rs", source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.read({
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
fn read_accepts_next_as_its_byte_cursor() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "cursor.txt", "abcdefghij");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({path: "cursor.txt"});
            return await z.read(snap, {next: 2, limit: 3});
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
fn read_symbol_selection_uses_in_process_structural_search() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/lib.rs", "pub fn alpha() {}\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.read({
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
fn snapshot_aware_edit_rejects_patch_outside_selection() {
    let root = tempdir().unwrap();
    let original = "const before = 1; const elsewhere = 2;\n";
    write_fixture(root.path(), "scope.ts", original);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({
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
fn apply_creates_file_and_updates_module_index_atomically() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/mod.rs", "mod old;\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.apply({
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
        .execute_cell(&format!("return await z.read({delta}, {{all: true}});"))
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
        .execute_cell(&format!("return await z.read({new_after}, {{all: true}});"))
        .unwrap();
    assert_eq!(model_json(&expanded)["text"], "pub fn created() {}\n");
    let module_index = targets
        .iter()
        .find(|target| target["name"] == "moduleIndex")
        .unwrap();
    let index_before = serde_json::to_string(module_index["before"].as_str().unwrap()).unwrap();
    let expanded = kernel
        .execute_cell(&format!(
            "return await z.read({index_before}, {{all: true}});"
        ))
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
fn apply_rejects_unconfined_verification_command_without_mutation() {
    let root = tempdir().unwrap();
    let first = "const first = 1;\n";
    let second = "const second = 2;\n";
    write_fixture(root.path(), "first.ts", first);
    write_fixture(root.path(), "second.ts", second);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.apply({
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
fn read_rejects_structural_evidence_for_another_source() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/lib.rs", "pub fn beta() {}\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.read({
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
fn read_accepts_nested_byte_line_and_all_selectors() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "selectors.txt", "alpha\nbeta\ngamma\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({path: "selectors.txt"});
            const bytes = await z.read(snap, {bytes: {start: 6, end: 10}});
            const lines = await z.read(snap, {lines: {start: 2, end: 2}});
            const all = await z.read(snap, {all: true});
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
fn read_binary_file_returns_exact_recovery_without_text_claims() {
    let root = tempdir().unwrap();
    let binary = [0, 0xff, b'\r', b'\n', b'\n'];
    fs::write(root.path().join("binary.bin"), binary).unwrap();
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({path: "binary.bin"});
            const expanded = await z.read(snap, {all: true});
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
fn read_reports_bom_and_mixed_newlines() {
    let root = tempdir().unwrap();
    let source = "\u{feff}alpha\r\nbeta\n";
    write_fixture(root.path(), "mixed.txt", source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(r#"return await z.read({target:{path:"mixed.txt"},view:{mode:"full"}});"#)
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let snap = model_json(&response);
    assert_eq!(snap["source"]["bom"], true);
    assert_eq!(snap["source"]["newline"], "mixed");
    assert_eq!(snap["view"]["text"], source);
}

#[test]
fn snapshot_edit_supports_line_replacement_and_anchored_insertion() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "typed.txt", "alpha\nbeta\ngamma\n");
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const line = await z.read({
              path: "typed.txt",
              selection: {lines: {start: 2, end: 2}},
            });
            await z.edit(line, {kind: "replace_lines", content: "BETA\n"});
            const anchor = await z.read({
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
fn snapshot_edit_replace_file_requires_unselected_snapshot() {
    let root = tempdir().unwrap();
    write_fixture(
        root.path(),
        "whole.txt",
        "old
",
    );
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({path: "whole.txt"});
            await z.edit(snap, {kind: "replace_file", content: "new
"});
            return await z.read("whole.txt");
            "#,
        )
        .unwrap();

    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(
        model_json(&response),
        "new
"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("whole.txt")).unwrap(),
        "new
"
    );

    let rejected = kernel
        .execute_cell(
            r#"
            const snap = await z.read({path: "whole.txt", selection: {exactText: "new"}});
            return await z.edit(snap, {kind: "replace_file", content: "bad
"});
            "#,
        )
        .unwrap();
    assert_eq!(rejected.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert!(
        rejected
            .error
            .as_ref()
            .unwrap()
            .detail
            .contains("replace_file requires an unselected snap"),
        "{:?}",
        rejected.error
    );
    assert_eq!(
        fs::read_to_string(root.path().join("whole.txt")).unwrap(),
        "new
"
    );
}

#[test]
fn snapshot_edit_replace_exact_requires_explicit_single_match() {
    let root = tempdir().unwrap();
    let original = "const alpha = 1;\n";
    write_fixture(root.path(), "count.ts", original);
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({
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
fn apply_existing_absence_target_rolls_back_related_edit() {
    let root = tempdir().unwrap();
    let index = "mod old;\n";
    write_fixture(root.path(), "src/mod.rs", index);
    write_fixture(root.path(), "src/new.rs", "already here\n");
    let kernel = production_kernel_relaxed(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.apply({
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
fn apply_refuses_symlink_substituted_parent() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let outside = tempdir().unwrap();
    symlink(outside.path(), root.path().join("escape")).unwrap();
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.apply({
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
fn full_read_view_fails_typed_instead_of_truncating() {
    let root = tempdir().unwrap();
    let source = "0123456789abcdef\n".repeat(2_500);
    write_fixture(root.path(), "large-full.txt", &source);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"return await z.read({target:{path:"large-full.txt"},view:{mode:"full"}});"#,
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
fn read_search_refuses_ambiguous_exact_target() {
    let root = tempdir().unwrap();
    write_fixture(root.path(), "src/lib.rs", "pub fn alpha() {}\n");
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.read({
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
fn apply_rejects_replace_file_after_prior_change() {
    let root = tempdir().unwrap();
    let original = "const alpha = 1;\n";
    write_fixture(root.path(), "overwrite.ts", original);
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            return await z.apply({
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
fn repeated_read_cursors_reconstruct_exact_source() {
    let root = tempdir().unwrap();
    let source = "0123456789".repeat(25);
    write_fixture(root.path(), "paged.txt", &source);
    let kernel = production_kernel(root.path());
    let response = kernel
        .execute_cell(r#"return await z.read({path: "paged.txt"});"#)
        .unwrap();
    let snap = model_json(&response);
    let exact = snap["source"]["exact"].as_str().unwrap();
    let mut next = 0_u64;
    let mut reconstructed = String::new();
    loop {
        let response = kernel
            .execute_cell(&format!(
                "return await z.read({:?}, {{next: {}, limit: 17}});",
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
    let source = "pub fn alpha() {}
";
    write_fixture(root.path(), "src/lib.rs", source);
    write_fixture(
        root.path(),
        "decoy.rs",
        "pub fn alpha() { alpha(); alpha(); alpha(); alpha(); }
",
    );
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
            const hits = await z.find("alpha", {
                mode: "natural",
                path: "src/lib.rs",
                language: "rust",
                limit: 1,
              });
            const expandedHit = await z.read(hits.hits[0].source);
            const exactModes = await Promise.all([
              z.find("alpha", {
                mode: "word",
                path: "src/lib.rs",
                language: "rust",
                limit: 1,
              }),
              z.find("alpha", {
                mode: "literal",
                path: "src/lib.rs",
                language: "rust",
                limit: 1,
              }),
            ]);
            const graph = await z.find("alpha", {
                mode: "symbols",
                path: "src/lib.rs",
                language: "rust",
                limit: 2,
              });
            const snap = await z.read({
              target: {search: {query: "alpha", under: "src/lib.rs", mode: "natural"}},
              cardinality: "exactly_one",
            });
            const symbol = await z.read({
              target: {path: "src/lib.rs"},
              cardinality: "exactly_one",
              selection: {symbol: "alpha"},
            });
            return {hits, exactModes, expandedHit, snap, symbol, graph};
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
    for result in value["exactModes"].as_array().unwrap() {
        assert_eq!(result["hits"][0]["path"], "src/lib.rs");
    }
    assert_eq!(value["hits"]["hits"][0]["path"], "src/lib.rs");
    assert_eq!(
        value["hits"]["hits"][0]["source"],
        value["snap"]["source"]["exact"]
    );
    assert_eq!(value["expandedHit"], source);
    assert_eq!(
        value["snap"]["structural"]["source"],
        value["snap"]["source"]["exact"]
    );
    assert_eq!(value["symbol"]["selection"]["kind"], "symbol");
    assert_eq!(value["symbol"]["selection"]["lineStart"], 1);
    assert_eq!(
        value["symbol"]["structural"]["source"],
        value["symbol"]["source"]["exact"]
    );
    assert_eq!(value["graph"]["hits"][0]["path"], "src/lib.rs");
    assert!(value["graph"]["hits"][0]["source"].is_string());
}

#[test]
fn optional_connector_properties_follow_javascript_undefined_semantics() {
    let root = tempdir().unwrap();
    fs::write(root.path().join("binary.bin"), [0, 0xff]).unwrap();
    let kernel = production_kernel(root.path());

    let response = kernel
        .execute_cell(
            r#"
            const snap = await z.read({path: "binary.bin"});
            const expanded = await z.read(snap, {all: true});
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
            await z.edit(p, {create: 'v1\nv2\nv3\n'});
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
            await z.edit(p, {create: 'v1
v2
v3
'});
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
        Some("\"v1\nV2-DONE\nv3\n\""),
        "substituted content must round-trip"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("guard-edit-sub.txt")).unwrap(),
        "v1
V2-DONE
v3
",
        "filesystem must reflect substitution"
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
            await z.edit(p, {create: 'alpha\nbeta\ngamma\n'});
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
            await z.edit(p, {create: 'old content
'});
            await z.edit(p, { kind: 'replace_file', content: 'fresh
' });
            return await z.read(p);
        "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let returned = response.value.unwrap();
    assert_eq!(
        returned.as_str(),
        Some("\"fresh\n\""),
        "deliberate replace_file content must round-trip"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("guard-edit-rf.txt")).unwrap(),
        "fresh
",
        "filesystem must contain fresh content"
    );
}

#[test]
fn noncanonical_methods_are_rejected() {
    let root = tempdir().unwrap();
    let kernel = production_kernel(root.path());
    for name in [
        "snap", "expand", "lookup", "asgrep", "write", "remove", "effect", "shell", "parallel",
        "pipeline", "transact", "measure", "project", "compress", "inspect", "help",
    ] {
        let response = kernel
            .execute_cell(&format!("return await z.{name}();"))
            .unwrap();
        assert_eq!(
            response.outcome,
            zero_abi::ZeroKernelOutcome::Failed,
            "{name} unexpectedly remained callable"
        );
        assert!(
            response
                .error
                .as_ref()
                .is_some_and(|error| error.detail.contains("is not a ZeroKernel method")),
            "{name}: {:?}",
            response.error
        );
    }
}

#[test]
fn find_accepts_single_object_and_positional_forms() {
    let root = tempdir().unwrap();
    std::fs::write(
        root.path().join("probe.rs"),
        "pub struct AsgrepProbeMarker;
",
    )
    .unwrap();
    std::fs::write(
        root.path().join("other.rs"),
        "nothing here
",
    )
    .unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let object_form = kernel
        .execute_cell(
            r#"
            const r = await z.find({ query: "AsgrepProbeMarker", mode: "natural" });
            return r;
        "#,
        )
        .unwrap();
    assert_eq!(object_form.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let obj = model_json(&object_form);
    assert!(
        obj["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["path"] == "probe.rs"),
        "object form must hit probe.rs: {obj:?}"
    );

    let positional = kernel
        .execute_cell(
            r#"
            const r = await z.find("AsgrepProbeMarker", { mode: "natural" });
            return r;
        "#,
        )
        .unwrap();
    assert_eq!(positional.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let pos = model_json(&positional);
    assert!(
        pos["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["path"] == "probe.rs"),
        "positional form must hit probe.rs: {pos:?}"
    );
    assert_eq!(
        obj["hits"].as_array().unwrap().len(),
        pos["hits"].as_array().unwrap().len()
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("probe.rs")).unwrap(),
        "pub struct AsgrepProbeMarker;
"
    );
}

#[test]
fn task_lens_with_query_only_engine_returns_canonical_unknown_without_effects() {
    let root = tempdir().unwrap();
    // The Graph mock implements only `query`; the default task_lens is
    // Unsupported, which must surface as a canonical Unknown verdict.
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let response = kernel
        .execute_cell(
            r#"
            const lens = await z.find({query: "alpha", taskLens: {}});
            return lens;
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let lens = model_json(&response);
    assert_eq!(
        lens["verdict"]["unknown"]["reasons"],
        json!(["task_lens_unsupported"])
    );
    assert_eq!(lens["reasons"], json!(["task_lens_unsupported"]));
    assert!(lens.get("locus").is_none());
    assert_eq!(lens["impact"]["complete"], false);
    assert_eq!(lens["proofSupport"], json!([]));
    assert_eq!(lens["evidenceRoots"], json!([]));
    // Query-only engines leave no content handles and no effects behind.
    assert_eq!(response.handles, vec![]);
    assert_eq!(response.ledger.bytes_read, 0);
    assert_eq!(response.ledger.bytes_written, 0);
}

#[test]
fn task_lens_safe_result_flows_after_validate() {
    let root = tempdir().unwrap();
    let kernel = kernel_with_structural(
        root.path(),
        Arc::new(Files::default()),
        Arc::new(SafeLensGraph),
    );
    let capsule = handle(b"capsule-root");
    let snapshot = handle(b"snapshot-root");
    let response = kernel
        .execute_cell(&format!(
            r#"
            const lens = await z.find({{
              query: "alpha",
              mode: "natural",
              limit: 3,
              taskLens: {{
                capsuleRoot: {:?},
                requiredSnapshot: {:?},
              }},
            }});
            return lens;
            "#,
            capsule.as_str(),
            snapshot.as_str(),
        ))
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let lens = model_json(&response);
    assert_eq!(lens["verdict"], "safe");
    assert_eq!(lens["reasons"], json!([]));
    assert_eq!(lens["locus"]["symbol"], "alpha");
    assert_eq!(lens["impact"]["complete"], true);
    assert_eq!(lens["indexDigest"], "a".repeat(64));
    assert_eq!(
        lens["proofSupport"],
        json!([handle(b"lens-proof").as_str()])
    );
    // Safe law 5: the requested roots appear among the evidence roots.
    assert_eq!(
        lens["evidenceRoots"],
        json!([capsule.as_str(), snapshot.as_str()])
    );
    assert_eq!(lens["coverage"]["tierAPct"], 99.5);
    assert_eq!(lens["coverage"]["freshnessVerified"], true);
    // The lens evidence binds its content handles into the cell.
    assert!(response.handles.contains(&handle(b"lens-proof")));
    assert!(response.handles.contains(&capsule));
    assert!(response.handles.contains(&snapshot));
}

#[test]
fn task_lens_invalid_safe_degrades_to_canonical_unknown() {
    let root = tempdir().unwrap();
    // BrokenLensGraph claims Safe while leaving the impact closure
    // incomplete; the kernel must never surface that invalid authority.
    let kernel = kernel_with_structural(
        root.path(),
        Arc::new(Files::default()),
        Arc::new(BrokenLensGraph),
    );
    let response = kernel
        .execute_cell(
            r#"
            const lens = await z.find({query: "alpha", taskLens: {}});
            return lens;
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let lens = model_json(&response);
    assert_eq!(
        lens["verdict"]["unknown"]["reasons"],
        json!(["incomplete_impact"])
    );
    assert_eq!(lens["reasons"], json!(["incomplete_impact"]));
    assert!(lens.get("locus").is_none());
    assert_eq!(lens["impact"]["complete"], false);
}

#[test]
fn malformed_task_lens_slot_is_rejected() {
    let root = tempdir().unwrap();
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let cases: &[(&str, &str)] = &[
        (
            r#"z.find({query: "alpha", taskLens: "capsule"})"#,
            "must be an object",
        ),
        (
            r#"z.find({query: "alpha", taskLens: {capsuleRoot: "not-a-handle"}})"#,
            "invalid ZeroHandle",
        ),
        (
            r#"z.find({query: "alpha", taskLens: {capsuleRoot: 7}})"#,
            "expected a string",
        ),
        (
            r#"z.find({query: "alpha", taskLens: {bogus: "z://blob/0000000000000000000000000000000000000000000000000000000000000000"}})"#,
            "unknown z.find taskLens field",
        ),
        (
            r#"z.find({query: "alpha", taskLens: {capsuleRoot: "z://blob/abcd"}})"#,
            "invalid ZeroHandle",
        ),
    ];
    for (call, needle) in cases {
        let response = kernel
            .execute_cell(&format!("return await {call};"))
            .unwrap();
        assert_eq!(
            response.outcome,
            zero_abi::ZeroKernelOutcome::Failed,
            "{call}"
        );
        let detail = response
            .error
            .as_ref()
            .expect("failed response carries a typed error")
            .detail
            .clone();
        assert!(detail.contains(needle), "{call}: {detail}");
    }
    // A rejected slot cannot publish content or effects.
    let response = kernel
        .execute_cell(&format!("return await {};", cases[1].0))
        .unwrap();
    assert_eq!(response.handles, vec![]);
    assert_eq!(response.ledger.bytes_written, 0);
}

#[test]
fn object_find_without_task_lens_is_unchanged() {
    let root = tempdir().unwrap();
    let kernel = kernel(root.path(), Arc::new(Files::default()));
    let object = kernel
        .execute_cell(
            r#"
            const r = await z.find({query: "alpha", mode: "natural", limit: 1});
            return r;
            "#,
        )
        .unwrap();
    assert_eq!(object.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let object_json = model_json(&object);
    let positional = kernel
        .execute_cell(
            r#"
            const r = await z.find("alpha", {mode: "natural", limit: 1});
            return r;
            "#,
        )
        .unwrap();
    assert_eq!(positional.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(model_json(&positional), object_json);
    // Ordinary object find returns the StructuralResult, not a lens verdict.
    assert_eq!(object_json["hits"][0]["symbol"], "alpha");
    assert!(object_json.get("verdict").is_none());
    assert!(object_json.get("taskLens").is_none());
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
fn run_output_accounting_is_truthful_against_visible_bytes() {
    // RACC truthfulness at the shell boundary: reported visible tokens must
    // equal a fresh measurement over the exact stdout bytes the model saw.
    let root = tempdir().unwrap();
    // The byte-faithful token engine makes the truthfulness property exact.
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), files);
    let payload = "SHELL_OK accounting probe";
    let response = kernel
        .execute_cell(&format!("return await z.run([\"printf\", {payload:?}]);"))
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
fn unknown_zero_member_is_a_catchable_type_error() {
    let root = tempdir().unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            r#"
            try {
                z.shell("ls");
                return false;
            } catch (error) {
                return error.name === "TypeError" && error.message.includes("not a ZeroKernel method");
            }
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(model_json(&response), json!(true));
}

#[test]
fn guest_throw_is_invalid_input_not_internal() {
    let root = tempdir().unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell("throw new Error('bad input');")
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    assert_eq!(
        response.error.as_ref().map(|error| &error.kind),
        Some(&EngineErrorKind::InvalidInput)
    );
}

#[test]
fn final_surface_read_lists_directories() {
    let root = tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/a.rs"), "a").unwrap();
    std::fs::write(root.path().join("src/b.rs"), "b").unwrap();
    std::fs::create_dir_all(root.path().join("src/nested")).unwrap();
    std::fs::write(root.path().join("src/nested/deep.rs"), "deep").unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel.execute_cell("return await z.read('src');").unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let raw = response.value.unwrap();
    let listing: Value = serde_json::from_str(raw.as_str().unwrap()).unwrap();
    let entries = listing.as_array().unwrap();
    assert!(
        entries.len() >= 3,
        "directory read must include immediate children"
    );
    assert!(
        entries
            .iter()
            .any(|v| v.as_str().unwrap().ends_with("a.rs"))
    );
    assert!(
        entries
            .iter()
            .all(|v| !v.as_str().unwrap().ends_with("deep.rs")),
        "default directory reads must not recurse"
    );

    let response = kernel
        .execute_cell("return await z.read('src', {recursive: true});")
        .unwrap();
    let raw = response.value.unwrap();
    let recursive: Value = serde_json::from_str(raw.as_str().unwrap()).unwrap();
    assert!(
        recursive
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap().ends_with("deep.rs")),
        "recursive directory reads must include descendants"
    );

    let response = kernel
        .execute_cell("return await z.read('src', {limit: 2});")
        .unwrap();
    let page: Value = serde_json::from_str(response.value.unwrap().as_str().unwrap()).unwrap();
    assert_eq!(page["entries"].as_array().unwrap().len(), 2);
    assert_eq!(page["next"], 2);
    assert_eq!(page["complete"], false);

    let response = kernel
        .execute_cell("return await z.read('src', {offset: 2, limit: 2});")
        .unwrap();
    let last: Value = serde_json::from_str(response.value.unwrap().as_str().unwrap()).unwrap();
    assert_eq!(last["complete"], true);
    assert!(last["next"].is_null());
}

#[test]
fn final_surface_read_expands_exact_handles_to_text() {
    let root = tempdir().unwrap();
    std::fs::write(root.path().join("blob.txt"), "handle payload\n").unwrap();
    let kernel = production_kernel_relaxed(root.path());
    let response = kernel
        .execute_cell(
            "const snap = await z.read({path: 'blob.txt'}); return await z.read(snap.source.exact);",
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
            let caught = null;
            try {
                await z.edit('existing.txt', {create:'overwrite'});
            } catch (error) {
                caught = String(error.message || error);
            }
            await z.edit('existing.txt', {find:'original', replacement:'mutated'});
            const after = await z.read('existing.txt');
            return {caught, after};
            "#,
        )
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let value = model_json(&response);
    assert!(
        value["caught"].as_str().unwrap().contains("exists"),
        "must have caught create conflict: {:?}",
        value["caught"]
    );
    assert_eq!(value["after"], "mutated");
    assert_eq!(
        std::fs::read_to_string(root.path().join("existing.txt")).unwrap(),
        "mutated"
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
fn canonical_find_and_run_work() {
    let root = tempdir().unwrap();
    write_fixture(
        root.path(),
        "probe.rs",
        "pub struct AlphaMarker;
",
    );
    write_fixture(
        root.path(),
        "other.rs",
        "pub fn beta() {}
",
    );
    let kernel = production_kernel_relaxed(root.path());
    let find = kernel
        .execute_cell("const r = await z.find({query:'AlphaMarker', mode:'natural'}); return r;")
        .unwrap();
    assert_eq!(
        find.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "{find:?}"
    );
    let hits = model_json(&find);
    assert!(
        hits["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["path"] == "probe.rs"),
        "natural mode must hit probe.rs: {hits:?}"
    );

    let literal = kernel
        .execute_cell("const r = await z.find({query:'AlphaMarker', mode:'literal'}); return r;")
        .unwrap();
    let literal_hits = model_json(&literal);
    assert!(
        literal_hits["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["path"] == "probe.rs"),
        "literal mode must hit probe.rs"
    );

    let word = kernel
        .execute_cell("const r = await z.find({query:'AlphaMarker', mode:'word'}); return r;")
        .unwrap();
    let word_hits = model_json(&word);
    assert!(
        word_hits["hits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h["path"] == "probe.rs"),
        "word mode must hit probe.rs"
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
fn canonical_method_table_has_only_six_operations() {
    assert_eq!(
        zero_abi::GUEST_METHODS,
        ["read", "find", "edit", "apply", "run", "state"]
    );
}
fn root64(label: char) -> String {
    sha256_hex(&[label as u8])
}

/// Read the published event object back from the kernel CAS.
fn published_event(
    root: &std::path::Path,
    response: &zero_abi::ZeroKernelResponse,
) -> ZeroKernelEvent {
    let cas = zero_store::ZeroCas::open(root.join(".zerostack"));
    let bytes = cas.get(&response.event).expect("event object");
    serde_json::from_slice(&bytes).expect("event JSON")
}

/// Seal a prepared cell from a live cell's exact capsule, publication, and
/// binding coordinates.
fn prepare_from_cell(cell: &zero_kernel::Cell, source: &str) -> PreparedCell {
    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    preparation
        .finish(
            cell.binding().clone(),
            cell.capsule().clone(),
            cell.publication().clone(),
        )
        .unwrap()
}

#[test]
fn capsule_completed_and_failed_events_carry_same_valid_root() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let source = "return 7;";

    let completed = kernel.begin_cell(source).unwrap();
    let capsule_root = completed.capsule_root().to_owned();
    let capsule_object = completed.publication().object.clone();
    let response = completed.finish(json!(7)).unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    let event = published_event(root.path(), &response);
    let capsule = event
        .capsule
        .expect("completed event must carry capsule roots");
    capsule.validate().unwrap();
    assert_eq!(capsule.capsule_root, capsule_root);
    assert_eq!(capsule.capsule_object, capsule_object);
    assert_eq!(event.input_handles, vec![capsule_object.clone()]);
    {
        let stored = files
            .1
            .lock()
            .get(capsule_object.digest())
            .cloned()
            .expect("published capsule");
        assert_eq!(stored.root().unwrap(), capsule_root);
        assert_eq!(capsule_object_digest(&stored), capsule_object.digest());
    }

    let failed = kernel.begin_cell(source).unwrap();
    let failed_root = failed.capsule_root().to_owned();
    let failed_object = failed.publication().object.clone();
    let response = failed
        .fail(EngineError::new(
            EngineErrorKind::InvalidInput,
            "boom",
            false,
        ))
        .unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Failed);
    let event = published_event(root.path(), &response);
    let capsule = event
        .capsule
        .expect("failed event must carry capsule roots");
    capsule.validate().unwrap();
    assert_eq!(capsule.capsule_root, failed_root);
    assert_eq!(capsule.capsule_object, failed_object);
    assert_eq!(event.input_handles, vec![failed_object.clone()]);
    {
        let stored = files
            .1
            .lock()
            .get(failed_object.digest())
            .cloned()
            .expect("published capsule");
        assert_eq!(stored.root().unwrap(), failed_root);
        assert_eq!(capsule_object_digest(&stored), failed_object.digest());
    }
}

#[test]
fn six_method_cell_traces_bind_one_capsule_root_with_strict_occurrences() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files
        .0
        .lock()
        .insert(PathBuf::from("a.txt"), b"alpha".to_vec());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let response = kernel
        .execute_cell(
            r#"
            const text = await z.read("a.txt");
            const hits = await z.find({query: "alpha", mode: "natural"});
            await z.apply([{path: "c.txt", create: "gamma"}]);
            await z.edit("b.txt", {create: "beta"});
            const ran = await z.run(["printf", "ok"]);
            z.state.set("seen", true);
            return {text, hits: hits.hits.length, ran: ran.stdout, state: z.state.get("seen")};
            "#,
        )
        .unwrap();
    assert_eq!(
        response.outcome,
        zero_abi::ZeroKernelOutcome::Completed,
        "error={:?}",
        response.error
    );

    // All six canonical methods are traced and share one capsule root with
    // strictly positive, strictly increasing occurrences.
    let mut methods = BTreeSet::new();
    let mut previous_sequence = 0_u64;
    let mut previous_occurrence = 0_u64;
    let capsule_root = response.operations[0].capsule_root.clone();
    for operation in &response.operations {
        methods.insert(operation.method.as_str());
        assert_eq!(operation.capsule_root, capsule_root);
        assert!(operation.occurrence > 0, "occurrence must be positive");
        assert!(
            operation.sequence > previous_sequence,
            "sequence must be strictly increasing"
        );
        assert!(
            operation.occurrence > previous_occurrence,
            "occurrence must be strictly increasing"
        );
        previous_sequence = operation.sequence;
        previous_occurrence = operation.occurrence;
    }
    for method in ["read", "find", "edit", "apply", "run", "state"] {
        assert!(
            methods.contains(method),
            "missing traced method {method}: {methods:?}"
        );
    }

    // The terminal event carries the same capsule root as every trace, and
    // the response validates end to end (occurrence monotonicity enforced).
    let event = published_event(root.path(), &response);
    let capsule = event
        .capsule
        .expect("terminal event must be capsule-rooted");
    assert_eq!(capsule.capsule_root, capsule_root);
    assert!(capsule.capsule_object.as_str().starts_with("z://blob/"));
    response.validate().unwrap();
}

#[test]
fn capsule_prepared_launch_uses_exact_coordinates_and_rejects_drift() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    files
        .0
        .lock()
        .insert(PathBuf::from("src/lib.rs"), b"content".to_vec());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let source = "return await z.read('src/lib.rs');";

    // Seal a prepared cell from the kernel's own current coordinates and
    // launch it: the exact capsule, publication, and binding must roundtrip.
    let probe = kernel.begin_cell(source).unwrap();
    let prepared = prepare_from_cell(&probe, source);
    drop(probe);
    let response = kernel.execute_prepared(&prepared).unwrap();
    assert_eq!(response.outcome, zero_abi::ZeroKernelOutcome::Completed);
    assert_eq!(response.value, Some(json!("\"content\"")));
    let event = published_event(root.path(), &response);
    let capsule = event.capsule.unwrap();
    assert_eq!(capsule.capsule_root, prepared.binding().capsule_root);
    assert_eq!(capsule.capsule_object, prepared.publication().object);
    assert_eq!(
        response.operations[0].capsule_root,
        prepared.binding().capsule_root
    );
    // The prepared launch must not publish a second capsule.
    assert_eq!(
        files.1.lock().len(),
        1,
        "prepared launch must not re-publish"
    );

    // Capsule/source drift: a capsule that was never published is
    // unrecoverable, so the launch fails closed at the roundtrip.
    let probe = kernel.begin_cell(source).unwrap();
    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    let mut unpublished = probe.capsule().clone();
    unpublished.roots.evidence = root64('a');
    let unpublished_root = unpublished.root().unwrap();
    let unpublished = preparation
        .finish(
            SpeculationBinding {
                capsule_root: unpublished_root.clone(),
                state_root: probe.binding().state_root.clone(),
                contract_root: probe.binding().contract_root.clone(),
                epoch: probe.binding().epoch,
            },
            unpublished,
            CapsulePublication {
                capsule_root: unpublished_root,
                object: ZeroHandle::from_digest(&root64('e')).unwrap(),
                created: true,
            },
        )
        .unwrap();
    drop(probe);
    let error = kernel.execute_prepared(&unpublished).unwrap_err();
    assert!(
        matches!(&error, HostError::Engine(engine) if engine.kind == EngineErrorKind::NotFound),
        "{error}"
    );
    // Object drift: the real capsule root paired with a wrong object handle
    // must fail the roundtrip (recovery is object-addressed, not
    // root-addressed).
    let probe = kernel.begin_cell(source).unwrap();
    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    let wrong_object = CapsulePublication {
        capsule_root: probe.publication().capsule_root.clone(),
        object: ZeroHandle::from_digest(&root64('f')).unwrap(),
        created: true,
    };
    let drifted = preparation
        .finish(
            probe.binding().clone(),
            probe.capsule().clone(),
            wrong_object,
        )
        .unwrap();
    drop(probe);
    let error = kernel.execute_prepared(&drifted).unwrap_err();
    assert!(
        matches!(&error, HostError::Engine(engine) if engine.kind == EngineErrorKind::NotFound),
        "wrong capsule object must fail the roundtrip: {error}"
    );

    // State drift: the same real capsule sealed against a different state
    // root must fail launch.
    let probe = kernel.begin_cell(source).unwrap();
    let mut drifted_binding = probe.binding().clone();
    drifted_binding.state_root = root64('b');
    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    let drifted = preparation
        .finish(
            drifted_binding,
            probe.capsule().clone(),
            probe.publication().clone(),
        )
        .unwrap();
    drop(probe);
    let error = kernel.execute_prepared(&drifted).unwrap_err();
    assert!(error.to_string().contains("drifted"), "{error}");

    // Contract drift: the same real capsule sealed against a different
    // contract root must fail launch.
    let probe = kernel.begin_cell(source).unwrap();
    let mut drifted_binding = probe.binding().clone();
    drifted_binding.contract_root = root64('c');
    let mut preparation = CellPreparation::new();
    preparation.feed(source).unwrap();
    let drifted = preparation
        .finish(
            drifted_binding,
            probe.capsule().clone(),
            probe.publication().clone(),
        )
        .unwrap();
    drop(probe);
    let error = kernel.execute_prepared(&drifted).unwrap_err();
    assert!(error.to_string().contains("drifted"), "{error}");
}

#[test]
fn capsule_put_or_get_failure_blocks_launch() {
    let root = tempdir().unwrap();
    let files = Arc::new(NoCapsuleFiles(Files::default()));
    let put_failing_kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let error = put_failing_kernel.execute_cell("return 1;").unwrap_err();
    assert!(
        matches!(&error, HostError::Engine(engine) if engine.kind == EngineErrorKind::Unsupported),
        "{error}"
    );
    assert_eq!(put_failing_kernel.live_frames(), 0);

    // Recovery failure: a valid published capsule cannot be recovered, so a
    // prepared launch fails closed at the roundtrip.
    let put_only = Arc::new(PutOnlyFiles(Files::default()));
    let kernel = kernel(root.path(), Arc::clone(&put_only) as Arc<dyn FileEngine>);
    let source = "return 2;";
    let probe = kernel.begin_cell(source).unwrap();
    let prepared = prepare_from_cell(&probe, source);
    drop(probe);
    let error = kernel.execute_prepared(&prepared).unwrap_err();
    assert!(
        matches!(&error, HostError::Engine(engine) if engine.kind == EngineErrorKind::Unsupported),
        "{error}"
    );
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn capsule_failure_publishes_no_event_and_leaves_no_effect() {
    let root = tempdir().unwrap();
    let files = Arc::new(NoCapsuleFiles(Files::default()));
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);
    let error = kernel
        .execute_cell(
            r#"
            await z.edit("created.txt", {create: "new"});
            return true;
            "#,
        )
        .unwrap_err();
    assert!(
        matches!(&error, HostError::Engine(engine) if engine.kind == EngineErrorKind::Unsupported),
        "{error}"
    );
    assert!(
        !files.0.0.lock().contains_key(&PathBuf::from("created.txt")),
        "no effect may exist without a published capsule"
    );
    let records = zero_store::EventLog::open(root.path().join(".zerostack"))
        .records("session")
        .unwrap();
    assert!(
        records.is_empty(),
        "no event may be published without a capsule"
    );
    assert_eq!(kernel.live_frames(), 0);
}

#[test]
fn capsule_terminal_tuple_roots_follow_actual_effects_and_operations() {
    let root = tempdir().unwrap();
    let files = Arc::new(Files::default());
    let kernel = kernel(root.path(), Arc::clone(&files) as Arc<dyn FileEngine>);

    // One cell with a traced operation and a committed effect...
    let mut cell = kernel.begin_cell("effect-cell").unwrap();
    cell.record_operations(
        vec![ZeroOperationTrace {
            sequence: 1,
            method: "read".into(),
            status: ZeroOperationStatus::Completed,
            capsule_root: cell.capsule_root().to_owned(),
            occurrence: 1,
            parallel_group: None,
            target: None,
            detail: None,
            result_count: None,
            changed_files: None,
            duration_ns: 0,
        }],
        false,
    )
    .unwrap();
    cell.create("made.txt", b"made".to_vec()).unwrap();
    let response = cell.finish(json!({"ok": true})).unwrap();
    let with_effect = published_event(root.path(), &response);

    // ...and one bare cell with neither.
    let cell = kernel.begin_cell("effect-cell").unwrap();
    let response = cell.finish(json!(true)).unwrap();
    let bare = published_event(root.path(), &response);

    let with_effect = with_effect.capsule.expect("capsule tuple");
    let bare = bare.capsule.expect("capsule tuple");
    // The constant planes stay identical across cells...
    assert_eq!(with_effect.provider_root, bare.provider_root);
    assert_eq!(with_effect.cache_root, bare.cache_root);
    assert_eq!(with_effect.speculation_root, bare.speculation_root);
    assert_eq!(with_effect.quality_root, bare.quality_root);
    // ...while the effect and occurrence roots bind the actual facts.
    assert_ne!(with_effect.effect_root, bare.effect_root);
    assert_ne!(with_effect.occurrence_root, bare.occurrence_root);
    // Every event still carries a valid capsule object.
    with_effect.validate().unwrap();
    bare.validate().unwrap();
}
