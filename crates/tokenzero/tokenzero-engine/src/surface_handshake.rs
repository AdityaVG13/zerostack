//! zerostack.surface capability handshake (tokenzero-irx9.4).
//!
//! Additive capability record for trusted composition: selected surface,
//! semantic contract version/digest, plan forms, ref protocol, limits, and
//! raw-worker protocol version. Handshake does **not** require listing the full
//! operation catalog.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokenzero_core::operation_abi::{SEMANTIC_CONTRACT_VERSION, contract_digest_hex};

/// Shared family capability schema name (identical field names across engines).
pub const SURFACE_CAPABILITY_SCHEMA: &str = "zerostack.surface";

/// Canonical private raw-worker protocol advertised in the handshake.
pub const RAW_WORKER_PROTOCOL_VERSION: &str = zero_abi::RAW_WORKER_PROTOCOL_VERSION;

/// Selected package / process surface for this binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HandshakeSurface {
    Mcp,
    /// Internal composition path (hub private worker) — not a user install.
    RawWorker,
}

impl HandshakeSurface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::RawWorker => "raw_worker",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "mcp" | "fastmcp" | "per-op" | "per_op" => Ok(Self::Mcp),
            "raw_worker" | "raw-worker" | "private_worker" | "private-worker" => {
                Ok(Self::RawWorker)
            }
            other => Err(format!(
                "unknown handshake surface {other:?}; expected mcp or raw_worker"
            )),
        }
    }
}

/// Who owns planning for this process (prevents double lifting).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlannerOwner {
    /// Outer hub / OMP client plans; this process is a raw executor.
    Client,
    /// No planner — pure per-op FastMCP surface.
    None,
}

impl PlannerOwner {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::None => "none",
        }
    }
}

/// Who may compress intermediate results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompressionOwner {
    Engine,
    Client,
    Both,
}

impl CompressionOwner {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Engine => "engine",
            Self::Client => "client",
            Self::Both => "both",
        }
    }
}

/// Additive capability record for surface selection and raw-worker composition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceCapability {
    pub schema: String,
    pub surface: String,
    pub planner_owner: String,
    pub compression_owner: String,
    pub semantic_contract_name: String,
    pub semantic_contract_version: String,
    pub semantic_contract_digest: String,
    pub operation_registry_digest: String,
    pub ref_scheme: String,
    pub ref_version: String,
    pub plan_forms: Vec<String>,
    pub raw_worker_version: String,
    pub cancellation: bool,
    pub transactions: bool,
    pub streaming: bool,
    pub limits: SurfaceLimits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceLimits {
    pub max_visible_tokens: u64,
    pub hard_max_wall_ms: u64,
    pub default_shell_timeout_secs: u64,
}

/// Build the handshake for a selected surface. Does not enumerate tools.
pub fn build_surface_capability(surface: HandshakeSurface) -> SurfaceCapability {
    let digest = contract_digest_hex();
    let (planner, plan_forms) = match surface {
        HandshakeSurface::Mcp => (PlannerOwner::None, vec!["none".into()]),
        HandshakeSurface::RawWorker => (PlannerOwner::Client, vec!["raw_frame".into()]),
    };
    SurfaceCapability {
        schema: SURFACE_CAPABILITY_SCHEMA.into(),
        surface: surface.as_str().into(),
        planner_owner: planner.as_str().into(),
        compression_owner: CompressionOwner::Engine.as_str().into(),
        semantic_contract_name: "tokenzero.operation_abi".into(),
        semantic_contract_version: SEMANTIC_CONTRACT_VERSION.into(),
        semantic_contract_digest: digest.clone(),
        operation_registry_digest: digest,
        ref_scheme: "tz".into(),
        ref_version: "v1".into(),
        plan_forms,
        raw_worker_version: RAW_WORKER_PROTOCOL_VERSION.into(),
        cancellation: true,
        transactions: false,
        streaming: false,
        limits: SurfaceLimits {
            max_visible_tokens: 4000,
            hard_max_wall_ms: tokenzero_core::operation_abi::ABI_HARD_MAX_WALL_MS,
            default_shell_timeout_secs:
                tokenzero_core::operation_abi::ABI_DEFAULT_SHELL_TIMEOUT_SECS,
        },
    }
}

/// Serialize handshake as JSON (catalog-free).
pub fn surface_capability_json(surface: HandshakeSurface) -> Value {
    serde_json::to_value(build_surface_capability(surface)).expect("SurfaceCapability serializes")
}

/// Compatibility check: peer digest must match local contract when provided.
pub fn check_contract_compatibility(
    local: &SurfaceCapability,
    peer_digest: Option<&str>,
    peer_version: Option<&str>,
) -> Result<(), String> {
    if let Some(peer_v) = peer_version
        && peer_v != local.semantic_contract_version
    {
        return Err(format!(
            "semantic contract version mismatch: local={} peer={peer_v} digest_local={}",
            local.semantic_contract_version, local.semantic_contract_digest
        ));
    }
    if let Some(peer_d) = peer_digest
        && peer_d != local.semantic_contract_digest
    {
        return Err(format!(
            "semantic contract digest mismatch: local={} peer={peer_d} version={}",
            local.semantic_contract_digest, local.semantic_contract_version
        ));
    }
    Ok(())
}

/// Trace fields attached to raw-worker / composition responses (irx9.4 AC).
pub fn composition_trace(
    surface: HandshakeSurface,
    planner_owner: PlannerOwner,
    compression_owner: CompressionOwner,
    boundary_count: u32,
) -> Value {
    let cap = build_surface_capability(surface);
    json!({
        "planner_owner": planner_owner.as_str(),
        "compression_owner": compression_owner.as_str(),
        "surface": surface.as_str(),
        "contract_digest": cap.semantic_contract_digest,
        "boundary_count": boundary_count,
        "raw_worker_version": RAW_WORKER_PROTOCOL_VERSION,
    })
}

