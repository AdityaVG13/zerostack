#![forbid(unsafe_code)]

//! Greenfield oracle harness for ZeroStack.
//!
//! This crate implements [`conformance/CONTRACT.md`](../../conformance/CONTRACT.md).
//! It is not a conformance CLI catalog and does not register MCP tools.

pub mod differential_v2;
pub mod engine_identity;
pub mod external_tool_oracle;
pub mod golden;
pub mod oracle;
pub mod oracle_preflight_doctor;
pub mod property_oracle;
pub mod repo;
pub mod roundtrip_oracle;
pub mod self_oracle;
pub mod spec_oracle;

pub use engine_identity::{
    EngineIdentity, EngineRole, SUBJECT_IDENTITY_LABEL, assert_subject_ne_oracle,
    oracle_label_is_allowed,
};
pub use oracle::{
    ExternalTool, OracleMode, ScenarioError, SubjectOutput, SubjectState, compare, scenario,
};
pub use repo::repo_root;
