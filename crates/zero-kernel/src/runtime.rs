use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};
use zero_abi::{
    AsgrepMode, AsgrepOptions, CapabilityDescriptor, EffectRequest, EngineError, EngineErrorKind,
    ExpandOptions, GUEST_METHODS, GlobalRegistration, LookupOptions, ReadOptions, ShellOptions,
    SnapRequest, SnapTargetRequest, SnapViewRequest, ZERO_KERNEL_PROTOCOL, ZeroHandle,
    ZeroKernelResponse,
};
use zero_codemode::{
    Connector, ConnectorCompletion, ConnectorError, DispatchContext, GuestContext, GuestSurface,
    Host, HostLimits,
};

use crate::host::{Cell, ZeroKernel};
use crate::shell::ShellCommand;
use crate::typescript::{TypeScriptError, erase_typescript};

const MAX_CONCURRENT_CONNECTOR_CALLS: usize = 2;

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
        let read_string = capability.method == "read"
            && args
                .as_array()
                .and_then(|values| values.first())
                .is_some_and(Value::is_string);
        let concurrent_read = if read_string {
            let slot = self.cell.borrow();
            !slot
                .as_ref()
                .ok_or_else(|| ConnectorError::new("ZeroKernel cell already settled"))?
                .has_active_transaction()
        } else {
            false
        };
        let concurrent = matches!(capability.method.as_str(), "find" | "run") || concurrent_read;
        if concurrent {
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
            if path.starts_with("z://") {
                let handle = ZeroHandle::parse(&path)
                    .map_err(|error| ConnectorError::new(error.to_string()))?;
                let selectors_requested = positional.get(1).is_some_and(|value| !value.is_null());
                let expanded = context
                    .expand(&handle, expand_options(positional.get(1))?)
                    .map_err(host_error)?;
                if selectors_requested {
                    return serde_json::to_value(expanded).map_err(json_error);
                }
                let text = expanded.text.ok_or_else(|| {
                    ConnectorError::new("z.read exact handle does not contain UTF-8 text")
                })?;
                return Ok(Value::String(text));
            }
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
                    .map(serde_json::from_value::<ReadOptions>)
                    .transpose()
                    .map_err(json_error)?
                    .unwrap_or_default();
                let paths = context
                    .lookup(
                        path_buf,
                        LookupOptions {
                            filter: None,
                            limit: None,
                            recursive: options.recursive,
                        },
                    )
                    .map_err(host_error)?;
                if options.offset.is_some() || options.limit.is_some() {
                    let start = usize::try_from(options.offset.unwrap_or(0)).unwrap_or(usize::MAX);
                    let limit = usize::try_from(options.limit.unwrap_or(100)).unwrap_or(usize::MAX);
                    let start = start.min(paths.len());
                    let end = start.saturating_add(limit).min(paths.len());
                    let next = (end < paths.len()).then_some(end as u32);
                    return Ok(serde_json::json!({
                        "entries": &paths[start..end],
                        "next": next,
                        "complete": next.is_none(),
                    }));
                }
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
        "find" => {
            let (query, options_source): (Value, Option<Value>) = match positional.first() {
                Some(Value::Object(first)) if positional.len() == 1 => {
                    let mut object = first.clone();
                    let query = object.remove("query").ok_or_else(|| {
                        ConnectorError::new("z.find object form requires a query field")
                    })?;
                    (query, Some(Value::Object(object)))
                }
                _ => (
                    Value::String(string_arg(&positional, 0, "z.find query")?),
                    positional.get(1).cloned(),
                ),
            };
            let query = query
                .as_str()
                .ok_or_else(|| ConnectorError::new("z.find query must be a string"))?
                .to_owned();
            let options = options_source
                .map(parse_asgrep_options)
                .transpose()?
                .unwrap_or_else(default_asgrep_options);
            serde_json::to_value(context.asgrep(query, options).map_err(host_error)?)
                .map_err(json_error)
        }
        "run" => {
            let (command, options) = shell_arguments(&positional)?;
            serde_json::to_value(context.shell(command, options).map_err(host_error)?)
                .map_err(json_error)
        }
        _ => Err(ConnectorError::new("not a read-only direct method")),
    }
}

impl ZeroKernel {
    /// Begin collecting streamed source without tying ZeroKernel to a harness protocol.
    pub fn begin_preparation(&self) -> crate::CellPreparation {
        crate::CellPreparation::new()
    }

