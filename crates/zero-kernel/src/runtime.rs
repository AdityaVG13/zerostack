use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use zero_abi::{
    AsgrepMode, AsgrepOptions, CapabilityDescriptor, CompressionRequest, EffectRequest,
    EngineError, EngineErrorKind, ExpandOptions, GUEST_METHODS, GlobalRegistration, LookupOptions,
    ProjectionRequest, ReadOptions, ShellOptions, SnapRequest, SnapTargetRequest, SnapViewRequest,
    ZERO_KERNEL_PROTOCOL, ZeroHandle, ZeroKernelResponse,
};
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, GuestContext, GuestSurface,
    Host, HostLimits,
};

use crate::host::{Cell, ZeroKernel};
use crate::shell::ShellCommand;
use crate::typescript::{TypeScriptError, erase_typescript};

const INTERNAL_BEGIN: &str = "__begin_transaction";
const INTERNAL_COMMIT: &str = "__commit_transaction";
const INTERNAL_ROLLBACK: &str = "__rollback_transaction";
const FRAME_SETTLE_GRACE: Duration = Duration::from_millis(1_500);

struct CellConnector {
    cell: Rc<RefCell<Option<Cell>>>,
}

impl Connector for CellConnector {
    fn dispatch(
        &self,
        capability: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
        completion: ConnectorCompletion,
    ) -> Result<(), ConnectorError> {
        if capability.surface != "z" {
            return Err(ConnectorError::new(
                "ZeroKernel connector accepts only direct z methods",
            ));
        }
        let args: Value = serde_json::from_str(args_json)
            .map_err(|error| ConnectorError::new(error.to_string()))?;
        if matches!(
            capability.method.as_str(),
            "read" | "lookup" | "asgrep" | "find" | "shell" | "run" | "measure" | "project" | "compress" | "expand"
        ) {
            let readonly = {
                let slot = self.cell.borrow();
                slot.as_ref()
                    .ok_or_else(|| ConnectorError::new("ZeroKernel cell already settled"))?
                    .direct_context()
            };
            return spawn_concurrent(
                readonly,
                capability.method.clone(),
                args,
                context.max_json_bytes,
                completion,
            );
        }
        let result = {
            let mut slot = self.cell.borrow_mut();
            let cell = slot
                .as_mut()
                .ok_or_else(|| ConnectorError::new("ZeroKernel cell already settled"))?;
            dispatch_direct(cell, &capability.method, args)
        };
        let encoded = result.and_then(|value| {
            serde_json::to_string(&value).map_err(|error| ConnectorError::new(error.to_string()))
        });
        if let Ok(encoded) = &encoded
            && encoded.len() > context.max_json_bytes
        {
            return completion.complete(Err(ConnectorError::new(
                "direct z result exceeds interpreter JSON budget",
            )));
        }
        completion.complete(encoded)
    }
}

fn spawn_concurrent(
    context: crate::host::DirectCallContext,
    method: String,
    args: Value,
    max_json_bytes: usize,
    completion: ConnectorCompletion,
) -> Result<(), ConnectorError> {
    std::thread::Builder::new()
        .name(format!("zero-kernel-{method}"))
        .spawn(move || {
            let result = dispatch_concurrent(&context, &method, args).and_then(|value| {
                serde_json::to_string(&value)
                    .map_err(|error| ConnectorError::new(error.to_string()))
            });
            let result = match result {
                Ok(encoded) if encoded.len() > max_json_bytes => Err(ConnectorError::new(
                    "direct z result exceeds interpreter JSON budget",
                )),
                other => other,
            };
            let _ = completion.complete(result);
        })
        .map(|_| ())
        .map_err(|error| ConnectorError::new(format!("spawn direct z task: {error}")))
}

