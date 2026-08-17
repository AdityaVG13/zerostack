#![forbid(unsafe_code)]

//! Greenfield oracle harness for ZeroStack.
//!
//! This crate implements [`conformance/CONTRACT.md`](../../conformance/CONTRACT.md).
//! It is not a conformance CLI catalog and does not register MCP tools.

pub mod crash_boundary;
pub mod differential_v2;
pub mod engine_identity;
pub mod eprocess;
pub mod external_tool_oracle;
pub mod failure_bundle;
pub mod fault_vfs;
pub mod golden;
pub mod hot_path_profile_snapshot;
pub mod measure;
pub mod metamorphic;
pub mod mismatch;
pub mod mismatch_minimizer;
pub mod oracle;
pub mod oracle_preflight_doctor;
pub mod property_oracle;
pub mod repo;
pub mod roundtrip_oracle;
pub mod self_oracle;
pub mod spec_oracle;

pub use engine_identity::{
    assert_subject_ne_oracle, oracle_label_is_allowed, EngineIdentity, EngineRole,
    SUBJECT_IDENTITY_LABEL,
};
pub use oracle::{
    compare, scenario, ExternalTool, OracleMode, ScenarioError, SubjectOutput, SubjectState,
};
pub use repo::repo_root;