    /// Execute a completed streamed cell through the same canonical path.
    pub fn execute_prepared(
        &self,
        prepared: &crate::PreparedCell,
    ) -> Result<ZeroKernelResponse, crate::HostError> {
        self.execute_cell(prepared.source())
    }

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
        let guest = Arc::new(GuestSurface::new(GuestContext {
            project_root: context.project_root.to_string_lossy().into_owned(),
            workspace_root: Some(context.workspace_root.to_string_lossy().into_owned()),
            request_root: Some(context.project_root.to_string_lossy().into_owned()),
            session_root: context.expected_state_root.clone(),
            session_id: context.session_id.clone(),
            protocol: ZERO_KERNEL_PROTOCOL.into(),
        }));
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
    GUEST_METHODS
        .iter()
        .copied()
        .filter(|method| *method != "state")
        .map(|method| CapabilityDescriptor::new("z", method))
        .collect()
}

fn host_limits(budget: &zero_abi::KernelBudget) -> Result<HostLimits, crate::HostError> {
    HostLimits::new(
        usize::try_from(budget.memory_bytes).unwrap_or(usize::MAX),
        1024 * 1024,
        Duration::from_millis(budget.wall_ms),
        budget.cpu_ms.saturating_mul(10_000).max(1),
        4_096,
        (budget.task_limit as usize)
            .min(zero_codemode::MAX_INFLIGHT_CONNECTOR_CALLS)
            .min(MAX_CONCURRENT_CONNECTOR_CALLS),
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
            let target = positional
                .first()
                .ok_or_else(|| ConnectorError::new("z.read target is required"))?;
            if let Some(object) = target.as_object() {
                let is_snapshot = object.contains_key("source") && object.contains_key("recovery");
                if is_snapshot {
                    let handle = expand_handle_arg(Some(target), "z.read snapshot")?;
                    let selectors_requested =
                        positional.get(1).is_some_and(|value| !value.is_null());
                    let expanded = cell
                        .expand(&handle, expand_options(positional.get(1))?)
                        .map_err(host_error)?;
                    if selectors_requested {
                        serde_json::to_value(expanded).map_err(json_error)
                    } else {
                        expanded.text.map(Value::String).ok_or_else(|| {
                            ConnectorError::new("z.read exact handle does not contain UTF-8 text")
                        })
                    }
                } else {
                    let request = snap_request(&positional)?;
                    serde_json::to_value(cell.snap(request).map_err(host_error)?)
                        .map_err(json_error)
                }
            } else {
                let path = string_arg(&positional, 0, "z.read path")?;
                if path.starts_with("z://") {
                    let handle = ZeroHandle::parse(&path)
                        .map_err(|error| ConnectorError::new(error.to_string()))?;
                    let selectors_requested =
                        positional.get(1).is_some_and(|value| !value.is_null());
                    let expanded = cell
                        .expand(&handle, expand_options(positional.get(1))?)
                        .map_err(host_error)?;
                    if selectors_requested {
                        return serde_json::to_value(expanded).map_err(json_error);
                    }
                    let text = expanded.text.ok_or_else(|| {
                        ConnectorError::new("z.read exact handle does not contain UTF-8 text")
                    })?;
                    return Ok(Value::String(text));
                }
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
                validate_edit_patch_fields(values)?;
                // {remove: true} -> delete file
                if values
                    .get("remove")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    serde_json::to_value(cell.remove(path, expected).map_err(host_error)?)
                        .map_err(json_error)
                }
                // Only the explicit create key bypasses read/preimage lookup.
                // Typed patches carrying content (replace_file/replace_lines/
                // insert_before/after) MUST stay on the verified edit path.
                else if let Some(content) = values.get("create").and_then(Value::as_str) {
                    serde_json::to_value(
                        cell.create(path, content.as_bytes().to_vec())
                            .map_err(host_error)?,
                    )
                    .map_err(json_error)
                } else {
                    // Fall through to standard edit path below
                    let source = cell.read_exact(&path).map_err(host_error)?;
                    let (postimage, patch_text) = apply_edit_patch(&source, target, patch)?;
                    serde_json::to_value(
                        cell.edit(path, postimage, patch_text, expected)
                            .map_err(host_error)?,
                    )
                    .map_err(json_error)
                }
            } else if patch.is_string() {
                // Whole-file replacement via string: allowed when file doesn't exist yet
                let exists = cell.read_exact(&path).is_ok();
                if exists {
                    return Err(ConnectorError::new(
                        "z.edit refuses a bare replacement string on an existing file (it would replace everything). Use {find, replacement} to substitute or {kind: 'replace_file', content} to overwrite deliberately.",
                    ));
                }
                serde_json::to_value(
                    cell.create(path, patch.as_str().unwrap_or("").as_bytes().to_vec())
                        .map_err(host_error)?,
                )
                .map_err(json_error)
            } else {
                return Err(ConnectorError::new(
                    "z.edit accepts {find, replacement}, {create: content}, {remove: true}, or {kind: 'replace_file', content}",
                ));
            }
        }
        "apply" => {
            let value = positional
                .first()
                .cloned()
                .ok_or_else(|| ConnectorError::new("z.apply request is required"))?;
            let request = if value.is_array() {
                simplified_apply_request(value, positional.get(1).cloned())?
            } else if value.is_object() {
                serde_json::from_value::<EffectRequest>(value).map_err(json_error)?
            } else {
                return Err(ConnectorError::new(
                    "z.apply expects an operation array or a full effect request object",
                ));
            };
            serde_json::to_value(cell.effect(request).map_err(host_error)?).map_err(json_error)
        }
        _ => Err(ConnectorError::new(format!(
            "unknown direct ZeroKernel method z.{method}"
        ))),
    }
}