fn dispatch_concurrent(
    context: &crate::host::DirectCallContext,
    method: &str,
    args: Value,
) -> Result<Value, ConnectorError> {
    let positional = positional(args);
    match method {
        "read" => {
            let path = string_arg(&positional, 0, "z.read target")?;
            let path_buf = PathBuf::from(&path);
            let full_path = if path_buf.is_absolute() {
                path_buf.clone()
            } else {
                context.project_root().join(&path_buf)
            };
            if full_path.is_dir() {
                let options = positional
                    .get(1)
                    .cloned()
                    .map(normalize_keys)
                    .map(serde_json::from_value::<LookupOptions>)
                    .transpose()
                    .map_err(json_error)?
                    .unwrap_or_default();
                let paths = context.lookup(path_buf, options).map_err(host_error)?;
                Ok(Value::Array(
                    paths
                        .into_iter()
                        .map(|p| Value::String(p.to_string_lossy().into_owned()))
                        .collect(),
                ))
            } else {
                let options = positional
                    .get(1)
                    .cloned()
                    .map(normalize_keys)
                    .map(serde_json::from_value::<ReadOptions>)
                    .transpose()
                    .map_err(json_error)?
                    .unwrap_or_default();
                context
                    .read(path_buf, options)
                    .map(Value::String)
                    .map_err(host_error)
            }
        }
        "lookup" => {
            let path = positional.first().and_then(Value::as_str).unwrap_or(".");
            let options = positional
                .get(1)
                .cloned()
                .map(normalize_keys)
                .map(serde_json::from_value::<LookupOptions>)
                .transpose()
                .map_err(json_error)?
                .unwrap_or_default();
            let paths = context
                .lookup(PathBuf::from(path), options)
                .map_err(host_error)?;
            Ok(Value::Array(
                paths
                    .into_iter()
                    .map(|path| Value::String(path.to_string_lossy().into_owned()))
                    .collect(),
            ))
        }
        "asgrep" | "find" => {
            // Accept both calling conventions: z.asgrep(query, {options}) and
            // the single-object form z.asgrep({query, path, ...}) that new
            // agents naturally try first (pc_cf4c50f47270).
            let (query, options_source): (Value, Option<Value>) =
                match positional.first() {
                    Some(Value::Object(first)) if positional.len() == 1 => {
                        let mut object = first.clone();
                        let query = object
                            .remove("query")
                            .ok_or_else(|| {
                                ConnectorError::new(
                                    "z.asgrep object form requires a query field",
                                )
                            })?;
                        (query, Some(Value::Object(object)))
                    }
                    _ => (
                        Value::String(string_arg(&positional, 0, "z.asgrep query")?),
                        positional.get(1).cloned(),
                    ),
                };
            let query = query
                .as_str()
                .ok_or_else(|| ConnectorError::new("z.asgrep query must be a string"))?
                .to_owned();
            let options = options_source
                .map(normalize_keys)
                .map(serde_json::from_value::<AsgrepOptions>)
                .transpose()
                .map_err(json_error)?
                .unwrap_or_else(default_asgrep_options);
            serde_json::to_value(context.asgrep(query, options).map_err(host_error)?)
                .map_err(json_error)
        }
        "shell" | "run" => {
            let (command, options) = shell_arguments(&positional)?;
            serde_json::to_value(context.shell(command, options).map_err(host_error)?)
                .map_err(json_error)
        }
        "measure" => {
            let bytes = value_bytes(positional.first(), "z.measure value")?;
            serde_json::to_value(context.measure(bytes).map_err(host_error)?).map_err(json_error)
        }
        "project" => {
            let bytes = value_bytes(positional.first(), "z.project value")?;
            let options = positional.get(1).and_then(Value::as_object);
            let visible_byte_limit = option_u32(options, "visibleBytes", "visible_bytes")?
                .unwrap_or_else(|| context.output_byte_limit());
            let media_type = option_string(options, "mediaType", "media_type")
                .unwrap_or_else(|| "text/plain".into());
            serde_json::to_value(
                context
                    .project(ProjectionRequest {
                        bytes,
                        visible_byte_limit,
                        media_type,
                    })
                    .map_err(host_error)?,
            )
            .map_err(json_error)
        }
        "compress" => {
            let bytes = value_bytes(positional.first(), "z.compress value")?;
            let options = positional.get(1).and_then(Value::as_object);
            let max_tokens = option_u32(options, "maxTokens", "max_tokens")?.unwrap_or(1_024);
            let mode = option_string(options, "mode", "mode").unwrap_or_default();
            let label = option_string(options, "label", "label");
            let media_type = option_string(options, "mediaType", "media_type")
                .unwrap_or_else(|| "text/plain".into());
            serde_json::to_value(
                context
                    .compress(CompressionRequest {
                        bytes,
                        max_tokens,
                        mode,
                        label,
                        media_type,
                    })
                    .map_err(host_error)?,
            )
            .map_err(json_error)
        }
        "expand" => {
            let handle = expand_handle_arg(positional.first(), "z.expand handle")?;
            let options = expand_options(positional.get(1))?;
            serde_json::to_value(context.expand(&handle, options).map_err(host_error)?)
                .map_err(json_error)
        }
        _ => Err(ConnectorError::new("not a read-only direct method")),
    }
}

impl ZeroKernel {
    /// Execute one TypeScript/JavaScript cell in a fresh bounded frame.
    pub fn execute_cell(&self, source: &str) -> Result<ZeroKernelResponse, crate::HostError> {
        self.execute_cell_with_cancellation(source, crate::AtomicCancellation::new())
    }

    pub fn execute_cell_with_cancellation(
        &self,
        source: &str,
        cancellation: crate::AtomicCancellation,
    ) -> Result<ZeroKernelResponse, crate::HostError> {
        let cell = self.begin_cell_with_cancellation(source, cancellation.clone())?;
        let erased = match erase_typescript(source) {
            Ok(erased) => erased,
            Err(error) => return cell.fail(type_engine_error(error)),
        };
        let context = cell.context().clone();
        let budget = cell.budget().clone();
        let guest = Arc::new(GuestSurface::new(
            GuestContext {
                project_root: context.project_root.to_string_lossy().into_owned(),
                workspace_root: Some(context.workspace_root.to_string_lossy().into_owned()),
                request_root: Some(context.project_root.to_string_lossy().into_owned()),
                session_root: context.expected_state_root.clone(),
                session_id: context.session_id.clone(),
                protocol: ZERO_KERNEL_PROTOCOL.into(),
            },
            zero_abi::PARALLEL_TASK_LIMIT,
        ));
        if let Err(error) = guest.state_hydrate(cell.state_values()) {
            return cell.fail(EngineError::new(
                EngineErrorKind::InvalidInput,
                error,
                false,
            ));
        }
        let registration = GlobalRegistration {
            root: "z".into(),
            capabilities: direct_capabilities(),
        };
        let limits = match host_limits(&budget) {
            Ok(limits) => limits,
            Err(error) => {
                return cell.fail(EngineError::new(
                    EngineErrorKind::Internal,
                    error.to_string(),
                    false,
                ));
            }
        };
        let host = match Host::new_zero_kernel(limits, registration) {
            Ok(host) => host.with_guest_surface(Arc::clone(&guest)),
            Err(error) => {
                return cell.fail(EngineError::new(
                    EngineErrorKind::Internal,
                    error.to_string(),
                    false,
                ));
            }
        };
        let slot = Rc::new(RefCell::new(Some(cell)));
        let connector: Rc<dyn Connector> = Rc::new(CellConnector {
            cell: Rc::clone(&slot),
        });
        let outcome = host.execute_measured_with_cancel_timeout(
            &erased,
            connector,
            cancellation.flag(),
            Duration::from_millis(budget.wall_ms),
        );
        if outcome.result.is_err() {
            cancellation.cancel();
        }
        let quiescence = {
            let slot = slot.borrow();
            slot.as_ref()
                .ok_or_else(|| crate::HostError::InvalidRequest("cell ownership lost".into()))?
                .wait_for_quiescence(FRAME_SETTLE_GRACE)
        };
        let mut cell = slot
            .borrow_mut()
            .take()
            .ok_or_else(|| crate::HostError::InvalidRequest("cell ownership lost".into()))?;
        cell.record_runtime_metrics(
            outcome.metrics.wall_time_ns,
            outcome.metrics.connector_dispatches,
            outcome.metrics.peak_inflight_connector_calls as u64,
        );
        cell.record_operations(outcome.operations, outcome.operations_truncated);
        if let Err(error) = quiescence {
            let kind = if cancellation.is_cancelled() {
                EngineErrorKind::Cancelled
            } else {
                EngineErrorKind::Deadline
            };
            return cell.fail(EngineError::new(kind, error.to_string(), false));
        }
        match outcome.result {
            Ok(value) => {
                cell.replace_state(guest.state_snapshot());
                cell.finish(value)
            }
            Err(error) => cell.fail(map_interpreter_error(error)),
        }
    }
}

