//! K0 guest `z` surface host side (`zerostack-fhcj`).
//!
//! The confined interpreter exposes a `z` global only when the host carries
//! a [`GuestSurface`] (the supervisor's per-call runtime installs one; the
//! native path never does). The surface is read-only by construction:
//!
//! - **context** — the injected operational roots, never model-typed;
//! - **state** — a small serializable per-call map (bounded keys, values,
//!   and total bytes) that dies with the fresh runtime;
//! - **help / inspect / capabilities.search** — bounded views over the
//!   registered V6 catalog plus this table;
//! - **invoke / parallel** — the read-only capability reach of
//!   [`zero_abi::guest::K0_READ_ONLY_CAPABILITIES`], dispatched through the
//!   same connector (and its call-scoped task group) as `zero.<s>.<m>()`,
//!   with deterministic input-order results and a hard spec bound;
//! - **resolve / expand / snap / view** — the W9-E wave-9 chain
//!   ([`GuestWave9`]); they fail typed when no live rooted evidence is
//!   attached, and consume only trusted host-minted handles;
//! - **return / persistHandle** — the structured-return entry point and
//!   exact-handle persistence into the call's response `ExactHandles`
//!   (only handles minted during the same call may be persisted).
//!
//! No effect, transaction, commit, OS, environment, network, spawn, FFI,
//! GPU, or dynamic-codegen authority exists anywhere in this module; the
//! `z.transaction` name is deliberately absent and fails typed.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use zero_abi::guest::{
    K0_GUEST_ABSENT_MEMBERS, K0_GUEST_MEMBERS, K0_READ_ONLY_CAPABILITIES,
    K0_STATE_MAX_KEY_BYTES, K0_STATE_MAX_KEYS, K0_STATE_MAX_TOTAL_BYTES,
    K0_STATE_MAX_VALUE_BYTES, is_k0_read_only_capability, is_k0_read_only_compound_op,
};

/// The injected operational context of one K0 call (the five named roots).
/// Every root is supplied by the caller; the guest never synthesizes
/// session or project identity.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestContext {
    pub project_root: String,
    pub workspace_root: Option<String>,
    pub request_root: Option<String>,
    pub session_root: Option<String>,
    pub capability_manifest_root: Option<String>,
    /// Session identity of the runtime (surfaced by `z.inspect`, never by
    /// `z.context`, which stays the five named roots).
    pub session_id: String,
    /// Protocol ABI version of the runtime (surfaced by `z.inspect`).
    pub abi_version: String,
}

/// Trusted W9-E wave-9 route consumed by the guest surface
/// (`z.resolve`/`z.expand`/`z.snap`/`z.view`).
///
/// Implemented by the hub supervisor over the Snap-to-File gate. Every
/// method returns bounded JSON or a typed error string; no handle is ever
/// minted by guest code.
pub trait GuestWave9 {
    /// `z.resolve(demand)`: safe handle only after completeness. JSON:
    /// `{handle, handle_id, packet, view, atoms, projection_root,
    /// visible_bytes, certified_atoms, first_try_sufficiency, terminal}`.
    fn resolve(&self, demand: JsonValue) -> Result<JsonValue, String>;

    /// `z.expand(handle)`: exactly one first expansion of a trusted handle.
    /// JSON: `{handle_id, atoms, projection_root, visible_bytes,
    /// certified_atoms, first_try_sufficiency, terminal}`.
    fn expand(&self, handle: JsonValue) -> Result<JsonValue, String>;

    /// `z.snap(task)`: read-only decision view for every outcome. JSON:
    /// `{outcome, packet, view}` (`outcome` is `snapped`/`escaped`/`refused`).
    fn snap(&self, task: JsonValue) -> Result<JsonValue, String>;

    /// `z.view(handle)`: live-revalidated Proved decision view of a trusted
    /// handle, or a typed refusal.
    fn view(&self, handle: JsonValue) -> Result<JsonValue, String>;
}

