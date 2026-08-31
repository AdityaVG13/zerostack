use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use napi::bindgen_prelude::{AbortSignal, AsyncTask, ToNapiValue, TypeName, ValueType};
use napi::{Env, Error, Result, Task};
use napi_derive::napi;
use parking_lot::Mutex;
use zero_abi::{KernelBudget, ProviderUsageObservation, ZeroHandle, ZeroKernelResponse};
use zero_kernel::{AtomicCancellation, GraphZeroCompletenessInput, ZeroKernel as CoreZeroKernel};
use zero_store::ProviderUsagePublication;

const DEFAULT_WALL_MS: u64 = 30_000;
const DEFAULT_MEMORY_BYTES: u64 = 256 * 1024 * 1024;
const DEFAULT_OUTPUT_BYTES: u32 = 64 * 1024;

#[napi(object)]
pub struct ZeroKernelOptions {
    pub root: String,
    pub session_id: Option<String>,
    pub state_root: Option<String>,
    pub tokenizer_model: Option<String>,
    pub wall_ms: Option<u32>,
    pub cpu_ms: Option<u32>,
    pub memory_bytes: Option<i64>,
    pub call_limit: Option<u32>,
    pub task_limit: Option<u32>,
    pub output_byte_limit: Option<u32>,
}

struct KernelConfig {
    root: String,
    session_id: String,
    state_root: String,
    tokenizer_model: Option<String>,
    budget: KernelBudget,
}

struct KernelCore {
    config: KernelConfig,
    kernel: Mutex<Option<Arc<CoreZeroKernel>>>,
    ready: AtomicBool,
    terminated: AtomicBool,
    inflight: AtomicUsize,
    completed: AtomicU64,
    next_task: AtomicU64,
    active: Mutex<BTreeMap<u64, AtomicCancellation>>,
}

/// Complete identity of a [`CoreZeroKernel`] as constructed from [`KernelConfig`]: canonical
/// project root, state root, session, tokenizer, and every budget coordinate.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct RegistryKey {
    root: String,
    state_root: String,
    session_id: String,
    tokenizer_model: Option<String>,
    wall_ms: u64,
    cpu_ms: u64,
    memory_bytes: u64,
    call_limit: u32,
    task_limit: u32,
    output_byte_limit: u32,
}

impl KernelConfig {
    fn registry_key(&self) -> RegistryKey {
        // The kernel canonicalizes the project root before opening engines, so
        // the key must too; two spellings of one directory are one identity.
        let root = std::fs::canonicalize(&self.root)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| self.root.clone());
        RegistryKey {
            root,
            state_root: self.state_root.clone(),
            session_id: self.session_id.clone(),
            tokenizer_model: self.tokenizer_model.clone(),
            wall_ms: self.budget.wall_ms,
            cpu_ms: self.budget.cpu_ms,
            memory_bytes: self.budget.memory_bytes,
            call_limit: self.budget.call_limit,
            task_limit: self.budget.task_limit,
            output_byte_limit: self.budget.output_byte_limit,
        }
    }
}

/// Process-wide singleflight registry. Embeddings with the same complete configuration reuse one
/// runtime instead of rebuilding and rescanning it.
static KERNEL_REGISTRY: Mutex<BTreeMap<RegistryKey, Weak<CoreZeroKernel>>> =
    Mutex::new(BTreeMap::new());

impl KernelCore {
    fn initialize(&self) -> Result<Arc<CoreZeroKernel>> {
        if self.terminated.load(Ordering::Acquire) {
            return Err(Error::from_reason("ZeroKernel is shut down"));
        }
        let mut slot = self.kernel.lock();
        if self.terminated.load(Ordering::Acquire) {
            return Err(Error::from_reason("ZeroKernel is shut down"));
        }
        if let Some(kernel) = slot.as_ref() {
            return Ok(Arc::clone(kernel));
        }
        // Singleflight gate: hold the registry lock across the reuse probe and any
        // construction. Concurrent initializations with this identity upgrade the same weak
        // handle onto one kernel; distinct identities serialize construction but never share an entry.
        let key = self.config.registry_key();
        let mut registry = KERNEL_REGISTRY.lock();
        if let Some(kernel) = registry.get(&key).and_then(Weak::upgrade) {
            if self.terminated.load(Ordering::Acquire) {
                return Err(Error::from_reason("ZeroKernel is shut down"));
            }
            *slot = Some(Arc::clone(&kernel));
            self.ready.store(true, Ordering::Release);
            return Ok(kernel);
        }
        let kernel = CoreZeroKernel::canonical_with_tokenizer(
            &self.config.root,
            &self.config.state_root,
            &self.config.session_id,
            self.config.budget.clone(),
            self.config.tokenizer_model.clone(),
        )
        .map_err(|error| Error::from_reason(error.to_string()))?;
        let kernel = Arc::new(kernel);
        if self.terminated.load(Ordering::Acquire) {
            return Err(Error::from_reason("ZeroKernel is shut down"));
        }
        registry.retain(|_, entry| entry.strong_count() != 0);
        registry.insert(key, Arc::downgrade(&kernel));
        *slot = Some(Arc::clone(&kernel));
        self.ready.store(true, Ordering::Release);
        Ok(kernel)
    }

