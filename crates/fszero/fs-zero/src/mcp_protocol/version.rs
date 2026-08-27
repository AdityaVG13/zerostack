//! MCP protocol version constants and negotiation.

pub const PROTOCOL_LEGACY: &str = "2024-11-05";
pub const PROTOCOL_2025: &str = "2025-11-25";
pub const PROTOCOL_RC: &str = "2026-07-28";

pub const SUPPORTED_VERSIONS: &[&str] = &[PROTOCOL_LEGACY, PROTOCOL_2025, PROTOCOL_RC];

/// Negotiate protocol version from client initialize or `_meta` hint.
pub fn negotiate_version(client_version: Option<&str>) -> &'static str {
    match client_version {
        Some(v) if v == PROTOCOL_RC => PROTOCOL_RC,
        Some(v) if v == PROTOCOL_2025 => PROTOCOL_2025,
        Some(v) if v == PROTOCOL_LEGACY => PROTOCOL_LEGACY,
        _ => PROTOCOL_LEGACY,
    }
}

pub fn is_stateless_version(version: &str) -> bool {
    version == PROTOCOL_RC
}
