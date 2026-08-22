//! Restricted, owned JavaScript-subset interpreter.
//!
//! The parser is Tree-sitter JavaScript. The evaluator owns the supported
//! value space and exposes only the registered capability tree. It never
//! evaluates source as host code.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::{Map, Number, Value as JsonValue};
use tree_sitter::{Node, Parser};
use tree_sitter_javascript::LANGUAGE;
use zero_abi::{
    CapabilityDescriptor, OPERATION_TRACE_LIMIT, ZeroOperationStatus, ZeroOperationTrace,
};

use crate::host::ConnectorCompletionMessage;
use crate::{
    Connector, ConnectorCompletion, DispatchContext, ExecutionMetrics, ExecutionOutcome, Host,
    HostError,
};

static INTERPRETER_CREATIONS: AtomicU64 = AtomicU64::new(0);
static PARSER_CREATIONS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Tree-sitter parsers are mutable but reusable. Keeping one per execution
    /// thread removes grammar setup from every ZeroKernel cell without serializing
    /// independent sessions behind a process-wide parser lock.
    static PARSER: RefCell<Option<Parser>> = const { RefCell::new(None) };
}

fn parse_source(source: &str) -> Result<tree_sitter::Tree, HostError> {
    PARSER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let mut parser = Parser::new();
            parser.set_language(&LANGUAGE.into()).map_err(|error| {
                HostError::Runtime(format!("JavaScript parser setup failed: {error}"))
            })?;
            *slot = Some(parser);
            PARSER_CREATIONS.fetch_add(1, Ordering::Relaxed);
        }
        let parser = slot.as_mut().ok_or_else(|| {
            HostError::Runtime("JavaScript parser cache initialization failed".into())
        })?;
        parser
            .parse(source, None)
            .ok_or_else(|| HostError::Parse("parser returned no syntax tree".into()))
    })
}

/// First named child that is not a `comment` extra node. Tree-sitter attaches
/// comments inside whatever node spans them, so field lookups skip them, but
/// positional lookups (`named_child(0)`) do not.
fn first_expression_child<'tree>(node: Node<'tree>) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() != "comment")
}

pub(crate) fn interpreter_creation_count() -> u64 {
    INTERPRETER_CREATIONS.load(Ordering::Relaxed)
}

/// Conservative estimate of one interpreter recursion frame's stack cost.
/// The derived evaluation-depth ceiling divides `stack_bytes` by this
/// divisor so the bound never approaches the real native stack.
const BYTES_PER_EVAL_FRAME: usize = 2 * 1024;

/// Absolute ceiling for any recursion depth derived from host stack bytes.
const MAX_DEPTH_HARD_CAP: usize = 128;

/// Ceiling for `to_string` coercion recursion, independent of host limits.
const MAX_TO_STRING_DEPTH: usize = 128;

/// Conservative retained cost for one connector promise, including its map
/// node, completion state, normalized result tree, and allocator overhead.
/// This converts the explicit memory budget into backpressure/failure without
/// imposing an operation-count ceiling on sequential plans.
const ESTIMATED_CONNECTOR_PROMISE_BYTES: usize = 4 * 1024;

/// RAII recursion-depth guard. Every entry increments the shared counter and
/// the guard's `Drop` decrements it on every return path, including errors
/// and thrown values, so recursion state always unwinds to zero.
struct DepthGuard {
    depth: Rc<Cell<usize>>,
}

