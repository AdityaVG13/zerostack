//! Restricted, owned JavaScript-subset interpreter.
//!
//! The parser is Tree-sitter JavaScript. The evaluator owns the supported
//! value space and exposes only the registered capability tree. It never
//! evaluates source as host code.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::{Map, Number, Value as JsonValue};
use tree_sitter::{Node, Parser};
use tree_sitter_javascript::LANGUAGE;

use crate::host::{
    ConnectorCompletionMessage, directly_expands_one_spill_ref, is_terminal_exact_token_expansion,
    normalize_public_result, spill_result,
};
use crate::{
    Connector, ConnectorCompletion, DispatchContext, Host, HostError, MAX_INFLIGHT_CONNECTOR_CALLS,
};

static INTERPRETER_CREATIONS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn interpreter_creation_count() -> u64 {
    INTERPRETER_CREATIONS.load(Ordering::Relaxed)
}

#[derive(Clone, Debug)]
enum Value<'tree> {
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Rc<RefCell<Vec<Value<'tree>>>>),
    Object(ObjectValue<'tree>),
    Namespace(String),
    Tool(String, String),
    Method(Box<Value<'tree>>, String),
    Function(FunctionValue<'tree>),
    Promise(u64),
    Error(ErrorValue),
}

#[derive(Clone, Debug)]
struct ObjectValue<'tree> {
    fields: BTreeMap<String, Value<'tree>>,
    strict: bool,
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
}

#[derive(Clone, Debug)]
enum PromiseKind<'tree> {
    Connector,
    Then {
        parent: u64,
        on_fulfilled: Value<'tree>,
    },
    All(Vec<u64>),
    AllSettled(Vec<u64>),
    Race(Vec<u64>),
}

enum Fault<'tree> {
    Host(HostError),
    Throw(Value<'tree>),
}

enum Control<'tree> {
    Normal,
    Return(Value<'tree>),
    Break,
    Continue,
    Throw(Value<'tree>),
}

pub(super) fn execute(
    host: &Host,
    source: &str,
    connector: Rc<dyn Connector>,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<JsonValue, HostError> {
    crate::wrap::validate_plan(source, host.limits.max_plan_bytes).map_err(HostError::Plan)?;
    let mut parser = Parser::new();
    parser
        .set_language(&LANGUAGE.into())
        .map_err(|error| HostError::Runtime(format!("JavaScript parser setup failed: {error}")))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| HostError::Parse("parser returned no syntax tree".into()))?;
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
    );
    let value = interpreter.run()?;
    let encoded = serde_json::to_string(&interpreter.to_json(&value)?)
        .map_err(|error| HostError::Json(error.to_string()))?;
    let public: JsonValue =
        serde_json::from_str(&encoded).map_err(|error| HostError::Json(error.to_string()))?;
    if encoded.len() > host.max_visible_result_bytes
        && !(directly_expands_one_spill_ref(source) && is_terminal_exact_token_expansion(&public))
    {
        return match host.spill_root.as_deref() {
            Some(root)
                if host
                    .registration
                    .capabilities
                    .iter()
                    .any(|cap| cap.surface == "token" && cap.method == "expand") =>
            {
                spill_result(root, &encoded)
            }
            Some(_) => Err(HostError::ResultSpill(
                "token.expand capability is required before publishing a result ref".into(),
            )),
            None => Err(HostError::ResultTooLarge {
                actual: encoded.len(),
                maximum: host.max_visible_result_bytes,
            }),
        };
    }
    Ok(public)
}

struct Interpreter<'tree> {
    host: &'tree Host,
    source: &'tree str,
    root: Node<'tree>,
    connector: Rc<dyn Connector>,
    receiver: Receiver<ConnectorCompletionMessage>,
    sender: SyncSender<ConnectorCompletionMessage>,
    promises: BTreeMap<u64, PromiseState<'tree>>,
    next_promise: u64,
    env: EnvRef<'tree>,
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
    instructions: u64,
    microtasks: usize,
}