    fn request_shutdown(&self) {
        self.terminated.store(true, Ordering::Release);
        self.ready.store(false, Ordering::Release);
        for cancellation in self.active.lock().values() {
            cancellation.cancel();
        }
    }

    fn shutdown(&self) -> Result<()> {
        self.request_shutdown();
        let deadline = Instant::now() + Duration::from_millis(800);
        while self.inflight.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let remaining = self.inflight.load(Ordering::Acquire);
        if remaining != 0 {
            return Err(Error::from_reason(format!(
                "ZeroKernel shutdown timed out with {remaining} in-flight cells"
            )));
        }
        let Some(mut kernel) = self.kernel.try_lock_until(deadline) else {
            return Err(Error::from_reason(
                "ZeroKernel shutdown timed out waiting for initialization",
            ));
        };
        kernel.take();
        self.ready.store(false, Ordering::Release);
        Ok(())
    }
}

#[napi]
pub struct ZeroKernel {
    core: Arc<KernelCore>,
}

#[napi]
impl ZeroKernel {
    #[napi(constructor)]
    pub fn new(options: ZeroKernelOptions) -> Result<Self> {
        if options.root.trim().is_empty() {
            return Err(Error::from_reason("ZeroKernel root must not be empty"));
        }
        let root = options.root;
        let state_root = options
            .state_root
            .unwrap_or_else(|| format!("{root}/.zerostack"));
        let memory_bytes = options.memory_bytes.unwrap_or(DEFAULT_MEMORY_BYTES as i64);
        if memory_bytes <= 0 {
            return Err(Error::from_reason("memoryBytes must be positive"));
        }
        let budget = KernelBudget {
            wall_ms: u64::from(options.wall_ms.unwrap_or(DEFAULT_WALL_MS as u32)),
            cpu_ms: u64::from(options.cpu_ms.unwrap_or(DEFAULT_WALL_MS as u32)),
            memory_bytes: memory_bytes as u64,
            call_limit: options.call_limit.unwrap_or(64),
            task_limit: options.task_limit.unwrap_or(16),
            output_byte_limit: options.output_byte_limit.unwrap_or(DEFAULT_OUTPUT_BYTES),
        };
        budget
            .validate()
            .map_err(|error| Error::from_reason(error.to_string()))?;
        Ok(Self {
            core: Arc::new(KernelCore {
                config: KernelConfig {
                    root,
                    session_id: options
                        .session_id
                        .unwrap_or_else(|| "zero-kernel-session".into()),
                    state_root,
                    tokenizer_model: options.tokenizer_model,
                    budget,
                },
                kernel: Mutex::new(None),
                ready: AtomicBool::new(false),
                terminated: AtomicBool::new(false),
                inflight: AtomicUsize::new(0),
                completed: AtomicU64::new(0),
                next_task: AtomicU64::new(0),
                active: Mutex::new(BTreeMap::new()),
            }),
        })
    }

    #[napi]
    pub fn initialize(&self) -> AsyncTask<InitializeTask> {
        AsyncTask::new(InitializeTask {
            core: Arc::clone(&self.core),
        })
    }