fn parse_asgrep_options(value: Value) -> Result<AsgrepOptions, ConnectorError> {
    let mut value = normalize_keys(value);
    let options = value
        .as_object_mut()
        .ok_or_else(|| ConnectorError::new("z.find options must be an object"))?;
    options
        .entry("mode")
        .or_insert_with(|| Value::String("natural".into()));
    serde_json::from_value(value).map_err(json_error)
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
                        .ok_or_else(|| ConnectorError::new("z.run argv values must be strings"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        _ => {
            return Err(ConnectorError::new(
                "z.run expects a string or string array",
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
        .ok_or_else(|| ConnectorError::new("z.read snapshot target is required"))?;
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
                        "z.read snapshot expects a path or {target:{path|search}, ...}",
                    )
                })?;
            let mut target = Map::new();
            target.insert("path".into(), Value::String(path));
            values.insert("target".into(), Value::Object(target));
            serde_json::from_value(Value::Object(values)).map_err(json_error)
        }
        _ => Err(ConnectorError::new(
            "z.read snapshot expects a path or request object",
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
        return Err(ConnectorError::new(
            "z.apply requires at least one operation",
        ));
    }

    let mut targets = Map::new();
    let mut path_targets: BTreeMap<String, (String, String)> = BTreeMap::new();
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
        const ALLOWED: &[&str] = &[
            "path",
            "edit",
            "create",
            "replace",
            "remove",
            "find",
            "old",
            "replacement",
            "new",
            "before",
            "after",
            "content",
        ];
        if let Some(key) = op.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
            return Err(ConnectorError::new(format!(
                "z.apply operation {index} has unknown field {key:?}; accepted fields: {}",
                ALLOWED.join(", ")
            )));
        }
        let action_count = usize::from(op.contains_key("edit"))
            + usize::from(op.contains_key("create"))
            + usize::from(op.contains_key("replace"))
            + usize::from(op.get("remove").and_then(Value::as_bool) == Some(true))
            + usize::from(
                op.get("find").or_else(|| op.get("old")).is_some()
                    && op.get("replacement").or_else(|| op.get("new")).is_some(),
            )
            + usize::from(op.contains_key("before") && op.contains_key("content"))
            + usize::from(op.contains_key("after") && op.contains_key("content"));
        if action_count != 1 {
            return Err(ConnectorError::new(format!(
                "z.apply operation {index} requires exactly one action, found {action_count}"
            )));
        }
        if let Some(edit) = op.get("edit").and_then(Value::as_object)
            && let Some(key) = edit.keys().find(|key| {
                !matches!(
                    key.as_str(),
                    "find" | "old" | "replacement" | "replace" | "new"
                )
            })
        {
            return Err(ConnectorError::new(format!(
                "z.apply operation {index} edit has unknown field {key:?}"
            )));
        }
        let target = path_targets
            .get(path)
            .map(|(target, _)| target.clone())
            .unwrap_or_else(|| format!("t{index}"));

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
            op.get("find")
                .or_else(|| op.get("old"))
                .and_then(Value::as_str),
            op.get("replacement")
                .or_else(|| op.get("new"))
                .and_then(Value::as_str),
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

        if let Some((_, existing_expect)) = path_targets.get(path) {
            if existing_expect != expect {
                return Err(ConnectorError::new(format!(
                    "z.apply operations for {path:?} disagree on whether the path must exist"
                )));
            }
        } else {
            path_targets.insert(path.to_owned(), (target.clone(), expect.to_owned()));
            targets.insert(
                target.clone(),
                serde_json::json!({"path": path, "expect": expect}),
            );
        }
        changes.push(change);
    }

    let verify =
        verify.unwrap_or_else(|| serde_json::json!({"changedTargetsOnly": true, "parse": false}));
    serde_json::from_value::<EffectRequest>(serde_json::json!({
        "targets": targets,
        "changes": changes,
        "verify": verify,
    }))
    .map_err(json_error)
}