impl<'tree> Interpreter<'tree> {
    fn new(
        host: &'tree Host,
        source: &'tree str,
        root: Node<'tree>,
        connector: Rc<dyn Connector>,
        cancelled: Arc<AtomicBool>,
        timeout: Duration,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::sync_channel(MAX_INFLIGHT_CONNECTOR_CALLS);
        let env = Rc::new(RefCell::new(Env {
            values: BTreeMap::new(),
            parent: None,
        }));
        let mut interpreter = Self {
            host,
            source,
            root,
            connector,
            receiver,
            sender,
            promises: BTreeMap::new(),
            next_promise: 1,
            env,
            cancelled,
            deadline: Instant::now() + timeout,
            instructions: 0,
            microtasks: 0,
        };
        interpreter.install_globals();
        interpreter
    }

    fn install_globals(&mut self) {
        let mut env = self.env.borrow_mut();
        let mut surfaces = BTreeMap::new();
        for capability in &self.host.registration.capabilities {
            surfaces
                .entry(capability.surface.clone())
                .or_insert_with(|| Value::Namespace(capability.surface.clone()));
        }
        env.values.insert(
            self.host.registration.root.clone(),
            Value::Object(ObjectValue {
                fields: surfaces,
                strict: true,
            }),
        );
        for name in [
            "Object",
            "Math",
            "JSON",
            "Array",
            "Date",
            "RegExp",
            "Map",
            "Set",
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
    }

    fn run(&mut self) -> Result<Value<'tree>, HostError> {
        match self.exec(self.root)? {
            Control::Return(value) => Ok(value),
            Control::Normal => Ok(Value::Undefined),
            Control::Throw(value) => Err(self.throw_error(value)),
            Control::Break | Control::Continue => Err(HostError::UnsupportedSyntax(
                "loop control escaped its loop".into(),
            )),
        }
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

    fn exec(&mut self, node: Node<'tree>) -> Result<Control<'tree>, HostError> {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let result = self.statement(child)?;
            if !matches!(result, Control::Normal) {
                return Ok(result);
            }
        }
        Ok(Control::Normal)
    }

    fn statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, HostError> {
        self.tick()?;
        match node.kind() {
            "program" | "statement_block" => self.exec(node),
            "expression_statement" => {
                if let Some(expression) = node.named_child(0) {
                    self.eval_host(expression)?;
                }
                Ok(Control::Normal)
            }
            "return_statement" => Ok(Control::Return(
                node.named_child(0)
                    .map(|child| self.eval_host(child))
                    .transpose()?
                    .unwrap_or(Value::Undefined),
            )),
            "lexical_declaration" | "variable_declaration" | "using_declaration" => {
                self.declare(node)?;
                Ok(Control::Normal)
            }
            "if_statement" => {
                let condition = self.eval_host(
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
                self.eval_host(
                    node.named_child(0)
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
            _ => Err(self.unsupported(node.kind())),
        }
    }

    fn declare(&mut self, node: Node<'tree>) -> Result<(), HostError> {
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
                .map(|child| self.eval_host(child))
                .transpose()?
                .unwrap_or(Value::Undefined);
            self.bind(name, value);
        }
        Ok(())
    }

    fn bind(&mut self, node: Node<'tree>, value: Value<'tree>) {
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
                    for part in node.named_children(&mut cursor) {
                        let Some(key) = part
                            .child_by_field_name("key")
                            .or_else(|| part.child_by_field_name("name"))
                        else {
                            continue;
                        };
                        let field = object
                            .fields
                            .get(self.text(key))
                            .cloned()
                            .unwrap_or(Value::Undefined);
                        let target = part.child_by_field_name("value").unwrap_or(key);
                        self.bind(target, field);
                    }
                }
            }
            "array_pattern" => {
                if let Value::Array(items) = value {
                    let items = items.borrow();
                    let mut cursor = node.walk();
                    for (index, part) in node.named_children(&mut cursor).enumerate() {
                        self.bind(part, items.get(index).cloned().unwrap_or(Value::Undefined));
                    }
                }
            }
            _ => {}
        }
    }

