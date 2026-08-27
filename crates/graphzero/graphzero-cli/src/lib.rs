//! GraphZero CLI library surface for multi-bin packaging (graphzero-o2uq.3).
//!
//! The primary user binary remains `graphzero` (`src/main.rs`). Release
//! artifacts `graphzero-mcp` and `graphzero-codemode` link this crate so they
//! share one core, digest, and doctor identity.

pub mod agent_cli;
pub mod agent_errors;
pub mod agent_output;
pub mod agent_subcommand_hints;
pub mod blast_tools;
pub mod cli_args;
pub mod commands;
pub mod daemon;
pub mod dispatch;
pub mod dispatch_help;
pub mod fastmcp_adapter;
#[cfg(feature = "surface-mcp")]
pub mod fastmcp_mode;
pub mod mcp;
pub mod mcp_catalog;
pub mod mcp_protocol;
pub mod pack_cmd;
pub mod packaging;
pub mod query_surface_tools;
pub mod reserve_tools;
pub mod why_cmd;
pub mod zeroref_fixture;

/// Fail closed if a packaged single-surface process has both surface features.
///
/// Release artifacts set `GRAPHZERO_PACKAGE_SURFACE` before startup; a dual
/// feature compile under that env is rejected (never advertise mixed catalogs).
pub fn assert_packaged_surface_features(locked: packaging::PackageSurface) -> Result<(), String> {
    packaging::assert_packaged_surface_features(locked)
}
