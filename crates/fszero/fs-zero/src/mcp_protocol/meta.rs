//! `_meta` extraction for MCP 2026-07-28 stateless requests.

use super::version::{PROTOCOL_RC, negotiate_version};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct RequestMeta {
    pub protocol_version: Option<String>,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
}

pub fn extract_request_meta(params: Option<&Value>) -> RequestMeta {
    let Some(params) = params else {
        return RequestMeta::default();
    };
    let Some(meta) = params.get("_meta") else {
        return RequestMeta::default();
    };
    let client_info = meta
        .get("io.modelcontextprotocol/clientInfo")
        .or_else(|| meta.get("clientInfo"));
    RequestMeta {
        protocol_version: meta
            .get("io.modelcontextprotocol/protocolVersion")
            .or_else(|| meta.get("protocolVersion"))
            .and_then(Value::as_str)
            .map(str::to_string),
        client_name: client_info
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        client_version: client_info
            .and_then(|v| v.get("version"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

pub fn effective_protocol_version(
    transport_version: Option<&str>,
    params: Option<&Value>,
) -> &'static str {
    if let Some(v) = transport_version {
        return negotiate_version(Some(v));
    }
    let meta = extract_request_meta(params);
    negotiate_version(meta.protocol_version.as_deref())
}

pub fn is_stateless_request(transport_version: Option<&str>, params: Option<&Value>) -> bool {
    effective_protocol_version(transport_version, params) == PROTOCOL_RC
}