/// The per-call guest surface attached to a K0 runtime host.
///
/// Interior mutability keeps the surface usable from the interpreter's
/// `&self` host access; the runtime is fresh per call, so the whole surface
/// (including its state map and minted-handle registry) dies with the call.
pub struct GuestSurface {
    context: GuestContext,
    state: RefCell<BTreeMap<String, JsonValue>>,
    state_bytes: Cell<usize>,
    w9e: Option<Rc<dyn GuestWave9>>,
    minted: RefCell<BTreeMap<String, JsonValue>>,
    persisted: RefCell<Option<String>>,
    parallel_limit: usize,
}

impl GuestSurface {
    /// Attach a surface with the injected context. No W9-E evidence is
    /// attached yet, so the wave-9 seam fails typed until
    /// [`GuestSurface::attach_w9e`] is called.
    pub fn new(context: GuestContext, parallel_limit: usize) -> Self {
        Self {
            context,
            state: RefCell::new(BTreeMap::new()),
            state_bytes: Cell::new(0),
            w9e: None,
            minted: RefCell::new(BTreeMap::new()),
            persisted: RefCell::new(None),
            parallel_limit,
        }
    }

    /// Attach live rooted W9-E evidence (the Snap-to-File route). Without
    /// it `z.resolve`/`z.expand`/`z.snap`/`z.view` fail typed.
    pub fn attach_w9e(&mut self, w9e: Rc<dyn GuestWave9>) {
        self.w9e = Some(w9e);
    }

    /// The injected context as JSON (five roots; absent roots are `null`).
    pub fn context_json(&self) -> JsonValue {
        serde_json::json!({
            "projectRoot": self.context.project_root,
            "workspaceRoot": self.context.workspace_root,
            "requestRoot": self.context.request_root,
            "sessionRoot": self.context.session_root,
            "capabilityManifestRoot": self.context.capability_manifest_root,
        })
    }

    /// Session identity of the runtime (for `z.inspect`).
    pub fn session_id(&self) -> &str {
        &self.context.session_id
    }

    /// Protocol ABI version of the runtime (for `z.inspect`).
    pub fn abi_version(&self) -> &str {
        &self.context.abi_version
    }

    /// The bound parallel-spec limit of this surface.
    pub fn parallel_limit(&self) -> usize {
        self.parallel_limit
    }

    /// Read one state key (`z.state.get`).
    pub fn state_get(&self, key: &str) -> Result<Option<JsonValue>, String> {
        self.check_key(key)?;
        Ok(self.state.borrow().get(key).cloned())
    }

    /// Whether one state key exists (`z.state.has`).
    pub fn state_has(&self, key: &str) -> Result<bool, String> {
        self.check_key(key)?;
        Ok(self.state.borrow().contains_key(key))
    }

    /// Sorted state keys (`z.state.list`).
    pub fn state_list(&self) -> Vec<String> {
        self.state.borrow().keys().cloned().collect()
    }

    /// Serialized state bytes (`z.inspect`).
    pub fn state_bytes(&self) -> usize {
        self.state_bytes.get()
    }

    /// Write one state key (`z.state.set`), bounded. The value must be
    /// JSON-serializable (the interpreter converts before calling) and
    /// within the per-value and total byte budgets.
    pub fn state_set(&self, key: &str, value: JsonValue) -> Result<(), String> {
        self.check_key(key)?;
        let encoded = serde_json::to_string(&value)
            .map_err(|error| format!("state value is not serializable: {error}"))?;
        if encoded.len() > K0_STATE_MAX_VALUE_BYTES {
            return Err(format!(
                "state value for '{key}' is {} bytes, above the {K0_STATE_MAX_VALUE_BYTES}-byte per-value bound",
                encoded.len()
            ));
        }
        let mut state = self.state.borrow_mut();
        let previous = state.get(key).map(|old| {
            serde_json::to_string(old)
                .map(|old_encoded| old_encoded.len())
                .unwrap_or(0)
        });
        let mut next_bytes = self.state_bytes.get();
        next_bytes = next_bytes.saturating_sub(previous.unwrap_or(0));
        next_bytes = next_bytes.saturating_add(encoded.len());
        if next_bytes > K0_STATE_MAX_TOTAL_BYTES {
            return Err(format!(
                "state total would reach {next_bytes} bytes, above the {K0_STATE_MAX_TOTAL_BYTES}-byte budget"
            ));
        }
        if !state.contains_key(key) && state.len() >= K0_STATE_MAX_KEYS {
            return Err(format!(
                "state holds {} keys, at the {K0_STATE_MAX_KEYS}-key bound",
                state.len()
            ));
        }
        state.insert(key.to_owned(), value);
        self.state_bytes.set(next_bytes);
        Ok(())
    }