fn validate_edit_patch_fields(values: &Map<String, Value>) -> Result<(), ConnectorError> {
    const ALLOWED: &[&str] = &[
        "remove",
        "create",
        "kind",
        "find",
        "old",
        "pattern",
        "replace",
        "new",
        "replacement",
        "expectedCount",
        "content",
    ];
    if let Some(key) = values.keys().find(|key| !ALLOWED.contains(&key.as_str())) {
        return Err(ConnectorError::new(format!(
            "z.edit patch has unknown field {key:?}; accepted fields: {}",
            ALLOWED.join(", ")
        )));
    }

    let remove = values.get("remove").and_then(Value::as_bool) == Some(true);
    if values.contains_key("remove") && !remove {
        return Err(ConnectorError::new(
            "z.edit remove field must be exactly true",
        ));
    }
    if values.contains_key("create") && values.get("create").and_then(Value::as_str).is_none() {
        return Err(ConnectorError::new("z.edit create field must be a string"));
    }
    let edit = values.keys().any(|key| {
        matches!(
            key.as_str(),
            "kind"
                | "find"
                | "old"
                | "pattern"
                | "replace"
                | "new"
                | "replacement"
                | "expectedCount"
                | "content"
        )
    });
    let action_count =
        usize::from(remove) + usize::from(values.contains_key("create")) + usize::from(edit);
    if action_count != 1 {
        return Err(ConnectorError::new(format!(
            "z.edit patch requires exactly one action, found {action_count}"
        )));
    }
    Ok(())
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
            "z.edit refuses a bare replacement string (it would replace the entire file): use {find, replacement} for substitution or {kind: 'replace_file', content} to replace a whole file deliberately",
        ));
    };
    let patch_text = Some(patch.to_string());
    let kind = values.get("kind").and_then(Value::as_str);
    match kind {
        None => {
            let find = patch_find(values)?;
            if let Some(range) = selection
                && &source[range] != find.as_bytes()
            {
                return Err(ConnectorError::new(
                    "selection_scope_mismatch: patch find must equal the snapped selection",
                ));
            }
            let replacement = patch_replacement(values)?;
            replace_exact(source, find, replacement).map(|postimage| (postimage, patch_text))
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
        .ok_or_else(|| edit_mismatch_error(text, old))?;
    if matches.next().is_some() {
        return Err(ConnectorError::new("z.edit patch is ambiguous"));
    }
    Ok(replace_range(
        source,
        first.0..first.0 + old.len(),
        replacement.as_bytes(),
    ))
}

fn edit_mismatch_error(source: &str, expected: &str) -> ConnectorError {
    const CONTEXT_BYTES: usize = 160;
    let anchor = expected.lines().find(|line| !line.trim().is_empty());
    let center = anchor.and_then(|line| source.find(line)).unwrap_or(0);
    let mut start = center.saturating_sub(CONTEXT_BYTES / 2);
    while start < source.len() && !source.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + CONTEXT_BYTES).min(source.len());
    while end > start && !source.is_char_boundary(end) {
        end -= 1;
    }
    let context = &source[start..end];
    ConnectorError::new(format!(
        "z.edit patch did not match (mismatch=not_found, expected_bytes={}): closest source context at byte {start}: {context:?}; the source may have changed, re-read with z.read(path) before retrying",
        expected.len()
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
            "z.read accepts exactly one bytes, lines, symbol, next, offset, or all selector",
        ));
    }
    if let Some(bytes) = values.get("bytes") {
        if values.len() != 1 {
            return Err(ConnectorError::new(
                "z.read bytes selector does not accept sibling options",
            ));
        }
        let bytes = bytes
            .as_object()
            .ok_or_else(|| ConnectorError::new("z.read bytes must be an object"))?;
        if bytes.len() != 2 {
            return Err(ConnectorError::new(
                "z.read bytes requires only start and end",
            ));
        }
        let start = required_u64(bytes.get("start"), "z.read bytes.start")?;
        let end = required_u64(bytes.get("end"), "z.read bytes.end")?;
        if end <= start {
            return Err(ConnectorError::new(
                "z.read bytes.end must exceed bytes.start",
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
                "z.read lines selector does not accept sibling options",
            ));
        }
        let lines = lines
            .as_object()
            .ok_or_else(|| ConnectorError::new("z.read lines must be an object"))?;
        if lines.len() != 2 {
            return Err(ConnectorError::new(
                "z.read lines requires only start and end",
            ));
        }
        let start = required_u64(lines.get("start"), "z.read lines.start")?;
        let end = required_u64(lines.get("end"), "z.read lines.end")?;
        return Ok(ExpandOptions {
            line_start: Some(u32::try_from(start).map_err(json_error)?),
            line_end: Some(u32::try_from(end).map_err(json_error)?),
            ..ExpandOptions::default()
        });
    }
    if let Some(all) = values.get("all") {
        if values.len() != 1 || all.as_bool() != Some(true) {
            return Err(ConnectorError::new("z.read all must be exactly {all:true}"));
        }
        return Ok(ExpandOptions::default());
    }
    if let Some(next) = values.get("next") {
        if values.keys().any(|key| key != "next" && key != "limit") {
            return Err(ConnectorError::new(
                "z.read next accepts only an optional limit",
            ));
        }
        let offset = match next {
            Value::String(next) => next
                .parse::<u64>()
                .map_err(|error| ConnectorError::new(format!("invalid z.read next: {error}")))?,
            value => required_u64(Some(value), "z.read next")?,
        };
        let limit = values
            .get("limit")
            .map(|value| required_u64(Some(value), "z.read limit"))
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
            "budgetTokens" => "budget_tokens",
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

fn default_asgrep_options() -> AsgrepOptions {
    AsgrepOptions {
        mode: AsgrepMode::Natural,
        path: None,
        language: None,
        source: None,
        sink: None,
        limit: None,
        budget_tokens: None,
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
    use zero_codemode::HostError;

    let text = error.to_string();
    let kind = match &error {
        HostError::Cancelled => EngineErrorKind::Cancelled,
        HostError::DeadlineExceeded => EngineErrorKind::Deadline,
        HostError::ResultTooLarge { .. }
        | HostError::MemoryLimit { .. }
        | HostError::MicrotaskLimit
        | HostError::CallBudgetExceeded { .. }
        | HostError::FuelExhausted => EngineErrorKind::Budget,
        HostError::Parse(_)
        | HostError::UnsupportedSyntax(_)
        | HostError::Data(_)
        | HostError::Execution(_)
        | HostError::MethodNotFound(_)
        | HostError::SurfaceNotFound(_)
        | HostError::Json(_)
        | HostError::Plan(_)
        | HostError::Registration(_)
        | HostError::Limits(_) => EngineErrorKind::InvalidInput,
        HostError::Connector(_)
            if text.contains("preimage")
                || text.contains("selection_")
                || text.contains("structural source")
                || text.contains("ambiguous") =>
        {
            EngineErrorKind::Conflict
        }
        HostError::Connector(_)
            if text.contains("budget") || text.contains("full_view_unavailable") =>
        {
            EngineErrorKind::Budget
        }
        HostError::Connector(_) if text.contains("invalid request") => {
            EngineErrorKind::InvalidInput
        }
        HostError::Runtime(_) | HostError::Connector(_) => EngineErrorKind::Internal,
    };
    EngineError::new(kind, text, false)
}
