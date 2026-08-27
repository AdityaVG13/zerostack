//! Shared MCP JSON-RPC request handler — legacy stdio and 2026-07-28 stateless.

use super::meta::{effective_protocol_version, extract_request_meta, is_stateless_request};
use super::surface::SurfaceKind;
use super::version::{PROTOCOL_RC, is_stateless_version};
use crate::core::FSZeroSession;
use crate::mcp_rpc::TOOLS_LIST_TTL_MS;
use crate::mcp_rpc::{
    error_response, resource_list_result, resource_read_result, server_discover_result,
    success_response, tools_list_result,
};
use serde_json::{Value, json};

/// Shared params gate for tools/call and resources/read.
fn require_params<'a>(id: &Value, params: Option<&'a Value>) -> Result<&'a Value, Value> {
    params.ok_or_else(|| error_response(id.clone(), -32602, "missing params"))
}

fn empty_result(id: Value, list_key: &str) -> Value {
    success_response(id, json!({ list_key: [] }))
}

/// Map domain Result to JSON-RPC success / invalid-params error.
fn result_or_invalid_params(id: Value, result: Result<Value, String>) -> Value {
    match result {
        Ok(v) => success_response(id, v),
        Err(message) => error_response(id, -32602, &message),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportProfile {
    StdioLegacy,
    HttpStateless,
}

pub struct McpHandler {
    pub surface: SurfaceKind,
    pub transport: TransportProfile,
    pub negotiated_version: String,
}

impl McpHandler {
    pub fn new(surface: SurfaceKind, transport: TransportProfile) -> Self {
        Self {
            surface,
            transport,
            negotiated_version: super::version::PROTOCOL_LEGACY.to_string(),
        }
    }

    pub fn handle_json(
        &mut self,
        sess: &mut FSZeroSession,
        req: Value,
        transport_version: Option<&str>,
    ) -> Option<Value> {
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = req.get("method").and_then(Value::as_str) else {
            return Some(error_response(id, -32600, "missing method"));
        };
        let params = req.get("params");
        let stateless = self.transport == TransportProfile::HttpStateless
            || is_stateless_request(transport_version, params);
        if !req.get("id").is_some() && method.starts_with("notifications/") {
            return self.handle_notification(method);
        }
        match method {
            "initialize" if !stateless => self.handle_initialize(id, params),
            "initialize" if stateless => Some(error_response(
                id,
                -32601,
                "initialize removed in 2026-07-28; use server/discover",
            )),
            "server/discover" => self.handle_server_discover(id),
            "tools/list" => Some(self.handle_tools_list(id)),
            "tools/call" => Some(self.handle_tools_call(sess, id, params)),
            "resources/list" => Some(self.handle_resources_list(id)),
            "resources/read" => Some(self.handle_resources_read(sess, id, params)),
            "resources/templates/list" => Some(empty_result(id, "resourceTemplates")),
            "prompts/list" => Some(empty_result(id, "prompts")),
            "ping" | "logging/setLevel" => Some(success_response(id, json!({}))),
            "roots/list" => Some(self.handle_roots_list_deprecated(id)),
            "sampling/createMessage" => Some(error_response(
                id,
                -32601,
                "sampling deprecated; integrate LLM APIs directly",
            )),
            _ => Some(error_response(id, -32601, "method not found")),
        }
    }

    fn handle_notification(&self, _method: &str) -> Option<Value> {
        None
    }

    fn handle_initialize(&mut self, id: Value, params: Option<&Value>) -> Option<Value> {
        let client_version = params
            .and_then(|p| p.get("protocolVersion"))
            .and_then(Value::as_str);
        let version = effective_protocol_version(None, params);
        if let Some(cv) = client_version {
            self.negotiated_version = effective_protocol_version(Some(cv), params).to_string();
        } else {
            self.negotiated_version = version.to_string();
        }
        let capabilities = server_discover_result(
            &self.negotiated_version,
            self.surface.server_name(),
            self.surface.server_description(),
            is_stateless_version(&self.negotiated_version),
        )["capabilities"]
            .clone();
        Some(success_response(
            id,
            json!({
                "protocolVersion": self.negotiated_version, "capabilities": capabilities,
                "serverInfo": { "name": self.surface.server_name(), "version": env!("CARGO_PKG_VERSION"), "description": self.surface.server_description() },
                // Additive surface discriminator (R-PAR-REC-004); name also distinct.
                "surface": self.surface.surface_field(),
            }),
        ))
    }

    fn handle_server_discover(&self, id: Value) -> Option<Value> {
        Some(success_response(
            id,
            server_discover_result(
                PROTOCOL_RC,
                self.surface.server_name(),
                self.surface.server_description(),
                true,
            ),
        ))
    }

    fn handle_tools_list(&self, id: Value) -> Value {
        success_response(id, tools_list_result(self.surface.tools()))
    }

    fn handle_tools_call(
        &self,
        sess: &mut FSZeroSession,
        id: Value,
        params: Option<&Value>,
    ) -> Value {
        let params = match require_params(&id, params) {
            Ok(p) => p,
            Err(err) => return err,
        };
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error_response(id, -32602, "missing tool name");
        };
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let _meta = extract_request_meta(Some(params));
        result_or_invalid_params(id, self.surface.call_tool(sess, name, &args))
    }

    fn handle_resources_list(&self, id: Value) -> Value {
        success_response(id, resource_list_result(TOOLS_LIST_TTL_MS))
    }

    fn handle_resources_read(
        &self,
        sess: &mut FSZeroSession,
        id: Value,
        params: Option<&Value>,
    ) -> Value {
        let params = match require_params(&id, params) {
            Ok(p) => p,
            Err(err) => return err,
        };
        match params.get("uri").and_then(Value::as_str) {
            Some(uri) => result_or_invalid_params(id, resource_read_result(sess, uri)),
            None => error_response(id, -32602, "missing resource uri"),
        }
    }

    fn handle_roots_list_deprecated(&self, id: Value) -> Value {
        success_response(
            id,
            json!({
                "roots": [],
                "_meta": {"io.modelcontextprotocol/deprecated": {
                    "feature": "roots",
                    "message": "roots deprecated; use tool parameters or server configuration",
                }},
            }),
        )
    }

    pub fn validate_http_routing(
        &self,
        header_method: Option<&str>,
        header_name: Option<&str>,
        body_method: &str,
        body_tool_name: Option<&str>,
    ) -> Result<(), String> {
        if let Some(hm) = header_method {
            if hm != body_method {
                return Err(format!(
                    "Mcp-Method header '{hm}' disagrees with body method '{body_method}'"
                ));
            }
        }
        if body_method == "tools/call" {
            let body_name =
                body_tool_name.ok_or_else(|| "missing tool name in body".to_string())?;
            if let Some(hn) = header_name {
                if hn != body_name {
                    return Err(format!(
                        "Mcp-Name header '{hn}' disagrees with body tool name '{body_name}'"
                    ));
                }
            }
        }
        Ok(())
    }
}

pub fn tool_name_from_params(params: Option<&Value>) -> Option<&str> {
    params.and_then(|p| p.get("name")).and_then(Value::as_str)
}