fn direct_capabilities() -> Vec<CapabilityDescriptor> {
    let mut methods = GUEST_METHODS
        .iter()
        .copied()
        .filter(|method| {
            !method.starts_with("state.")
                && !matches!(
                    *method,
                    "help" | "inspect" | "parallel" | "pipeline" | "transact"
                )
        })
        .map(|method| CapabilityDescriptor::new("z", method))
        .collect::<Vec<_>>();
    methods.extend([
        CapabilityDescriptor::new("z", INTERNAL_BEGIN),
        CapabilityDescriptor::new("z", INTERNAL_COMMIT),
        CapabilityDescriptor::new("z", INTERNAL_ROLLBACK),
    ]);
    methods
}

fn host_limits(budget: &zero_abi::KernelBudget) -> Result<HostLimits, crate::HostError> {
    HostLimits::new(
        usize::try_from(budget.memory_bytes).unwrap_or(usize::MAX),
        1024 * 1024,
        Duration::from_millis(budget.wall_ms),
        budget.cpu_ms.saturating_mul(10_000).max(1),
        4_096,
        (budget.task_limit as usize).min(zero_codemode::MAX_INFLIGHT_CONNECTOR_CALLS),
        u64::from(budget.call_limit),
        zero_abi::SOURCE_BYTE_LIMIT,
        usize::try_from(budget.memory_bytes).unwrap_or(usize::MAX),
    )
    .map_err(|error| crate::HostError::InvalidRequest(error.to_string()))
}

