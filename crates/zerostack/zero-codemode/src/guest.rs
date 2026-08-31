//! Direct ZeroKernel guest state and context. The reusable host installs one
//! `GuestSurface` per fresh cell. It carries only immutable operational context
//! and bounded serializable state. Engine calls are bound directly by the interpreter.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value as JsonValue;
use zero_abi::{
    STATE_KEY_BYTE_LIMIT, STATE_KEY_LIMIT, STATE_TOTAL_BYTE_LIMIT, STATE_VALUE_BYTE_LIMIT,
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuestContext {
    pub project_root: String,
    pub workspace_root: Option<String>,
    pub request_root: Option<String>,
    pub session_root: Option<String>,
    pub session_id: String,
    pub protocol: String,
    /// Exact work-capsule root this cell runs under: the canonical lowercase hexadecimal SHA256 root of
    /// the finalized source capsule. Every guest operation trace binds to this root at dispatch start;
    /// nothing stamps traces after the fact and no root is ever synthesized here.
    pub capsule_root: String,
}

pub struct GuestSurface {
    context: GuestContext,
    state: RefCell<BTreeMap<String, JsonValue>>,
    state_bytes: Cell<usize>,
}

impl GuestSurface {
    pub fn new(context: GuestContext) -> Self {
        Self {
            context,
            state: RefCell::new(BTreeMap::new()),
            state_bytes: Cell::new(0),
        }
    }

    pub fn context_json(&self) -> JsonValue {
        serde_json::json!({
            "projectRoot": self.context.project_root,
            "workspaceRoot": self.context.workspace_root,
            "requestRoot": self.context.request_root,
            "sessionRoot": self.context.session_root,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.context.session_id
    }
    /// Capsule root every operation trace produced by this guest is bound to. Non-model-facing:
    /// deliberately excluded from [`Self::context_json`] and from every value the guest can read.
    pub fn capsule_root(&self) -> &str {
        &self.context.capsule_root
    }

    pub fn protocol(&self) -> &str {
        &self.context.protocol
    }

    pub fn state_get(&self, key: &str) -> Result<Option<JsonValue>, String> {
        self.check_key(key)?;
        Ok(self.state.borrow().get(key).cloned())
    }

    pub fn state_has(&self, key: &str) -> Result<bool, String> {
        self.check_key(key)?;
        Ok(self.state.borrow().contains_key(key))
    }

    pub fn state_list(&self) -> Vec<String> {
        self.state.borrow().keys().cloned().collect()
    }

    pub fn state_bytes(&self) -> usize {
        self.state_bytes.get()
    }

    pub fn state_set(&self, key: &str, value: JsonValue) -> Result<(), String> {
        self.check_key(key)?;
        let encoded = serde_json::to_vec(&value)
            .map_err(|error| format!("state value is not serializable: {error}"))?;
        if encoded.len() > STATE_VALUE_BYTE_LIMIT {
            return Err(format!(
                "state value for {key:?} is {} bytes; limit is {STATE_VALUE_BYTE_LIMIT}",
                encoded.len()
            ));
        }

        let mut state = self.state.borrow_mut();
        let previous_bytes = state
            .get(key)
            .and_then(|previous| serde_json::to_vec(previous).ok())
            .map_or(0, |bytes| bytes.len());
        let next_bytes = self
            .state_bytes
            .get()
            .saturating_sub(previous_bytes)
            .saturating_add(encoded.len());
        if next_bytes > STATE_TOTAL_BYTE_LIMIT {
            return Err(format!(
                "state values would total {next_bytes} bytes; limit is {STATE_TOTAL_BYTE_LIMIT}"
            ));
        }
        if !state.contains_key(key) && state.len() >= STATE_KEY_LIMIT {
            return Err(format!("state already contains {STATE_KEY_LIMIT} keys"));
        }

        state.insert(key.to_owned(), value);
        self.state_bytes.set(next_bytes);
        Ok(())
    }

    pub fn state_delete(&self, key: &str) -> Result<bool, String> {
        self.check_key(key)?;
        let mut state = self.state.borrow_mut();
        let Some(removed) = state.remove(key) else {
            return Ok(false);
        };
        let removed_bytes = serde_json::to_vec(&removed).map_or(0, |bytes| bytes.len());
        self.state_bytes
            .set(self.state_bytes.get().saturating_sub(removed_bytes));
        Ok(true)
    }

    pub fn state_hydrate(&self, state: BTreeMap<String, JsonValue>) -> Result<(), String> {
        let mut next = BTreeMap::new();
        let mut total_bytes = 0_usize;
        for (key, value) in state {
            self.check_key(&key)?;
            if next.len() >= STATE_KEY_LIMIT {
                return Err(format!("hydrated state exceeds {STATE_KEY_LIMIT} keys"));
            }
            let encoded = serde_json::to_vec(&value)
                .map_err(|error| format!("hydrated state is not serializable: {error}"))?;
            if encoded.len() > STATE_VALUE_BYTE_LIMIT {
                return Err(format!(
                    "hydrated state value for {key:?} is {} bytes; limit is {STATE_VALUE_BYTE_LIMIT}",
                    encoded.len()
                ));
            }
            total_bytes = total_bytes.saturating_add(encoded.len());
            if total_bytes > STATE_TOTAL_BYTE_LIMIT {
                return Err(format!(
                    "hydrated state exceeds {STATE_TOTAL_BYTE_LIMIT} bytes"
                ));
            }
            next.insert(key, value);
        }
        *self.state.borrow_mut() = next;
        self.state_bytes.set(total_bytes);
        Ok(())
    }

    pub fn state_snapshot(&self) -> BTreeMap<String, JsonValue> {
        self.state.borrow().clone()
    }

    fn check_key(&self, key: &str) -> Result<(), String> {
        if key.is_empty() || key.len() > STATE_KEY_BYTE_LIMIT {
            return Err(format!(
                "state key must contain 1..={STATE_KEY_BYTE_LIMIT} bytes"
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for GuestSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GuestSurface")
            .field("context", &self.context)
            .field("state_keys", &self.state.borrow().len())
            .field("state_bytes", &self.state_bytes.get())
            .finish()
    }
}
