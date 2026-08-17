//! `zsx exec`: the canonical single-process execution path.
//!
//! This module deliberately contains no process spawning and no session
//! socket: it builds a `zsx_core` session with the three real in-process
//! engine adapters — FSZero, GraphZero, and TokenZero — through the
//! canonical builder (`with_session_id` + `build_canonical`) and calls the
//! embedded core directly. No fixture and no process-backed compatibility
//! domain implementation is linked: the `fszero`/`graphzero`/`tokenzero`
//! features register the engine repositories' concrete `DomainAdapter`s.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use zsx_core::{
    ZsxSession, fs_write_grant_count_for_plan, harness_fs_write_grants,
};

pub use zsx_core::ZSX_PROTOCOL;
/// Default execution timeout, matching the historical 30s zsx default.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// Execute one plan in-process and return the zsx result envelope.
pub fn exec(
    root: PathBuf,
    source: &str,
    timeout: Duration,
) -> Result<Value, Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    let session_id = format!("zsx-{:x}", std::process::id());
    let state_root = root.join(".zerostack");
    let root_text = root.to_string_lossy().into_owned();
    let session = ZsxSession::builder(root)
        .with_state_root(state_root)
        .with_session_id(session_id)
        .build_canonical()?;
    let grants = harness_fs_write_grants(
        &root_text,
        1,
        1,
        fs_write_grant_count_for_plan(source),
    );
    let result = session.execute_with_approvals(1, 1, source, timeout, grants)?;
    session.shutdown()?;
    Ok(json!({
        "protocol": ZSX_PROTOCOL,
        "ok": true,
        "generation": result.generation,
        "request_id": result.request_id,
        "result": result.value,
    }))
}