fn dispatch_direct(cell: &mut Cell, method: &str, args: Value) -> Result<Value, ConnectorError> {
    let positional = positional(args);
    match method {
        "read" => {
            let path = string_arg(&positional, 0, "z.read path")?;
            let options = positional
                .get(1)
                .cloned()
                .map(normalize_keys)
                .map(serde_json::from_value::<ReadOptions>)
                .transpose()
                .map_err(json_error)?
                .unwrap_or_default();
            cell.read(path, options)
                .map(Value::String)
                .map_err(host_error)
        }
        "snap" => {
            let request = snap_request(&positional)?;
            serde_json::to_value(cell.snap(request).map_err(host_error)?).map_err(json_error)
        }
        "write" => {
            let path = string_arg(&positional, 0, "z.write path")?;
            let content = string_arg(&positional, 1, "z.write content")?;
            let expected = expected_preimage(positional.get(2))?;
            serde_json::to_value(
                cell.write(path, content.into_bytes(), expected)
                    .map_err(host_error)?,
            )
            .map_err(json_error)
        }
        "edit" => {
            let target = positional
                .first()
                .ok_or_else(|| ConnectorError::new("z.edit target is required"))?;
            let (path, expected) = edit_target(target, positional.get(2))?;
            let patch = positional
                .get(1)
                .cloned()
                .ok_or_else(|| ConnectorError::new("z.edit patch is required"))?;

            // Shape inference: create / remove / substitute / replace_file
            if let Some(values) = patch.as_object() {
                // {remove: true} -> delete file
                if values.get("remove").and_then(Value::as_bool).unwrap_or(false) {
                    serde_json::to_value(cell.remove(path, expected).map_err(host_error)?)
                        .map_err(json_error)
                }
                // Only the explicit create key bypasses read/preimage lookup.
                // Typed patches carrying content (replace_file/replace_lines/
                // insert_before/after) MUST stay on the verified edit path.
                else if let Some(content) = values.get("create").and_then(Value::as_str) {
                    serde_json::to_value(
                        cell.write(path, content.as_bytes().to_vec(), None).map_err(host_error)?,
                    )
                    .map_err(json_error)
                }
                else {
                    // Fall through to standard edit path below
                    let source = cell.read_exact(&path).map_err(host_error)?;
                    let (postimage, patch_text) = apply_edit_patch(&source, target, patch)?;
                    serde_json::to_value(
                        cell.edit(path, postimage, patch_text, expected).map_err(host_error)?,
                    )
                    .map_err(json_error)
                }
            }
            else if patch.is_string() {
                // Whole-file replacement via string: allowed when file doesn't exist yet
                let exists = cell.read_exact(&path).is_ok();
                if exists {
                    return Err(ConnectorError::new(
                        "z.edit refuses a bare replacement string on an existing file (it would replace everything). Use {find, replacement} to substitute, {kind: 'replace_file', content} to overwrite deliberately, or z.write(path, content) to create.",
                    ));
                }
                serde_json::to_value(
                    cell.write(path, patch.as_str().unwrap_or("").as_bytes().to_vec(), None)
                        .map_err(host_error)?,
                )
                .map_err(json_error)
            }
            else {
                return Err(ConnectorError::new(
                    "z.edit accepts {find, replacement}, {create: content}, {remove: true}, or {kind: 'replace_file', content}",
                ));
            }
        }
        "apply" => {
            let request = simplified_apply_request(
                positional
                    .first()
                    .cloned()
                    .ok_or_else(|| ConnectorError::new("z.apply operations are required"))?,
                positional.get(1).cloned(),
            )?;
            serde_json::to_value(cell.effect(request).map_err(host_error)?).map_err(json_error)
        }
        "effect" => {
            let request = positional
                .first()
                .cloned()
                .ok_or_else(|| ConnectorError::new("z.effect request is required"))
                .and_then(|value| {
                    serde_json::from_value::<EffectRequest>(value).map_err(json_error)
                })?;
            serde_json::to_value(cell.effect(request).map_err(host_error)?).map_err(json_error)
        }
        "remove" => {
            let path = string_arg(&positional, 0, "z.remove path")?;
            let expected = expected_preimage(positional.get(1))?;
            serde_json::to_value(cell.remove(path, expected).map_err(host_error)?)
                .map_err(json_error)
        }
        "lookup" => {
            let path = positional.first().and_then(Value::as_str).unwrap_or(".");
            let options = positional
                .get(1)
                .cloned()
                .map(normalize_keys)
                .map(serde_json::from_value::<LookupOptions>)
                .transpose()
                .map_err(json_error)?
                .unwrap_or_default();
            let paths = cell.lookup(path, options).map_err(host_error)?;
            Ok(Value::Array(
                paths
                    .into_iter()
                    .map(|path| Value::String(path.to_string_lossy().into_owned()))
                    .collect(),
            ))
        }
        "asgrep" => {
            let query = string_arg(&positional, 0, "z.asgrep query")?;
            let options = positional
                .get(1)
                .cloned()
                .map(normalize_keys)
                .map(serde_json::from_value::<AsgrepOptions>)
                .transpose()
                .map_err(json_error)?
                .unwrap_or_else(default_asgrep_options);
            serde_json::to_value(cell.asgrep(query, options).map_err(host_error)?)
                .map_err(json_error)
        }
        "shell" => {
            let command = match positional.first() {
                Some(Value::String(command)) => ShellCommand::Script(command.clone()),
                Some(Value::Array(values)) => ShellCommand::Argv(
                    values
                        .iter()
                        .map(|value| {
                            value.as_str().map(str::to_owned).ok_or_else(|| {
                                ConnectorError::new("z.shell argv values must be strings")
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                _ => {
                    return Err(ConnectorError::new(
                        "z.shell expects a string or string array",
                    ));
                }
            };
            let options = positional
                .get(1)
                .cloned()
                .map(normalize_keys)
                .map(serde_json::from_value::<ShellOptions>)
                .transpose()
                .map_err(json_error)?
                .unwrap_or_default();
            serde_json::to_value(cell.shell(command, options).map_err(host_error)?)
                .map_err(json_error)
        }
        "expand" => {
            let handle = expand_handle_arg(positional.first(), "z.expand handle")?;
            let options = expand_options(positional.get(1))?;
            serde_json::to_value(cell.expand(&handle, options).map_err(host_error)?)
                .map_err(json_error)
        }
        INTERNAL_BEGIN => {
            cell.begin_transaction().map_err(host_error)?;
            Ok(Value::Null)
        }
        // A successful z.transact scope remains staged until the cell's one
        // terminal commit. This prevents a later frame failure from publishing
        // effects that the model can no longer observe.
        INTERNAL_COMMIT => Ok(Value::Null),
        INTERNAL_ROLLBACK => {
            cell.rollback_transaction().map_err(host_error)?;
            Ok(Value::Null)
        }
        _ => Err(ConnectorError::new(format!(
            "unknown direct ZeroKernel method z.{method}"
        ))),
    }
}

fn value_bytes(value: Option<&Value>, label: &str) -> Result<Vec<u8>, ConnectorError> {
    match value {
        Some(Value::String(text)) => Ok(text.as_bytes().to_vec()),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_u64()
                    .and_then(|num| u8::try_from(num).ok())
                    .ok_or_else(|| {
                        ConnectorError::new(format!(
                            "{label} must be a string or an array of bytes (integers 0..=255)"
                        ))
                    })
            })
            .collect(),
        Some(_) => Err(ConnectorError::new(format!(
            "{label} must be a string or an array of bytes (integers 0..=255)"
        ))),
        None => Err(ConnectorError::new(format!("{label} is required"))),
    }
}

fn option_string(options: Option<&Map<String, Value>>, camel: &str, snake: &str) -> Option<String> {
    options
        .and_then(|options| options.get(camel).or_else(|| options.get(snake)))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn option_u32(
    options: Option<&Map<String, Value>>,
    camel: &str,
    snake: &str,
) -> Result<Option<u32>, ConnectorError> {
    let Some(value) = options.and_then(|options| options.get(camel).or_else(|| options.get(snake)))
    else {
        return Ok(None);
    };
    let value = value
        .as_u64()
        .ok_or_else(|| ConnectorError::new(format!("{camel} must be a positive integer")))?;
    let value =
        u32::try_from(value).map_err(|_| ConnectorError::new(format!("{camel} exceeds u32")))?;
    if value == 0 {
        return Err(ConnectorError::new(format!("{camel} must be positive")));
    }
    Ok(Some(value))
}

fn shell_arguments(positional: &[Value]) -> Result<(ShellCommand, ShellOptions), ConnectorError> {
    let command = match positional.first() {
        Some(Value::String(command)) => ShellCommand::Script(command.clone()),
        Some(Value::Array(values)) => ShellCommand::Argv(
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| ConnectorError::new("z.shell argv values must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        _ => {
            return Err(ConnectorError::new(
                "z.shell expects a string or string array",
            ));
        }
    };
    let options = positional
        .get(1)
        .cloned()
        .map(normalize_keys)
        .map(serde_json::from_value::<ShellOptions>)
        .transpose()
        .map_err(json_error)?
        .unwrap_or_default();
    Ok((command, options))
}

fn positional(args: Value) -> Vec<Value> {
    match args {
        Value::Array(values) => values,
        Value::Null => Vec::new(),
        value => vec![value],
    }
}

fn string_arg(args: &[Value], index: usize, label: &str) -> Result<String, ConnectorError> {
    args.get(index)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ConnectorError::new(format!("{label} must be a string")))
}

fn snap_request(args: &[Value]) -> Result<SnapRequest, ConnectorError> {
    let value = args
        .first()
        .cloned()
        .ok_or_else(|| ConnectorError::new("z.snap target is required"))?;
    match value {
        Value::String(path) => Ok(SnapRequest {
            target: SnapTargetRequest::Path {
                path: PathBuf::from(path),
            },
            cardinality: None,
            selection: None,
            view: SnapViewRequest::default(),
        }),
        Value::Object(values) if values.contains_key("target") => {
            serde_json::from_value(Value::Object(values)).map_err(json_error)
        }
        Value::Object(mut values) => {
            let path = values
                .remove("path")
                .and_then(|value| value.as_str().map(str::to_owned))
                .ok_or_else(|| {
                    ConnectorError::new(
                        "z.snap expects a path string or {target:{path|search}, ...}",
                    )
                })?;
            let mut target = Map::new();
            target.insert("path".into(), Value::String(path));
            values.insert("target".into(), Value::Object(target));
            serde_json::from_value(Value::Object(values)).map_err(json_error)
        }
        _ => Err(ConnectorError::new(
            "z.snap expects a path string or request object",
        )),
    }
}

fn edit_target(
    target: &Value,
    options: Option<&Value>,
) -> Result<(String, Option<ZeroHandle>), ConnectorError> {
    let explicit = expected_preimage(options)?;
    match target {
        Value::String(path) => Ok((path.clone(), explicit)),
        Value::Object(values) => {
            let path = values
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| ConnectorError::new("snap-aware z.edit target has no path"))?;
            let snap = values
                .get("source")
                .and_then(Value::as_object)
                .and_then(|source| source.get("exact"))
                .or_else(|| {
                    values
                        .get("recovery")
                        .and_then(Value::as_object)
                        .and_then(|recovery| recovery.get("exact"))
                })
                .and_then(Value::as_str)
                .map(ZeroHandle::parse)
                .transpose()
                .map_err(|error| ConnectorError::new(error.to_string()))?;
            if explicit.is_some() && snap.is_some() && explicit != snap {
                return Err(ConnectorError::new(
                    "z.edit explicit preimage does not match snap source",
                ));
            }
            Ok((path.to_owned(), explicit.or(snap)))
        }
        _ => Err(ConnectorError::new(
            "z.edit target must be a path string or SnapResult",
        )),
    }
}


/// Compile the final ergonomic z.apply array into the strict EffectRequest IR.
/// The model supplies flat path-local operations; target indirection and effect
/// kinds stay internal to the kernel.
fn simplified_apply_request(
    operations: Value,
    verify: Option<Value>,
) -> Result<EffectRequest, ConnectorError> {
    let ops = operations.as_array().ok_or_else(|| {
        ConnectorError::new(
            "z.apply expects an array, e.g. [{path: 'a.rs', edit: {find: 'old', replacement: 'new'}}]",
        )
    })?;
    if ops.is_empty() {
        return Err(ConnectorError::new("z.apply requires at least one operation"));
    }

    let mut targets = Map::new();
    let mut changes = Vec::with_capacity(ops.len());
    for (index, operation) in ops.iter().enumerate() {
        let op = operation.as_object().ok_or_else(|| {
            ConnectorError::new(format!(
                "z.apply operation {index} must be an object with path + edit/create/remove"
            ))
        })?;
        let path = op.get("path").and_then(Value::as_str).ok_or_else(|| {
            ConnectorError::new(format!("z.apply operation {index} requires a string path"))
        })?;
        let target = format!("t{index}");

        let (expect, change) = if let Some(content) = op.get("create").and_then(Value::as_str) {
            (
                "absent",
                serde_json::json!({
                    "target": target,
                    "kind": "create_file",
                    "content": content,
                }),
            )
        } else if op.get("remove").and_then(Value::as_bool) == Some(true) {
            (
                "exists",
                serde_json::json!({"target": target, "kind": "remove_file"}),
            )
        } else if let Some(content) = op.get("replace").and_then(Value::as_str) {
            (
                "exists",
                serde_json::json!({
                    "target": target,
                    "kind": "replace_file",
                    "content": content,
                }),
            )
        } else if let Some(edit) = op.get("edit").and_then(Value::as_object) {
            let old = edit
                .get("find")
                .or_else(|| edit.get("old"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ConnectorError::new(format!(
                        "z.apply operation {index} edit requires find + replacement"
                    ))
                })?;
            let replacement = edit
                .get("replacement")
                .or_else(|| edit.get("replace"))
                .or_else(|| edit.get("new"))
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ConnectorError::new(format!(
                        "z.apply operation {index} edit requires replacement"
                    ))
                })?;
            (
                "exists",
                serde_json::json!({
                    "target": target,
                    "kind": "replace_exact",
                    "old": old,
                    "replacement": replacement,
                    "expectedCount": 1,
                }),
            )
        } else if let (Some(old), Some(replacement)) = (
            op.get("find").or_else(|| op.get("old")).and_then(Value::as_str),
            op.get("replacement").or_else(|| op.get("new")).and_then(Value::as_str),
        ) {
            (
                "exists",
                serde_json::json!({
                    "target": target,
                    "kind": "replace_exact",
                    "old": old,
                    "replacement": replacement,
                    "expectedCount": 1,
                }),
            )
        } else if let (Some(after), Some(content)) = (
            op.get("after").and_then(Value::as_str),
            op.get("content").and_then(Value::as_str),
        ) {
            (
                "exists",
                serde_json::json!({
                    "target": target,
                    "kind": "insert_after",
                    "anchor": {"exactText": after},
                    "content": content,
                }),
            )
        } else if let (Some(before), Some(content)) = (
            op.get("before").and_then(Value::as_str),
            op.get("content").and_then(Value::as_str),
        ) {
            (
                "exists",
                serde_json::json!({
                    "target": target,
                    "kind": "insert_before",
                    "anchor": {"exactText": before},
                    "content": content,
                }),
            )
        } else {
            return Err(ConnectorError::new(format!(
                "z.apply operation {index} must use one of: edit, create, replace, remove, before+content, after+content"
            )));
        };

        targets.insert(
            target.clone(),
            serde_json::json!({"path": path, "expect": expect}),
        );
        changes.push(change);
    }

    let verify = verify.unwrap_or_else(|| {
        serde_json::json!({"changedTargetsOnly": true, "parse": false})
    });
    serde_json::from_value::<EffectRequest>(serde_json::json!({
        "targets": targets,
        "changes": changes,
        "verify": verify,
    }))
    .map_err(json_error)
}

fn apply_edit_patch(
    source: &[u8],
    target: &Value,
    patch: Value,
) -> Result<(Vec<u8>, Option<String>), ConnectorError> {
    let selection = validate_snap_selection(target, source)?;
    let Some(values) = patch.as_object() else {
        // pc_b1050fe2d6fd: a bare replacement string on a path target used to
        // replace the ENTIRE file silently. Whole-file replacement stays
        // available only as a deliberate typed operation.
        return Err(ConnectorError::new(
            "z.edit refuses a bare replacement string (it would replace the entire file): use {find, replacement} for substitution, {kind: 'replace_file', content} to replace a whole file deliberately, or z.write to overwrite explicitly",
        ));
    };
    let patch_text = Some(patch.to_string());
    let kind = values.get("kind").and_then(Value::as_str);
    match kind {
        None => {
            if let Some(range) = selection {
                let find = patch_find(values)?;
                if &source[range] != find.as_bytes() {
                    return Err(ConnectorError::new(
                        "selection_scope_mismatch: patch find must equal the snapped selection",
                    ));
                }
            }
            apply_patch(source, patch)
        }
        Some("replace_exact") => {
            if values.get("expectedCount").and_then(Value::as_u64) != Some(1) {
                return Err(ConnectorError::new(
                    "z.edit replace_exact requires expectedCount: 1",
                ));
            }
            if let Some(range) = selection {
                let old = patch_find(values)?;
                if &source[range] != old.as_bytes() {
                    return Err(ConnectorError::new(
                        "selection_scope_mismatch: replace_exact old must equal the snapped selection",
                    ));
                }
            }
            let old = patch_find(values)?;
            let replacement = patch_replacement(values)?;
            replace_exact(source, old, replacement).map(|postimage| (postimage, patch_text))
        }
        Some("replace_lines") => {
            let range = selection.ok_or_else(|| {
                ConnectorError::new("z.edit replace_lines requires a snap selection")
            })?;
            if snap_selection_kind(target) != Some("lines") {
                return Err(ConnectorError::new(
                    "z.edit replace_lines requires a line selection",
                ));
            }
            let content = patch_content(values, "replace_lines")?;
            Ok((replace_range(source, range, content.as_bytes()), patch_text))
        }
        Some("insert_before") | Some("insert_after") => {
            let range = selection
                .ok_or_else(|| ConnectorError::new("z.edit insertion requires a snap selection"))?;
            let content = patch_content(values, kind.unwrap_or("insertion"))?;
            let offset = if kind == Some("insert_before") {
                range.start
            } else {
                range.end
            };
            Ok((
                replace_range(source, offset..offset, content.as_bytes()),
                patch_text,
            ))
        }
        Some("replace_file") => {
            if selection.is_some() {
                return Err(ConnectorError::new(
                    "selection_scope_mismatch: replace_file requires an unselected snap",
                ));
            }
            Ok((
                patch_content(values, "replace_file")?.as_bytes().to_vec(),
                patch_text,
            ))
        }
        Some(other) => Err(ConnectorError::new(format!(
            "unsupported z.edit patch kind {other:?}"
        ))),
    }
}

fn validate_snap_selection(
    target: &Value,
    source: &[u8],
) -> Result<Option<std::ops::Range<usize>>, ConnectorError> {
    let Some(selection) = target
        .as_object()
        .and_then(|target| target.get("selection"))
        .and_then(Value::as_object)
    else {
        return Ok(None);
    };
    let start = selection
        .get("byteStart")
        .or_else(|| selection.get("byte_start"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| ConnectorError::new("z.edit snap selection has no valid byteStart"))?;
    let end = selection
        .get("byteEnd")
        .or_else(|| selection.get("byte_end"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| ConnectorError::new("z.edit snap selection has no valid byteEnd"))?;
    let expected = selection
        .get("selectedDigest")
        .or_else(|| selection.get("selected_digest"))
        .and_then(Value::as_str)
        .ok_or_else(|| ConnectorError::new("z.edit snap selection has no selectedDigest"))?;
    if start >= end || end > source.len() {
        return Err(ConnectorError::new(
            "selection_changed: snap selection is outside the current source",
        ));
    }
    if blake3::hash(&source[start..end]).to_hex().as_str() != expected {
        return Err(ConnectorError::new(
            "selection_changed: selected bytes do not match the snap digest",
        ));
    }
    Ok(Some(start..end))
}

fn snap_selection_kind(target: &Value) -> Option<&str> {
    target
        .as_object()?
        .get("selection")?
        .as_object()?
        .get("kind")?
        .as_str()
}

fn patch_find(values: &Map<String, Value>) -> Result<&str, ConnectorError> {
    values
        .get("find")
        .or_else(|| values.get("old"))
        .or_else(|| values.get("pattern"))
        .and_then(Value::as_str)
        .ok_or_else(|| ConnectorError::new("z.edit patch requires find or old"))
}

fn patch_replacement(values: &Map<String, Value>) -> Result<&str, ConnectorError> {
    values
        .get("replace")
        .or_else(|| values.get("new"))
        .or_else(|| values.get("replacement"))
        .and_then(Value::as_str)
        .ok_or_else(|| ConnectorError::new("z.edit patch requires replacement"))
}

fn patch_content<'a>(
    values: &'a Map<String, Value>,
    kind: &str,
) -> Result<&'a str, ConnectorError> {
    values
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| ConnectorError::new(format!("z.edit {kind} requires content")))
}

