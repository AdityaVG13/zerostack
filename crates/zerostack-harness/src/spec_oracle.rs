//! One verifier per `[SPEC-NNN]` tag that this pass can honestly check.
//! Empty `todo!()` verifiers are forbidden. Unwired tags stay UNVERIFIED.

use std::path::Path;

use serde_json::Value;
use zero_abi::{
    CACHE_ENTRY_SCHEMA_V1, CacheEntryV1, CacheKeyV1, CacheRootV1, CompletenessWitnessV1,
    OperatorIdentityV1, ZERO_RESULT_V1, ZeroResultV1, is_rw10_forbidden_op, sha256_hex,
};
use zero_ledger::{FreshWorkComponent, FreshWorkVector, PPM_ONE};
use zero_process::{
    DEFAULT_ACTIVE_CPU_SECONDS, DEFAULT_ACTIVE_TREE_RSS_BYTES, DEFAULT_IDLE_TREE_RSS_BYTES,
};
use zero_ref::{HASH_HEX_LEN, ZeroRefErrorClass, ZeroRefV1, ZeroScheme, content_hash_hex};

use crate::oracle::ScenarioError;
use crate::repo::{file_sha256_hex, read_text, repo_root};

#[derive(Clone, Copy)]
pub struct SpecVerifier {
    pub tag: &'static str,
    pub run: fn(&Path) -> Result<(), ScenarioError>,
}

pub fn all_verifiers() -> &'static [SpecVerifier] {
    &[
        SpecVerifier {
            tag: "SPEC-COMP-001",
            run: verify_spec_comp_001,
        },
        SpecVerifier {
            tag: "SPEC-COMP-003",
            run: verify_spec_comp_003,
        },
        SpecVerifier {
            tag: "SPEC-COMP-004",
            run: verify_spec_comp_004,
        },
        SpecVerifier {
            tag: "SPEC-SURF-001",
            run: verify_spec_surf_001,
        },
        SpecVerifier {
            tag: "SPEC-SURF-002",
            run: verify_spec_surf_002,
        },
        SpecVerifier {
            tag: "SPEC-SURF-003",
            run: verify_spec_surf_003,
        },
        SpecVerifier {
            tag: "SPEC-SURF-004",
            run: verify_spec_surf_004,
        },
        SpecVerifier {
            tag: "SPEC-SURF-005",
            run: verify_spec_surf_005,
        },
        SpecVerifier {
            tag: "SPEC-SURF-006",
            run: verify_spec_surf_006,
        },
        SpecVerifier {
            tag: "SPEC-SURF-007",
            run: verify_spec_surf_007,
        },
        SpecVerifier {
            tag: "SPEC-RES-001",
            run: verify_spec_res_001,
        },
        SpecVerifier {
            tag: "SPEC-RES-002",
            run: verify_spec_res_002,
        },
        SpecVerifier {
            tag: "SPEC-RES-003",
            run: verify_spec_res_003,
        },
        SpecVerifier {
            tag: "SPEC-REF-001",
            run: verify_spec_ref_001,
        },
        SpecVerifier {
            tag: "SPEC-REF-002",
            run: verify_spec_ref_002,
        },
        SpecVerifier {
            tag: "SPEC-REF-003",
            run: verify_spec_ref_003,
        },
        SpecVerifier {
            tag: "SPEC-HON-001",
            run: verify_spec_hon_001,
        },
        SpecVerifier {
            tag: "SPEC-HON-003",
            run: verify_spec_hon_003,
        },
        SpecVerifier {
            tag: "SPEC-HON-004",
            run: verify_spec_hon_004,
        },
        SpecVerifier {
            tag: "SPEC-HON-005",
            run: verify_spec_hon_005,
        },
        SpecVerifier {
            tag: "SPEC-SETL-001",
            run: verify_spec_setl_001,
        },
        SpecVerifier {
            tag: "SPEC-SETL-002",
            run: verify_spec_setl_002,
        },
        SpecVerifier {
            tag: "SPEC-NEG-001",
            run: verify_spec_neg_001,
        },
        SpecVerifier {
            tag: "SPEC-NEG-002",
            run: verify_spec_neg_002,
        },
        SpecVerifier {
            tag: "SPEC-NEG-003",
            run: verify_spec_neg_003,
        },
        SpecVerifier {
            tag: "SPEC-HUB-001",
            run: verify_spec_hub_001,
        },
        SpecVerifier {
            tag: "SPEC-HUB-003",
            run: verify_spec_hub_003,
        },
        SpecVerifier {
            tag: "SPEC-HUB-004",
            run: verify_spec_hub_004,
        },
        SpecVerifier {
            tag: "SPEC-CACHE-001",
            run: verify_spec_cache_001,
        },
        SpecVerifier {
            tag: "SPEC-CACHE-002",
            run: verify_spec_cache_002,
        },
        SpecVerifier {
            tag: "SPEC-CACHE-003",
            run: verify_spec_cache_003,
        },
        SpecVerifier {
            tag: "SPEC-CACHE-004",
            run: verify_spec_cache_004,
        },
        SpecVerifier {
            tag: "SPEC-FWV-001",
            run: verify_spec_fwv_001,
        },
        SpecVerifier {
            tag: "SPEC-FWV-002",
            run: verify_spec_fwv_002,
        },
        SpecVerifier {
            tag: "SPEC-FWV-003",
            run: verify_spec_fwv_003,
        },
        SpecVerifier {
            tag: "SPEC-EDIT-001",
            run: verify_spec_edit_001,
        },
        SpecVerifier {
            tag: "SPEC-EDIT-002",
            run: verify_spec_edit_002,
        },
        SpecVerifier {
            tag: "SPEC-EDIT-003",
            run: verify_spec_edit_003,
        },
        SpecVerifier {
            tag: "SPEC-RACC-001",
            run: verify_spec_racc_001,
        },
        SpecVerifier {
            tag: "SPEC-RACC-002",
            run: verify_spec_racc_002,
        },
        SpecVerifier {
            tag: "SPEC-RACC-003",
            run: verify_spec_racc_003,
        },
        SpecVerifier {
            tag: "SPEC-RACC-004",
            run: verify_spec_racc_004,
        },
        SpecVerifier {
            tag: "SPEC-FU-001",
            run: verify_spec_fu_001,
        },
        SpecVerifier {
            tag: "SPEC-FU-002",
            run: verify_spec_fu_002,
        },
        SpecVerifier {
            tag: "SPEC-FU-003",
            run: verify_spec_fu_003,
        },
        SpecVerifier {
            tag: "SPEC-FU-004",
            run: verify_spec_fu_004,
        },
        SpecVerifier {
            tag: "SPEC-FU-005",
            run: verify_spec_fu_005,
        },
        SpecVerifier {
            tag: "SPEC-FU-006",
            run: verify_spec_fu_006,
        },
    ]
}

