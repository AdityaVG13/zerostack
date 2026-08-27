//! Shared indexed-repo fixtures for integration tests across GraphZero crates.
//!
//! These helpers are test-only and intentionally panic when fixture setup encounters an
//! environment failure (such as filesystem, process, serialization, or lock failure). Such
//! panics include contextual messages identifying the failed setup operation.

pub mod env;
pub use env::{ScopedEnvVars, lock_env};

pub mod basic;
pub mod blast;
pub mod gates;
pub mod git;
pub mod reserve;
pub mod scaled;
pub mod store_helpers;

pub use basic::{BasicFixture, FILE_A, FILE_B, write_alpha_beta_repo};
pub use blast::{
    BlastFixture, blast_git_indexed_fixture, blast_indexed_fixture, blast_indexed_fixture_from,
    write_blast_repo,
};
pub use git::{git_commit_all, unique_session_id};
pub use reserve::{
    ReserveFixture, load_config_ops, parse_ref_ops, reserve_indexed_fixture, use_parse_ref_ops,
};
pub use scaled::{ScaledFixture, indexed_scaled_repo, write_scaled_repo};
pub use store_helpers::{evidence_for_file, minimal_batch, publish_token};