fn replace_exact(source: &[u8], old: &str, replacement: &str) -> Result<Vec<u8>, ConnectorError> {
    let text = std::str::from_utf8(source)
        .map_err(|_| ConnectorError::new("z.edit target is not UTF-8"))?;
    let mut matches = text.match_indices(old);
    let first = matches
        .next()
        .ok_or_else(|| ConnectorError::new("z.edit patch did not match"))?;
    if matches.next().is_some() {
        return Err(ConnectorError::new("z.edit patch is ambiguous"));
    }
    Ok(replace_range(
        source,
        first.0..first.0 + old.len(),
        replacement.as_bytes(),
    ))
}

fn replace_range(source: &[u8], range: std::ops::Range<usize>, replacement: &[u8]) -> Vec<u8> {
    let mut postimage = Vec::with_capacity(source.len() - range.len() + replacement.len());
    postimage.extend_from_slice(&source[..range.start]);
    postimage.extend_from_slice(replacement);
    postimage.extend_from_slice(&source[range.end..]);
    postimage
}

fn expand_options(value: Option<&Value>) -> Result<ExpandOptions, ConnectorError> {
    let Some(value) = value else {
        return Ok(ExpandOptions::default());
    };
    let Value::Object(values) = value else {
        return serde_json::from_value(normalize_keys(value.clone())).map_err(json_error);
    };
    let has_lines = values.contains_key("lines")
        || values.contains_key("lineStart")
        || values.contains_key("line_start")
        || values.contains_key("lineEnd")
        || values.contains_key("line_end");
    let selectors = usize::from(values.contains_key("bytes"))
        + usize::from(has_lines)
        + usize::from(values.contains_key("symbol"))
        + usize::from(values.contains_key("next"))
        + usize::from(values.contains_key("offset"))
        + usize::from(values.contains_key("all"));
    if selectors > 1 {
        return Err(ConnectorError::new(
            "z.expand accepts exactly one bytes, lines, symbol, next, offset, or all selector",
        ));
    }
    if let Some(bytes) = values.get("bytes") {
        if values.len() != 1 {
            return Err(ConnectorError::new(
                "z.expand bytes selector does not accept sibling options",
            ));
        }
        let bytes = bytes
            .as_object()
            .ok_or_else(|| ConnectorError::new("z.expand bytes must be an object"))?;
        if bytes.len() != 2 {
            return Err(ConnectorError::new(
                "z.expand bytes requires only start and end",
            ));
        }
        let start = required_u64(bytes.get("start"), "z.expand bytes.start")?;
        let end = required_u64(bytes.get("end"), "z.expand bytes.end")?;
        if end <= start {
            return Err(ConnectorError::new(
                "z.expand bytes.end must exceed bytes.start",
            ));
        }
        return Ok(ExpandOptions {
            offset: Some(start),
            limit: Some(end - start),
            ..ExpandOptions::default()
        });
    }
    if let Some(lines) = values.get("lines") {
        if values.len() != 1 {
            return Err(ConnectorError::new(
                "z.expand lines selector does not accept sibling options",
            ));
        }
        let lines = lines
            .as_object()
            .ok_or_else(|| ConnectorError::new("z.expand lines must be an object"))?;
        if lines.len() != 2 {
            return Err(ConnectorError::new(
                "z.expand lines requires only start and end",
            ));
        }
        let start = required_u64(lines.get("start"), "z.expand lines.start")?;
        let end = required_u64(lines.get("end"), "z.expand lines.end")?;
        return Ok(ExpandOptions {
            line_start: Some(u32::try_from(start).map_err(json_error)?),
            line_end: Some(u32::try_from(end).map_err(json_error)?),
            ..ExpandOptions::default()
        });
    }
    if let Some(all) = values.get("all") {
        if values.len() != 1 || all.as_bool() != Some(true) {
            return Err(ConnectorError::new(
                "z.expand all must be exactly {all:true}",
            ));
        }
        return Ok(ExpandOptions::default());
    }
    if let Some(next) = values.get("next") {
        if values.keys().any(|key| key != "next" && key != "limit") {
            return Err(ConnectorError::new(
                "z.expand next accepts only an optional limit",
            ));
        }
        let offset = match next {
            Value::String(next) => next
                .parse::<u64>()
                .map_err(|error| ConnectorError::new(format!("invalid z.expand next: {error}")))?,
            value => required_u64(Some(value), "z.expand next")?,
        };
        let limit = values
            .get("limit")
            .map(|value| required_u64(Some(value), "z.expand limit"))
            .transpose()?;
        return Ok(ExpandOptions {
            offset: Some(offset),
            limit,
            ..ExpandOptions::default()
        });
    }
    serde_json::from_value(normalize_keys(value.clone())).map_err(json_error)
}