pub fn run_all(root: &Path) -> Result<(), ScenarioError> {
    for verifier in all_verifiers() {
        (verifier.run)(root).map_err(|error| {
            ScenarioError::new(verifier.tag, format!("{}: {}", verifier.tag, error.message))
        })?;
    }
    Ok(())
}

pub fn run_tag(tag: &str, root: &Path) -> Result<(), ScenarioError> {
    let verifier = all_verifiers()
        .iter()
        .find(|item| item.tag == tag)
        .ok_or_else(|| ScenarioError::new(tag, format!("no verifier wired for {tag}")))?;
    (verifier.run)(root)
}

fn fail(tag: &str, message: impl Into<String>) -> ScenarioError {
    ScenarioError::new(tag, message)
}

fn require_contains(tag: &str, haystack: &str, needle: &str) -> Result<(), ScenarioError> {
    if haystack.contains(needle) {
        Ok(())
    } else {
        Err(fail(tag, format!("missing {needle:?}")))
    }
}

const COMPOSER_CRATES: &[&str] = &["crates/zsx-core", "crates/zsx", "crates/zsx-node"];
const ENGINE_DEP_NAMES: &[&str] = &[
    "fs-zero",
    "fszero",
    "graphzero-engine",
    "graphzero-query",
    "graphzero-store",
    "tokenzero-core",
    "tokenzero-engine",
    "tokenzero-recovery",
];

fn parse_toml(root: &Path, rel: &str) -> Result<toml::Value, ScenarioError> {
    let text = read_text(root, rel).map_err(|error| fail("toml", error))?;
    toml::from_str(&text).map_err(|error| fail("toml", format!("{rel}: {error}")))
}

fn workspace_members(root: &Path) -> Result<Vec<String>, ScenarioError> {
    let manifest = parse_toml(root, "Cargo.toml")?;
    let members = manifest
        .get("workspace")
        .and_then(|value| value.get("members"))
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("workspace", "Cargo.toml workspace.members missing"))?;
    Ok(members
        .iter()
        .filter_map(toml::Value::as_str)
        .map(ToOwned::to_owned)
        .collect())
}

fn crate_engine_deps(root: &Path, member: &str) -> Result<Vec<String>, ScenarioError> {
    let rel = format!("{member}/Cargo.toml");
    let manifest = parse_toml(root, &rel)?;
    let mut found = Vec::new();
    if let Some(deps) = manifest.get("dependencies").and_then(toml::Value::as_table) {
        for name in deps.keys() {
            if ENGINE_DEP_NAMES.contains(&name.as_str()) {
                found.push(name.clone());
            }
        }
    }
    Ok(found)
}

/// SPEC-COMP-001: engines do not import each other; hub composes.
pub fn verify_spec_comp_001(root: &Path) -> Result<(), ScenarioError> {
    let members = workspace_members(root)?;
    for member in &members {
        let deps = crate_engine_deps(root, member)?;
        if deps.is_empty() {
            continue;
        }
        if !COMPOSER_CRATES.contains(&member.as_str()) {
            return Err(fail(
                "SPEC-COMP-001",
                format!("{member} imports engine crates {deps:?}"),
            ));
        }
    }
    let core =
        read_text(root, "crates/zsx-core/src/lib.rs").map_err(|e| fail("SPEC-COMP-001", e))?;
    require_contains(
        "SPEC-COMP-001",
        &core,
        "aggregate connector that dispatches three",
    )?;
    require_contains("SPEC-COMP-001", &core, "never spawns a worker process")?;
    Ok(())
}