    /// Remove one state key (`z.state.delete`). Returns whether it existed.
    pub fn state_delete(&self, key: &str) -> Result<bool, String> {
        self.check_key(key)?;
        let mut state = self.state.borrow_mut();
        let Some(removed) = state.remove(key) else {
            return Ok(false);
        };
        let removed_bytes = serde_json::to_string(&removed)
            .map(|encoded| encoded.len())
            .unwrap_or(0);
        self.state_bytes
            .set(self.state_bytes.get().saturating_sub(removed_bytes));
        Ok(true)
    }

    fn check_key(&self, key: &str) -> Result<(), String> {
        if key.is_empty() {
            return Err("state key must not be empty".into());
        }
        if key.len() > K0_STATE_MAX_KEY_BYTES {
            return Err(format!(
                "state key is {} bytes, above the {K0_STATE_MAX_KEY_BYTES}-byte bound",
                key.len()
            ));
        }
        Ok(())
    }

    /// Validate one `z.invoke` / `z.parallel` target: it must be a
    /// registered read-only capability (or a read-only `fs.compound` op).
    pub fn check_read_only_target(&self, surface: &str, method: &str, args: &JsonValue) -> Result<(), String> {
        if surface == "fs" && method == "compound" {
            // The op rides the object `name` key or the positional first
            // element, exactly like the lowering authority reads it.
            let op = args
                .get("name")
                .and_then(JsonValue::as_str)
                .or_else(|| args.get(0).and_then(JsonValue::as_str))
                .unwrap_or("");
            if is_k0_read_only_compound_op(op) {
                return Ok(());
            }
            return Err(format!(
                "fs.compound operation '{op}' is not in the read-only K0 reach; \
                 read-only compound operations: read, search, find, grep, list, tree, inventory, resolve, lookup"
            ));
        }
        if is_k0_read_only_capability(surface, method) {
            return Ok(());
        }
        let mut reach = String::new();
        for (s, m) in K0_READ_ONLY_CAPABILITIES {
            if !reach.is_empty() {
                reach.push_str(", ");
            }
            reach.push_str(&format!("{s}.{m}"));
        }
        Err(format!(
            "{surface}.{method} is not in the read-only K0 reach of z.invoke/z.parallel; \
             read-only capabilities: {reach}"
        ))
    }

    /// The attached W9-E route, if live rooted evidence exists.
    pub fn w9e(&self) -> Option<&Rc<dyn GuestWave9>> {
        self.w9e.as_ref()
    }

    /// Record one host-minted handle (called by the resolve wrapper after a
    /// successful issuance, before the guest ever sees it).
    pub fn record_minted(&self, handle_id: &str, handle: JsonValue) {
        self.minted.borrow_mut().insert(handle_id.to_owned(), handle);
    }

    /// `z.persistHandle`: only handles minted during this call may persist
    /// into the response's exact-handle slot (JS cannot mint authority).
    pub fn persist(&self, handle_id: &str) -> Result<String, String> {
        if !self.minted.borrow().contains_key(handle_id) {
            return Err(format!(
                "handle {handle_id} is not a host-minted handle of this call; JS cannot mint authority"
            ));
        }
        *self.persisted.borrow_mut() = Some(handle_id.to_owned());
        Ok(handle_id.to_owned())
    }