fn required_u64(value: Option<&Value>, label: &str) -> Result<u64, ConnectorError> {
    value
        .and_then(Value::as_u64)
        .ok_or_else(|| ConnectorError::new(format!("{label} must be an unsigned integer")))
}

fn expand_handle_arg(value: Option<&Value>, label: &str) -> Result<ZeroHandle, ConnectorError> {
    let encoded = match value {
        Some(Value::String(handle)) => Some(handle.as_str()),
        Some(Value::Object(values)) => values
            .get("source")
            .and_then(Value::as_object)
            .and_then(|source| source.get("exact"))
            .or_else(|| {
                values
                    .get("recovery")
                    .and_then(Value::as_object)
                    .and_then(|recovery| recovery.get("exact"))
            })
            .or_else(|| values.get("exact"))
            .and_then(Value::as_str),
        _ => None,
    }
    .ok_or_else(|| ConnectorError::new(format!("{label} must be a handle or SnapResult")))?;
    ZeroHandle::parse(encoded).map_err(|error| ConnectorError::new(error.to_string()))
}

fn expected_preimage(options: Option<&Value>) -> Result<Option<ZeroHandle>, ConnectorError> {
    options
        .and_then(Value::as_object)
        .and_then(|options| {
            options
                .get("expectedPreimage")
                .or_else(|| options.get("expected_preimage"))
        })
        .and_then(Value::as_str)
        .map(ZeroHandle::parse)
        .transpose()
        .map_err(|error| ConnectorError::new(error.to_string()))
}