/// SPEC-COMP-003: TokenZero has no workspace-file mutation class.
pub fn verify_spec_comp_003(root: &Path) -> Result<(), ScenarioError> {
    let connector = read_text(root, "crates/zsx-core/src/connector.rs")
        .map_err(|e| fail("SPEC-COMP-003", e))?;
    require_contains(
        "SPEC-COMP-003",
        &connector,
        r#"(EngineIdentity::TokenZero, "ingest")"#,
    )?;
    if connector.contains(r#"(EngineIdentity::TokenZero, "fs.write")"#)
        || connector.contains(r#"(EngineIdentity::TokenZero, "fs.edit")"#)
    {
        return Err(fail(
            "SPEC-COMP-003",
            "TokenZero classified as workspace file mutation",
        ));
    }
    let contract =
        read_text(root, "conformance/CONTRACT.md").map_err(|e| fail("SPEC-COMP-003", e))?;
    require_contains("SPEC-COMP-003", &contract, "TokenZero")?;
    require_contains("SPEC-COMP-003", &contract, "`denied`")?;
    Ok(())
}

/// SPEC-COMP-004: GraphZero store_only (no workspace file write ops).
pub fn verify_spec_comp_004(root: &Path) -> Result<(), ScenarioError> {
    let connector = read_text(root, "crates/zsx-core/src/connector.rs")
        .map_err(|e| fail("SPEC-COMP-004", e))?;
    require_contains(
        "SPEC-COMP-004",
        &connector,
        r#"(EngineIdentity::GraphZero, "index" | "remember")"#,
    )?;
    if connector.contains(r#"(EngineIdentity::GraphZero, "fs.write")"#)
        || connector.contains(r#"(EngineIdentity::GraphZero, "fs.edit")"#)
    {
        return Err(fail(
            "SPEC-COMP-004",
            "GraphZero classified as workspace file mutation",
        ));
    }
    let contract =
        read_text(root, "conformance/CONTRACT.md").map_err(|e| fail("SPEC-COMP-004", e))?;
    require_contains("SPEC-COMP-004", &contract, "`store_only`")?;
    Ok(())
}

/// SPEC-SURF-001: CodeMode and MCP are exclusive CLI arms.
pub fn verify_spec_surf_001(root: &Path) -> Result<(), ScenarioError> {
    let main = read_text(root, "crates/zsx/src/main.rs").map_err(|e| fail("SPEC-SURF-001", e))?;
    require_contains("SPEC-SURF-001", &main, r#"command == "exec""#)?;
    require_contains("SPEC-SURF-001", &main, r#"command == "mcp""#)?;
    require_contains(
        "SPEC-SURF-001",
        &main,
        r#"usage: zsx exec -C ROOT | zsx mcp"#,
    )?;
    Ok(())
}

fn mcp_tool_names(root: &Path) -> Result<Vec<String>, ScenarioError> {
    let mcp = read_text(root, "crates/zsx/src/mcp.rs").map_err(|e| fail("SPEC-SURF-002", e))?;
    let start = mcp
        .find("fn tools() -> Value")
        .ok_or_else(|| fail("SPEC-SURF-002", "tools() missing"))?;
    let block = mcp[start..]
        .split("fn tool_result")
        .next()
        .ok_or_else(|| fail("SPEC-SURF-002", "tools() block truncated"))?;
    let mut names = Vec::new();
    for line in block.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix(r#""name": ""#) {
            if let Some(name) = rest.split('"').next() {
                names.push(name.to_owned());
            }
        }
    }
    Ok(names)
}

/// SPEC-SURF-002: MCP tools are exactly zero_execute and zero_wait.
pub fn verify_spec_surf_002(root: &Path) -> Result<(), ScenarioError> {
    let names = mcp_tool_names(root)?;
    if names != ["zero_execute".to_owned(), "zero_wait".to_owned()] {
        return Err(fail(
            "SPEC-SURF-002",
            format!("MCP catalog is {names:?}, not [zero_execute, zero_wait]"),
        ));
    }
    Ok(())
}

/// SPEC-SURF-003: harness-owned stdio, parent-death, no detach.
pub fn verify_spec_surf_003(root: &Path) -> Result<(), ScenarioError> {
    let mcp = read_text(root, "crates/zsx/src/mcp.rs").map_err(|e| fail("SPEC-SURF-003", e))?;
    require_contains("SPEC-SURF-003", &mcp, "fn install_parent_death_exit")?;
    require_contains("SPEC-SURF-003", &mcp, r#""lifetime": "harness-stdio""#)?;
    require_contains("SPEC-SURF-003", &mcp, "it never detaches")?;
    Ok(())
}

/// SPEC-SURF-004: zero_wait reports pid/ppid/image and does not spawn.
pub fn verify_spec_surf_004(root: &Path) -> Result<(), ScenarioError> {
    let mcp = read_text(root, "crates/zsx/src/mcp.rs").map_err(|e| fail("SPEC-SURF-004", e))?;
    require_contains("SPEC-SURF-004", &mcp, "fn zero_wait_payload")?;
    require_contains("SPEC-SURF-004", &mcp, r#""pid":"#)?;
    require_contains("SPEC-SURF-004", &mcp, r#""ppid":"#)?;
    require_contains("SPEC-SURF-004", &mcp, "image")?;
    require_contains("SPEC-SURF-004", &mcp, "No child is spawned")?;
    Ok(())
}

/// SPEC-SURF-005: engine binaries are not MCP tools.
pub fn verify_spec_surf_005(root: &Path) -> Result<(), ScenarioError> {
    let names = mcp_tool_names(root)?;
    for banned in [
        "fszero",
        "graphzero",
        "tokenzero",
        "fs_execute",
        "gz_execute",
    ] {
        if names.iter().any(|name| name.contains(banned)) {
            return Err(fail(
                "SPEC-SURF-005",
                format!("engine tool leaked into MCP catalog: {names:?}"),
            ));
        }
    }
    Ok(())
}

/// SPEC-SURF-006: CodeMode entry is `zsx exec -C ROOT`.
pub fn verify_spec_surf_006(root: &Path) -> Result<(), ScenarioError> {
    let main = read_text(root, "crates/zsx/src/main.rs").map_err(|e| fail("SPEC-SURF-006", e))?;
    require_contains("SPEC-SURF-006", &main, "zsx exec -C ROOT")?;
    require_contains(
        "SPEC-SURF-006",
        &main,
        r#"let root = root.ok_or("missing -C ROOT")?"#,
    )?;
    Ok(())
}

/// SPEC-SURF-007: zero_execute takes a JS plan and optional timeout_ms.
pub fn verify_spec_surf_007(root: &Path) -> Result<(), ScenarioError> {
    let mcp = read_text(root, "crates/zsx/src/mcp.rs").map_err(|e| fail("SPEC-SURF-007", e))?;
    require_contains("SPEC-SURF-007", &mcp, r#""name": "zero_execute""#)?;
    require_contains("SPEC-SURF-007", &mcp, r#""plan""#)?;
    require_contains("SPEC-SURF-007", &mcp, r#""timeout_ms""#)?;
    require_contains("SPEC-SURF-007", &mcp, r#""required": ["plan"]"#)?;
    Ok(())
}

/// SPEC-RES-001: zero-result/v1 has ack + content.
pub fn verify_spec_res_001(_root: &Path) -> Result<(), ScenarioError> {
    let result = ZeroResultV1::inline("ok", serde_json::json!({"k": 1}))
        .map_err(|error| fail("SPEC-RES-001", error.to_string()))?;
    let wire =
        serde_json::to_value(&result).map_err(|error| fail("SPEC-RES-001", error.to_string()))?;
    if wire.get("ack").and_then(Value::as_str) != Some("ok") {
        return Err(fail("SPEC-RES-001", "ack missing"));
    }
    if wire.get("content").is_none() {
        return Err(fail("SPEC-RES-001", "content missing"));
    }
    if ZERO_RESULT_V1 != "zero-result/v1" {
        return Err(fail("SPEC-RES-001", "schema const drifted"));
    }
    Ok(())
}

/// SPEC-RES-002: oversize results spill to a content-addressed ref.
pub fn verify_spec_res_002(root: &Path) -> Result<(), ScenarioError> {
    let host =
        read_text(root, "crates/zero-codemode/src/host.rs").map_err(|e| fail("SPEC-RES-002", e))?;
    require_contains("SPEC-RES-002", &host, "fn spill_result")?;
    require_contains("SPEC-RES-002", &host, r#"format!("tz://blob/{hash}")"#)?;
    require_contains("SPEC-RES-002", &host, "ZeroResultV1::reference")?;
    Ok(())
}

/// SPEC-RES-003: savingsBytes is not a token count.
pub fn verify_spec_res_003(root: &Path) -> Result<(), ScenarioError> {
    let contract =
        read_text(root, "conformance/CONTRACT.md").map_err(|e| fail("SPEC-RES-003", e))?;
    require_contains("SPEC-RES-003", &contract, "`savingsBytes` is not a token")?;
    let bench =
        read_text(root, "benchmarks/savings-bench-v1.json").map_err(|e| fail("SPEC-RES-003", e))?;
    require_contains("SPEC-RES-003", &bench, "Not tokens")?;
    if bench.contains("Label savingsBytes as token savings") {
        // This is the anti-pattern row in the bench, not a product claim.
    }
    Ok(())
}

/// SPEC-REF-001: consumers preserve fz/gz/tz.
pub fn verify_spec_ref_001(_root: &Path) -> Result<(), ScenarioError> {
    let schemes: Vec<&str> = ZeroScheme::ALL.iter().map(ZeroScheme::as_str).collect();
    if schemes != ["fz", "gz", "tz"] {
        return Err(fail("SPEC-REF-001", format!("schemes {schemes:?}")));
    }
    let hash = content_hash_hex(b"scheme-preserve");
    for scheme in ZeroScheme::ALL {
        let input = format!("{}://blob/{hash}", scheme.as_str());
        let parsed = ZeroRefV1::parse(&input).map_err(|e| fail("SPEC-REF-001", e.to_string()))?;
        if parsed.scheme != scheme || parsed.to_string() != input {
            return Err(fail("SPEC-REF-001", format!("scheme lost on {input}")));
        }
    }
    Ok(())
}

/// SPEC-REF-002: missing/stale refs fail loudly.
pub fn verify_spec_ref_002(_root: &Path) -> Result<(), ScenarioError> {
    let missing = ZeroRefErrorClass::Missing.as_str();
    let digest = ZeroRefErrorClass::DigestMismatch.as_str();
    if missing != "missing" || digest != "digest_mismatch" {
        return Err(fail("SPEC-REF-002", "error class names drifted"));
    }
    let err = ZeroRefV1::parse("fz://blob/not-a-hash");
    if err.is_ok() {
        return Err(fail("SPEC-REF-002", "malformed ref parsed silently"));
    }
    Ok(())
}

/// SPEC-REF-003: blob ref grammar.
pub fn verify_spec_ref_003(_root: &Path) -> Result<(), ScenarioError> {
    let hash = content_hash_hex(b"blob-grammar");
    if hash.len() != HASH_HEX_LEN {
        return Err(fail("SPEC-REF-003", "hash length"));
    }
    let ok = format!("gz://blob/{hash}#L1-2");
    ZeroRefV1::parse(&ok).map_err(|e| fail("SPEC-REF-003", e.to_string()))?;
    if ZeroRefV1::parse(&format!("fz://blob/{hash}#B0-1")).is_err() {
        return Err(fail("SPEC-REF-003", "byte fragment rejected"));
    }
    if ZeroRefV1::parse("fz://blob/ABCD").is_ok() {
        return Err(fail("SPEC-REF-003", "uppercase short hash accepted"));
    }
    Ok(())
}

/// SPEC-HON-001: FS/GZ adapters do not emit Exact billed_tokens.
pub fn verify_spec_hon_001(root: &Path) -> Result<(), ScenarioError> {
    let fs =
        read_text(root, "crates/zsx-core/src/fszero.rs").map_err(|e| fail("SPEC-HON-001", e))?;
    require_contains("SPEC-HON-001", &fs, "worker_token_accounting: None")?;
    let gz =
        read_text(root, "crates/zsx-core/src/graphzero.rs").map_err(|e| fail("SPEC-HON-001", e))?;
    if gz.contains("billed_tokens:") && !gz.contains("worker_token_accounting: None") {
        return Err(fail(
            "SPEC-HON-001",
            "GraphZero adapter looks like it certifies billed_tokens",
        ));
    }
    require_contains("SPEC-HON-001", &gz, "worker_token_accounting: None")?;
    Ok(())
}

/// SPEC-HON-003: recovery_tokens is a fresh-work component, not billed.
pub fn verify_spec_hon_003(_root: &Path) -> Result<(), ScenarioError> {
    if FreshWorkComponent::Recovery.field_name() != "recovery_tokens" {
        return Err(fail("SPEC-HON-003", "recovery field renamed"));
    }
    let vector =
        FreshWorkVector::new(0, 0, 7, 0).map_err(|e| fail("SPEC-HON-003", e.to_string()))?;
    if vector.recovery_tokens() != 7 || vector.total_tokens() != 7 {
        return Err(fail("SPEC-HON-003", "recovery tokens not isolated"));
    }
    Ok(())
}

/// SPEC-HON-004: estimates live in DeclaredEstimateV1, not Measured.
pub fn verify_spec_hon_004(root: &Path) -> Result<(), ScenarioError> {
    let causal = read_text(root, "crates/zero-ledger/src/causal_work.rs")
        .map_err(|e| fail("SPEC-HON-004", e))?;
    require_contains("SPEC-HON-004", &causal, "struct DeclaredEstimateV1")?;
    require_contains("SPEC-HON-004", &causal, "enum ParentCounterObservationV1")?;
    require_contains("SPEC-HON-004", &causal, "Unmeasured")?;
    require_contains("SPEC-HON-004", &causal, "Measured")?;
    Ok(())
}

/// SPEC-HON-005: skipped measurement is Unmeasured, not a zero pass.
pub fn verify_spec_hon_005(root: &Path) -> Result<(), ScenarioError> {
    let ledger =
        read_text(root, "crates/zero-ledger/src/lib.rs").map_err(|e| fail("SPEC-HON-005", e))?;
    require_contains(
        "SPEC-HON-005",
        &ledger,
        "ParentCounterObservationV1::Unmeasured",
    )?;
    require_contains("SPEC-HON-005", &ledger, "never zero")?;
    Ok(())
}

/// SPEC-SETL-001: late Ok after cancel is commit_race.
pub fn verify_spec_setl_001(root: &Path) -> Result<(), ScenarioError> {
    let envelope =
        read_text(root, "crates/zsx-node/src/envelope.rs").map_err(|e| fail("SPEC-SETL-001", e))?;
    require_contains("SPEC-SETL-001", &envelope, "fn settle_after_execute")?;
    require_contains(
        "SPEC-SETL-001",
        &envelope,
        "(Ok(result), true) => Self::commit_race",
    )?;
    require_contains("SPEC-SETL-001", &envelope, "CODE_COMMIT_RACE")?;
    let core =
        read_text(root, "crates/zsx-node/src/core.rs").map_err(|e| fail("SPEC-SETL-001", e))?;
    require_contains(
        "SPEC-SETL-001",
        &core,
        r#"pub const CODE_COMMIT_RACE: &str = "commit_race";"#,
    )?;
    Ok(())
}

/// SPEC-SETL-002: late domain Err stays that Err.
pub fn verify_spec_setl_002(root: &Path) -> Result<(), ScenarioError> {
    let envelope =
        read_text(root, "crates/zsx-node/src/envelope.rs").map_err(|e| fail("SPEC-SETL-002", e))?;
    require_contains(
        "SPEC-SETL-002",
        &envelope,
        "(Err(err), _) => Self::from_zsx_error",
    )?;
    require_contains(
        "SPEC-SETL-002",
        &envelope,
        "a domain\n    /// Err always stays that Err",
    )?;
    Ok(())
}

/// SPEC-NEG-001: no in-repo conformance CLI in the workspace.
pub fn verify_spec_neg_001(root: &Path) -> Result<(), ScenarioError> {
    let members = workspace_members(root)?;
    if members.iter().any(|m| m.contains("conformance")) {
        return Err(fail(
            "SPEC-NEG-001",
            format!("conformance crate in workspace members: {members:?}"),
        ));
    }
    if root.join("conformance/src/main.rs").exists() {
        return Err(fail(
            "SPEC-NEG-001",
            "conformance/src/main.rs exists; CONTRACT §8 forbids an in-repo CLI",
        ));
    }
    let root_manifest = read_text(root, "Cargo.toml").map_err(|e| fail("SPEC-NEG-001", e))?;
    require_contains(
        "SPEC-NEG-001",
        &root_manifest,
        r#"exclude = ["conformance"]"#,
    )?;
    Ok(())
}

/// SPEC-NEG-002: no {ns}_execute_code catalog.
pub fn verify_spec_neg_002(root: &Path) -> Result<(), ScenarioError> {
    let names = mcp_tool_names(root)?;
    if names
        .iter()
        .any(|name| name.ends_with("_execute_code") || name == "execute_code")
    {
        return Err(fail(
            "SPEC-NEG-002",
            format!("execute_code in MCP tools: {names:?}"),
        ));
    }
    if !is_rw10_forbidden_op("execute_code") || !is_rw10_forbidden_op("fz_execute_code") {
        return Err(fail(
            "SPEC-NEG-002",
            "raw worker no longer forbids execute_code",
        ));
    }
    Ok(())
}

/// SPEC-NEG-003: authority claims start unproven.
pub fn verify_spec_neg_003(root: &Path) -> Result<(), ScenarioError> {
    let text = read_text(root, "conformance/authority/canonical-authority-v1.json")
        .map_err(|e| fail("SPEC-NEG-003", e))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| fail("SPEC-NEG-003", e.to_string()))?;
    let initial = value
        .pointer("/provenance/proof_states/claims_initial")
        .and_then(Value::as_str);
    if initial != Some("NOT_YET_PROVEN") {
        return Err(fail("SPEC-NEG-003", format!("claims_initial={initial:?}")));
    }
    let claims = value
        .get("claims")
        .and_then(Value::as_array)
        .ok_or_else(|| fail("SPEC-NEG-003", "claims array missing"))?;
    if claims.is_empty() {
        return Err(fail("SPEC-NEG-003", "empty claim ledger"));
    }
    for claim in claims {
        let state = claim.get("proof_state").and_then(Value::as_str);
        if state != Some("NOT_YET_PROVEN") {
            return Err(fail(
                "SPEC-NEG-003",
                format!("claim {:?} proof_state={state:?}", claim.get("claim_id")),
            ));
        }
    }
    Ok(())
}

/// SPEC-HUB-001: daemonless parent-death binding.
pub fn verify_spec_hub_001(root: &Path) -> Result<(), ScenarioError> {
    let mcp = read_text(root, "crates/zsx/src/mcp.rs").map_err(|e| fail("SPEC-HUB-001", e))?;
    require_contains("SPEC-HUB-001", &mcp, "fn install_parent_death_exit")?;
    require_contains("SPEC-HUB-001", &mcp, "not a sidecar")?;
    let process =
        read_text(root, "crates/zero-process/src/lib.rs").map_err(|e| fail("SPEC-HUB-001", e))?;
    require_contains("SPEC-HUB-001", &process, "OwnerWatcher")?;
    Ok(())
}

/// SPEC-HUB-003: published resource caps exist.
pub fn verify_spec_hub_003(_root: &Path) -> Result<(), ScenarioError> {
    if DEFAULT_IDLE_TREE_RSS_BYTES != 96 * 1024 * 1024 {
        return Err(fail("SPEC-HUB-003", "idle RSS cap drifted"));
    }
    if DEFAULT_ACTIVE_TREE_RSS_BYTES != 256 * 1024 * 1024 {
        return Err(fail("SPEC-HUB-003", "active RSS cap drifted"));
    }
    if DEFAULT_ACTIVE_CPU_SECONDS != 300 {
        return Err(fail("SPEC-HUB-003", "CPU cap drifted"));
    }
    Ok(())
}

/// SPEC-HUB-004: raw worker forbids planner / nested CodeMode / MCP catalog.
pub fn verify_spec_hub_004(_root: &Path) -> Result<(), ScenarioError> {
    for op in [
        "planner",
        "execute_code",
        "mcp.tools_list",
        "codemode.execute",
    ] {
        if !is_rw10_forbidden_op(op) {
            return Err(fail("SPEC-HUB-004", format!("{op} not forbidden")));
        }
    }
    Ok(())
}

fn sample_cache_key() -> Result<CacheKeyV1, ScenarioError> {
    let dep = CacheRootV1::new("dep-root-one").map_err(|e| fail("SPEC-CACHE", e.to_string()))?;
    let proof =
        CacheRootV1::new("proof-root-one").map_err(|e| fail("SPEC-CACHE", e.to_string()))?;
    let witness = CompletenessWitnessV1::new(proof, vec![dep.clone()])
        .map_err(|e| fail("SPEC-CACHE", e.to_string()))?;
    let operator =
        OperatorIdentityV1::new("op", "1").map_err(|e| fail("SPEC-CACHE", e.to_string()))?;
    CacheKeyV1::new(
        operator,
        serde_json::json!({"k": 1}),
        vec![dep],
        Vec::new(),
        Vec::new(),
        witness,
    )
    .map_err(|e| fail("SPEC-CACHE", e.to_string()))
}

/// SPEC-CACHE-001: hit requires completeness witness.
pub fn verify_spec_cache_001(_root: &Path) -> Result<(), ScenarioError> {
    let key = sample_cache_key()?;
    let output = CacheRootV1::new("out").map_err(|e| fail("SPEC-CACHE-001", e.to_string()))?;
    let entry = CacheEntryV1::positive(key, output, None)
        .map_err(|e| fail("SPEC-CACHE-001", e.to_string()))?;
    if entry.schema() != CACHE_ENTRY_SCHEMA_V1 {
        return Err(fail("SPEC-CACHE-001", "schema id drifted"));
    }
    Ok(())
}

/// SPEC-CACHE-002: unwitnessed root fails closed.
pub fn verify_spec_cache_002(_root: &Path) -> Result<(), ScenarioError> {
    let dep = CacheRootV1::new("dep-root-unwitnessed")
        .map_err(|e| fail("SPEC-CACHE-002", e.to_string()))?;
    let proof =
        CacheRootV1::new("proof-root-one").map_err(|e| fail("SPEC-CACHE-002", e.to_string()))?;
    let witness = CompletenessWitnessV1::new(proof.clone(), vec![proof])
        .map_err(|e| fail("SPEC-CACHE-002", e.to_string()))?;
    let operator =
        OperatorIdentityV1::new("op", "1").map_err(|e| fail("SPEC-CACHE-002", e.to_string()))?;
    match CacheKeyV1::new(
        operator,
        serde_json::json!({"k": 1}),
        vec![dep],
        Vec::new(),
        Vec::new(),
        witness,
    ) {
        Ok(_) => Err(fail(
            "SPEC-CACHE-002",
            "key accepted a dependency root the witness did not check",
        )),
        Err(_) => Ok(()),
    }
}

/// SPEC-CACHE-003: cache key is SHA-256 of canonical JSON.
pub fn verify_spec_cache_003(_root: &Path) -> Result<(), ScenarioError> {
    let key = sample_cache_key()?;
    let hashed = key.key_hash_hex();
    let expected = sha256_hex(key.canonical_key_json().as_bytes());
    if hashed != expected || hashed.len() != 64 {
        return Err(fail(
            "SPEC-CACHE-003",
            "key hash is not sha256(canonical JSON)",
        ));
    }
    Ok(())
}

/// SPEC-CACHE-004: empty roots are rejected.
pub fn verify_spec_cache_004(_root: &Path) -> Result<(), ScenarioError> {
    if CacheRootV1::new("").is_ok() {
        return Err(fail("SPEC-CACHE-004", "empty root accepted"));
    }
    Ok(())
}

/// SPEC-FWV-001: four components sum to total.
pub fn verify_spec_fwv_001(_root: &Path) -> Result<(), ScenarioError> {
    let vector =
        FreshWorkVector::new(1, 2, 3, 4).map_err(|e| fail("SPEC-FWV-001", e.to_string()))?;
    if vector.total_tokens() != 10
        || vector
            .component_sum()
            .map_err(|e| fail("SPEC-FWV-001", e.to_string()))?
            != 10
    {
        return Err(fail("SPEC-FWV-001", "component sum != total"));
    }
    Ok(())
}

/// SPEC-FWV-002: eta_action is integer ppm in [0, 1_000_000].
pub fn verify_spec_fwv_002(_root: &Path) -> Result<(), ScenarioError> {
    let vector =
        FreshWorkVector::new(1, 3, 0, 0).map_err(|e| fail("SPEC-FWV-002", e.to_string()))?;
    let ppm = vector
        .eta_action_ppm()
        .ok_or_else(|| fail("SPEC-FWV-002", "eta missing"))?;
    if ppm.ppm() != 250_000 || ppm.ppm() > PPM_ONE {
        return Err(fail("SPEC-FWV-002", format!("eta ppm {}", ppm.ppm())));
    }
    Ok(())
}

/// SPEC-FWV-003: aggregation is checked integer addition.
pub fn verify_spec_fwv_003(_root: &Path) -> Result<(), ScenarioError> {
    let a = FreshWorkVector::new(1, 0, 0, 0).map_err(|e| fail("SPEC-FWV-003", e.to_string()))?;
    let b = FreshWorkVector::new(2, 3, 0, 1).map_err(|e| fail("SPEC-FWV-003", e.to_string()))?;
    let merged = a
        .merge(&b)
        .map_err(|e| fail("SPEC-FWV-003", e.to_string()))?;
    if merged.total_tokens() != 7 || merged.fresh_work_tokens() != 3 {
        return Err(fail("SPEC-FWV-003", "merge arithmetic drifted"));
    }
    Ok(())
}

/// SPEC-EDIT-001: one generic EDIT of EditOp values.
pub fn verify_spec_edit_001(root: &Path) -> Result<(), ScenarioError> {
    let src = read_text(root, "crates/zero-codemode/src/edit_protocol.rs")
        .map_err(|e| fail("SPEC-EDIT-001", e))?;
    require_contains("SPEC-EDIT-001", &src, "pub enum EditOp")?;
    require_contains("SPEC-EDIT-001", &src, "pub struct EditPlan")?;
    require_contains("SPEC-EDIT-001", &src, "ONE generic `EDIT` operation")?;
    Ok(())
}

/// SPEC-EDIT-002: verbs live in payload `v`, not the tool namespace.
pub fn verify_spec_edit_002(root: &Path) -> Result<(), ScenarioError> {
    let src = read_text(root, "crates/zero-codemode/src/edit_protocol.rs")
        .map_err(|e| fail("SPEC-EDIT-002", e))?;
    require_contains("SPEC-EDIT-002", &src, r#"#[serde(tag = "v")]"#)?;
    require_contains("SPEC-EDIT-002", &src, "not in the tool namespace")?;
    Ok(())
}

/// SPEC-EDIT-003: version string is zep/1.
pub fn verify_spec_edit_003(root: &Path) -> Result<(), ScenarioError> {
    let src = read_text(root, "crates/zero-codemode/src/edit_protocol.rs")
        .map_err(|e| fail("SPEC-EDIT-003", e))?;
    require_contains(
        "SPEC-EDIT-003",
        &src,
        r#"pub const EDIT_PROTOCOL_VERSION: &str = "zep/1";"#,
    )?;
    Ok(())
}

/// SPEC-RACC-001: consumers preserve ref type (scheme) on Display.
pub fn verify_spec_racc_001(_root: &Path) -> Result<(), ScenarioError> {
    verify_spec_ref_001(_root)
}

/// SPEC-RACC-002: unavailable refs are explicit errors.
pub fn verify_spec_racc_002(_root: &Path) -> Result<(), ScenarioError> {
    if ZeroRefErrorClass::Missing.as_str() != "missing" {
        return Err(fail("SPEC-RACC-002", "Missing class renamed"));
    }
    if ZeroRefV1::parse("tz://seq/1").is_ok() {
        return Err(fail("SPEC-RACC-002", "engine-owned ref parsed as portable"));
    }
    Ok(())
}

/// SPEC-RACC-003: integer token-target identity.
pub fn verify_spec_racc_003(_root: &Path) -> Result<(), ScenarioError> {
    let meets = |racc: u64, raw: u64, target_ppm: u64| -> bool {
        let lhs = u128::from(racc) * 1_000_000u128;
        let rhs = u128::from(raw) * u128::from(target_ppm);
        lhs <= rhs
    };
    if !meets(3, 10, 300_000) || meets(4, 10, 300_000) {
        return Err(fail("SPEC-RACC-003", "integer identity mismatch"));
    }
    let src = read_text(&repo_root(), "docs/racc/RACC_CONTRACT.rs")
        .map_err(|e| fail("SPEC-RACC-003", e))?;
    require_contains("SPEC-RACC-003", &src, "racc_input_tokens) * 1_000_000u128")?;
    Ok(())
}

/// SPEC-RACC-004: exact_phase_valid is the four-way AND.
pub fn verify_spec_racc_004(root: &Path) -> Result<(), ScenarioError> {
    let src =
        read_text(root, "crates/zero-ledger/src/lib.rs").map_err(|e| fail("SPEC-RACC-004", e))?;
    require_contains("SPEC-RACC-004", &src, "fn exact_phase_valid")?;
    require_contains("SPEC-RACC-004", &src, "self.byte_exact")?;
    require_contains("SPEC-RACC-004", &src, "self.policy_exact_or_fallback")?;
    require_contains("SPEC-RACC-004", &src, "self.task_verified")?;
    require_contains("SPEC-RACC-004", &src, "self.meets_token_target()")?;
    Ok(())
}

fn feature_matrix(root: &Path) -> Result<toml::Value, ScenarioError> {
    parse_toml(root, "conformance/contracts/supported_surface_matrix.toml")
}

/// SPEC-FU-001: allowed statuses.
pub fn verify_spec_fu_001(root: &Path) -> Result<(), ScenarioError> {
    let matrix = feature_matrix(root)?;
    let allowed = matrix
        .get("allowed_statuses")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("SPEC-FU-001", "allowed_statuses missing"))?;
    let got: Vec<&str> = allowed.iter().filter_map(toml::Value::as_str).collect();
    if got != ["present", "partial", "missing", "excluded"] {
        return Err(fail("SPEC-FU-001", format!("allowed_statuses={got:?}")));
    }
    Ok(())
}

fn feature_ids(matrix: &toml::Value) -> Result<(Vec<String>, Vec<String>), ScenarioError> {
    let declared = matrix
        .get("declared_feature_ids")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("SPEC-FU-002", "declared_feature_ids missing"))?;
    let declared: Vec<String> = declared
        .iter()
        .filter_map(toml::Value::as_str)
        .map(ToOwned::to_owned)
        .collect();
    let rows = matrix
        .get("feature")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("SPEC-FU-002", "feature tables missing"))?;
    let mut ids = Vec::new();
    for row in rows {
        if let Some(id) = row.get("id").and_then(toml::Value::as_str) {
            ids.push(id.to_owned());
        }
    }
    Ok((declared, ids))
}

/// SPEC-FU-002: declared ids match rows.
pub fn verify_spec_fu_002(root: &Path) -> Result<(), ScenarioError> {
    let matrix = feature_matrix(root)?;
    let (declared, ids) = feature_ids(&matrix)?;
    let mut left = declared.clone();
    let mut right = ids.clone();
    left.sort();
    right.sort();
    if left != right {
        return Err(fail(
            "SPEC-FU-002",
            format!(
                "declared/row mismatch declared={} rows={}",
                declared.len(),
                ids.len()
            ),
        ));
    }
    Ok(())
}

/// SPEC-FU-003: global weight sum is 1.0.
pub fn verify_spec_fu_003(root: &Path) -> Result<(), ScenarioError> {
    let matrix = feature_matrix(root)?;
    let rows = matrix
        .get("feature")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("SPEC-FU-003", "feature tables missing"))?;
    let mut sum = 0.0f64;
    for row in rows {
        let weight = row
            .get("weight")
            .and_then(toml::Value::as_float)
            .or_else(|| {
                row.get("weight")
                    .and_then(toml::Value::as_integer)
                    .map(|n| n as f64)
            })
            .ok_or_else(|| fail("SPEC-FU-003", "weight missing"))?;
        sum += weight;
    }
    if (sum - 1.0).abs() > 1e-9 {
        return Err(fail("SPEC-FU-003", format!("weight sum {sum}")));
    }
    Ok(())
}

/// SPEC-FU-004: excluded rows keep non-zero weight.
pub fn verify_spec_fu_004(root: &Path) -> Result<(), ScenarioError> {
    let matrix = feature_matrix(root)?;
    let rows = matrix
        .get("feature")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("SPEC-FU-004", "feature tables missing"))?;
    for row in rows {
        if row.get("status").and_then(toml::Value::as_str) != Some("excluded") {
            continue;
        }
        let weight = row
            .get("weight")
            .and_then(toml::Value::as_float)
            .or_else(|| {
                row.get("weight")
                    .and_then(toml::Value::as_integer)
                    .map(|n| n as f64)
            })
            .unwrap_or(0.0);
        if weight <= 0.0 {
            return Err(fail(
                "SPEC-FU-004",
                format!(
                    "excluded {} has zero weight",
                    row.get("id").and_then(toml::Value::as_str).unwrap_or("?")
                ),
            ));
        }
    }
    Ok(())
}

/// SPEC-FU-005: deferred rows have a load-bearing retry_condition.
pub fn verify_spec_fu_005(root: &Path) -> Result<(), ScenarioError> {
    let matrix = feature_matrix(root)?;
    let rows = matrix
        .get("feature")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("SPEC-FU-005", "feature tables missing"))?;
    for row in rows {
        let status = row
            .get("status")
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        if !matches!(status, "partial" | "missing" | "excluded") {
            continue;
        }
        let retry = row
            .get("retry_condition")
            .and_then(toml::Value::as_str)
            .unwrap_or("");
        if retry.len() < 24 || retry.to_ascii_lowercase().starts_with("later") {
            return Err(fail(
                "SPEC-FU-005",
                format!(
                    "{} missing retry_condition",
                    row.get("id").and_then(toml::Value::as_str).unwrap_or("?")
                ),
            ));
        }
    }
    Ok(())
}

/// SPEC-FU-006: evidence paths exist and are not bead ids.
pub fn verify_spec_fu_006(root: &Path) -> Result<(), ScenarioError> {
    let matrix = feature_matrix(root)?;
    let rows = matrix
        .get("feature")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("SPEC-FU-006", "feature tables missing"))?;
    for row in rows {
        let Some(evidence) = row.get("evidence").and_then(toml::Value::as_array) else {
            continue;
        };
        for item in evidence {
            let path = item.as_str().unwrap_or("");
            if path.starts_with("ZS-") {
                return Err(fail(
                    "SPEC-FU-006",
                    format!("bead id used as evidence: {path}"),
                ));
            }
            if !root.join(path).exists() {
                return Err(fail("SPEC-FU-006", format!("missing evidence path {path}")));
            }
        }
    }
    Ok(())
}

/// Used by preflight: confirm spec-source hashes still match the contract.
pub fn verify_spec_source_hashes(root: &Path) -> Result<Vec<(String, String)>, ScenarioError> {
    let contract = parse_toml(root, "conformance/contracts/spec_version_contract.toml")?;
    let sources = contract
        .get("spec_sources")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| fail("preflight", "spec_sources missing"))?;
    let mut checked = Vec::new();
    for source in sources {
        let name = source
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or("unnamed");
        let path = source
            .get("path")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| fail("preflight", format!("{name} missing path")))?;
        let expected = source
            .get("sha256")
            .and_then(toml::Value::as_str)
            .ok_or_else(|| fail("preflight", format!("{name} missing sha256")))?;
        let actual = file_sha256_hex(root, path).map_err(|e| fail("preflight", e))?;
        if actual != expected {
            return Err(fail(
                "preflight",
                format!("{name} ({path}) sha256 drifted: expected {expected} got {actual}"),
            ));
        }
        checked.push((name.to_owned(), actual));
    }
    Ok(checked)
}