    fn for_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, HostError> {
        if let Some(initializer) = node.child_by_field_name("initializer") {
            if initializer.kind().ends_with("declaration") {
                self.declare(initializer)?;
            } else {
                self.eval_host(initializer)?;
            }
        }
        loop {
            self.tick()?;
            if let Some(condition) = node.child_by_field_name("condition") {
                if !truthy(&self.eval_host(condition)?) {
                    break;
                }
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
            {
                self.eval_host(update)?;
            }
        }
        Ok(Control::Normal)
    }

    fn for_in_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, HostError> {
        let source = self.eval_host(
            node.child_by_field_name("right")
                .ok_or_else(|| self.unsupported("for-in without source"))?,
        )?;
        let is_of = self.text(node).contains(" of ");
        let items = match (is_of, source) {
            (true, Value::Array(items)) => items.borrow().clone(),
            (false, Value::Array(items)) => (0..items.borrow().len())
                .map(|index| Value::String(index.to_string()))
                .collect(),
            (false, Value::Object(object)) => {
                object.fields.keys().cloned().map(Value::String).collect()
            }
            _ => {
                return Err(HostError::Data(
                    "for-in/of requires an array or object".into(),
                ));
            }
        };
        let left = node
            .child_by_field_name("left")
            .ok_or_else(|| self.unsupported("for-in without binding"))?;
        for item in items {
            if left.kind().ends_with("declaration") {
                self.bind(
                    left.child_by_field_name("name")
                        .ok_or_else(|| self.unsupported("for-in binding"))?,
                    item,
                );
            } else {
                self.assign(left, item)?;
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

    fn while_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, HostError> {
        let do_first = node.kind() == "do_statement";
        let mut first = true;
        loop {
            self.tick()?;
            if !do_first || !first {
                let condition = self.eval_host(
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

    fn try_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, HostError> {
        let mut result = self.statement(
            node.child_by_field_name("body")
                .ok_or_else(|| self.unsupported("try without body"))?,
        )?;
        if let Control::Throw(value) = &result {
            let value = value.clone();
            if let Some(handler) = node.child_by_field_name("handler") {
                if let Some(parameter) = handler.child_by_field_name("parameter") {
                    self.bind(parameter, value);
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

    fn switch_statement(&mut self, node: Node<'tree>) -> Result<Control<'tree>, HostError> {
        let target = self.eval_host(
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
                let value = self.eval_host(
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

    fn eval_host(&mut self, node: Node<'tree>) -> Result<Value<'tree>, HostError> {
        self.eval(node).map_err(|fault| self.fault(fault))
    }

    fn eval(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        self.tick().map_err(Fault::Host)?;
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
            "number" => self
                .text(node)
                .parse()
                .map(Value::Number)
                .map_err(|_| Fault::Host(HostError::Parse("invalid number".into()))),
            "string" => Ok(Value::String(unquote(self.text(node)))),
            "array" => self.eval_array(node),
            "object" => self.eval_object(node),
            "template_string" => self.eval_template(node),
            "regex" => Ok(Value::String(self.text(node).into())),
            "parenthesized_expression" => self.eval(
                node.named_child(0)
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
                let expression = node
                    .named_child(0)
                    .ok_or_else(|| Fault::Host(self.unsupported("await without expression")))?;
                let value = self.eval(expression)?;
                self.await_value(value)
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
                let message = node
                    .child_by_field_name("arguments")
                    .and_then(|args| args.named_child(0))
                    .map(|argument| self.eval_host(argument).map(|value| to_string(&value)))
                    .transpose()
                    .map_err(Fault::Host)?
                    .unwrap_or_default();
                match constructor {
                    Value::Namespace(name) => Ok(Value::Error(ErrorValue { name, message })),
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
            if child.kind() == "spread_element" {
                match self.eval(
                    child
                        .named_child(0)
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
                "pair" => {
                    let key = child
                        .child_by_field_name("key")
                        .ok_or_else(|| Fault::Host(self.unsupported("object key")))?;
                    let value = self.eval(
                        child
                            .child_by_field_name("value")
                            .ok_or_else(|| Fault::Host(self.unsupported("object value")))?,
                    )?;
                    fields.insert(unquote(self.text(key)), value);
                }
                "shorthand_property_identifier" => {
                    let key = self.text(child).to_owned();
                    let value = self.lookup(&key).ok_or_else(|| {
                        Fault::Host(HostError::Data(format!("unknown identifier '{key}'")))
                    })?;
                    fields.insert(key, value);
                }
                "spread_element" => match self.eval(
                    child
                        .named_child(0)
                        .ok_or_else(|| Fault::Host(self.unsupported("object spread")))?,
                )? {
                    Value::Object(object) => fields.extend(object.fields),
                    _ => {
                        return Err(Fault::Host(HostError::Data(
                            "object spread requires an object".into(),
                        )));
                    }
                },
                _ => return Err(Fault::Host(self.unsupported("object member"))),
            }
        }
        Ok(Value::Object(ObjectValue {
            fields,
            strict: false,
        }))
    }

    fn eval_template(&mut self, node: Node<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let mut output = String::new();
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() == "template_substitution" {
                let value = self.eval(
                    child
                        .named_child(0)
                        .ok_or_else(|| Fault::Host(self.unsupported("template expression")))?,
                )?;
                output.push_str(&to_string(&value));
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
            let children = arguments.named_children(&mut cursor).collect::<Vec<_>>();
            if let [function] = children.as_slice()
                && matches!(function.kind(), "arrow_function" | "function_expression")
            {
                let value = self.eval(*function)?;
                return self.await_value(value);
            }
        }

        let function = self.eval(function_node)?;
        let mut values = Vec::new();
        let mut cursor = arguments.walk();
        for child in arguments.named_children(&mut cursor) {
            if child.kind() == "spread_element" {
                match self.eval(
                    child
                        .named_child(0)
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
            Value::Namespace(name) => Err(Fault::Host(HostError::UnsupportedSyntax(format!(
                "namespace '{name}' is not callable"
            )))),
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
            function.parameters.named_children(&mut cursor).collect()
        };
        for (index, parameter) in parameters.into_iter().enumerate() {
            self.bind_in(
                env.clone(),
                parameter,
                args.get(index).cloned().unwrap_or(Value::Undefined),
            );
        }
        let previous = self.env.clone();
        self.env = env;
        let result = if function.expression {
            self.eval(function.body)
        } else {
            match self.statement(function.body).map_err(Fault::Host)? {
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

    fn bind_in(&mut self, env: EnvRef<'tree>, node: Node<'tree>, value: Value<'tree>) {
        if node.kind() == "identifier" {
            env.borrow_mut()
                .values
                .insert(self.text(node).into(), value);
        } else {
            self.bind(node, value);
        }
    }

    fn call_tool(
        &mut self,
        surface: &str,
        method: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        let descriptor = self
            .host
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
                        "method_not_found: unknown method '{surface}.{method}'"
                    )))
                } else {
                    Fault::Host(HostError::SurfaceNotFound(format!(
                        "surface_not_found: unknown surface '{surface}'"
                    )))
                }
            })?;
        let value = if args.len() == 1 {
            args.into_iter().next().unwrap_or(Value::Undefined)
        } else {
            new_array(args)
        };
        let json = self.to_json(&value).map_err(Fault::Host)?;
        let encoded = serde_json::to_string(&json)
            .map_err(|error| Fault::Host(HostError::Data(error.to_string())))?;
        if encoded.len() > self.host.limits.max_json_bytes {
            return Err(Fault::Host(HostError::Data(
                "arguments exceed JSON limit".into(),
            )));
        }
        if self.promises.len() >= MAX_INFLIGHT_CONNECTOR_CALLS {
            return Err(Fault::Host(HostError::Data(
                "connector in-flight capacity exhausted".into(),
            )));
        }
        let id = self.next_promise;
        self.next_promise = self.next_promise.saturating_add(1);
        self.promises
            .insert(id, PromiseState::Pending(PromiseKind::Connector));
        let completion = ConnectorCompletion::new(id, self.sender.clone());
        self.connector
            .dispatch(
                &descriptor,
                &encoded,
                DispatchContext {
                    deadline: self.deadline,
                    max_json_bytes: self.host.limits.max_json_bytes,
                },
                completion,
            )
            .map_err(|error| Fault::Host(HostError::Connector(error.to_string())))?;
        Ok(Value::Promise(id))
    }

    fn await_value(&mut self, value: Value<'tree>) -> Result<Value<'tree>, Fault<'tree>> {
        let Value::Promise(id) = value else {
            return Ok(value);
        };
        self.resolve(id)
    }

    fn resolve(&mut self, id: u64) -> Result<Value<'tree>, Fault<'tree>> {
        self.pump(id).map_err(Fault::Host)?;
        match self
            .promises
            .get(&id)
            .cloned()
            .ok_or_else(|| Fault::Host(HostError::Data("unknown promise".into())))?
        {
            PromiseState::Fulfilled(value) => Ok(value),
            PromiseState::Rejected(value) => Err(Fault::Throw(value)),
            PromiseState::Pending(PromiseKind::All(ids)) => {
                let mut values = Vec::new();
                for child in ids {
                    values.push(self.resolve(child)?);
                }
                let value = new_array(values);
                self.promises
                    .insert(id, PromiseState::Fulfilled(value.clone()));
                Ok(value)
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
                let value = self.resolve(winner)?;
                self.promises
                    .insert(id, PromiseState::Fulfilled(value.clone()));
                Ok(value)
            }
            PromiseState::Pending(PromiseKind::Then { .. }) => Err(Fault::Host(
                HostError::Connector("promise chain did not settle".into()),
            )),
            PromiseState::Pending(PromiseKind::Connector) => Err(Fault::Host(
                HostError::Connector("promise did not settle".into()),
            )),
        }
    }

    fn pump(&mut self, target: u64) -> Result<(), HostError> {
        loop {
            self.tick()?;
            self.drain()?;
            let Some(state) = self.promises.get(&target).cloned() else {
                return Err(HostError::Data("unknown promise".into()));
            };
            match state {
                PromiseState::Pending(PromiseKind::Connector) => {
                    self.microtasks = self.microtasks.saturating_add(1);
                    if self.microtasks > self.host.limits.microtask_ceiling {
                        return Err(HostError::MicrotaskLimit);
                    }
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
                PromiseState::Pending(PromiseKind::Then {
                    parent,
                    on_fulfilled,
                }) => {
                    self.microtasks = self.microtasks.saturating_add(1);
                    if self.microtasks > self.host.limits.microtask_ceiling {
                        return Err(HostError::MicrotaskLimit);
                    }
                    let state = match self.resolve(parent) {
                        Ok(value) => match self.call(on_fulfilled, vec![value]) {
                            Ok(Value::Promise(child)) => match self.resolve(child) {
                                Ok(value) => PromiseState::Fulfilled(value),
                                Err(Fault::Throw(value)) => PromiseState::Rejected(value),
                                Err(Fault::Host(error)) => return Err(error),
                            },
                            Ok(value) => PromiseState::Fulfilled(value),
                            Err(Fault::Throw(value)) => PromiseState::Rejected(value),
                            Err(Fault::Host(error)) => return Err(error),
                        },
                        Err(Fault::Throw(value)) => PromiseState::Rejected(value),
                        Err(Fault::Host(error)) => return Err(error),
                    };
                    self.promises.insert(target, state);
                }
                PromiseState::Pending(PromiseKind::All(_))
                | PromiseState::Pending(PromiseKind::AllSettled(_))
                | PromiseState::Pending(PromiseKind::Race(_))
                | PromiseState::Fulfilled(_)
                | PromiseState::Rejected(_) => return Ok(()),
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
        let state = match completion.result {
            Ok(encoded) if encoded.len() > self.host.limits.max_json_bytes => {
                PromiseState::Rejected(Value::Error(ErrorValue {
                    name: "DataError".into(),
                    message: "connector result exceeds JSON limit".into(),
                }))
            }
            Ok(encoded) => match normalize_public_result(&encoded)
                .and_then(|value| {
                    serde_json::from_str::<JsonValue>(&value)
                        .map_err(|error| HostError::Json(error.to_string()))
                })
                .and_then(|value| self.from_json(value, true))
            {
                Ok(value) => PromiseState::Fulfilled(value),
                Err(error) => PromiseState::Rejected(Value::Error(ErrorValue {
                    name: "DataError".into(),
                    message: error.to_string(),
                })),
            },
            Err(error) => PromiseState::Rejected(Value::Error(ErrorValue {
                name: "ToolError".into(),
                message: error.to_string(),
            })),
        };
        self.promises.insert(completion.sequence, state);
        Ok(())
    }

    fn race(&mut self, ids: &[u64]) -> Result<u64, HostError> {
        if ids.is_empty() {
            return Err(HostError::Data(
                "Promise.race expects a non-empty array".into(),
            ));
        }
        loop {
            self.tick()?;
            self.drain()?;
            if let Some(id) = ids
                .iter()
                .copied()
                .find(|id| !matches!(self.promises.get(id), Some(PromiseState::Pending(_))))
            {
                return Ok(id);
            }
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

    fn property(&mut self, object: Value<'tree>, key: &str) -> Result<Value<'tree>, Fault<'tree>> {
        match object.clone() {
            Value::Object(value) => {
                if let Some(result) = value.fields.get(key) {
                    Ok(result.clone())
                } else if value.strict {
                    Err(Fault::Host(HostError::SurfaceNotFound(format!(
                        "surface_not_found: unknown surface '{key}'"
                    ))))
                } else {
                    Ok(Value::Undefined)
                }
            }
            Value::Namespace(namespace) => {
                if self
                    .host
                    .registration()
                    .capabilities
                    .iter()
                    .any(|capability| capability.surface == namespace)
                {
                    Ok(Value::Tool(namespace, key.into()))
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
                    Ok(items.borrow().get(index).cloned().unwrap_or(Value::Undefined))
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
                Ok(Value::Bool(args.first().is_some_and(|key| {
                    object.fields.contains_key(&to_key(key))
                })))
            }
            Value::Promise(parent) if name == "then" => {
                let on_fulfilled = args.into_iter().next().unwrap_or(Value::Undefined);
                if !matches!(on_fulfilled, Value::Function(_)) {
                    return Err(Fault::Host(HostError::Data(
                        "Promise.then expects a function".into(),
                    )));
                }
                Ok(self.new_promise(PromiseState::Pending(PromiseKind::Then {
                    parent,
                    on_fulfilled,
                })))
            }
            Value::Promise(_) if matches!(name, "catch" | "finally") => {
                Err(Fault::Host(HostError::UnsupportedSyntax(format!(
                    "Promise.prototype.{name} is not supported; use await with try/catch"
                ))))
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
            ("Array", "from") => {
                let source = args.first().cloned().unwrap_or(Value::Undefined);
                let mapper = args.get(1).cloned();
                let values = match source {
                    Value::Array(items) => items.borrow().clone(),
                    Value::String(value) => value
                        .chars()
                        .map(|character| Value::String(character.to_string()))
                        .collect(),
                    Value::Object(object) => {
                        let length = object
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
                        (0..length)
                            .map(|index| {
                                object
                                    .fields
                                    .get(&index.to_string())
                                    .cloned()
                                    .unwrap_or(Value::Undefined)
                            })
                            .collect()
                    }
                    _ => {
                        return Err(Fault::Host(HostError::Data(
                            "Array.from expects an array, string, or array-like object".into(),
                        )));
                    }
                };
                let values = if let Some(mapper) = mapper {
                    if !matches!(mapper, Value::Function(_)) {
                        return Err(Fault::Host(HostError::Data(
                            "Array.from mapper must be a function".into(),
                        )));
                    }
                    values
                        .into_iter()
                        .enumerate()
                        .map(|(index, value)| {
                            self.call(
                                mapper.clone(),
                                vec![value, Value::Number(index as f64)],
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?
                } else {
                    values
                };
                Ok(new_array(values))
            },
            ("Object", "keys") => Ok(new_array(object_keys(
                args.first().cloned().unwrap_or(Value::Undefined),
            ))),
            ("Object", "values") => Ok(new_array(object_values(
                args.first().cloned().unwrap_or(Value::Undefined),
            ))),
            ("Object", "entries") => Ok(new_array(object_entries(
                args.first().cloned().unwrap_or(Value::Undefined),
            ))),
            ("JSON", "parse") => {
                let json: JsonValue =
                    serde_json::from_str(&to_string(args.first().unwrap_or(&Value::Undefined)))
                        .map_err(|error| {
                            Fault::Throw(Value::Error(ErrorValue {
                                name: "SyntaxError".into(),
                                message: error.to_string(),
                            }))
                        })?;
                self.from_json(json, false).map_err(Fault::Host)
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
            ("console", _) => Ok(Value::Undefined),
            _ => Err(Fault::Host(HostError::UnsupportedSyntax(format!(
                "global method {namespace}.{name} is not supported"
            )))),
        }
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
            "includes" => {
                Ok(Value::Bool(args.first().is_some_and(|value| {
                    snapshot.iter().any(|item| same_value(item, value))
                })))
            }
            "indexOf" => Ok(Value::Number(
                args.first()
                    .and_then(|value| snapshot.iter().position(|item| same_value(item, value)))
                    .map(|index| index as f64)
                    .unwrap_or(-1.0),
            )),
            "map" | "filter" | "find" | "findIndex" | "some" | "every" | "forEach" => {
                let callback = args.first().cloned().ok_or_else(|| {
                    Fault::Host(HostError::Data("array callback is required".into()))
                })?;
                let mut output = Vec::new();
                let mut found = None;
                for (index, item) in snapshot.iter().cloned().enumerate() {
                    let result = self.call(
                        callback.clone(),
                        vec![
                            item,
                            Value::Number(index as f64),
                            new_array(snapshot.clone()),
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
            "push" => {
                let mut target = items.borrow_mut();
                target.extend(args);
                Ok(Value::Number(target.len() as f64))
            }
            _ => Err(Fault::Host(self.unsupported_name(name, "array method"))),
        }
    }

    fn string_method(
        &mut self,
        value: &str,
        name: &str,
        args: Vec<Value<'tree>>,
    ) -> Result<Value<'tree>, Fault<'tree>> {
        match name {
            "toLowerCase" => Ok(Value::String(value.to_lowercase())),
            "toUpperCase" => Ok(Value::String(value.to_uppercase())),
            "trim" => Ok(Value::String(value.trim().into())),
            "includes" => Ok(Value::Bool(
                value.contains(&to_string(args.first().unwrap_or(&Value::Undefined))),
            )),
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
            _ => Err(Fault::Host(self.unsupported_name(name, "string method"))),
        }
    }

    fn new_promise(&mut self, state: PromiseState<'tree>) -> Value<'tree> {
        let id = self.next_promise;
        self.next_promise = self.next_promise.saturating_add(1);
        self.promises.insert(id, state);
        Value::Promise(id)
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

    fn from_json(&self, value: JsonValue, strict: bool) -> Result<Value<'tree>, HostError> {
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
                    .map(|value| self.from_json(value, false))
                    .collect::<Result<_, _>>()?,
            ),
            JsonValue::Object(values) => Value::Object(ObjectValue {
                fields: values
                    .into_iter()
                    .map(|(key, value)| Ok((key, self.from_json(value, false)?)))
                    .collect::<Result<_, HostError>>()?,
                strict,
            }),
        })
    }

    fn to_json(&self, value: &Value<'tree>) -> Result<JsonValue, HostError> {
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
            Value::Array(values) => JsonValue::Array(
                values
                    .borrow()
                    .iter()
                    .map(|value| self.to_json(value))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(value) => JsonValue::Object(
                value
                    .fields
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), self.to_json(value)?)))
                    .collect::<Result<Map<_, _>, HostError>>()?,
            ),
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
    Value::Object(ObjectValue {
        fields,
        strict: false,
    })
}

fn object_keys<'tree>(value: Value<'tree>) -> Vec<Value<'tree>> {
    match value {
        Value::Object(value) => value.fields.keys().cloned().map(Value::String).collect(),
        Value::Array(value) => (0..value.borrow().len())
            .map(|index| Value::String(index.to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn object_values<'tree>(value: Value<'tree>) -> Vec<Value<'tree>> {
    match value {
        Value::Object(value) => value.fields.into_values().collect(),
        Value::Array(value) => value.borrow().clone(),
        _ => Vec::new(),
    }
}

fn object_entries<'tree>(value: Value<'tree>) -> Vec<Value<'tree>> {
    match value {
        Value::Object(value) => value
            .fields
            .into_iter()
            .map(|(key, value)| new_array(vec![Value::String(key), value]))
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
    match value {
        Value::Undefined => "undefined".into(),
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(values) => values
            .borrow()
            .iter()
            .map(to_string)
            .collect::<Vec<_>>()
            .join(","),
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
        "<" => Ok(Value::Bool(to_string(&left) < to_string(&right))),
        ">" => Ok(Value::Bool(to_string(&left) > to_string(&right))),
        "<=" => Ok(Value::Bool(to_string(&left) <= to_string(&right))),
        ">=" => Ok(Value::Bool(to_string(&left) >= to_string(&right))),
        _ => Err(HostError::UnsupportedSyntax(format!(
            "binary operator '{operator}' is not supported"
        ))),
    }
}

fn unquote(value: &str) -> String {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1]
                .replace("\\'", "'")
                .replace("\\\"", "\"")
                .replace("\\n", "\n")
                .replace("\\t", "\t")
                .replace("\\\\", "\\");
        }
    }
    value.into()
}