impl DepthGuard {
    fn enter(depth: &Rc<Cell<usize>>, max_depth: usize) -> Result<Self, HostError> {
        let current = depth.get();
        if current >= max_depth {
            return Err(HostError::Data(format!(
                "evaluation depth exceeds the limit of {max_depth}"
            )));
        }
        depth.set(current + 1);
        Ok(Self {
            depth: Rc::clone(depth),
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        self.depth.set(self.depth.get().saturating_sub(1));
    }
}

#[derive(Clone, Debug)]
enum Value<'tree> {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Rc<RefCell<Vec<Value<'tree>>>>),
    Object(Rc<RefCell<ObjectValue<'tree>>>),
    Namespace(String),
    Tool(String, String),
    Method(Box<Value<'tree>>, String),
    Function(FunctionValue<'tree>),
    Promise(u64),
    Resolver { promise: u64, reject: bool },
    Error(ErrorValue),
    Unreadable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ObjectAccess {
    Open,
    Strict,
}

#[derive(Clone, Debug)]
struct ObjectValue<'tree> {
    fields: BTreeMap<String, Value<'tree>>,
    getters: BTreeMap<String, Value<'tree>>,
    access: ObjectAccess,
}

/// Human-readable kind label for destructure/type faults.
fn value_kind(value: &Value<'_>) -> &'static str {
    match value {
        Value::Undefined => "undefined",
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object (connector result?)",
        _ => "this value",
    }
}

#[derive(Clone, Debug)]
struct FunctionValue<'tree> {
    parameters: Node<'tree>,
    body: Node<'tree>,
    expression: bool,
    env: EnvRef<'tree>,
}

type EnvRef<'tree> = Rc<RefCell<Env<'tree>>>;

#[derive(Clone, Debug)]
struct Env<'tree> {
    values: BTreeMap<String, Value<'tree>>,
    parent: Option<EnvRef<'tree>>,
}

#[derive(Clone, Debug)]
struct ErrorValue {
    name: String,
    message: String,
}

#[derive(Clone, Debug)]
enum PromiseState<'tree> {
    Pending(PromiseKind<'tree>),
    Fulfilled(Value<'tree>),
    Rejected(Value<'tree>),
    Failed(HostError),
}

#[derive(Clone, Debug)]
enum PromiseKind<'tree> {
    Connector,
    Then {
        parent: u64,
        on_fulfilled: Option<Value<'tree>>,
        on_rejected: Option<Value<'tree>>,
    },
    All(Vec<u64>),
    AllSettled(Vec<u64>),
    Race(Vec<u64>),
    Manual,
}

enum Fault<'tree> {
    Host(HostError),
    Throw(Value<'tree>),
}
impl<'tree> From<HostError> for Fault<'tree> {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

enum Control<'tree> {
    Normal,
    Return(Value<'tree>),
    Break,
    Continue,
    Throw(Value<'tree>),
}

struct PendingOperation {
    trace_index: usize,
    started: Instant,
}

#[derive(Default)]
struct OperationSummary {
    target: Option<String>,
    detail: Option<String>,
    result_count: Option<u64>,
    changed_files: Option<u32>,
}

pub(super) fn execute(
    host: &Host,
    source: &str,
    connector: Rc<dyn Connector>,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<JsonValue, HostError> {
    execute_measured(host, source, connector, cancelled, timeout).result
}

pub(super) fn execute_measured(
    host: &Host,
    source: &str,
    connector: Rc<dyn Connector>,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> ExecutionOutcome {
    let started = Instant::now();
    let metrics = Rc::new(RefCell::new(ExecutionMetrics::default()));
    let operations = Rc::new(RefCell::new(Vec::new()));
    let operations_truncated = Rc::new(Cell::new(false));
    let result = execute_inner(
        host,
        source,
        connector,
        cancelled,
        timeout,
        Rc::clone(&metrics),
        Rc::clone(&operations),
        Rc::clone(&operations_truncated),
    );
    let mut metrics = metrics.borrow().clone();
    metrics.wall_time_ns = started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX);
    if metrics.first_saturation_cause.is_none() {
        metrics.first_saturation_cause = match &result {
            Err(HostError::DeadlineExceeded) => Some("deadline".into()),
            Err(HostError::FuelExhausted) => Some("instruction_budget".into()),
            Err(HostError::MicrotaskLimit) => Some("microtask_budget".into()),
            Err(HostError::CallBudgetExceeded { .. }) => Some("call_budget".into()),
            Err(HostError::MemoryLimit { .. }) => Some("memory_budget".into()),
            Err(HostError::ResultTooLarge { .. }) => Some("output_budget".into()),
            Err(HostError::Cancelled) => Some("cancellation".into()),
            _ => None,
        };
    }
    let operations = operations.borrow().clone();
    ExecutionOutcome {
        result,
        metrics,
        operations,
        operations_truncated: operations_truncated.get(),
    }
}

fn execute_inner(
    host: &Host,
    source: &str,
    connector: Rc<dyn Connector>,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
    metrics: Rc<RefCell<ExecutionMetrics>>,
    operations: Rc<RefCell<Vec<ZeroOperationTrace>>>,
    operations_truncated: Rc<Cell<bool>>,
) -> Result<JsonValue, HostError> {
    crate::wrap::validate_plan(source, host.limits.max_plan_bytes).map_err(HostError::Plan)?;
    let tree = parse_source(source)?;
    if tree.root_node().has_error() {
        return Err(HostError::Parse("invalid JavaScript syntax".into()));
    }

    INTERPRETER_CREATIONS.fetch_add(1, Ordering::Relaxed);
    let mut interpreter = Interpreter::new(
        host,
        source,
        tree.root_node(),
        connector,
        cancelled,
        timeout,
        operations,
        operations_truncated,
    );
    let value = interpreter.run();
    interpreter.finalize_operation_trace();
    *metrics.borrow_mut() = interpreter.metrics_snapshot();
    let value = value?;
    let (serialized, degraded) = interpreter.serialize_public_json(&value)?;
    if degraded {
        Ok(serde_json::json!({
            "serializationDegraded": true,
            "result": serialized,
        }))
    } else {
        Ok(serialized)
    }
}

struct Interpreter<'tree> {
    host: &'tree Host,
    source: &'tree str,
    root: Node<'tree>,
    connector: Rc<dyn Connector>,
    receiver: Receiver<ConnectorCompletionMessage>,
    sender: SyncSender<ConnectorCompletionMessage>,
    promises: BTreeMap<u64, PromiseState<'tree>>,
    inflight_connector_calls: usize,
    next_promise: u64,
    operations: Rc<RefCell<Vec<ZeroOperationTrace>>>,
    operations_truncated: Rc<Cell<bool>>,
    pending_operations: BTreeMap<u64, PendingOperation>,
    next_parallel_group: u64,
    active_parallel_group: Option<u64>,
    env: EnvRef<'tree>,
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
    instructions: u64,
    microtasks: usize,
    microtask_streak: usize,
    depth: Rc<Cell<usize>>,
    max_depth: usize,
    metrics: ExecutionMetrics,
}

impl<'tree> Interpreter<'tree> {
    fn new(
        host: &'tree Host,
        source: &'tree str,
        root: Node<'tree>,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
        operations: Rc<RefCell<Vec<ZeroOperationTrace>>>,
        operations_truncated: Rc<Cell<bool>>,
    ) -> Self {
        let (sender, receiver) =
            std::sync::mpsc::sync_channel(host.limits.max_inflight_connector_calls);
        let env = Rc::new(RefCell::new(Env {
            values: BTreeMap::new(),
            parent: None,
        }));
        let depth = Rc::new(Cell::new(0));
        let max_depth =
            (host.limits.stack_bytes / BYTES_PER_EVAL_FRAME).clamp(1, MAX_DEPTH_HARD_CAP);
        let mut interpreter = Self {
            host,
            source,
            root,
            connector,
            receiver,
            sender,
            promises: BTreeMap::new(),
            inflight_connector_calls: 0,
            next_promise: 1,
            operations,
            operations_truncated,
            pending_operations: BTreeMap::new(),
            next_parallel_group: 1,
            active_parallel_group: None,
            env,
            cancelled,
            deadline: Instant::now() + timeout,
            instructions: 0,
            microtasks: 0,
            microtask_streak: 0,
            depth,
            max_depth,
            metrics: ExecutionMetrics::default(),
        };
        interpreter.install_globals();
        interpreter
    }

    fn install_globals(&mut self) {
        let mut env = self.env.borrow_mut();
        for name in [
            "Object",
            "Reflect",
            "Math",
            "JSON",
            "Array",
            "Date",
            "RegExp",
            "URL",
            "URLSearchParams",
            "Promise",
            "Number",
            "String",
            "Boolean",
            "console",
        ] {
            env.values
                .insert(name.to_owned(), Value::Namespace(name.to_owned()));
        }
        for name in ["Error", "TypeError", "RangeError", "SyntaxError"] {
            env.values
                .insert(name.to_owned(), Value::Namespace(name.to_owned()));
        }
        for name in [
            "parseInt",
            "parseFloat",
            "encodeURI",
            "encodeURIComponent",
            "decodeURI",
            "decodeURIComponent",
        ] {
            env.values.insert(
                name.to_owned(),
                Value::Method(Box::new(Value::Undefined), name.into()),
            );
        }
        env.values.insert("undefined".into(), Value::Undefined);
        env.values.insert("NaN".into(), Value::Number(f64::NAN));
        env.values
            .insert("Infinity".into(), Value::Number(f64::INFINITY));
        env.values
            .insert("globalThis".into(), Value::Namespace("globalThis".into()));
        // The host installs one direct `z` object for each fresh cell.
        if self.host.guest.is_some() {
            env.values.insert("z".into(), Value::Namespace("z".into()));
        }
    }

    fn run(&mut self) -> Result<Value<'tree>, HostError> {
        let result = match self.exec(self.root) {
            Ok(Control::Return(value)) => {
                self.await_value(value).map_err(|fault| self.fault(fault))
            }
            Ok(Control::Normal) => Ok(Value::Undefined),
            Ok(Control::Throw(value)) | Err(Fault::Throw(value)) => Err(self.throw_error(value)),
            Ok(Control::Break | Control::Continue) => Err(HostError::UnsupportedSyntax(
                "loop control escaped its loop".into(),
            )),
            Err(Fault::Host(error)) => Err(error),
        };
        if result.is_ok() {
            self.finish_inflight()?;
        }
        result
    }

    fn finish_inflight(&mut self) -> Result<(), HostError> {
        while self.inflight_connector_calls > 0 {
            // Sitting ConnectorCompletion Ok wins over cancel/deadline.
            self.drain()?;
            if self.inflight_connector_calls == 0 {
                break;
            }
            self.tick()?;
            self.microtask_streak = 0;
            match self.receiver.recv_timeout(
                self.deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(25)),
            ) {
                Ok(completion) => self.settle(completion)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(HostError::Connector(
                        "connector completion channel closed".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn tick(&mut self) -> Result<(), HostError> {
        self.instructions = self.instructions.saturating_add(1);
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(HostError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(HostError::DeadlineExceeded);
        }
        if self.instructions > self.host.limits.instruction_budget {
            return Err(HostError::FuelExhausted);
        }
        Ok(())
    }

    fn metrics_snapshot(&self) -> ExecutionMetrics {
        let mut metrics = self.metrics.clone();
        metrics.instructions = self.instructions;
        metrics.microtasks = self.microtasks;
        metrics
    }

    fn exec(&mut self, node: Node<'tree>) -> Result<Control<'tree>, Fault<'tree>> {
        let _guard = DepthGuard::enter(&self.depth, self.max_depth).map_err(Fault::Host)?;
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let result = self.statement(child)?;
            if !matches!(result, Control::Normal) {
                return Ok(result);
            }
        }
        Ok(Control::Normal)
    }

    fn statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, Fault<'tree>> {
        self.tick()?;
        let _guard = DepthGuard::enter(&self.depth, self.max_depth).map_err(Fault::Host)?;
        match node.kind() {
            "program" | "statement_block" => self.exec(node),
            "empty_statement" => Ok(Control::Normal),
            "comment" => Ok(Control::Normal),
            "expression_statement" => match node
                .named_child(0)
                .map(|child| self.eval(child))
                .transpose()?
            {
                Some(_) | None => Ok(Control::Normal),
            },
            "return_statement" => match first_expression_child(node)
                .map(|child| self.eval(child))
                .transpose()
            {
                Ok(value) => Ok(Control::Return(value.unwrap_or(Value::Undefined))),
                Err(Fault::Throw(value)) => Ok(Control::Throw(value)),
                Err(error) => Err(error),
            },
            "lexical_declaration" | "variable_declaration" | "using_declaration" => {
                self.declare(node)?;
                Ok(Control::Normal)
            }
            "if_statement" => {
                let condition = self.eval(
                    node.child_by_field_name("condition")
                        .ok_or_else(|| self.unsupported("if without condition"))?,
                )?;
                let branch = if truthy(&condition) {
                    node.child_by_field_name("consequence")
                } else {
                    node.child_by_field_name("alternative")
                };
                branch
                    .map(|child| self.statement(child))
                    .transpose()
                    .map(|value| value.unwrap_or(Control::Normal))
            }
            "for_statement" => self.for_statement(node),
            "for_in_statement" => self.for_in_statement(node),
            "while_statement" | "do_statement" => self.while_statement(node),
            "break_statement" => Ok(Control::Break),
            "continue_statement" => Ok(Control::Continue),
            "throw_statement" => Ok(Control::Throw(
                self.eval(
                    first_expression_child(node)
                        .ok_or_else(|| self.unsupported("throw without argument"))?,
                )?,
            )),
            "try_statement" => self.try_statement(node),
            "switch_statement" => self.switch_statement(node),
            "function_declaration" => {
                let name = node
                    .child_by_field_name("name")
                    .ok_or_else(|| self.unsupported("anonymous function declaration"))?;
                let parameters = node
                    .child_by_field_name("parameters")
                    .ok_or_else(|| self.unsupported("function parameters"))?;
                let body = node
                    .child_by_field_name("body")
                    .ok_or_else(|| self.unsupported("function body"))?;
                let key = self.text(name).to_owned();
                let function = Value::Function(FunctionValue {
                    parameters,
                    body,
                    expression: false,
                    env: self.env.clone(),
                });
                self.env.borrow_mut().values.insert(key, function);
                Ok(Control::Normal)
            }
            _ => Err(self.unsupported(node.kind()).into()),
        }
    }

    fn declare(&mut self, node: Node<'tree>) -> Result<(), Fault<'tree>> {
        let mut cursor = node.walk();
        for item in node
            .named_children(&mut cursor)
            .filter(|child| child.kind() == "variable_declarator")
        {
            let name = item
                .child_by_field_name("name")
                .ok_or_else(|| self.unsupported("declaration without name"))?;
            let value = item
                .child_by_field_name("value")
                .map(|child| self.eval(child))
                .transpose()?
                .unwrap_or(Value::Undefined);
            self.bind(name, value)?;
        }
        Ok(())
    }

    fn bind(&mut self, node: Node<'tree>, value: Value<'tree>) -> Result<(), Fault<'tree>> {
        match node.kind() {
            "identifier" | "shorthand_property_identifier_pattern" => {
                self.env
                    .borrow_mut()
                    .values
                    .insert(self.text(node).to_owned(), value);
            }
            "object_pattern" => {
                if let Value::Object(object) = value {
                    let mut cursor = node.walk();
                    let parts: Vec<_> = node.named_children(&mut cursor).collect();
                    // Snapshot fields and drop the RefCell borrow before
                    // recursive bind. Nested pair patterns can alias the
                    // same object (`const { self: { x: y } } = obj` when
                    // `obj.self === obj`) and would otherwise panic.
                    let bindings: Vec<(Node<'tree>, Value<'tree>)> = {
                        let object = object.borrow();
                        parts
                            .into_iter()
                            .filter_map(|part| {
                                // `{ x }` is a bare shorthand node with no
                                // key/name field. `{ x: y }` / `{ x: { y } }`
                                // are pair_pattern children that do.
                                let key = part
                                    .child_by_field_name("key")
                                    .or_else(|| part.child_by_field_name("name"))
                                    .or_else(|| {
                                        matches!(
                                            part.kind(),
                                            "shorthand_property_identifier_pattern" | "identifier"
                                        )
                                        .then_some(part)
                                    })?;
                                let field = object
                                    .fields
                                    .get(self.text(key))
                                    .cloned()
                                    .unwrap_or(Value::Undefined);
                                let target = part.child_by_field_name("value").unwrap_or(key);
                                Some((target, field))
                            })
                            .collect()
                    };
                    for (target, field) in bindings {
                        self.bind(target, field)?;
                    }
                } else {
                    // Silent skip here surfaced later as a misleading
                    // "unknown identifier"; fail loud at the destructure site.
                    return Err(Fault::Host(HostError::Data(format!(
                        "cannot destructure {} with an object pattern; bind the result to one name first and access its fields (e.g. `const out = await ...; out.content`)",
                        value_kind(&value),
                    ))));
                }
            }
            "array_pattern" => {
                if let Value::Array(items) = value {
                    let mut cursor = node.walk();
                    // Comments inside the pattern are not elements; filtering
                    // keeps each part aligned with its source index.
                    let parts: Vec<_> = node
                        .named_children(&mut cursor)
                        .filter(|child| child.kind() != "comment")
                        .collect();
                    let bindings: Vec<(Node<'tree>, Value<'tree>)> = {
                        let items = items.borrow();
                        parts
                            .into_iter()
                            .enumerate()
                            .map(|(index, part)| {
                                (part, items.get(index).cloned().unwrap_or(Value::Undefined))
                            })
                            .collect()
                    };
                    for (part, item) in bindings {
                        self.bind(part, item)?;
                    }
                } else {
                    return Err(Fault::Host(HostError::Data(format!(
                        "cannot destructure {} with an array pattern; bind the result to one name before selecting fields",
                        value_kind(&value),
                    ))));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn for_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, Fault<'tree>> {
        if let Some(initializer) = node.child_by_field_name("initializer")
            && initializer.kind() != "empty_statement"
        {
            if initializer.kind().ends_with("declaration") {
                self.declare(initializer)?;
            } else {
                self.eval(initializer)?;
            }
        }
        loop {
            self.tick()?;
            if let Some(condition) = node.child_by_field_name("condition")
                && condition.kind() != "empty_statement"
                && !truthy(&self.eval(condition)?)
            {
                break;
            }
            match self.statement(
                node.child_by_field_name("body")
                    .ok_or_else(|| self.unsupported("for without body"))?,
            )? {
                Control::Return(value) => return Ok(Control::Return(value)),
                Control::Throw(value) => return Ok(Control::Throw(value)),
                Control::Break => break,
                Control::Continue | Control::Normal => {}
            }
            if let Some(update) = node
                .child_by_field_name("update")
                .or_else(|| node.child_by_field_name("increment"))
                && update.kind() != "empty_statement"
            {
                self.eval(update)?;
            }
        }
        Ok(Control::Normal)
    }

    fn for_in_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, Fault<'tree>> {
        let source = self.eval(
            node.child_by_field_name("right")
                .ok_or_else(|| self.unsupported("for-in without source"))?,
        )?;
        let is_of = self.text(node).contains(" of ");
        let items = match (is_of, source) {
            (true, Value::Array(items)) => items.borrow().clone(),
            (false, Value::Array(items)) => (0..items.borrow().len())
                .map(|index| Value::String(index.to_string()))
                .collect(),
            (false, Value::Object(object)) => object
                .borrow()
                .fields
                .keys()
                .cloned()
                .map(Value::String)
                .collect(),
            _ => {
                return Err(HostError::Data("for-in/of requires an array or object".into()).into());
            }
        };
        let left = node
            .child_by_field_name("left")
            .ok_or_else(|| self.unsupported("for-in without binding"))?;
        for item in items {
            match left.kind() {
                // Tree-sitter-javascript attaches the binding pattern (or a
                // bare identifier) directly as `left`; destructuring heads
                // bind fresh, other left-hand sides assign.
                "array_pattern" | "object_pattern" => self.bind(left, item)?,
                kind if kind.ends_with("declaration") => {
                    self.bind(
                        left.child_by_field_name("name")
                            .ok_or_else(|| self.unsupported("for-in binding"))?,
                        item,
                    )?;
                }
                _ => self.assign(left, item)?,
            }
            match self.statement(
                node.child_by_field_name("body")
                    .ok_or_else(|| self.unsupported("for-in without body"))?,
            )? {
                Control::Return(value) => return Ok(Control::Return(value)),
                Control::Throw(value) => return Ok(Control::Throw(value)),
                Control::Break => break,
                Control::Continue | Control::Normal => {}
            }
        }
        Ok(Control::Normal)
    }

    fn while_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, Fault<'tree>> {
        let do_first = node.kind() == "do_statement";
        let mut first = true;
        loop {
            self.tick()?;
            if !do_first || !first {
                let condition = self.eval(
                    node.child_by_field_name("condition")
                        .ok_or_else(|| self.unsupported("while without condition"))?,
                )?;
                if !truthy(&condition) {
                    break;
                }
            }
            first = false;
            match self.statement(
                node.child_by_field_name("body")
                    .ok_or_else(|| self.unsupported("while without body"))?,
            )? {
                Control::Return(value) => return Ok(Control::Return(value)),
                Control::Throw(value) => return Ok(Control::Throw(value)),
                Control::Break => break,
                Control::Continue | Control::Normal => {}
            }
        }
        Ok(Control::Normal)
    }

    fn try_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, Fault<'tree>> {
        let mut result = match self.statement(
            node.child_by_field_name("body")
                .ok_or_else(|| self.unsupported("try without body"))?,
        ) {
            Ok(result) => result,
            Err(Fault::Throw(value)) => Control::Throw(value),
            Err(error) => return Err(error),
        };
        if let Control::Throw(value) = &result {
            let value = value.clone();
            if let Some(handler) = node.child_by_field_name("handler") {
                if let Some(parameter) = handler.child_by_field_name("parameter") {
                    self.bind(parameter, value)?;
                }
                result = self.statement(
                    handler
                        .child_by_field_name("body")
                        .ok_or_else(|| self.unsupported("catch without body"))?,
                )?;
            }
        }
        if let Some(finalizer) = node.child_by_field_name("finalizer") {
            let final_result = self.statement(finalizer)?;
            if !matches!(final_result, Control::Normal) {
                result = final_result;
            }
        }
        Ok(result)
    }

    fn switch_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, Fault<'tree>> {
        let target = self.eval(
            node.child_by_field_name("value")
                .ok_or_else(|| self.unsupported("switch without value"))?,
        )?;
        let body = node
            .child_by_field_name("body")
            .ok_or_else(|| self.unsupported("switch without body"))?;
        let mut matched = false;
        let mut cursor = body.walk();
        for case in body.named_children(&mut cursor) {
            if case.kind() == "switch_case" {
                let value = self.eval(
                    case.child_by_field_name("value")
                        .ok_or_else(|| self.unsupported("case without value"))?,
                )?;
                matched |= same_value(&target, &value);
            } else if case.kind() == "switch_default" {
                matched = true;
            }
            if matched {
                let mut inner = case.walk();
                for statement in case.named_children(&mut inner) {
                    if statement.kind() == "switch_case" || statement.kind() == "switch_default" {
                        continue;
                    }
                    match self.statement(statement)? {
                        Control::Break => return Ok(Control::Normal),
                        Control::Normal | Control::Continue => {}
                        other => return Ok(other),
                    }
                }
            }
        }
        Ok(Control::Normal)
    }

    fn eval(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        self.tick().map_err(Fault::Host)?;
        let _guard = DepthGuard::enter(&self.depth, self.max_depth).map_err(Fault::Host)?;
        match node.kind() {
            "identifier" | "property_identifier" | "shorthand_property_identifier" => {
                self.lookup(self.text(node)).ok_or_else(|| {
                    Fault::Host(HostError::Data(format!(
                        "unknown identifier '{}'",
                        self.text(node)
                    )))
                })
            }
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            "null" => Ok(Value::Null),
            "undefined" => Ok(Value::Undefined),
            "comment" => Ok(Value::Undefined),
            "number" => self
                .text(node)
                .parse()
                .map(Value::Number)
                .map_err(|_| Fault::Host(HostError::Parse("invalid number".into()))),
            "string" => Ok(Value::String(unquote(self.text(node)).map_err(Fault::Host)?)),
            "array" => self.eval_array(node),
            "object" => self.eval_object(node),
            "template_string" => self.eval_template(node),
            "regex" => Ok(Value::String(self.text(node).into())),
            "parenthesized_expression" => self.eval(
                first_expression_child(node)
                    .ok_or_else(|| Fault::Host(self.unsupported("empty parentheses")))?,
            ),
            "sequence_expression" => {
                let mut result = Value::Undefined;
                let mut cursor = node.walk();
                for child in node.named_children(&mut cursor) {
                    result = self.eval(child)?;
                }
                Ok(result)
            }
            "await_expression" => {
                let expression = first_expression_child(node)
                    .ok_or_else(|| Fault::Host(self.unsupported("await without expression")))?;
                let value = self.eval(expression)?;
                let transient_promise = if expression.kind() == "call_expression" {
                    match &value {
                        Value::Promise(id) => Some(*id),
                        _ => None,
                    }
                } else {
                    None
                };
                let result = self.await_value(value);
                if let Some(id) = transient_promise {
                    self.release_settled_promise(id);
                }
                result
            }
            "call_expression" => self.eval_call(node),
            "member_expression" | "subscript_expression" => self.eval_member(node),
            "assignment_expression" | "augmented_assignment_expression" => {
                self.eval_assignment(node)
            }
            "binary_expression" | "logical_expression" => self.eval_binary(node),
            "unary_expression" => {
                let operator = self.operator(node).to_owned();
                let value = self.eval(
                    node.child_by_field_name("argument")
                        .ok_or_else(|| Fault::Host(self.unsupported("unary argument")))?,
                )?;
                unary(&operator, value).map_err(Fault::Host)
            }
            "update_expression" => {
                let target = node
                    .child_by_field_name("argument")
                    .ok_or_else(|| Fault::Host(self.unsupported("update argument")))?;
                let old = self.eval(target)?;
                let operator = if self.text(node).contains("++") {
                    "+"
                } else {
                    "-"
                };
                let next =
                    binary(operator, old.clone(), Value::Number(1.0)).map_err(Fault::Host)?;
                self.assign(target, next.clone()).map_err(Fault::Host)?;
                Ok(next)
            }
            "ternary_expression" => {
                let condition = self.eval(
                    node.child_by_field_name("condition")
                        .ok_or_else(|| Fault::Host(self.unsupported("conditional condition")))?,
                )?;
                self.eval(
                    if truthy(&condition) {
                        node.child_by_field_name("consequence")
                    } else {
                        node.child_by_field_name("alternative")
                    }
                    .ok_or_else(|| Fault::Host(self.unsupported("conditional branch")))?,
                )
            }
            "arrow_function" | "function_expression" => Ok(Value::Function(FunctionValue {
                parameters: node
                    .child_by_field_name("parameters")
                    .or_else(|| node.child_by_field_name("parameter"))
                    .ok_or_else(|| Fault::Host(self.unsupported("function parameters")))?,
                body: node
                    .child_by_field_name("body")
                    .ok_or_else(|| Fault::Host(self.unsupported("function body")))?,
                expression: node
                    .child_by_field_name("body")
                    .is_some_and(|body| body.kind() != "statement_block"),
                env: self.env.clone(),
            })),
            "new_expression" => {
                let constructor = self.eval(
                    node.child_by_field_name("constructor")
                        .ok_or_else(|| Fault::Host(self.unsupported("new constructor")))?,
                )?;
                let argument = node
                    .child_by_field_name("arguments")
                    .and_then(first_expression_child)
                    .map(|argument| self.eval(argument))
                    .transpose()?;
                match constructor {
                    Value::Namespace(name) if name == "Promise" => {
                        self.new_manual_promise(argument.unwrap_or(Value::Undefined))
                    }
                    Value::Namespace(name) => Ok(Value::Error(ErrorValue {
                        name,
                        message: argument.map(|value| to_string(&value)).unwrap_or_default(),
                    })),
                    _ => Err(Fault::Host(self.unsupported("constructor"))),
                }
            }
            _ => Err(Fault::Host(self.unsupported(node.kind()))),
        }
    }

    fn eval_array(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let mut values = Vec::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            // Comments are extra nodes inside the literal; they are not
            // elements and must not shift the element list.
            if child.kind() == "comment" {
                continue;
            }
            if child.kind() == "spread_element" {
                match self.eval(
                    first_expression_child(child)
                        .ok_or_else(|| Fault::Host(self.unsupported("array spread")))?,
                )? {
                    Value::Array(items) => values.extend(items.borrow().iter().cloned()),
                    _ => {
                        return Err(Fault::Host(HostError::Data(
                            "array spread requires an array".into(),
                        )));
                    }
                }
            } else {
                values.push(self.eval(child)?);
            }
        }
        Ok(new_array(values))
    }

    fn eval_object(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let mut fields = BTreeMap::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match child.kind() {
                // Comments are extra nodes inside the literal; ignore them.
                "comment" => {}
                "pair" => {
                    let key = child
                        .child_by_field_name("key")
                        .ok_or_else(|| Fault::Host(self.unsupported("object key")))?;
                    let value = self.eval(
                        child
                            .child_by_field_name("value")
                            .ok_or_else(|| Fault::Host(self.unsupported("object value")))?,
                    )?;
                    fields.insert(unquote(self.text(key)).map_err(Fault::Host)?, value);
                }
                "shorthand_property_identifier" => {
                    let key = self.text(child).to_owned();
                    let value = self.lookup(&key).ok_or_else(|| {
                        Fault::Host(HostError::Data(format!("unknown identifier '{key}'")))
                    })?;
                    fields.insert(key, value);
                }
                "spread_element" => match self.eval(
                    first_expression_child(child)
                        .ok_or_else(|| Fault::Host(self.unsupported("object spread")))?,
                )? {
                    Value::Object(object) => fields.extend(object.borrow().fields.clone()),
                    _ => {
                        return Err(Fault::Host(HostError::Data(
                            "object spread requires an object".into(),
                        )));
                    }
                },
                "method_definition" => {
                    let name = child
                        .child_by_field_name("name")
                        .ok_or_else(|| Fault::Host(self.unsupported("method name")))?;
                    let parameters = child
                        .child_by_field_name("parameters")
                        .ok_or_else(|| Fault::Host(self.unsupported("method parameters")))?;
                    let body = child
                        .child_by_field_name("body")
                        .ok_or_else(|| Fault::Host(self.unsupported("method body")))?;
                    fields.insert(
                        unquote(self.text(name)).map_err(Fault::Host)?,
                        Value::Function(FunctionValue {
                            parameters,
                            body,
                            expression: false,
                            env: self.env.clone(),
                        }),
                    );
                    }
                _ => return Err(Fault::Host(self.unsupported("object member"))),
            }
        }
        Ok(Value::Object(Rc::new(RefCell::new(ObjectValue {
            fields,
            getters: BTreeMap::new(),
            access: ObjectAccess::Open,
        }))))
    }

    fn eval_template(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let mut output = String::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "template_substitution" {
                let value = self.eval(
                    first_expression_child(child)
                        .ok_or_else(|| Fault::Host(self.unsupported("template expression")))?,
                )?;
                output.push_str(&to_string(&value));
            } else if child.kind() == "escape_sequence" {
                let decoded = decode_escape_node(self.text(child)).map_err(Fault::Host)?;
                if let Some(character) = decoded {
                    output.push(character);
                }
            } else {
                output.push_str(self.text(child));
            }
        }
        Ok(Value::String(output))
    }

    fn eval_member(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let object = self.eval(
            node.child_by_field_name("object")
                .ok_or_else(|| Fault::Host(self.unsupported("member object")))?,
        )?;
        let key = if let Some(property) = node.child_by_field_name("property") {
            self.text(property).to_owned()
        } else if let Some(index) = node.child_by_field_name("index") {
            to_key(&self.eval(index)?)
        } else {
            return Err(Fault::Host(self.unsupported("member key")));
        };
        self.property(object, &key)
    }

    fn eval_call(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let function_node = node
            .child_by_field_name("function")
            .ok_or_else(|| Fault::Host(self.unsupported("call function")))?;
        let arguments = node
            .child_by_field_name("arguments")
            .ok_or_else(|| Fault::Host(self.unsupported("call arguments")))?;

        // Tree-sitter parses program-level `await (async () => { ... })()` as
        // `(await(async () => { ... }))()`: the contextual keyword becomes an
        // identifier because the source is a script. Accept only that exact
        // recovery shape. This does not install an ambient `await` function.
        if function_node.kind() == "identifier" && self.text(function_node) == "await" {
            let mut cursor = arguments.walk();
            let children: Vec<_> = arguments
                .named_children(&mut cursor)
                .filter(|child| child.kind() != "comment")
                .collect();
            if let [function] = children.as_slice()
                && matches!(function.kind(), "arrow_function" | "function_expression")
            {
                let value = self.eval(*function)?;
                return self.await_value(value);
            }
        }

        let function = self.eval(function_node)?;
        let mut values = Vec::new();
        if arguments.kind() == "template_string" {
            values.push(self.eval_template(arguments)?);
            return self.call(function, values);
        }
        let mut cursor = arguments.walk();
        for child in arguments.named_children(&mut cursor) {
            // Comments are extra nodes in the argument list; they are not
            // arguments and must not shift positions.
            if child.kind() == "comment" {
                continue;
            }
            if child.kind() == "spread_element" {
                match self.eval(
                    first_expression_child(child)
                        .ok_or_else(|| Fault::Host(self.unsupported("call spread")))?,
                )? {
                    Value::Array(items) => values.extend(items.borrow().iter().cloned()),
                    _ => {
                        return Err(Fault::Host(HostError::Data(
                            "call spread requires an array".into(),
                        )));
                    }
                }
            } else {
                values.push(self.eval(child)?);
            }
        }
        self.call(function, values)
    }

    fn eval_assignment(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let target = node
            .child_by_field_name("left")
            .ok_or_else(|| Fault::Host(self.unsupported("assignment left")))?;
        let right = self.eval(
            node.child_by_field_name("right")
                .ok_or_else(|| Fault::Host(self.unsupported("assignment right")))?,
        )?;
        let operator = if node.kind() == "assignment_expression" {
            "=".to_owned()
        } else {
            self.operator(node).to_owned()
        };
        let value = if operator == "=" {
            right
        } else {
            binary(operator.trim_end_matches('='), self.eval(target)?, right)
                .map_err(Fault::Host)?
        };
        self.assign(target, value.clone()).map_err(Fault::Host)?;
        Ok(value)
    }

    fn eval_binary(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let left = self.eval(
            node.child_by_field_name("left")
                .ok_or_else(|| Fault::Host(self.unsupported("binary left")))?,
        )?;
        let operator = self.operator(node).to_owned();
        if (operator == "&&" && !truthy(&left))
            || (operator == "||" && truthy(&left))
            || (operator == "??" && !matches!(left, Value::Null | Value::Undefined))
        {
            return Ok(left);
        }
        let right = self.eval(
            node.child_by_field_name("right")
                .ok_or_else(|| Fault::Host(self.unsupported("binary right")))?,
        )?;
        if matches!(operator.as_str(), "&&" | "||" | "??") {
            return Ok(right);
        }
        binary(&operator, left, right).map_err(Fault::Host)
    }

    fn call(
        &mut self,
        function: Value<'tree>,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        match function {
            Value::Tool(surface, method) => self.call_tool(&surface, &method, args),
            Value::Method(receiver, name) => self.call_method(*receiver, &name, args),
            Value::Function(function) => self.call_function(function, args),
            Value::Resolver { promise, reject } => {
                let value = args.into_iter().next().unwrap_or(Value::Undefined);
                if matches!(
                    self.promises.get(&promise),
                    Some(PromiseState::Pending(PromiseKind::Manual))
                ) {
                    self.promises.insert(
                        promise,
                        if reject {
                            PromiseState::Rejected(value)
                        } else {
                            PromiseState::Fulfilled(value)
                        },
                    );
                }
                Ok(Value::Undefined)
            }
            Value::Namespace(name) if name == "String" => Ok(Value::String(to_string(
                args.first().unwrap_or(&Value::Undefined),
            ))),
            Value::Namespace(name) if name == "Number" => Ok(Value::Number(
                args.first().and_then(number).unwrap_or(f64::NAN),
            )),
            Value::Namespace(name) if name == "Boolean" => {
                Ok(Value::Bool(args.first().is_some_and(truthy)))
            }
            Value::Namespace(name) => {
                let guidance = if name == "z.state" {
                    ": z.state is a namespace - use z.state.get(key), z.state.set(key, value), z.state.has(key), z.state.delete(key), or z.state.list()".to_string()
                } else if name.starts_with("z.") {
                    format!(": z.{name} is a namespace - inspect z.help() for its members")
                } else {
                    String::new()
                };
                // JS semantics: calling a non-function raises TypeError,
                // which guest try/catch can intercept.
                Err(Fault::Throw(Value::Error(ErrorValue {
                    name: "TypeError".into(),
                    message: format!("namespace '{name}' is not callable{guidance}"),
                })))
            }
            _ => Err(Fault::Host(HostError::Data("value is not callable".into()))),
        }
    }

    fn call_function(
        &mut self,
        function: FunctionValue<'tree>,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        let env = Rc::new(RefCell::new(Env {
            values: BTreeMap::new(),
            parent: Some(function.env.clone()),
        }));
        let mut cursor = function.parameters.walk();
        let parameters: Vec<_> = if function.parameters.kind() == "identifier" {
            vec![function.parameters]
        } else {
            function
                .parameters
                .named_children(&mut cursor)
                .filter(|child| child.kind() != "comment")
                .collect()
        };
        for (index, parameter) in parameters.into_iter().enumerate() {
            self.bind_in(
                env.clone(),
                parameter,
                args.get(index).cloned().unwrap_or(Value::Undefined),
            )?;
        }
        let previous = self.env.clone();
        self.env = env;
        let result = if function.expression {
            self.eval(function.body)
        } else {
            match self.statement(function.body)? {
                Control::Return(value) => Ok(value),
                Control::Normal => Ok(Value::Undefined),
                Control::Throw(value) => Err(Fault::Throw(value)),
                Control::Break | Control::Continue => Err(Fault::Host(HostError::Data(
                    "loop control escaped function".into(),
                ))),
            }
        };
        self.env = previous;
        result
    }

    fn bind_in(
        &mut self,
        env: EnvRef<'tree>,
        node: Node<'tree>,
        value: Value<'tree>,
    ) -> Result<(), Fault<'tree>> {
        if node.kind() == "identifier" {
            env.borrow_mut()
                .values
                .insert(self.text(node).into(), value);
            Ok(())
        } else {
            let previous = std::mem::replace(&mut self.env, env);
            let result = self.bind(node, value);
            self.env = previous;
            result
        }
    }

    /// Resolve one `surface.method` capability, or fail with the typed
    /// MethodNotFound / SurfaceNotFound error carrying closest-name hints.
    fn resolve_capability(
        &self,
        surface: &str,
        method: &str,
    ) -> Result<CapabilityDescriptor, Fault<'tree>> {
        self.host
            .registration
            .capabilities
            .iter()
            .find(|capability| capability.surface == surface && capability.method == method)
            .cloned()
            .ok_or_else(|| {
                if self
                    .host
                    .registration
                    .capabilities
                    .iter()
                    .any(|capability| capability.surface == surface)
                {
                    Fault::Host(HostError::MethodNotFound(format!(
                        "method_not_found: unknown method '{method}' on {}.{surface}; closest methods: {}",
                        self.host.registration.root,
                        closest_names(method, self.host.registration.capabilities.iter().filter(|capability| capability.surface == surface).map(|capability| capability.method.as_str()))
                    )))
                } else {
                    Fault::Host(HostError::SurfaceNotFound(format!(
                        "surface_not_found: unknown surface '{surface}' on {}; closest surfaces: {}",
                        self.host.registration.root,
                        closest_names(surface, self.host.registration.capabilities.iter().map(|capability| capability.surface.as_str()))
                    )))
                }
            })
    }

    fn start_operation(&mut self, sequence: u64, method: &str, args: &JsonValue) {
        if method.starts_with("__") {
            return;
        }
        let mut operations = self.operations.borrow_mut();
        if operations.len() >= OPERATION_TRACE_LIMIT {
            self.operations_truncated.set(true);
            return;
        }
        let trace_index = operations.len();
        operations.push(ZeroOperationTrace {
            sequence,
            method: method.to_owned(),
            status: ZeroOperationStatus::Failed,
            parallel_group: self.active_parallel_group,
            target: operation_target(method, args),
            detail: Some("operation did not complete".into()),
            result_count: None,
            changed_files: None,
            duration_ns: 0,
        });
        drop(operations);
        self.pending_operations.insert(
            sequence,
            PendingOperation {
                trace_index,
                started: Instant::now(),
            },
        );
    }

    fn operation_summary(&self, sequence: u64, value: &JsonValue) -> OperationSummary {
        let Some(pending) = self.pending_operations.get(&sequence) else {
            return OperationSummary::default();
        };
        let operations = self.operations.borrow();
        let Some(operation) = operations.get(pending.trace_index) else {
            return OperationSummary::default();
        };
        operation_result_summary(&operation.method, value)
    }

    fn complete_operation(&mut self, sequence: u64, result: Result<OperationSummary, &str>) {
        let Some(pending) = self.pending_operations.remove(&sequence) else {
            return;
        };
        let mut operations = self.operations.borrow_mut();
        let Some(operation) = operations.get_mut(pending.trace_index) else {
            return;
        };
        operation.duration_ns = pending
            .started
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX);
        match result {
            Ok(summary) => {
                operation.status = ZeroOperationStatus::Completed;
                if let Some(target) = summary.target {
                    operation.target = Some(target);
                }
                operation.detail = summary.detail;
                operation.result_count = summary.result_count;
                operation.changed_files = summary.changed_files;
            }
            Err(detail) => {
                operation.status = ZeroOperationStatus::Failed;
                operation.detail = Some(truncate_operation_text(detail));
            }
        }
    }

    fn finalize_operation_trace(&mut self) {
        let pending = self.pending_operations.keys().copied().collect::<Vec<_>>();
        for sequence in pending {
            self.complete_operation(sequence, Err("operation did not settle before cell end"));
        }
    }

    fn call_tool(
        &mut self,
        surface: &str,
        method: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        let descriptor = self.resolve_capability(surface, method)?;
        // Preserve the positional boundary even when the sole argument is
        // itself an array (for example `z.shell(["printf", "ok"])`).
        let value = new_array(args);
        let json = self.to_json(&value).map_err(Fault::Host)?;
        let encoded = serde_json::to_string(&json)
            .map_err(|error| Fault::Host(HostError::Data(error.to_string())))?;
        if encoded.len() > self.host.limits.max_json_bytes {
            return Err(Fault::Host(HostError::Data(
                "arguments exceed JSON limit".into(),
            )));
        }
        // Total host-call budget: every admitted direct method and each
        // `z.parallel` fan-out call counts against
        // the per-execution bound. The next dispatch past the bound fails
        // typed before any adapter work, so sequential or parallel call
        // floods are bounded by the budget, not by wall alone.
        if self.metrics.connector_dispatches >= self.host.limits.max_connector_calls {
            self.metrics
                .first_saturation_cause
                .get_or_insert_with(|| "call_budget".into());
            return Err(Fault::Host(HostError::CallBudgetExceeded {
                made: self.metrics.connector_dispatches,
                maximum: self.host.limits.max_connector_calls,
            }));
        }
        self.metrics.logical_operations = self.metrics.logical_operations.saturating_add(1);
        self.wait_for_dispatch_slot()?;
        let retained_promises = self.promises.len().checked_add(1).ok_or_else(|| {
            Fault::Host(HostError::MemoryLimit {
                requested: usize::MAX,
                maximum: self.host.limits.memory_bytes,
            })
        })?;
        let estimated_promise_bytes = retained_promises
            .checked_mul(ESTIMATED_CONNECTOR_PROMISE_BYTES)
            .unwrap_or(usize::MAX);
        if estimated_promise_bytes > self.host.limits.memory_bytes {
            self.metrics
                .first_saturation_cause
                .get_or_insert_with(|| "memory_budget".into());
            return Err(Fault::Host(HostError::MemoryLimit {
                requested: estimated_promise_bytes,
                maximum: self.host.limits.memory_bytes,
            }));
        }
        let id = self.next_promise;
        self.next_promise = self
            .next_promise
            .checked_add(1)
            .ok_or_else(|| Fault::Host(HostError::Data("promise sequence exhausted".into())))?;
        self.promises
            .insert(id, PromiseState::Pending(PromiseKind::Connector));
        self.metrics.peak_retained_promises =
            self.metrics.peak_retained_promises.max(self.promises.len());
        self.metrics.peak_estimated_promise_bytes = self
            .metrics
            .peak_estimated_promise_bytes
            .max(estimated_promise_bytes);
        self.start_operation(id, method, &json);
        let completion = ConnectorCompletion::new(id, self.sender.clone());
        if let Err(error) = self.connector.dispatch(
            &descriptor,
            &encoded,
            DispatchContext {
                deadline: self.deadline,
                max_json_bytes: self.host.limits.max_json_bytes,
            },
            completion,
        ) {
            self.promises.remove(&id);
            self.complete_operation(id, Err(error.message()));
            return Err(Fault::Throw(Value::Error(ErrorValue {
                name: "TypeError".into(),
                message: error.to_string(),
            })));
        }
        self.inflight_connector_calls = self.inflight_connector_calls.saturating_add(1);
        self.metrics.connector_dispatches = self.metrics.connector_dispatches.saturating_add(1);
        self.metrics.physical_dispatches = self.metrics.physical_dispatches.saturating_add(1);
        self.metrics.peak_inflight_connector_calls = self
            .metrics
            .peak_inflight_connector_calls
            .max(self.inflight_connector_calls);
        Ok(Value::Promise(id))
    }

    fn wait_for_dispatch_slot(&mut self) -> Result<(), Fault<'tree>> {
        let limit = self.host.limits.max_inflight_connector_calls;
        if self.inflight_connector_calls < limit {
            return Ok(());
        }
        self.metrics.backpressure_events = self.metrics.backpressure_events.saturating_add(1);
        self.metrics
            .first_saturation_cause
            .get_or_insert_with(|| "connector_concurrency".into());
        while self.inflight_connector_calls >= limit {
            self.drain().map_err(Fault::Host)?;
            if self.inflight_connector_calls < limit {
                break;
            }
            self.tick().map_err(Fault::Host)?;
            self.microtask_streak = 0;
            match self.receiver.recv_timeout(
                self.deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(25)),
            ) {
                Ok(completion) => self.settle(completion).map_err(Fault::Host)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Fault::Host(HostError::Connector(
                        "connector completion channel closed".into(),
                    )));
                }
            }
        }
        Ok(())
    }

    fn await_value(&mut self, value: Value<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let Value::Promise(id) = value else {
            return Ok(value);
        };
        self.resolve(id)
    }

    fn release_settled_promise(&mut self, id: u64) {
        if matches!(
            self.promises.get(&id),
            Some(PromiseState::Fulfilled(_) | PromiseState::Rejected(_) | PromiseState::Failed(_))
        ) {
            self.promises.remove(&id);
        }
    }

    fn apply_then_callback(
        &mut self,
        callback: Option<Value<'tree>>,
        value: Value<'tree>,
        passthrough_rejected: bool,
    ) -> Result<PromiseState<'tree>, HostError> {
        match callback {
            Some(callback) => match self.call(callback, vec![value]) {
                Ok(Value::Promise(child)) => match self.resolve(child) {
                    Ok(value) => Ok(PromiseState::Fulfilled(value)),
                    Err(Fault::Throw(value)) => Ok(PromiseState::Rejected(value)),
                    Err(Fault::Host(error)) => Err(error),
                },
                Ok(value) => Ok(PromiseState::Fulfilled(value)),
                Err(Fault::Throw(value)) => Ok(PromiseState::Rejected(value)),
                Err(Fault::Host(error)) => Err(error),
            },
            None if passthrough_rejected => Ok(PromiseState::Rejected(value)),
            None => Ok(PromiseState::Fulfilled(value)),
        }
    }

    fn resolve(&mut self, id: u64) -> Result<Value<'tree>, Fault<'tree>> {
        let _depth = DepthGuard::enter(&self.depth, self.max_depth).map_err(Fault::Host)?;
        self.pump(id).map_err(Fault::Host)?;
        match self
            .promises
            .get(&id)
            .cloned()
            .ok_or_else(|| Fault::Host(HostError::Data("unknown promise".into())))?
        {
            PromiseState::Fulfilled(value) => Ok(value),
            PromiseState::Rejected(value) => Err(Fault::Throw(value)),
            PromiseState::Failed(error) => Err(Fault::Host(error)),
            PromiseState::Pending(PromiseKind::All(ids)) => {
                // Persist reject/fail so progress_race_child cannot
                // resolve() the same All forever (a6wz leftover).
                match self.all(&ids) {
                    Ok(values) => {
                        let value = new_array(values);
                        self.promises
                            .insert(id, PromiseState::Fulfilled(value.clone()));
                        Ok(value)
                    }
                    Err(fault) => Err(self.persist_combinator_fault(id, fault)),
                }
            }
            PromiseState::Pending(PromiseKind::AllSettled(ids)) => {
                let mut values = Vec::new();
                for child in ids {
                    match self.resolve(child) {
                        Ok(value) => values.push(settled(true, value)),
                        Err(Fault::Throw(value)) => values.push(settled(false, value)),
                        Err(error) => return Err(error),
                    }
                }
                let value = new_array(values);
                self.promises
                    .insert(id, PromiseState::Fulfilled(value.clone()));
                Ok(value)
            }
            PromiseState::Pending(PromiseKind::Race(ids)) => {
                let winner = self.race(&ids).map_err(Fault::Host)?;
                match self.resolve(winner) {
                    Ok(value) => {
                        self.promises
                            .insert(id, PromiseState::Fulfilled(value.clone()));
                        Ok(value)
                    }
                    Err(fault) => Err(self.persist_combinator_fault(id, fault)),
                }
            }
            PromiseState::Pending(PromiseKind::Then { .. }) => Err(Fault::Host(
                HostError::Connector("promise chain did not settle".into()),
            )),
            PromiseState::Pending(PromiseKind::Connector) => Err(Fault::Host(
                HostError::Connector("promise did not settle".into()),
            )),
            PromiseState::Pending(PromiseKind::Manual) => Err(Fault::Host(HostError::Connector(
                "promise did not settle".into(),
            ))),
        }
    }

    fn pump(&mut self, target: u64) -> Result<(), HostError> {
        let _depth = DepthGuard::enter(&self.depth, self.max_depth)?;
        loop {
            self.drain()?;
            let Some(state) = self.promises.get(&target).cloned() else {
                return Err(HostError::Data("unknown promise".into()));
            };
            match state {
                PromiseState::Pending(PromiseKind::Connector) => {
                    self.tick()?;
                    self.microtask_streak = 0;
                    match self.receiver.recv_timeout(
                        self.deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(25)),
                    ) {
                        Ok(completion) => self.settle(completion)?,
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => {
                            return Err(HostError::Connector(
                                "connector completion channel closed".into(),
                            ));
                        }
                    }
                }
                PromiseState::Pending(PromiseKind::Manual) => {
                    let next = self.promises.iter().find_map(|(id, state)| match state {
                        PromiseState::Pending(PromiseKind::Then { parent, .. })
                            if matches!(
                                self.promises.get(parent),
                                Some(
                                    PromiseState::Fulfilled(_)
                                        | PromiseState::Rejected(_)
                                        | PromiseState::Failed(_)
                                )
                            ) =>
                        {
                            Some(*id)
                        }
                        _ => None,
                    });
                    let Some(next) = next else {
                        // No pending chain can settle this promise right
                        // now. Keep making progress: poll connector
                        // completions (their then-callbacks may settle this
                        // promise) under the wall deadline and loop. A
                        // genuinely unresolved promise reaches the wall
                        // deadline and fails typed; it can never hang the
                        // call past the budget (zerostack-zksb).
                        self.tick()?;
                        self.microtask_streak = 0;
                        match self.receiver.recv_timeout(
                            self.deadline
                                .saturating_duration_since(Instant::now())
                                .min(Duration::from_millis(25)),
                        ) {
                            Ok(completion) => self.settle(completion)?,
                            Err(RecvTimeoutError::Timeout) => {}
                            Err(RecvTimeoutError::Disconnected) => {
                                return Err(HostError::Connector(
                                    "connector completion channel closed".into(),
                                ));
                            }
                        }
                        continue;
                    };
                    self.pump(next)?;
                }
                PromiseState::Pending(PromiseKind::Then {
                    parent,
                    on_fulfilled,
                    on_rejected,
                }) => {
                    self.microtasks = self.microtasks.saturating_add(1);
                    self.microtask_streak = self.microtask_streak.saturating_add(1);
                    if self.microtask_streak > self.host.limits.microtask_ceiling {
                        return Err(HostError::MicrotaskLimit);
                    }
                    let state = match self.resolve(parent) {
                        Ok(value) => self.apply_then_callback(on_fulfilled, value, false)?,
                        Err(Fault::Throw(value)) => {
                            self.apply_then_callback(on_rejected, value, true)?
                        }
                        Err(Fault::Host(error)) => return Err(error),
                    };
                    self.promises.insert(target, state);
                }
                PromiseState::Pending(PromiseKind::All(_))
                | PromiseState::Pending(PromiseKind::AllSettled(_))
                | PromiseState::Pending(PromiseKind::Race(_))
                | PromiseState::Fulfilled(_)
                | PromiseState::Rejected(_)
                | PromiseState::Failed(_) => return Ok(()),
            }
        }
    }

    fn drain(&mut self) -> Result<(), HostError> {
        loop {
            match self.receiver.try_recv() {
                Ok(completion) => self.settle(completion)?,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }

    fn settle(&mut self, completion: ConnectorCompletionMessage) -> Result<(), HostError> {
        if !matches!(
            self.promises.get(&completion.sequence),
            Some(PromiseState::Pending(PromiseKind::Connector))
        ) {
            return Err(HostError::Connector(format!(
                "unknown or duplicate completion {}",
                completion.sequence
            )));
        }
        self.inflight_connector_calls = self.inflight_connector_calls.saturating_sub(1);
        let sequence = completion.sequence;
        let state = match completion.result {
            Ok(encoded) if encoded.len() > self.host.limits.max_json_bytes => {
                self.complete_operation(sequence, Err("connector result exceeds JSON limit"));
                PromiseState::Rejected(Value::Error(ErrorValue {
                    name: "DataError".into(),
                    message: "connector result exceeds JSON limit".into(),
                }))
            }
            Ok(encoded) => match serde_json::from_str::<JsonValue>(&encoded) {
                Ok(decoded) => {
                    let summary = self.operation_summary(sequence, &decoded);
                    match self.convert_from_json(decoded, true) {
                        Ok(value) => {
                            self.complete_operation(sequence, Ok(summary));
                            PromiseState::Fulfilled(value)
                        }
                        Err(error) => {
                            let detail = error.to_string();
                            self.complete_operation(sequence, Err(&detail));
                            PromiseState::Failed(error)
                        }
                    }
                }
                Err(error) => {
                    let error = HostError::Json(error.to_string());
                    let detail = error.to_string();
                    self.complete_operation(sequence, Err(&detail));
                    PromiseState::Failed(error)
                }
            },
            Err(error) => {
                let detail = error.to_string();
                self.complete_operation(sequence, Err(&detail));
                PromiseState::Rejected(Value::Error(ErrorValue {
                    name: "ToolError".into(),
                    message: detail,
                }))
            }
        };
        self.promises.insert(completion.sequence, state);
        Ok(())
    }

    fn promise_is_settled(&self, id: u64) -> bool {
        matches!(
            self.promises.get(&id),
            Some(PromiseState::Fulfilled(_) | PromiseState::Rejected(_) | PromiseState::Failed(_))
        )
    }

    fn first_settled_race_sibling(&self, ids: &[u64]) -> Option<u64> {
        ids.iter().copied().find(|id| self.promise_is_settled(*id))
    }

    fn first_rejected_all_sibling(&self, ids: &[u64]) -> Option<u64> {
        ids.iter().copied().find(|id| {
            matches!(
                self.promises.get(id),
                Some(PromiseState::Rejected(_) | PromiseState::Failed(_))
            )
        })
    }

    fn collect_fulfilled_all_siblings(&self, ids: &[u64]) -> Option<Vec<Value<'tree>>> {
        let mut values = Vec::with_capacity(ids.len());
        for id in ids {
            match self.promises.get(id) {
                Some(PromiseState::Fulfilled(value)) => values.push(value.clone()),
                _ => return None,
            }
        }
        Some(values)
    }

    fn settled_fault(&self, id: u64) -> Option<Fault<'tree>> {
        match self.promises.get(&id) {
            Some(PromiseState::Rejected(value)) => Some(Fault::Throw(value.clone())),
            Some(PromiseState::Failed(error)) => Some(Fault::Host(error.clone())),
            _ => None,
        }
    }

    fn persist_combinator_fault(&mut self, id: u64, fault: Fault<'tree>) -> Fault<'tree> {
        match &fault {
            Fault::Throw(value) => {
                self.promises
                    .insert(id, PromiseState::Rejected(value.clone()));
            }
            Fault::Host(error) => {
                self.promises
                    .insert(id, PromiseState::Failed(error.clone()));
            }
        }
        fault
    }

    fn combinator_can_settle(&self, kind: &PromiseKind) -> bool {
        match kind {
            PromiseKind::All(child_ids) => {
                child_ids.iter().any(|child| {
                    matches!(
                        self.promises.get(child),
                        Some(PromiseState::Rejected(_) | PromiseState::Failed(_))
                    )
                }) || child_ids.iter().all(|child| {
                    matches!(self.promises.get(child), Some(PromiseState::Fulfilled(_)))
                })
            }
            PromiseKind::AllSettled(child_ids) => child_ids
                .iter()
                .all(|child| self.promise_is_settled(*child)),
            PromiseKind::Race(child_ids) => child_ids
                .iter()
                .any(|child| self.promise_is_settled(*child)),
            PromiseKind::Then { parent, .. } => self.promise_is_settled(*parent),
            PromiseKind::Connector | PromiseKind::Manual => false,
        }
    }

    /// Advance one race child without waiting on a pending host Connector.
    /// Then/All/Race used to `resolve()`/`pump()` here and host-wait, so a
    /// later sibling that settled first still lost (gtoj leftover).
    fn progress_race_child(&mut self, id: u64) -> Result<bool, HostError> {
        match self.promises.get(&id).cloned() {
            Some(PromiseState::Pending(PromiseKind::Then { parent, .. })) => {
                if !self.promise_is_settled(parent) {
                    return Ok(false);
                }
                self.pump(id)?;
                Ok(true)
            }
            Some(PromiseState::Pending(
                kind @ (PromiseKind::All(_) | PromiseKind::AllSettled(_) | PromiseKind::Race(_)),
            )) => {
                let child_ids = match &kind {
                    PromiseKind::All(ids)
                    | PromiseKind::AllSettled(ids)
                    | PromiseKind::Race(ids) => ids.clone(),
                    _ => unreachable!(),
                };
                let mut progressed = false;
                for child in &child_ids {
                    if self.progress_race_child(*child)? {
                        progressed = true;
                    }
                }
                if self.combinator_can_settle(&kind) {
                    return match self.resolve(id) {
                        Ok(_) | Err(Fault::Throw(_)) => Ok(true),
                        Err(Fault::Host(error)) => Err(error),
                    };
                }
                Ok(progressed)
            }
            _ => Ok(false),
        }
    }

    fn race(&mut self, ids: &[u64]) -> Result<u64, HostError> {
        if ids.is_empty() {
            return Err(HostError::Data(
                "Promise.race expects a non-empty array".into(),
            ));
        }
        loop {
            self.drain()?;
            // Already-settled siblings win before Then/All pumps. pitl pumped
            // first, so race([resolve(1).then(x=>x+1), resolve('fast')])
            // returned 2 (zerostack-gtoj).
            if let Some(id) = self.first_settled_race_sibling(ids) {
                return Ok(id);
            }
            // Cheap microtasks only: Then with a settled parent, or a
            // combinator whose children are already terminal. Full
            // resolve()/pump() host-waits on Pending(Connector) and lets
            // the earlier sibling steal the win (zerostack-a6wz).
            let mut progressed = false;
            for id in ids {
                if self.progress_race_child(*id)? {
                    progressed = true;
                    if let Some(winner) = self.first_settled_race_sibling(ids) {
                        return Ok(winner);
                    }
                }
            }
            if progressed {
                continue;
            }
            self.tick()?;
            self.microtask_streak = 0;
            match self.receiver.recv_timeout(
                self.deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(25)),
            ) {
                Ok(completion) => self.settle(completion)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(HostError::Connector(
                        "connector completion channel closed".into(),
                    ));
                }
            }
        }
    }

    /// Fail-fast All: a settled reject/fail wins before host-waiting an
    /// earlier Pending(Connector). Sequential resolve() waited the first
    /// child, so all([slowPing, reject('x')]) timed out instead of
    /// throwing x (zerostack-hzms; a6wz leftover).
    fn all(&mut self, ids: &[u64]) -> Result<Vec<Value<'tree>>, Fault<'tree>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        loop {
            self.drain().map_err(Fault::Host)?;
            if let Some(id) = self.first_rejected_all_sibling(ids) {
                return Err(self.settled_fault(id).ok_or_else(|| {
                    Fault::Host(HostError::Data(
                        "rejected All sibling was not terminal".into(),
                    ))
                })?);
            }
            if let Some(values) = self.collect_fulfilled_all_siblings(ids) {
                return Ok(values);
            }
            let mut progressed = false;
            for id in ids {
                if self.progress_race_child(*id).map_err(Fault::Host)? {
                    progressed = true;
                    if self.first_rejected_all_sibling(ids).is_some() {
                        break;
                    }
                }
            }
            if progressed {
                continue;
            }
            self.tick().map_err(Fault::Host)?;
            self.microtask_streak = 0;
            match self.receiver.recv_timeout(
                self.deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(25)),
            ) {
                Ok(completion) => self.settle(completion).map_err(Fault::Host)?,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Fault::Host(HostError::Connector(
                        "connector completion channel closed".into(),
                    )));
                }
            }
        }
    }

    fn property(&mut self, object: Value<'tree>, key: &str) -> Result<Value<'tree>, Fault<'tree>> {
        match object.clone() {
            Value::Object(value) => {
                let found = {
                    let object = value.borrow();
                    if let Some(result) = object.fields.get(key) {
                        Some(Ok(result.clone()))
                    } else {
                        object.getters.get(key).map(|getter| Err(getter.clone()))
                    }
                };
                match found {
                    Some(Ok(result)) => Ok(result),
                    Some(Err(getter)) => self.call(getter, Vec::new()),
                    None => {
                        let object = value.borrow();
                        match object.access {
                            ObjectAccess::Strict | ObjectAccess::Open => Ok(Value::Undefined),
                        }
                    }
                }
            }
            Value::Namespace(namespace) if namespace == "globalThis" => {
                Ok(self.lookup(key).unwrap_or(Value::Undefined))
            }
            Value::Namespace(namespace) if namespace == "z" => {
                return self.z_property(key);
            }
            Value::Namespace(namespace) => {
                if matches!(key, "then" | "toJSON" | "toString") {
                    return Ok(Value::Undefined);
                }
                if self
                    .host
                    .registration()
                    .capabilities
                    .iter()
                    .any(|capability| capability.surface == namespace && capability.method == key)
                {
                    Ok(Value::Tool(namespace, key.into()))
                } else if self
                    .host
                    .registration()
                    .capabilities
                    .iter()
                    .any(|capability| capability.surface == namespace)
                {
                    Err(Fault::Host(HostError::MethodNotFound(format!(
                        "method_not_found: unknown method '{key}' on {}.{namespace}; closest methods: {}",
                        self.host.registration.root,
                        closest_names(
                            key,
                            self.host
                                .registration
                                .capabilities
                                .iter()
                                .filter(|capability| capability.surface == namespace)
                                .map(|capability| capability.method.as_str())
                        )
                    ))))
                } else {
                    Ok(Value::Method(
                        Box::new(Value::Namespace(namespace)),
                        key.into(),
                    ))
                }
            }
            Value::Array(items) => {
                if key == "length" {
                    Ok(Value::Number(items.borrow().len() as f64))
                } else if let Ok(index) = key.parse::<usize>() {
                    Ok(items
                        .borrow()
                        .get(index)
                        .cloned()
                        .unwrap_or(Value::Undefined))
                } else {
                    Ok(Value::Method(Box::new(Value::Array(items)), key.into()))
                }
            }
            Value::String(value) => {
                if key == "length" {
                    Ok(Value::Number(value.chars().count() as f64))
                } else {
                    Ok(Value::Method(Box::new(Value::String(value)), key.into()))
                }
            }
            Value::Promise(id) if matches!(key, "then" | "catch" | "finally") => {
                Ok(Value::Method(Box::new(Value::Promise(id)), key.into()))
            }
            Value::Promise(_) => Err(Fault::Host(HostError::Data(
                "un-awaited Promise; use await".into(),
            ))),
            Value::Error(error) => match key {
                "name" => Ok(Value::String(error.name)),
                "message" => Ok(Value::String(error.message)),
                _ => Ok(Value::Undefined),
            },
            Value::Null | Value::Undefined => Err(Fault::Host(HostError::Data(format!(
                "cannot read property '{key}' of nullish value"
            )))),
            other => Ok(Value::Method(Box::new(other), key.into())),
        }
    }

    fn call_method(
        &mut self,
        receiver: Value<'tree>,
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        match receiver {
            Value::Namespace(namespace) => self.namespace(&namespace, name, args),
            Value::Array(items) => self.array_method(items, name, args),
            Value::String(value) => self.string_method(&value, name, args),
            Value::Object(object) if name == "hasOwnProperty" => {
                let key = to_key(args.first().unwrap_or(&Value::Undefined));
                let object = object.borrow();
                Ok(Value::Bool(
                    object.fields.contains_key(&key) || object.getters.contains_key(&key),
                ))
            }
            Value::Promise(parent) if name == "then" => {
                let mut args = args.into_iter();
                let on_fulfilled =
                    optional_promise_callback(args.next().unwrap_or(Value::Undefined), "then")?;
                let on_rejected =
                    optional_promise_callback(args.next().unwrap_or(Value::Undefined), "then")?;
                if on_fulfilled.is_none() && on_rejected.is_none() {
                    return Err(Fault::Host(HostError::Data(
                        "Promise.then expects a function".into(),
                    )));
                }
                Ok(self.new_promise(PromiseState::Pending(PromiseKind::Then {
                    parent,
                    on_fulfilled,
                    on_rejected,
                })))
            }
            Value::Promise(parent) if name == "catch" => {
                let on_rejected = optional_promise_callback(
                    args.into_iter().next().unwrap_or(Value::Undefined),
                    "catch",
                )?;
                if on_rejected.is_none() {
                    return Err(Fault::Host(HostError::Data(
                        "Promise.catch expects a function".into(),
                    )));
                }
                Ok(self.new_promise(PromiseState::Pending(PromiseKind::Then {
                    parent,
                    on_fulfilled: None,
                    on_rejected,
                })))
            }
            Value::Promise(_) if name == "finally" => {
                Err(Fault::Host(HostError::UnsupportedSyntax(
                    "Promise.prototype.finally is not supported; use await with try/catch".into(),
                )))
            }
            _ => Err(Fault::Host(self.unsupported_name(name, "method"))),
        }
    }

    fn namespace(
        &mut self,
        namespace: &str,
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        match (namespace, name) {
            ("Promise", "resolve") => Ok(self.new_promise(PromiseState::Fulfilled(
                args.into_iter().next().unwrap_or(Value::Undefined),
            ))),
            ("Promise", "reject") => Ok(self.new_promise(PromiseState::Rejected(
                args.into_iter().next().unwrap_or(Value::Undefined),
            ))),
            ("String", "raw") => Ok(Value::String(to_string(
                args.first().unwrap_or(&Value::Undefined),
            ))),
            ("Promise", name @ ("all" | "allSettled" | "race")) => {
                let values = match args.into_iter().next().unwrap_or(Value::Undefined) {
                    Value::Array(values) => values.borrow().clone(),
                    _ => {
                        return Err(Fault::Host(HostError::Data(format!(
                            "Promise.{name} expects an array"
                        ))));
                    }
                };
                let ids = values
                    .into_iter()
                    .map(|value| match value {
                        Value::Promise(id) => id,
                        value => match self.new_promise(PromiseState::Fulfilled(value)) {
                            Value::Promise(id) => id,
                            _ => unreachable!(),
                        },
                    })
                    .collect();
                let kind = match name {
                    "all" => PromiseKind::All(ids),
                    "allSettled" => PromiseKind::AllSettled(ids),
                    _ => PromiseKind::Race(ids),
                };
                Ok(self.new_promise(PromiseState::Pending(kind)))
            }
            ("Array", "isArray") => Ok(Value::Bool(matches!(args.first(), Some(Value::Array(_))))),
            ("Array", "from") => self.array_from(args),
            ("Object", "keys") => Ok(new_array(object_keys(
                args.first().cloned().unwrap_or(Value::Undefined),
            ))),
            ("Object", "values") => Ok(new_array(object_values(
                args.first().cloned().unwrap_or(Value::Undefined),
            ))),
            ("Object", "entries") => Ok(new_array(object_entries(
                args.first().cloned().unwrap_or(Value::Undefined),
            ))),
            ("Object", "getPrototypeOf") => {
                let value = args.first().unwrap_or(&Value::Undefined);
                if matches!(value, &Value::Null | &Value::Undefined) {
                    Err(Fault::Host(HostError::Data(
                        "Object.getPrototypeOf expects an object".into(),
                    )))
                } else {
                    Ok(Value::Null)
                }
            }
            ("Reflect", "ownKeys") => Ok(new_array(object_keys(
                args.first().cloned().unwrap_or(Value::Undefined),
            ))),
            ("Object", "defineProperty") => object_define_property(args),
            ("JSON", "parse") => {
                let encoded = to_string(args.first().unwrap_or(&Value::Undefined));
                if encoded.len() > self.host.limits.max_json_bytes {
                    return Err(Fault::Host(HostError::Data(
                        "JSON.parse input exceeds JSON limit".into(),
                    )));
                }
                let json: JsonValue = serde_json::from_str(&encoded).map_err(|error| {
                    Fault::Throw(Value::Error(ErrorValue {
                        name: "SyntaxError".into(),
                        message: error.to_string(),
                    }))
                })?;
                self.convert_from_json(json, false).map_err(Fault::Host)
            }
            ("JSON", "stringify") => Ok(Value::String(
                serde_json::to_string(
                    &self
                        .to_json(args.first().unwrap_or(&Value::Undefined))
                        .map_err(Fault::Host)?,
                )
                .map_err(|error| Fault::Host(HostError::Data(error.to_string())))?,
            )),
            ("Math", "max") => Ok(Value::Number(
                args.iter()
                    .filter_map(number)
                    .fold(f64::NEG_INFINITY, f64::max),
            )),
            ("Math", "min") => Ok(Value::Number(
                args.iter().filter_map(number).fold(f64::INFINITY, f64::min),
            )),
            ("Math", "round") => Ok(Value::Number(
                args.first().and_then(number).unwrap_or(0.0).round(),
            )),
            ("Math", "floor") => Ok(Value::Number(
                args.first().and_then(number).unwrap_or(0.0).floor(),
            )),
            ("Math", "ceil") => Ok(Value::Number(
                args.first().and_then(number).unwrap_or(0.0).ceil(),
            )),
            // Wall-clock epoch milliseconds; a clock before the UNIX epoch
            // (misconfigured host) yields 0 instead of failing the cell.
            ("Date", "now") => Ok(Value::Number(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_millis() as f64)
                    .unwrap_or(0.0),
            )),
            ("console", _) => Ok(Value::Undefined),
            ("z", name) => self.z_member(name, args),
            ("z.state", name) => self.z_state_member(name, args),
            _ => Err(Fault::Host(HostError::UnsupportedSyntax(format!(
                "global method {namespace}.{name} is not supported"
            )))),
        }
    }

    /// `z.<property>` resolution: the context data object, the two method
    /// groups, or the default method fallthrough (an unknown member fails
    /// typed at call time in [`Interpreter::z_member`]).
    fn z_property(&mut self, key: &str) -> Result<Value<'tree>, Fault<'tree>> {
        let Some(guest) = &self.host.guest else {
            return Err(Fault::Host(HostError::Data(
                "ZeroKernel guest surface is not installed".into(),
            )));
        };
        match key {
            "context" => {
                let json = guest.context_json();
                self.convert_from_json(json, false).map_err(Fault::Host)
            }
            "state" => Ok(Value::Namespace("z.state".into())),
            // Same promise-shape guard as every other namespace.
            "then" | "toJSON" | "toString" => Ok(Value::Undefined),
            _ => Ok(Value::Method(
                Box::new(Value::Namespace("z".into())),
                key.into(),
            )),
        }
    }

    /// Direct `z.<member>(...)` calls.
    fn z_member(
        &mut self,
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        if self.host.guest.is_none() {
            return Err(Fault::Host(HostError::Data(
                "ZeroKernel guest surface is not installed".into(),
            )));
        }
        self.zero_kernel_member(name, args)
    }

    /// Canonical ZeroKernel direct-z surface. Method names map one-to-one to
    /// typed host operations; no engine namespace or operation catalog exists.
    fn zero_kernel_member(
        &mut self,
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        const DIRECT: &[&str] = &[
            "read", "snap", "write", "edit", "effect", "remove", "transact", "asgrep", "lookup",
            "parallel", "pipeline", "shell", "measure", "project", "compress", "expand", "help",
            "inspect",
        ];
        if !DIRECT.contains(&name) {
            return Err(Fault::Host(HostError::Data(format!(
                "z.{name} is not a ZeroKernel method; methods: {}",
                DIRECT.join(", ")
            ))));
        }
        match name {
            "help" => self.z_help(),
            "inspect" => self.z_inspect(),
            "transact" => self.zero_kernel_transact(args),
            "parallel" => self.zero_kernel_parallel(args),
            "pipeline" => self.zero_kernel_pipeline(args),
            "read" | "snap" | "write" | "edit" | "effect" | "remove" | "asgrep" | "lookup"
            | "shell" | "measure" | "project" | "compress" | "expand" => {
                self.call_tool("z", name, args)
            }
            _ => unreachable!("direct methods are matched above"),
        }
    }

    fn zero_kernel_transact(
        &mut self,
        mut args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        if args.len() != 1 {
            return Err(Fault::Host(HostError::Data(
                "z.transact expects exactly one async thunk".into(),
            )));
        }
        let thunk = args.remove(0);
        if !matches!(thunk, Value::Function(_)) {
            return Err(Fault::Host(HostError::Data(
                "z.transact expects a function".into(),
            )));
        }
        let begin = self.call_tool("z", "__begin_transaction", Vec::new())?;
        self.await_value(begin)?;
        match self
            .call(thunk, Vec::new())
            .and_then(|value| self.await_value(value))
        {
            Ok(value) => {
                let commit = self.call_tool("z", "__commit_transaction", Vec::new())?;
                self.await_value(commit)?;
                Ok(value)
            }
            Err(original) => {
                let rollback = self.call_tool("z", "__rollback_transaction", Vec::new())?;
                match self.await_value(rollback) {
                    Ok(_) => Err(original),
                    Err(rollback_error) => Err(rollback_error),
                }
            }
        }
    }

    fn zero_kernel_parallel(
        &mut self,
        mut args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        if args.len() != 1 {
            return Err(Fault::Host(HostError::Data(
                "z.parallel expects one array of thunks".into(),
            )));
        }
        let thunks = match args.remove(0) {
            Value::Array(values) => values.borrow().clone(),
            _ => {
                return Err(Fault::Host(HostError::Data(
                    "z.parallel expects an array of thunks".into(),
                )));
            }
        };
        let limit = self
            .host
            .guest
            .as_ref()
            .map(|guest| guest.parallel_limit())
            .unwrap_or(zero_abi::PARALLEL_TASK_LIMIT);
        if thunks.is_empty() || thunks.len() > limit {
            return Err(Fault::Host(HostError::Data(format!(
                "z.parallel expects 1..={limit} thunks"
            ))));
        }
        if thunks
            .iter()
            .any(|thunk| !matches!(thunk, Value::Function(_)))
        {
            return Err(Fault::Host(HostError::Data(
                "every z.parallel item must be a function".into(),
            )));
        }
        let group = self.next_parallel_group;
        self.next_parallel_group = self.next_parallel_group.checked_add(1).ok_or_else(|| {
            Fault::Host(HostError::Data("parallel group sequence exhausted".into()))
        })?;
        let previous_group = self.active_parallel_group.replace(group);
        let dispatched = thunks
            .into_iter()
            .map(|thunk| self.call(thunk, Vec::new()))
            .collect::<Result<Vec<_>, _>>();
        self.active_parallel_group = previous_group;
        let values = dispatched?;
        let mut settled = Vec::with_capacity(values.len());
        for value in values {
            settled.push(self.await_value(value)?);
        }
        Ok(new_array(settled))
    }

    fn zero_kernel_pipeline(
        &mut self,
        mut args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        if args.len() < 2 || args.len() - 1 > zero_abi::PIPELINE_STAGE_LIMIT {
            return Err(Fault::Host(HostError::Data(format!(
                "z.pipeline expects items and 1..={} stages",
                zero_abi::PIPELINE_STAGE_LIMIT
            ))));
        }
        let mut items = match args.remove(0) {
            Value::Array(values) => values.borrow().clone(),
            _ => {
                return Err(Fault::Host(HostError::Data(
                    "z.pipeline items must be an array".into(),
                )));
            }
        };
        if args
            .iter()
            .any(|stage| !matches!(stage, Value::Function(_)))
        {
            return Err(Fault::Host(HostError::Data(
                "every z.pipeline stage must be a function".into(),
            )));
        }
        for stage in args {
            let mut pending = Vec::with_capacity(items.len());
            for item in items {
                pending.push(self.call(stage.clone(), vec![item])?);
            }
            let mut next = Vec::with_capacity(pending.len());
            for value in pending {
                next.push(self.await_value(value)?);
            }
            items = next;
        }
        Ok(new_array(items))
    }

    /// Bounded serializable cell state.
    fn z_state_member(
        &mut self,
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        let Some(guest) = &self.host.guest else {
            return Err(Fault::Host(HostError::Data(
                "ZeroKernel guest surface is not installed".into(),
            )));
        };
        let qualified = format!("state.{name}");
        if !zero_abi::GUEST_METHODS.contains(&qualified.as_str()) {
            return Err(Fault::Host(HostError::Data(format!(
                "z.state.{name} is not a ZeroKernel method"
            ))));
        }
        match name {
            "get" => {
                let key = self.guest_state_key(&args)?;
                match guest.state_get(&key) {
                    Ok(Some(value)) => self.convert_from_json(value, false).map_err(Fault::Host),
                    Ok(None) => Ok(Value::Undefined),
                    Err(detail) => Err(Fault::Host(HostError::Data(detail))),
                }
            }
            "has" => {
                let key = self.guest_state_key(&args)?;
                guest
                    .state_has(&key)
                    .map(Value::Bool)
                    .map_err(|detail| Fault::Host(HostError::Data(detail)))
            }
            "set" => {
                let key = self.guest_state_key(&args)?;
                let value = args.get(1).cloned().unwrap_or(Value::Undefined);
                let json = self.to_json(&value).map_err(Fault::Host)?;
                guest
                    .state_set(&key, json)
                    .map(|()| Value::Undefined)
                    .map_err(|detail| Fault::Host(HostError::Data(detail)))
            }
            "delete" => {
                let key = self.guest_state_key(&args)?;
                guest
                    .state_delete(&key)
                    .map(Value::Bool)
                    .map_err(|detail| Fault::Host(HostError::Data(detail)))
            }
            "list" => {
                let keys = guest.state_list();
                Ok(new_array(keys.into_iter().map(Value::String).collect()))
            }
            _ => unreachable!("state members are matched above"),
        }
    }

    fn guest_state_key(&self, args: &[Value<'tree>]) -> Result<String, Fault<'tree>> {
        match args.first() {
            Some(Value::String(key)) => Ok(key.clone()),
            _ => Err(Fault::Host(HostError::Data(
                "z.state member expects a string key as its first argument".into(),
            ))),
        }
    }

    fn z_help(&mut self) -> Result<Value<'tree>, Fault<'tree>> {
        let json = serde_json::json!({
            "surface": "z",
            "methods": zero_abi::GUEST_METHODS,
            "signatures": {
                "asgrep": "z.asgrep(query, {mode?, path?, language?, source?, sink?, limit?}) -> StructuralResult",
                "snap": "z.snap(path | {path?, target?:{path|search}, cardinality?, selection?:{lines|bytes|symbol|exactText}, view?}) -> SnapResult",
                "expand": "z.expand(handle | SnapResult, {bytes?|lines?|symbol?|next?|offset?|limit?|all?}) -> ExpandResult",
                "edit": "z.edit(path | selectedSnap, {find, replacement} | {kind, ...} patch, {expectedPreimage?}) - bare replacement strings are refused",
                "parallel": "z.parallel([async () => operation, ...]) -> results in input order",
                "effect": "z.effect({targets: {name: {path, expect?: 'exists'|'absent'}}, changes: [{target, kind, old?, replacement?, content?, expectedCount?, anchor?}], verify?: {parse?, changedTargetsOnly?, command?: {argv, timeoutMs}}}) -> staged EffectResult",
            },
            "selectedEditPatches": [
                "{find, replacement} on a path target: replaces the first unique occurrence",
                "{find, replacement} on a snap: find must equal the entire snapped selection byte-for-byte including trailing newline",
                "{kind: 'replace_file', content} deliberately replaces a whole file",
                "{kind: 'replace_exact', old, replacement, expectedCount: 1}",
                "{kind: 'replace_lines', content}",
                "{kind: 'insert_before', content}",
                "{kind: 'insert_after', content}"
            ],
            "effectKinds": [
                "replace_exact",
                "replace_file",
                "insert_before",
                "insert_after",
                "create_file",
                "remove_file"
            ],
            "effectVerification": "changedTargetsOnly is enforced; use verify.command argv for language-specific checks",
            "effectComposition": "z.effect must be the cell's only mutation family; use one z.effect or start a separate ZeroKernel call",
        });
        self.convert_from_json(json, false).map_err(Fault::Host)
    }

    fn z_inspect(&mut self) -> Result<Value<'tree>, Fault<'tree>> {
        let Some(guest) = &self.host.guest else {
            return Err(Fault::Host(HostError::Data(
                "ZeroKernel guest surface is not installed".into(),
            )));
        };
        let json = serde_json::json!({
            "protocol": guest.protocol(),
            "context": guest.context_json(),
            "sessionId": guest.session_id(),
            "stateKeys": guest.state_list(),
            "stateBytes": guest.state_bytes(),
            "parallelLimit": guest.parallel_limit(),
        });
        self.convert_from_json(json, false).map_err(Fault::Host)
    }

    /// `Array.from`: resolve the source to (length, element producer) and run
    /// every fuel/length preflight in source order before any allocation.
    fn array_from(&mut self, args: Vec<Value<'tree>>) -> Result<Value<'tree>, Fault<'tree>> {
        let source = args.first().cloned().unwrap_or(Value::Undefined);
        let mapper = args.get(1).cloned();
        if let Some(mapper) = &mapper
            && !matches!(mapper, Value::Function(_))
        {
            return Err(Fault::Host(HostError::Data(
                "Array.from mapper must be a function".into(),
            )));
        }
        // Resolve the source to (length, element producer) without
        // materializing the output, so every length check runs
        // before any allocation.
        let (length, producer): (usize, Box<dyn Fn(usize) -> Value<'tree>>) = match source {
            Value::Array(items) => {
                let length = items.borrow().len();
                (
                    length,
                    Box::new(move |index| {
                        items
                            .borrow()
                            .get(index)
                            .cloned()
                            .unwrap_or(Value::Undefined)
                    }),
                )
            }
            Value::String(value) => {
                let length = value.chars().count();
                let remaining = self
                    .host
                    .limits
                    .instruction_budget
                    .saturating_sub(self.instructions);
                if length as u64 > remaining {
                    return Err(Fault::Host(HostError::FuelExhausted));
                }
                let staged_allocation = length
                    .checked_mul(
                        std::mem::size_of::<char>()
                            .saturating_add(std::mem::size_of::<Value<'tree>>()),
                    )
                    .ok_or_else(|| {
                        Fault::Host(HostError::Data(
                            "Array.from string output length is too large".into(),
                        ))
                    })?;
                if staged_allocation > self.host.limits.memory_bytes {
                    return Err(Fault::Host(HostError::MemoryLimit {
                        requested: staged_allocation,
                        maximum: self.host.limits.memory_bytes,
                    }));
                }
                let mut characters = Vec::new();
                characters.try_reserve_exact(length).map_err(|error| {
                    Fault::Host(HostError::Data(format!(
                        "Array.from string staging could not be reserved: {error}"
                    )))
                })?;
                characters.extend(value.chars());
                (
                    length,
                    Box::new(move |index| {
                        Value::String(characters.get(index).copied().unwrap_or('\0').to_string())
                    }),
                )
            }
            Value::Object(object) => {
                let length = object
                    .borrow()
                    .fields
                    .get("length")
                    .and_then(number)
                    .ok_or_else(|| {
                        Fault::Host(HostError::Data(
                            "Array.from length must be a finite number".into(),
                        ))
                    })?;
                if !length.is_finite() || length < 0.0 {
                    return Err(Fault::Host(HostError::Data(
                        "Array.from length must be finite and non-negative".into(),
                    )));
                }
                let length = length.floor() as usize;
                (
                    length,
                    Box::new(move |index| {
                        object
                            .borrow()
                            .fields
                            .get(&index.to_string())
                            .cloned()
                            .unwrap_or(Value::Undefined)
                    }),
                )
            }
            _ => {
                return Err(Fault::Host(HostError::Data(
                    "Array.from expects an array, string, or array-like object".into(),
                )));
            }
        };
        // Pre-flight length checks: the output allocation must fit
        // the memory limit and the emission loop must fit the
        // remaining fuel, both before any memory is reserved.
        let allocation = length
            .checked_mul(std::mem::size_of::<Value<'tree>>())
            .ok_or_else(|| {
                Fault::Host(HostError::Data(
                    "Array.from output length is too large".into(),
                ))
            })?;
        if allocation > self.host.limits.memory_bytes {
            return Err(Fault::Host(HostError::MemoryLimit {
                requested: allocation,
                maximum: self.host.limits.memory_bytes,
            }));
        }
        let remaining = self
            .host
            .limits
            .instruction_budget
            .saturating_sub(self.instructions);
        if length as u64 > remaining {
            return Err(Fault::Host(HostError::FuelExhausted));
        }
        let mut values = Vec::new();
        values.try_reserve_exact(length).map_err(|error| {
            Fault::Host(HostError::Data(format!(
                "Array.from output allocation could not be reserved: {error}"
            )))
        })?;
        for index in 0..length {
            self.tick()?;
            let item = match &mapper {
                Some(mapper) => self.call(
                    mapper.clone(),
                    vec![producer(index), Value::Number(index as f64)],
                )?,
                None => producer(index),
            };
            values.push(item);
        }
        Ok(new_array(values))
    }

    fn array_method(
        &mut self,
        items: Rc<RefCell<Vec<Value<'tree>>>>,
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        let snapshot = items.borrow().clone();
        match name {
            "join" => Ok(Value::String(
                snapshot
                    .iter()
                    .map(to_string)
                    .collect::<Vec<_>>()
                    .join(&to_string(
                        args.first().unwrap_or(&Value::String(",".into())),
                    )),
            )),
            "slice" => {
                let start = args.first().and_then(number).unwrap_or(0.0).max(0.0) as usize;
                let end = args
                    .get(1)
                    .and_then(number)
                    .map(|value| value.max(0.0) as usize)
                    .unwrap_or(snapshot.len());
                Ok(new_array(
                    snapshot
                        .get(start.min(snapshot.len())..end.min(snapshot.len()))
                        .unwrap_or(&[])
                        .to_vec(),
                ))
            }
            "includes" => Ok(Value::Bool(args.first().is_some_and(|value| {
                snapshot.iter().any(|item| same_value(item, value))
            }))),
            "sort" => {
                let mut sorted = snapshot;
                sorted.sort_by(|left, right| match (left, right) {
                    (&Value::Undefined, &Value::Undefined) => std::cmp::Ordering::Equal,
                    (&Value::Undefined, _) => std::cmp::Ordering::Greater,
                    (_, &Value::Undefined) => std::cmp::Ordering::Less,
                    _ => to_string(left).cmp(&to_string(right)),
                });
                *items.borrow_mut() = sorted;
                Ok(Value::Array(items))
            }
            "indexOf" => Ok(Value::Number(
                args.first()
                    .and_then(|value| snapshot.iter().position(|item| same_value(item, value)))
                    .map(|index| index as f64)
                    .unwrap_or(-1.0),
            )),
            "map" | "filter" | "find" | "findIndex" | "some" | "every" | "forEach" => {
                self.array_callback_method(&snapshot, name, args)
            }
            "push" => {
                let mut target = items.borrow_mut();
                target.extend(args);
                Ok(Value::Number(target.len() as f64))
            }
            _ => Err(Fault::Host(self.unsupported_name(name, "array method"))),
        }
    }

    /// Shared callback loop for the seven callback-taking array methods.
    /// The `_ => Undefined` result tail is **forEach**, not dead code.
    fn array_callback_method(
        &mut self,
        snapshot: &[Value<'tree>],
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        let callback = args
            .first()
            .cloned()
            .ok_or_else(|| Fault::Host(HostError::Data("array callback is required".into())))?;
        let mut output = Vec::new();
        let mut found = None;
        for (index, item) in snapshot.iter().cloned().enumerate() {
            let result = self.call(
                callback.clone(),
                vec![
                    item,
                    Value::Number(index as f64),
                    new_array(snapshot.to_vec()),
                ],
            )?;
            match name {
                "map" => output.push(result),
                "filter" if truthy(&result) => output.push(snapshot[index].clone()),
                "find" if truthy(&result) => {
                    found = Some(snapshot[index].clone());
                    break;
                }
                "findIndex" if truthy(&result) => {
                    found = Some(Value::Number(index as f64));
                    break;
                }
                "some" if truthy(&result) => return Ok(Value::Bool(true)),
                "every" if !truthy(&result) => return Ok(Value::Bool(false)),
                _ => {}
            }
        }
        match name {
            "map" | "filter" => Ok(new_array(output)),
            "find" => Ok(found.unwrap_or(Value::Undefined)),
            "findIndex" => Ok(found.unwrap_or(Value::Number(-1.0))),
            "some" => Ok(Value::Bool(false)),
            "every" => Ok(Value::Bool(true)),
            _ => Ok(Value::Undefined),
        }
    }

    fn string_method(
        &mut self,
        value: &str,
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        match name {
            "repeat" => self.string_repeat(value, &args),
            "padStart" => self.string_pad_start(value, &args),
            "toLowerCase" => Ok(Value::String(value.to_lowercase())),
            "toUpperCase" => Ok(Value::String(value.to_uppercase())),
            "trim" => Ok(Value::String(value.trim().into())),
            "includes" => Ok(Value::Bool(
                value.contains(&to_string(args.first().unwrap_or(&Value::Undefined))),
            )),
            // Advertised in the fallback error below since its introduction,
            // but never implemented — align the arm with the message.
            // Char-indexed to match this interpreter's slice/substring.
            "indexOf" => {
                let needle = to_string(args.first().unwrap_or(&Value::Undefined));
                Ok(Value::Number(match value.find(&needle) {
                    Some(byte_index) => value[..byte_index].chars().count() as f64,
                    None => -1.0,
                }))
            }
            "startsWith" => Ok(Value::Bool(
                value.starts_with(&to_string(args.first().unwrap_or(&Value::Undefined))),
            )),
            "endsWith" => Ok(Value::Bool(
                value.ends_with(&to_string(args.first().unwrap_or(&Value::Undefined))),
            )),
            "split" => {
                let separator = args.first().map(to_string).unwrap_or_default();
                Ok(new_array(if separator.is_empty() {
                    value
                        .chars()
                        .map(|character| Value::String(character.to_string()))
                        .collect()
                } else {
                    value
                        .split(&separator)
                        .map(|part| Value::String(part.into()))
                        .collect()
                }))
            }
            "slice" | "substring" => {
                let start = args.first().and_then(number).unwrap_or(0.0).max(0.0) as usize;
                let end = args
                    .get(1)
                    .and_then(number)
                    .map(|value| value.max(0.0) as usize)
                    .unwrap_or(value.chars().count());
                Ok(Value::String(
                    value
                        .chars()
                        .skip(start)
                        .take(end.saturating_sub(start))
                        .collect(),
                ))
            }
            _ => Err(Fault::Host(HostError::UnsupportedSyntax(format!(
                "string method '{name}' is not supported in CodeMode (supported: includes, indexOf, startsWith, endsWith, split, slice, substring, trim, toLowerCase, toUpperCase, repeat, padStart; compose these instead of match/replace)"
            )))),
        }
    }

    /// `String.repeat`: NaN/zero counts and empty subjects are no-ops;
    /// negative or non-finite counts are errors; allocation is preflighted
    /// against the memory limit before repeating.
    fn string_repeat(
        &self,
        value: &str,
        args: &[Value<'tree>],
    ) -> Result<Value<'tree>, Fault<'tree>> {
        let raw_count = args.first().and_then(number).unwrap_or(0.0);
        if raw_count.is_nan() || raw_count == 0.0 {
            return Ok(Value::String(String::new()));
        }
        if raw_count < 0.0 || !raw_count.is_finite() {
            return Err(Fault::Host(HostError::Data(
                "String.repeat count must be a finite non-negative number".into(),
            )));
        }
        if value.is_empty() {
            return Ok(Value::String(String::new()));
        }
        let count = raw_count.floor();
        let maximum = self.host.limits.memory_bytes / value.len();
        if count >= usize::MAX as f64 || count > maximum as f64 {
            return Err(Fault::Host(HostError::MemoryLimit {
                requested: usize::MAX,
                maximum: self.host.limits.memory_bytes,
            }));
        }
        let count = count as usize;
        let bytes = value.len().checked_mul(count).ok_or_else(|| {
            Fault::Host(HostError::Data(
                "String.repeat allocation is too large".into(),
            ))
        })?;
        if bytes > self.host.limits.memory_bytes {
            return Err(Fault::Host(HostError::MemoryLimit {
                requested: bytes,
                maximum: self.host.limits.memory_bytes,
            }));
        }
        Ok(Value::String(value.repeat(count)))
    }

    /// `String.padStart`: NaN/short/empty-pad targets are no-ops returning the
    /// original; the `!is_finite` check after `<= 0.0` catches **+Inf** and
    /// must stay; UTF-8 remainder bytes are preflighted before allocating.
    fn string_pad_start(
        &self,
        value: &str,
        args: &[Value<'tree>],
    ) -> Result<Value<'tree>, Fault<'tree>> {
        let raw_target = args.first().and_then(number).unwrap_or(0.0);
        if raw_target.is_nan() || raw_target <= 0.0 {
            return Ok(Value::String(value.into()));
        }
        if !raw_target.is_finite() {
            return Err(Fault::Host(HostError::Data(
                "String.padStart target length must be finite".into(),
            )));
        }
        let target = raw_target.floor();
        let current = value.chars().count();
        if target <= current as f64 {
            return Ok(Value::String(value.into()));
        }
        if target >= usize::MAX as f64 {
            return Err(Fault::Host(HostError::Data(
                "String.padStart target length is too large".into(),
            )));
        }
        let target = target as usize;
        let needed = target.saturating_sub(current);
        let pad = match args.get(1) {
            None | Some(&Value::Undefined) => " ".to_owned(),
            Some(value) => to_string(value),
        };
        if pad.is_empty() {
            return Ok(Value::String(value.into()));
        }
        let pad_chars = pad.chars().count();
        let full_repeats = needed / pad_chars;
        let remainder = needed % pad_chars;
        let remainder_bytes = pad
            .chars()
            .take(remainder)
            .map(|character| character.len_utf8())
            .sum::<usize>();
        let padding_bytes = full_repeats
            .checked_mul(pad.len())
            .and_then(|bytes| bytes.checked_add(remainder_bytes))
            .ok_or_else(|| {
                Fault::Host(HostError::Data(
                    "String.padStart allocation is too large".into(),
                ))
            })?;
        let output_bytes = value.len().checked_add(padding_bytes).ok_or_else(|| {
            Fault::Host(HostError::Data(
                "String.padStart allocation is too large".into(),
            ))
        })?;
        if output_bytes > self.host.limits.memory_bytes {
            return Err(Fault::Host(HostError::MemoryLimit {
                requested: output_bytes,
                maximum: self.host.limits.memory_bytes,
            }));
        }
        let mut output = String::with_capacity(output_bytes);
        output.extend(pad.chars().cycle().take(needed));
        output.push_str(value);
        Ok(Value::String(output))
    }

    fn new_promise(&mut self, state: PromiseState<'tree>) -> Value<'tree> {
        let id = self.next_promise;
        self.next_promise = self.next_promise.saturating_add(1);
        self.promises.insert(id, state);
        Value::Promise(id)
    }

    fn new_manual_promise(&mut self, executor: Value<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let promise = self.new_promise(PromiseState::Pending(PromiseKind::Manual));
        let Value::Promise(id) = promise else {
            unreachable!();
        };
        let Value::Function(function) = executor else {
            return Err(Fault::Throw(Value::Error(ErrorValue {
                name: "TypeError".into(),
                message: "Promise executor must be a function".into(),
            })));
        };
        let resolve = Value::Resolver {
            promise: id,
            reject: false,
        };
        let reject = Value::Resolver {
            promise: id,
            reject: true,
        };
        match self.call_function(function, vec![resolve, reject]) {
            Ok(_) => {}
            Err(Fault::Throw(value)) => {
                if matches!(
                    self.promises.get(&id),
                    Some(PromiseState::Pending(PromiseKind::Manual))
                ) {
                    self.promises.insert(id, PromiseState::Rejected(value));
                }
            }
            Err(error) => return Err(error),
        }
        Ok(Value::Promise(id))
    }

    fn lookup(&self, name: &str) -> Option<Value<'tree>> {
        let mut current = Some(self.env.clone());
        while let Some(environment) = current {
            if let Some(value) = environment.borrow().values.get(name) {
                return Some(value.clone());
            }
            current = environment.borrow().parent.clone();
        }
        None
    }

    fn assign(&mut self, node: Node<'tree>, value: Value<'tree>) -> Result<(), HostError> {
        if matches!(node.kind(), "member_expression" | "subscript_expression") {
            let object = node
                .child_by_field_name("object")
                .ok_or_else(|| self.unsupported("assignment object"))?;
            if object.kind() == "identifier" && self.text(object) == "globalThis" {
                let key = if let Some(property) = node.child_by_field_name("property") {
                    self.text(property).to_owned()
                } else if let Some(index) = node.child_by_field_name("index") {
                    to_key(&self.eval(index).map_err(|fault| self.fault(fault))?)
                } else {
                    return Err(self.unsupported("assignment property"));
                };
                self.env.borrow_mut().values.insert(key, value);
                return Ok(());
            }
            let key = if let Some(property) = node.child_by_field_name("property") {
                self.text(property).to_owned()
            } else if let Some(index) = node.child_by_field_name("index") {
                to_key(&self.eval(index).map_err(|fault| self.fault(fault))?)
            } else {
                return Err(self.unsupported("assignment property"));
            };
            let target = self.eval(object).map_err(|fault| self.fault(fault))?;
            return match target {
                Value::Object(target) => {
                    let mut target = target.borrow_mut();
                    match target.access {
                        ObjectAccess::Open => {
                            target.fields.insert(key, value);
                            Ok(())
                        }
                        ObjectAccess::Strict => Err(HostError::Data(format!(
                            "cannot write property '{key}' on a connector result"
                        ))),
                    }
                }
                _ => Err(self.unsupported("assignment target")),
            };
        }
        if node.kind() != "identifier" {
            return Err(self.unsupported("assignment target"));
        }
        let name = self.text(node).to_owned();
        let mut current = Some(self.env.clone());
        while let Some(environment) = current {
            if environment.borrow().values.contains_key(&name) {
                environment.borrow_mut().values.insert(name, value);
                return Ok(());
            }
            current = environment.borrow().parent.clone();
        }
        self.env.borrow_mut().values.insert(name, value);
        Ok(())
    }

    fn convert_from_json(&self, value: JsonValue, strict: bool) -> Result<Value<'tree>, HostError> {
        self.convert_from_json_depth(value, strict, 0)
    }

    /// Depth-aware JSON import. Every nesting level is checked against the
    /// interpreter's derived depth ceiling before recursion, so hostile
    /// connector payloads cannot build unboundedly deep value trees.
    fn convert_from_json_depth(
        &self,
        value: JsonValue,
        strict: bool,
        depth: usize,
    ) -> Result<Value<'tree>, HostError> {
        if depth >= self.max_depth {
            return Err(HostError::Data(format!(
                "JSON parsing depth exceeds the limit of {}",
                self.max_depth
            )));
        }
        Ok(match value {
            JsonValue::Null => Value::Null,
            JsonValue::Bool(value) => Value::Bool(value),
            JsonValue::Number(value) => Value::Number(
                value
                    .as_f64()
                    .ok_or_else(|| HostError::Data("non-finite number".into()))?,
            ),
            JsonValue::String(value) => Value::String(value),
            JsonValue::Array(values) => new_array(
                values
                    .into_iter()
                    .map(|value| self.convert_from_json_depth(value, strict, depth + 1))
                    .collect::<Result<_, _>>()?,
            ),
            JsonValue::Object(values) => Value::Object(Rc::new(RefCell::new(ObjectValue {
                fields: values
                    .into_iter()
                    .map(|(key, value)| {
                        Ok((key, self.convert_from_json_depth(value, strict, depth + 1)?))
                    })
                    .collect::<Result<_, HostError>>()?,
                getters: BTreeMap::new(),
                access: if strict {
                    ObjectAccess::Strict
                } else {
                    ObjectAccess::Open
                },
            }))),
        })
    }

    fn to_json(&self, value: &Value<'tree>) -> Result<JsonValue, HostError> {
        let mut active = BTreeSet::new();
        self.to_json_depth(value, 0, &mut active)
    }

    /// Depth-aware JSON export. Container nesting is capped at the derived
    /// depth ceiling, and active Rc pointers are tracked so a cyclic value
    /// is rejected with a typed data error before recursion can loop.
    fn to_json_depth(
        &self,
        value: &Value<'tree>,
        depth: usize,
        active: &mut BTreeSet<usize>,
    ) -> Result<JsonValue, HostError> {
        if depth >= self.max_depth {
            return Err(HostError::Data(format!(
                "JSON serialization depth exceeds the limit of {}",
                self.max_depth
            )));
        }
        Ok(match value {
            Value::Undefined | Value::Null => JsonValue::Null,
            Value::Bool(value) => JsonValue::Bool(*value),
            Value::Number(value)
                if value.is_finite()
                    && value.fract() == 0.0
                    && value.abs() <= 9_007_199_254_740_991.0 =>
            {
                JsonValue::Number(Number::from(*value as i64))
            }
            Value::Number(value) if value.is_finite() => Number::from_f64(*value)
                .map(JsonValue::Number)
                .ok_or_else(|| HostError::Data("invalid number".into()))?,
            Value::Number(_) => JsonValue::Null,
            Value::String(value) => JsonValue::String(value.clone()),
            Value::Array(values) => {
                let pointer = Rc::as_ptr(values) as usize;
                if !active.insert(pointer) {
                    return Err(HostError::Data(
                        "cyclic value cannot be serialized as JSON".into(),
                    ));
                }
                let result = JsonValue::Array(
                    values
                        .borrow()
                        .iter()
                        .map(|value| self.to_json_depth(value, depth + 1, active))
                        .collect::<Result<_, _>>()?,
                );
                active.remove(&pointer);
                result
            }
            Value::Object(value) => {
                let pointer = Rc::as_ptr(value) as usize;
                if !active.insert(pointer) {
                    return Err(HostError::Data(
                        "cyclic value cannot be serialized as JSON".into(),
                    ));
                }
                let object = value.borrow();
                let result = JsonValue::Object(
                    object
                        .fields
                        .iter()
                        .map(|(key, value)| {
                            Ok((key.clone(), self.to_json_depth(value, depth + 1, active)?))
                        })
                        .collect::<Result<Map<_, _>, HostError>>()?,
                );
                active.remove(&pointer);
                result
            }
            Value::Error(value) => JsonValue::Object(
                [
                    (String::from("name"), JsonValue::String(value.name.clone())),
                    (
                        String::from("message"),
                        JsonValue::String(value.message.clone()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            Value::Unreadable => JsonValue::String("[unreadable]".into()),
            Value::Promise(_)
            | Value::Resolver { .. }
            | Value::Namespace(_)
            | Value::Tool(_, _)
            | Value::Method(_, _)
            | Value::Function(_) => {
                return Err(HostError::Data(
                    "runtime value cannot cross the data boundary; use await and return data"
                        .into(),
                ));
            }
        })
    }
    /// Serialize the public plan result, degrading cycles and unreadable
    /// values instead of failing, and reporting whether degradation occurred.
    fn serialize_public_json(
        &mut self,
        value: &Value<'tree>,
    ) -> Result<(JsonValue, bool), HostError> {
        let mut degraded = false;
        let mut seen = BTreeSet::new();
        let json = self.serialize_public(value, 0, &mut seen, &mut degraded)?;
        Ok((json, degraded))
    }

    fn serialize_public(
        &mut self,
        value: &Value<'tree>,
        depth: usize,
        seen: &mut BTreeSet<usize>,
        degraded: &mut bool,
    ) -> Result<JsonValue, HostError> {
        if depth >= self.max_depth {
            return Err(HostError::Data(format!(
                "result serialization depth exceeds the limit of {}",
                self.max_depth
            )));
        }
        Ok(match value {
            Value::Undefined | Value::Null => JsonValue::Null,
            Value::Bool(value) => JsonValue::Bool(*value),
            Value::Number(value)
                if value.is_finite()
                    && value.fract() == 0.0
                    && value.abs() <= 9_007_199_254_740_991.0 =>
            {
                JsonValue::Number(Number::from(*value as i64))
            }
            Value::Number(value) if value.is_finite() => Number::from_f64(*value)
                .map(JsonValue::Number)
                .ok_or_else(|| HostError::Data("invalid number".into()))?,
            Value::Number(_) => JsonValue::Null,
            Value::String(value) => JsonValue::String(value.clone()),
            Value::Unreadable => {
                *degraded = true;
                JsonValue::String("[unreadable]".into())
            }
            Value::Array(values) => {
                let pointer = Rc::as_ptr(values) as usize;
                if !seen.insert(pointer) {
                    *degraded = true;
                    self.serialize_public(&Value::Unreadable, depth + 1, seen, degraded)?
                } else {
                    let items = values.borrow().clone();
                    let json = JsonValue::Array(
                        items
                            .iter()
                            .map(|item| self.serialize_public(item, depth + 1, seen, degraded))
                            .collect::<Result<_, _>>()?,
                    );
                    seen.remove(&pointer);
                    json
                }
            }
            Value::Object(value) => self.serialize_public_object(value, depth, seen, degraded)?,
            Value::Error(value) => JsonValue::Object(
                [
                    (String::from("name"), JsonValue::String(value.name.clone())),
                    (
                        String::from("message"),
                        JsonValue::String(value.message.clone()),
                    ),
                ]
                .into_iter()
                .collect(),
            ),
            Value::Promise(_)
            | Value::Resolver { .. }
            | Value::Namespace(_)
            | Value::Tool(_, _)
            | Value::Method(_, _)
            | Value::Function(_) => {
                return Err(HostError::Data(
                    "runtime value cannot cross the data boundary; use await and return data"
                        .into(),
                ));
            }
        })
    }

    /// Serialize one user object arm: cycle detection degrades to
    /// `[unreadable]`, getters run once (a throw degrades, a host error
    /// aborts), and fields/getters merge into one sorted key set.
    fn serialize_public_object(
        &mut self,
        value: &Rc<RefCell<ObjectValue<'tree>>>,
        depth: usize,
        seen: &mut BTreeSet<usize>,
        degraded: &mut bool,
    ) -> Result<JsonValue, HostError> {
        let pointer = Rc::as_ptr(value) as usize;
        if !seen.insert(pointer) {
            *degraded = true;
            return self.serialize_public(&Value::Unreadable, depth + 1, seen, degraded);
        }
        let (fields, getters) = {
            let object = value.borrow();
            (object.fields.clone(), object.getters.clone())
        };
        let mut keys = fields.keys().cloned().collect::<BTreeSet<_>>();
        keys.extend(getters.keys().cloned());
        let mut map = Map::new();
        for key in keys {
            let entry = if let Some(getter) = getters.get(&key) {
                match self.call(getter.clone(), Vec::new()) {
                    Ok(result) => self.serialize_public(&result, depth + 1, seen, degraded)?,
                    Err(Fault::Throw(_)) => {
                        *degraded = true;
                        self.serialize_public(&Value::Unreadable, depth + 1, seen, degraded)?
                    }
                    Err(Fault::Host(error)) => return Err(error),
                }
            } else {
                self.serialize_public(&fields[&key], depth + 1, seen, degraded)?
            };
            map.insert(key, entry);
        }
        seen.remove(&pointer);
        Ok(JsonValue::Object(map))
    }

    fn fault(&self, fault: Fault<'tree>) -> HostError {
        match fault {
            Fault::Host(error) => error,
            Fault::Throw(value) => self.throw_error(value),
        }
    }

    fn throw_error(&self, value: Value<'tree>) -> HostError {
        HostError::Execution(format!("Uncaught: {}", to_string(&value)))
    }

    fn text(&self, node: Node<'tree>) -> &str {
        node.utf8_text(self.source.as_bytes()).unwrap_or("")
    }

    fn operator(&self, node: Node<'tree>) -> &str {
        if let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) {
            return self.source[left.end_byte()..right.start_byte()].trim();
        }
        if let Some(argument) = node.child_by_field_name("argument") {
            let prefix = self.source[node.start_byte()..argument.start_byte()].trim();
            if !prefix.is_empty() {
                return prefix;
            }
            return self.source[argument.end_byte()..node.end_byte()].trim();
        }
        ""
    }

    fn unsupported(&self, kind: &str) -> HostError {
        HostError::UnsupportedSyntax(format!("Syntax '{kind}' is not supported in CodeMode"))
    }

    fn unsupported_name(&self, name: &str, context: &str) -> HostError {
        HostError::UnsupportedSyntax(format!("{context} '{name}' is not supported in CodeMode"))
    }
}

fn optional_promise_callback<'tree>(
    value: Value<'tree>,
    method: &str,
) -> Result<Option<Value<'tree>>, Fault<'tree>> {
    match value {
        Value::Function(_) => Ok(Some(value)),
        Value::Undefined | Value::Null => Ok(None),
        _ => Err(Fault::Host(HostError::Data(format!(
            "Promise.{method} expects a function"
        )))),
    }
}

fn closest_names<'a>(target: &str, names: impl Iterator<Item = &'a str>) -> String {
    let mut names = names.collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names.sort_by_key(|name| (edit_distance(target, name), *name));
    let closest = names.into_iter().take(3).collect::<Vec<_>>();
    if closest.is_empty() {
        "(none)".into()
    } else {
        closest.join(", ")
    }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut row = (0..=right.len()).collect::<Vec<_>>();
    for (left_index, left_byte) in left.bytes().enumerate() {
        let mut next = vec![left_index + 1];
        for (right_index, right_byte) in right.bytes().enumerate() {
            next.push(
                (next[right_index] + 1)
                    .min(row[right_index + 1] + 1)
                    .min(row[right_index] + usize::from(left_byte != right_byte)),
            );
        }
        row = next;
    }
    row[right.len()]
}

fn new_array<'tree>(values: Vec<Value<'tree>>) -> Value<'tree> {
    Value::Array(Rc::new(RefCell::new(values)))
}

fn settled<'tree>(ok: bool, value: Value<'tree>) -> Value<'tree> {
    let mut fields = BTreeMap::new();
    fields.insert(
        "status".into(),
        Value::String(if ok { "fulfilled" } else { "rejected" }.into()),
    );
    fields.insert(if ok { "value" } else { "reason" }.into(), value);
    Value::Object(Rc::new(RefCell::new(ObjectValue {
        fields,
        getters: BTreeMap::new(),
        access: ObjectAccess::Open,
    })))
}

fn object_keys<'tree>(value: Value<'tree>) -> Vec<Value<'tree>> {
    match value {
        Value::Object(value) => value
            .borrow()
            .fields
            .keys()
            .cloned()
            .map(Value::String)
            .collect(),
        Value::Array(value) => (0..value.borrow().len())
            .map(|index| Value::String(index.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn object_values<'tree>(value: Value<'tree>) -> Vec<Value<'tree>> {
    match value {
        Value::Object(value) => value.borrow().fields.values().cloned().collect(),
        Value::Array(value) => value.borrow().clone(),
        _ => Vec::new(),
    }
}

fn operation_target(method: &str, args: &JsonValue) -> Option<String> {
    let args = args.as_array()?;
    let first = args.first()?;
    let target = match method {
        "asgrep" => first.as_str().map(str::to_owned),
        "shell" => match first {
            JsonValue::String(command) => Some(command.clone()),
            JsonValue::Array(argv) => Some(
                argv.iter()
                    .filter_map(JsonValue::as_str)
                    .take(6)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        },
        "effect" => first
            .get("targets")
            .and_then(JsonValue::as_object)
            .map(|targets| {
                targets
                    .values()
                    .filter_map(|target| target.get("path").and_then(JsonValue::as_str))
                    .take(4)
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
        _ => operation_path(first),
    }?;
    (!target.is_empty()).then(|| truncate_operation_text(&target))
}

fn operation_path(value: &JsonValue) -> Option<String> {
    if let Some(path) = value.as_str() {
        return Some(path.to_owned());
    }
    value
        .get("path")
        .and_then(JsonValue::as_str)
        .or_else(|| {
            value
                .get("target")
                .and_then(|target| target.get("path"))
                .and_then(JsonValue::as_str)
        })
        .or_else(|| {
            value
                .get("target")
                .and_then(|target| target.get("search"))
                .and_then(|search| search.get("query"))
                .and_then(JsonValue::as_str)
        })
        .or_else(|| value.get("source").and_then(JsonValue::as_str))
        .map(str::to_owned)
}

fn operation_result_summary(method: &str, value: &JsonValue) -> OperationSummary {
    let mut summary = OperationSummary::default();
    match method {
        "read" => {
            summary.detail = value
                .as_str()
                .map(|text| format!("{} bytes visible", text.len()));
        }
        "lookup" => {
            summary.result_count = value.as_array().map(|items| items.len() as u64);
            summary.detail = summary.result_count.map(|count| format!("{count} paths"));
        }
        "asgrep" => {
            summary.result_count = value
                .get("hits")
                .and_then(JsonValue::as_array)
                .map(|hits| hits.len() as u64);
            let complete = value
                .get("complete")
                .and_then(JsonValue::as_bool)
                .unwrap_or(false);
            summary.detail = summary.result_count.map(|count| {
                format!(
                    "{count} hits · {}",
                    if complete { "complete" } else { "incomplete" }
                )
            });
        }
        "snap" => {
            summary.target = value
                .get("path")
                .and_then(JsonValue::as_str)
                .map(str::to_owned);
            let kind = value
                .get("selection")
                .and_then(|selection| selection.get("kind"))
                .and_then(JsonValue::as_str)
                .unwrap_or("full_file");
            let visible = value
                .get("view")
                .and_then(|view| view.get("visibleBytes"))
                .and_then(JsonValue::as_u64)
                .unwrap_or(0);
            summary.detail = Some(format!("{kind} · {visible} bytes visible"));
        }
        "write" | "edit" | "remove" => {
            summary.target = value
                .get("path")
                .and_then(JsonValue::as_str)
                .map(str::to_owned);
            summary.detail = value
                .get("kind")
                .and_then(JsonValue::as_str)
                .map(str::to_owned);
            summary.changed_files = Some(1);
        }
        "effect" => {
            summary.changed_files = value
                .get("changedFiles")
                .and_then(JsonValue::as_u64)
                .and_then(|count| u32::try_from(count).ok());
            summary.detail = summary.changed_files.map(|count| {
                let verified = value
                    .get("verification")
                    .and_then(|verification| verification.get("changedTargetsOnly"))
                    .and_then(JsonValue::as_bool)
                    .unwrap_or(false);
                format!(
                    "{count} files · {}",
                    if verified { "verified" } else { "unverified" }
                )
            });
        }
        "expand" => {
            let start = value.get("byteStart").and_then(JsonValue::as_u64);
            let end = value.get("byteEnd").and_then(JsonValue::as_u64);
            if let (Some(start), Some(end)) = (start, end) {
                summary.detail = Some(format!("bytes {start}..{end}"));
            }
        }
        "shell" => {
            summary.detail = value
                .get("status")
                .and_then(JsonValue::as_i64)
                .map(|status| format!("exit {status}"));
        }
        "measure" => {
            summary.detail = value
                .get("billed")
                .and_then(JsonValue::as_u64)
                .map(|tokens| format!("{tokens} tokens"));
        }
        "project" | "compress" => {
            summary.detail = value
                .get("visible")
                .and_then(JsonValue::as_str)
                .map(|visible| format!("{} bytes visible", visible.len()));
        }
        _ => {}
    }
    summary
}

fn truncate_operation_text(text: &str) -> String {
    const LIMIT: usize = 1_024;
    if text.len() <= LIMIT {
        return text.to_owned();
    }
    let mut end = LIMIT.saturating_sub(3);
    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    format!("{}...", &text[..end])
}

fn object_entries<'tree>(value: Value<'tree>) -> Vec<Value<'tree>> {
    match value {
        Value::Object(value) => value
            .borrow()
            .fields
            .iter()
            .map(|(key, value)| new_array(vec![Value::String(key.clone()), value.clone()]))
            .collect(),
        _ => Vec::new(),
    }
}

fn number(value: &Value<'_>) -> Option<f64> {
    match value {
        Value::Number(value) => Some(*value),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn truthy(value: &Value<'_>) -> bool {
    match value {
        Value::Undefined | Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => *value != 0.0 && !value.is_nan(),
        Value::String(value) => !value.is_empty(),
        _ => true,
    }
}

fn to_string(value: &Value<'_>) -> String {
    to_string_depth(value, &mut BTreeSet::new(), 0)
}

/// Coercion with an independent pointer/depth guard. Arrays can reference
/// themselves (for example via `push`), and string, template, join, sort,
/// and error formatting all recurse through this helper, so cycles and
/// excessive nesting fall back to a JS-like empty element instead of
/// overflowing the native stack.
fn to_string_depth(value: &Value<'_>, active: &mut BTreeSet<usize>, depth: usize) -> String {
    if depth >= MAX_TO_STRING_DEPTH {
        return String::new();
    }
    match value {
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(values) => {
            let pointer = Rc::as_ptr(values) as usize;
            if !active.insert(pointer) {
                return String::new();
            }
            let joined = values
                .borrow()
                .iter()
                .map(|item| to_string_depth(item, active, depth + 1))
                .collect::<Vec<_>>()
                .join(",");
            active.remove(&pointer);
            joined
        }
        Value::Error(value) => value.message.clone(),
        _ => "[object Object]".into(),
    }
}

fn to_key(value: &Value<'_>) -> String {
    match value {
        Value::Number(value) => (*value as usize).to_string(),
        _ => to_string(value),
    }
}

/// `Object.defineProperty`: clone get/value out of the descriptor **before**
/// taking `borrow_mut` on the target. The same object can be both target and
/// descriptor (`Object.defineProperty(o, "x", o)`); holding borrow_mut +
/// borrow on one RefCell panics.
fn object_define_property<'tree>(args: Vec<Value<'tree>>) -> Result<Value<'tree>, Fault<'tree>> {
    let mut iter = args.into_iter();
    let target = iter.next().unwrap_or(Value::Undefined);
    let key = to_key(&iter.next().unwrap_or(Value::Undefined));
    let descriptor = iter.next().unwrap_or(Value::Undefined);
    let Value::Object(target) = target else {
        return Err(Fault::Host(HostError::Data(
            "Object.defineProperty target must be a mutable user object".into(),
        )));
    };
    let defined = match &descriptor {
        Value::Object(descriptor) => {
            let descriptor = descriptor.borrow();
            if let Some(getter) = descriptor.fields.get("get").cloned() {
                Some(Ok(getter))
            } else if let Some(value) = descriptor.fields.get("value").cloned() {
                Some(Err(value))
            } else {
                None
            }
        }
        _ => {
            return Err(Fault::Host(HostError::Data(
                "Object.defineProperty descriptor must be an object".into(),
            )));
        }
    };
    {
        let mut object = target.borrow_mut();
        if !matches!(object.access, ObjectAccess::Open) {
            return Err(Fault::Host(HostError::Data(format!(
                "cannot define property '{key}' on an immutable object"
            ))));
        }
        match defined {
            Some(Ok(getter)) => {
                object.getters.insert(key, getter);
            }
            Some(Err(value)) => {
                object.fields.insert(key, value);
            }
            None => {
                return Err(Fault::Host(HostError::Data(
                    "Object.defineProperty descriptor must provide get or value".into(),
                )));
            }
        }
    }
    Ok(Value::Object(target))
}

fn same_value(left: &Value<'_>, right: &Value<'_>) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) | (Value::Undefined, Value::Undefined) => true,
        (Value::Bool(left), Value::Bool(right)) => left == right,
        (Value::Number(left), Value::Number(right)) => left == right,
        (Value::String(left), Value::String(right)) => left == right,
        _ => false,
    }
}