    #[napi(js_name = "executeCell")]
    pub fn execute_cell(
        &self,
        source: String,
        signal: Option<AbortSignal>,
    ) -> Result<AsyncTask<ExecuteTask>> {
        let cancellation = AtomicCancellation::new();
        let task_id = self.core.next_task.fetch_add(1, Ordering::Relaxed) + 1;
        {
            let mut active = self.core.active.lock();
            if self.core.terminated.load(Ordering::Acquire) {
                return Err(Error::from_reason("ZeroKernel is shut down"));
            }
            self.core.inflight.fetch_add(1, Ordering::AcqRel);
            active.insert(task_id, cancellation.clone());
        }
        let task = ExecuteTask {
            core: Arc::clone(&self.core),
            task_id,
            source,
            cancellation: cancellation.clone(),
        };
        if let Some(signal) = signal.as_ref() {
            signal.on_abort(move || cancellation.cancel());
        }
        Ok(AsyncTask::new(task))
    }

    #[napi(js_name = "recordProviderUsage")]
    pub fn record_provider_usage(
        &self,
        event: String,
        observation_json: String,
    ) -> Result<AsyncTask<RecordProviderUsageTask>> {
        let kernel_event = ZeroHandle::parse(event.trim())
            .map_err(|error| Error::from_reason(error.to_string()))?;
        let observation = serde_json::from_str::<ProviderUsageObservation>(&observation_json)
            .map_err(|error| {
                Error::from_reason(format!("invalid provider usage observation: {error}"))
            })?;
        let active = self.core.active.lock();
        if self.core.terminated.load(Ordering::Acquire) {
            return Err(Error::from_reason("ZeroKernel is shut down"));
        }
        self.core.inflight.fetch_add(1, Ordering::AcqRel);
        drop(active);
        Ok(AsyncTask::new(RecordProviderUsageTask {
            core: Arc::clone(&self.core),
            kernel_event,
            observation,
        }))
    }
    #[napi(js_name = "registerSnapToFileCompleteness")]
    pub fn register_snap_to_file_completeness(
        &self,
        completeness_json: String,
    ) -> Result<AsyncTask<RegisterSnapCompletenessTask>> {
        let completeness = serde_json::from_str::<GraphZeroCompletenessInput>(&completeness_json)
            .map_err(|error| {
            Error::from_reason(format!("invalid GraphZero completeness input: {error}"))
        })?;
        let active = self.core.active.lock();
        if self.core.terminated.load(Ordering::Acquire) {
            return Err(Error::from_reason("ZeroKernel is shut down"));
        }
        self.core.inflight.fetch_add(1, Ordering::AcqRel);
        drop(active);
        Ok(AsyncTask::new(RegisterSnapCompletenessTask {
            core: Arc::clone(&self.core),
            completeness: Some(completeness),
        }))
    }

    #[napi]
    pub fn status(&self) -> ZeroKernelStatus {
        let (live_frames, live_tasks, live_processes) = self
            .core
            .kernel
            .try_lock()
            .and_then(|slot| {
                slot.as_ref().map(|kernel| {
                    (
                        kernel.live_frames(),
                        kernel.live_tasks(),
                        kernel.live_processes(),
                    )
                })
            })
            .unwrap_or((0, 0, 0));
        ZeroKernelStatus {
            runtime: "ZeroKernel".into(),
            ready: self.core.ready.load(Ordering::Acquire),
            terminated: self.core.terminated.load(Ordering::Acquire),
            inflight: u32::try_from(self.core.inflight.load(Ordering::Acquire)).unwrap_or(u32::MAX),
            completed: i64::try_from(self.core.completed.load(Ordering::Acquire))
                .unwrap_or(i64::MAX),
            live_frames: i64::try_from(live_frames).unwrap_or(i64::MAX),
            live_tasks: i64::try_from(live_tasks).unwrap_or(i64::MAX),
            live_processes: i64::try_from(live_processes).unwrap_or(i64::MAX),
        }
    }

    #[napi]
    pub fn shutdown(&self) -> AsyncTask<ShutdownTask> {
        AsyncTask::new(ShutdownTask {
            core: Arc::clone(&self.core),
        })
    }
}

impl Drop for ZeroKernel {
    fn drop(&mut self) {
        self.core.request_shutdown();
    }
}

#[napi(object)]
pub struct ZeroKernelStatus {
    pub runtime: String,
    pub ready: bool,
    pub terminated: bool,
    pub inflight: u32,
    pub completed: i64,
    pub live_frames: i64,
    pub live_tasks: i64,
    pub live_processes: i64,
}

pub struct JsJson(serde_json::Value);

