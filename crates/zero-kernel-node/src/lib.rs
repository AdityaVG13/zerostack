use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use napi::bindgen_prelude::{AbortSignal, AsyncTask, ToNapiValue, TypeName, ValueType};
use napi::{Env, Error, Result, Task};
use napi_derive::napi;
use parking_lot::Mutex;
use zero_abi::{KernelBudget, ZeroKernelResponse};
use zero_kernel::{AtomicCancellation, ZeroKernel as CoreZeroKernel};

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
        let kernel = CoreZeroKernel::canonical_with_tokenizer(
            &self.config.root,
            &self.config.state_root,
            &self.config.session_id,
            self.config.budget.clone(),
            self.config.tokenizer_model.clone(),
        )
        .map_err(|error| Error::from_reason(error.to_string()))?;
        let kernel = Arc::new(kernel);
        *slot = Some(Arc::clone(&kernel));
        self.ready.store(true, Ordering::Release);
        Ok(kernel)
    }

    fn request_shutdown(&self) {
        self.terminated.store(true, Ordering::Release);
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
        self.ready.store(false, Ordering::Release);
        self.kernel.lock().take();
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

    #[napi]
    pub fn status(&self) -> ZeroKernelStatus {
        let (live_frames, live_tasks, live_processes) = self
            .core
            .kernel
            .lock()
            .as_ref()
            .map(|kernel| {
                (
                    kernel.live_frames(),
                    kernel.live_tasks(),
                    kernel.live_processes(),
                )
            })
            .unwrap_or((0, 0, 0));
        ZeroKernelStatus {
            runtime: "ZeroKernel".into(),
            ready: self.core.ready.load(Ordering::Acquire),
            terminated: self.core.terminated.load(Ordering::Acquire),
            inflight: self.core.inflight.load(Ordering::Acquire) as u32,
            completed: self.core.completed.load(Ordering::Acquire) as i64,
            live_frames: live_frames as i64,
            live_tasks: live_tasks as i64,
            live_processes: live_processes as i64,
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
