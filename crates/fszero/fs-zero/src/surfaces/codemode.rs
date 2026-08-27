//! FSZero CodeMode tool catalog (contract launch-mode surface).
//!
//! Catalog schemas are owned by `contracts/operation-abi-schemas-v1.json` and
//! materialized through the operation ABI (fszero-ncib.1).

use crate::core::operation_schemas::materialize_codemode_tools;
use serde_json::Value;

/// Live CodeMode tool catalog — exact materialization of the canonical schema doc.
pub fn codemode_tools() -> Vec<Value> {
    materialize_codemode_tools()
}