fn unary<'tree>(operator: &str, value: Value<'tree>) -> Result<Value<'tree>, HostError> {
    match operator {
        "!" => Ok(Value::Bool(!truthy(&value))),
        "-" => Ok(Value::Number(-number(&value).unwrap_or(0.0))),
        "+" => Ok(Value::Number(number(&value).unwrap_or(0.0))),
        "typeof" => Ok(Value::String(
            match value {
                Value::Undefined => "undefined",
                Value::Bool(_) => "boolean",
                Value::Number(_) => "number",
                Value::String(_) => "string",
                Value::Function(_) | Value::Tool(_, _) | Value::Method(_, _) => "function",
                _ => "object",
            }
            .into(),
        )),
        _ => Err(HostError::UnsupportedSyntax(format!(
            "unary operator '{operator}' is not supported"
        ))),
    }
}

fn binary<'tree>(
    operator: &str,
    left: Value<'tree>,
    right: Value<'tree>,
) -> Result<Value<'tree>, HostError> {
    match operator {
        "+" => {
            if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) {
                Ok(Value::String(format!(
                    "{}{}",
                    to_string(&left),
                    to_string(&right)
                )))
            } else {
                Ok(Value::Number(
                    number(&left).unwrap_or(0.0) + number(&right).unwrap_or(0.0),
                ))
            }
        }
        "-" => Ok(Value::Number(
            number(&left).unwrap_or(0.0) - number(&right).unwrap_or(0.0),
        )),
        "*" => Ok(Value::Number(
            number(&left).unwrap_or(0.0) * number(&right).unwrap_or(0.0),
        )),
        "/" => Ok(Value::Number(
            number(&left).unwrap_or(0.0) / number(&right).unwrap_or(0.0),
        )),
        "%" => Ok(Value::Number(
            number(&left).unwrap_or(0.0) % number(&right).unwrap_or(0.0),
        )),
        "===" | "==" => Ok(Value::Bool(same_value(&left, &right))),
        "!==" | "!=" => Ok(Value::Bool(!same_value(&left, &right))),
        "<" => Ok(Value::Bool(relational(&left, &right, |ordering| {
            ordering.is_lt()
        }))),
        ">" => Ok(Value::Bool(relational(&left, &right, |ordering| {
            ordering.is_gt()
        }))),
        "<=" => Ok(Value::Bool(relational(&left, &right, |ordering| {
            ordering.is_le()
        }))),
        ">=" => Ok(Value::Bool(relational(&left, &right, |ordering| {
            ordering.is_ge()
        }))),
        _ => Err(HostError::UnsupportedSyntax(format!(
            "binary operator '{operator}' is not supported"
        ))),
    }
}