fn normalize_keys(value: Value) -> Value {
    let Value::Object(values) = value else {
        return value;
    };
    let mut normalized = Map::new();
    for (key, value) in values {
        let key = match key.as_str() {
            "maxBytes" => "max_bytes",
            "maxVisibleBytes" => "max_visible_bytes",
            "timeoutMs" => "timeout_ms",
            "lineStart" => "line_start",
            "lineEnd" => "line_end",
            "next" => "offset",
            "expectedPreimage" => "expected_preimage",
            other => other,
        };
        normalized.insert(key.into(), value);
    }
    Value::Object(normalized)
}

fn apply_patch(source: &[u8], patch: Value) -> Result<(Vec<u8>, Option<String>), ConnectorError> {
    match patch {
        Value::Object(values) => {
            let find = values
                .get("find")
                .or_else(|| values.get("old"))
                .or_else(|| values.get("pattern"))
                .and_then(Value::as_str)
                .ok_or_else(|| ConnectorError::new("z.edit patch requires find or pattern"))?;
            let replacement = values
                .get("replace")
                .or_else(|| values.get("new"))
                .or_else(|| values.get("replacement"))
                .and_then(Value::as_str)
                .ok_or_else(|| ConnectorError::new("z.edit patch requires replacement"))?;
            let text = std::str::from_utf8(source)
                .map_err(|_| ConnectorError::new("z.edit target is not UTF-8"))?;
            let mut matches = text.match_indices(find);
            let first = matches
                .next()
                .ok_or_else(|| ConnectorError::new("z.edit patch did not match"))?;
            if matches.next().is_some() {
                return Err(ConnectorError::new("z.edit patch is ambiguous"));
            }
            let mut postimage = String::with_capacity(text.len() - find.len() + replacement.len());
            postimage.push_str(&text[..first.0]);
            postimage.push_str(replacement);
            postimage.push_str(&text[first.0 + find.len()..]);
            Ok((
                postimage.into_bytes(),
                Some(Value::Object(values).to_string()),
            ))
        }
        _ => Err(ConnectorError::new("z.edit patch must be a patch object")),
    }
}