    /// The persisted continuation handle id, if any (`z.persistHandle`).
    pub fn persisted_continuation(&self) -> Option<String> {
        self.persisted.borrow().clone()
    }

    /// Whether `member` is deliberately absent from K0 (typed refusal).
    pub fn is_absent_member(member: &str) -> bool {
        K0_GUEST_ABSENT_MEMBERS.contains(&member)
    }

    /// Stable sorted member list for help/error text.
    pub fn member_list() -> Vec<&'static str> {
        let mut members: Vec<&'static str> = K0_GUEST_MEMBERS.to_vec();
        members.sort_unstable();
        members
    }
}

impl std::fmt::Debug for GuestSurface {
    /// The route trait object has no Debug; print the observable surface
    /// (context roots, state summary, persisted handle) instead.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestSurface")
            .field("context", &self.context)
            .field("state_keys", &self.state.borrow().len())
            .field("state_bytes", &self.state_bytes.get())
            .field("w9e_attached", &self.w9e.is_some())
            .field("minted_handles", &self.minted.borrow().len())
            .field("persisted", &self.persisted.borrow())
            .field("parallel_limit", &self.parallel_limit)
            .finish()
    }
}

/// `z.capabilities.search(query)`: deterministic bounded substring search
/// over the registered catalog (surface.method sorted pairs). Returns the
/// matching `surface.method` strings, at most
/// [`zero_abi::guest::K0_CAPABILITIES_SEARCH_MAX_RESULTS`].
pub fn search_capabilities(
    registered: &[(String, String)],
    query: &str,
) -> Vec<String> {
    let mut hits = Vec::new();
    for (surface, method) in registered {
        let qualified = format!("{surface}.{method}");
        let haystack = format!("{surface} {method} {qualified}");
        if haystack.contains(query) {
            hits.push(qualified);
        }
        if hits.len() >= zero_abi::guest::K0_CAPABILITIES_SEARCH_MAX_RESULTS {
            break;
        }
    }
    hits
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvokeSpec {
    #[serde(default)]
    surface: Option<String>,
    #[serde(default)]
    method: Option<String>,
}

/// Normalize one `z.parallel` spec into `(surface, method, args)`: a
/// `"surface.method"` string or a `{surface, method, args?}` object.
pub fn parallel_spec(spec: JsonValue) -> Result<(String, String, JsonValue), String> {
    match spec {
        JsonValue::String(qualified) => {
            let (surface, method) = split_qualified(&qualified)?;
            Ok((surface.to_owned(), method.to_owned(), JsonValue::Object(Default::default())))
        }
        JsonValue::Object(mut object) => {
            let args = object
                .remove("args")
                .unwrap_or(JsonValue::Object(Default::default()));
            let spec: InvokeSpec = serde_json::from_value(JsonValue::Object(object))
                .map_err(|error| format!("parallel spec is not {{surface, method, args?}}: {error}"))?;
            let surface = spec
                .surface
                .ok_or_else(|| "parallel spec object requires 'surface'".to_owned())?;
            let method = spec
                .method
                .ok_or_else(|| "parallel spec object requires 'method'".to_owned())?;
            if !args.is_object() {
                return Err("parallel spec args must be an object".into());
            }
            Ok((surface, method, args))
        }
        _ => Err("each parallel spec must be a \"surface.method\" string or a {surface, method, args?} object".into()),
    }
}

/// Split `"surface.method"` with exactly one dot.
pub fn split_qualified(qualified: &str) -> Result<(&str, &str), String> {
    let Some((surface, method)) = qualified.split_once('.') else {
        return Err(format!(
            "'{qualified}' is not a surface.method capability name"
        ));
    };
    if surface.is_empty() || method.is_empty() || method.contains('.') {
        return Err(format!(
            "'{qualified}' is not a surface.method capability name"
        ));
    }
    Ok((surface, method))
}