fn relational<'tree>(
    left: &Value<'tree>,
    right: &Value<'tree>,
    predicate: impl FnOnce(std::cmp::Ordering) -> bool,
) -> bool {
    let ordering = match (left, right) {
        (Value::String(left), Value::String(right)) => Some(left.cmp(right)),
        _ => number(left).and_then(|left| number(right).and_then(|right| left.partial_cmp(&right))),
    };
    ordering.is_some_and(predicate)
}

/// Decode a quoted string literal body, processing every JavaScript escape
/// sequence exactly once, left to right. Every possible character after a
/// backslash is handled explicitly, so no escape can silently pass through
/// as raw text (the corruption class behind pc_78f6e48133fb).
fn unquote(value: &str) -> Result<String, HostError> {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return decode_string_body(&value[1..value.len() - 1]);
        }
    }
    Ok(value.into())
}

/// Build the typed error for an escape sequence CodeMode refuses to decode.
fn unsupported_escape(escape: &str) -> HostError {
    HostError::UnsupportedSyntax(format!(
        "escape sequence '{escape}' is not supported in CodeMode"
    ))
}

/// Scan a string body and decode every backslash escape in one pass.
fn decode_string_body(body: &str) -> Result<String, HostError> {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(slash) = rest.find('\\') {
        out.push_str(&rest[..slash]);
        let after = &rest[slash + 1..];
        let (decoded, consumed) = apply_escape(after)?;
        if let Some(character) = decoded {
            out.push(character);
        }
        rest = &after[consumed..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Decode one escape sequence given the text after its leading backslash.
/// Returns the decoded character (None for line continuations) and how many
/// characters of `after` were consumed.
fn apply_escape(after: &str) -> Result<(Option<char>, usize), HostError> {
    let mut characters = after.chars();
    let first = characters
        .next()
        .ok_or_else(|| HostError::UnsupportedSyntax("string ends with a lone backslash".into()))?;
    let single = |character: char| Ok((Some(character), 1));
    match first {
        '\'' | '"' | '\\' | '`' => single(first),
        'n' => single('\n'),
        'r' => single('\r'),
        't' => single('\t'),
        'b' => single('\u{8}'),
        '0' => {
            if characters.as_str().starts_with(|c: char| c.is_ascii_digit()) {
                let shown: String = after.chars().take(4).collect();
                Err(unsupported_escape(&shown))
            } else {
                single('\0')
            }
        }
        digit @ '1'..='9' => {
            // Strict-mode JS rejects \\1-\\9 and legacy octal; so do we,
            // rather than silently emitting the digit as text.
            let _ = digit;
            let shown: String = after.chars().take(2).collect();
            Err(unsupported_escape(&shown))
        }
        'x' => {
            let hex = after.get(1..3).ok_or_else(|| unsupported_escape("\\x"))?;
            let code = parse_hex(hex).ok_or_else(|| unsupported_escape(&format!("\\x{hex}")))?;
            let decoded = char::from_u32(code).ok_or_else(|| unsupported_escape(&format!("\\x{hex}")))?;
            Ok((Some(decoded), 3))
        }
        'u' => {
            let rest = characters.as_str();
            if let Some(braced) = rest.strip_prefix('{') {
                let inner = braced
                    .split_once('}')
                    .ok_or_else(|| unsupported_escape(&format!("\\u{rest}")))?;
                if inner.0.is_empty() || inner.0.len() > 6 {
                    return Err(unsupported_escape(&format!("\\u{{{}}}", inner.0)));
                }
                let code = parse_hex(inner.0)
                    .ok_or_else(|| unsupported_escape(&format!("\\u{{{}}}", inner.0)))?;
                let decoded = char::from_u32(code)
                    .ok_or_else(|| unsupported_escape(&format!("\\u{{{}}}", inner.0)))?;
                let consumed = 1 + inner.0.len() + 2;
                Ok((Some(decoded), consumed))
            } else {
                let hex = after.get(1..5).ok_or_else(|| unsupported_escape("\\u"))?;
                let code = parse_hex(hex).ok_or_else(|| unsupported_escape(&format!("\\u{hex}")))?;
                let decoded = char::from_u32(code).ok_or_else(|| unsupported_escape(&format!("\\u{hex}")))?;
                Ok((Some(decoded), 5))
            }
        }
        // ECMA-262 NonEscapeCharacter: the escaped character itself.
        other => single(other),
    }
}

/// Decode one complete escape_sequence node body, including its backslash.
fn decode_escape_node(text: &str) -> Result<Option<char>, HostError> {
    let after = text
        .strip_prefix('\\')
        .ok_or_else(|| HostError::UnsupportedSyntax(format!("invalid escape sequence '{text}'")))?;
    apply_escape(after).map(|(decoded, _)| decoded)
}

fn parse_hex(text: &str) -> Option<u32> {
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(text, 16).ok()
}