fn default_asgrep_options() -> AsgrepOptions {
    AsgrepOptions {
        mode: AsgrepMode::Natural,
        path: None,
        language: None,
        source: None,
        sink: None,
        limit: None,
    }
}

fn host_error(error: crate::HostError) -> ConnectorError {
    ConnectorError::new(error.to_string())
}

fn json_error(error: impl std::fmt::Display) -> ConnectorError {
    ConnectorError::new(error.to_string())
}

fn type_engine_error(error: TypeScriptError) -> EngineError {
    EngineError::new(EngineErrorKind::InvalidInput, error.to_string(), false)
}

fn map_interpreter_error(error: zero_codemode::HostError) -> EngineError {
    let text = error.to_string();
    let kind = if text.contains("cancel") {
        EngineErrorKind::Cancelled
    } else if text.contains("deadline") || text.contains("timed out") {
        EngineErrorKind::Deadline
    } else if text.contains("budget")
        || text.contains("limit")
        || text.contains("full_view_unavailable")
    {
        EngineErrorKind::Budget
    } else if text.contains("preimage")
        || text.contains("selection_")
        || text.contains("structural source")
        || text.contains("ambiguous")
    {
        EngineErrorKind::Conflict
    } else if text.contains("invalid request") || text.contains("parse") || text.contains("syntax")
    {
        EngineErrorKind::InvalidInput
    } else {
        EngineErrorKind::Internal
    };
    EngineError::new(kind, text, false)
}
