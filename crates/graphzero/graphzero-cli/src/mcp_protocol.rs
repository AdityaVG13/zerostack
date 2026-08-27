//! JSON-RPC protocol framing for MCP stdio.

use std::io::Write;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
pub struct JsonRpcResponse<'a> {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<&'a Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

pub fn write_response(out: &mut impl Write, resp: &JsonRpcResponse<'_>) -> Result<()> {
    serde_json::to_writer(&mut *out, resp)?;
    writeln!(out)?;
    out.flush()?;
    Ok(())
}