impl TypeName for JsJson {
    fn type_name() -> &'static str {
        "Object"
    }

    fn value_type() -> ValueType {
        ValueType::Object
    }
}

impl ToNapiValue for JsJson {
    unsafe fn to_napi_value(
        env: napi::sys::napi_env,
        value: Self,
    ) -> Result<napi::sys::napi_value> {
        // SAFETY: napi's serde_json conversion constructs plain data only.
        unsafe { <&serde_json::Value as ToNapiValue>::to_napi_value(env, &value.0) }
    }
}

pub struct InitializeTask {
    core: Arc<KernelCore>,
}

impl Task for InitializeTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        catch_unwind(AssertUnwindSafe(|| self.core.initialize()))
            .map_err(|_| Error::from_reason("ZeroKernel initialization panicked"))??;
        Ok(())
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct ExecuteTask {
    core: Arc<KernelCore>,
    task_id: u64,
    source: String,
    cancellation: AtomicCancellation,
}

impl Task for ExecuteTask {
    type Output = ZeroKernelResponse;
    type JsValue = JsJson;

    fn compute(&mut self) -> Result<Self::Output> {
        let kernel = catch_unwind(AssertUnwindSafe(|| self.core.initialize()))
            .map_err(|_| Error::from_reason("ZeroKernel initialization panicked"))??;
        catch_unwind(AssertUnwindSafe(|| {
            kernel.execute_cell_with_cancellation(&self.source, self.cancellation.clone())
        }))
        .map_err(|_| Error::from_reason("ZeroKernel execution panicked"))?
        .map_err(|error| Error::from_reason(error.to_string()))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        self.core.completed.fetch_add(1, Ordering::Relaxed);
        serde_json::to_value(output)
            .map(JsJson)
            .map_err(|error| Error::from_reason(error.to_string()))
    }

    fn finally(self, _env: Env) -> Result<()> {
        self.core.active.lock().remove(&self.task_id);
        self.core.inflight.fetch_sub(1, Ordering::AcqRel);
        Ok(())
    }
}

pub struct RecordProviderUsageTask {
    core: Arc<KernelCore>,
    kernel_event: ZeroHandle,
    observation: ProviderUsageObservation,
}

impl Task for RecordProviderUsageTask {
    type Output = ProviderUsagePublication;
    type JsValue = JsJson;

    fn compute(&mut self) -> Result<Self::Output> {
        let kernel = catch_unwind(AssertUnwindSafe(|| self.core.initialize()))
            .map_err(|_| Error::from_reason("ZeroKernel initialization panicked"))??;
        catch_unwind(AssertUnwindSafe(|| {
            kernel.record_provider_usage(&self.kernel_event, self.observation.clone())
        }))
        .map_err(|_| Error::from_reason("ZeroKernel provider usage recording panicked"))?
        .map_err(|error| Error::from_reason(error.to_string()))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        serde_json::to_value(output)
            .map(JsJson)
            .map_err(|error| Error::from_reason(error.to_string()))
    }

    fn finally(self, _env: Env) -> Result<()> {
        self.core.inflight.fetch_sub(1, Ordering::AcqRel);
        Ok(())
    }
}
pub struct RegisterSnapCompletenessTask {
    core: Arc<KernelCore>,
    completeness: Option<GraphZeroCompletenessInput>,
}

impl Task for RegisterSnapCompletenessTask {
    type Output = ZeroHandle;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        let kernel = catch_unwind(AssertUnwindSafe(|| self.core.initialize()))
            .map_err(|_| Error::from_reason("ZeroKernel initialization panicked"))??;
        let completeness = self.completeness.take().ok_or_else(|| {
            Error::from_reason("ZeroKernel completeness registration already consumed")
        })?;
        catch_unwind(AssertUnwindSafe(|| {
            kernel.register_snap_to_file_completeness(completeness)
        }))
        .map_err(|_| Error::from_reason("ZeroKernel completeness registration panicked"))?
        .map_err(|error| Error::from_reason(error.to_string()))
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output.to_string())
    }

    fn finally(self, _env: Env) -> Result<()> {
        self.core.inflight.fetch_sub(1, Ordering::AcqRel);
        Ok(())
    }
}

pub struct ShutdownTask {
    core: Arc<KernelCore>,
}

impl Task for ShutdownTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        self.core.shutdown()
    }

    fn resolve(&mut self, _env: Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}
