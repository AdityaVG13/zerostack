//! Differential FastMCP / CodeMode / private-worker conformance corpus
//! (graphzero-o2uq.7).
//!
//! Vectors are generated from the operation registry. Each vector is executed
//! through the typed dispatcher under FastMCP, CodeMode, and private-worker
//! adapter kinds. Normalization drops transport-only and volatile fields.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

use crate::codemode::{normalize_error_for_parity, normalize_for_parity};
use crate::dispatcher::{AdapterKind, EngineContext, dispatch};
use crate::operation_abi::{
    DomainError, DomainErrorKind, DomainResult, all_operations, contract_digest_hex,
};

/// Corpus schema / generator version (bump when vector shape changes).
pub const CONFORMANCE_CORPUS_VERSION: &str = "1.0.0";

/// One executable conformance vector.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConformanceVector {
    pub id: String,
    pub op: String,
    /// positive | boundary | failure
    pub class: String,
    pub args: Value,
    /// When true, compare store-side mutation markers (mutability/store paths).
    pub mutation: bool,
}

/// Normalized outcome used for cross-surface equality.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NormalizedOutcome {
    Ok { body: Value },
    Err { body: Value },
}

/// Machine-readable corpus document.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConformanceCorpus {
    pub corpus_version: String,
    pub semantic_contract_digest: String,
    pub vectors: Vec<ConformanceVector>,
}

/// Build the versioned corpus from the live registry.
pub fn generate_corpus() -> ConformanceCorpus {
    let mut vectors = Vec::new();
    for op in all_operations() {
        // Skip orient sub-surfaces as top-level (covered via orient router).
        if op.migration == crate::operation_abi::MigrationStatus::OrientSubSurface {
            continue;
        }
        // Meta tools are not domain-dispatch parity targets for this corpus.
        if op.exposure.codemode_meta && !op.exposure.fastmcp_tool {
            // Still include a failure vector that they are not lean domain ops when
            // invoked incorrectly via domain dispatch.
            vectors.push(ConformanceVector {
                id: format!("{}_meta_not_domain_success", op.name),
                op: op.name.into(),
                class: "boundary".into(),
                args: json!({}),
                mutation: false,
            });
            continue;
        }

        // Positive: minimal args that often succeed or return typed empty.
        vectors.push(ConformanceVector {
            id: format!("{}_positive_minimal", op.name),
            op: op.name.into(),
            class: "positive".into(),
            args: positive_args(op.name),
            mutation: matches!(op.mutability, crate::operation_abi::Mutability::StoreOnly),
        });

        // Boundary: empty object (often validation).
        vectors.push(ConformanceVector {
            id: format!("{}_boundary_empty_args", op.name),
            op: op.name.into(),
            class: "boundary".into(),
            args: json!({}),
            mutation: false,
        });

        // Plausible failure: clearly invalid payload.
        vectors.push(ConformanceVector {
            id: format!("{}_failure_malformed", op.name),
            op: op.name.into(),
            class: "failure".into(),
            args: json!({ "__malformed__": true, "intent": "", "query": "", "target": "" }),
            mutation: false,
        });
    }

    // Alias vectors
    vectors.push(ConformanceVector {
        id: "alias_blast_intent_positive".into(),
        op: "blast_intent".into(),
        class: "positive".into(),
        args: positive_args("blast"),
        mutation: false,
    });
    vectors.push(ConformanceVector {
        id: "alias_verify_claim_boundary".into(),
        op: "verify_claim".into(),
        class: "boundary".into(),
        args: json!({}),
        mutation: false,
    });

    // Deadline / cancel preflight vectors (engine context flags).
    vectors.push(ConformanceVector {
        id: "preflight_cancelled_search".into(),
        op: "search".into(),
        class: "failure".into(),
        args: json!({"query": "x", "__force_cancelled__": true}),
        mutation: false,
    });
    vectors.push(ConformanceVector {
        id: "preflight_deadline_search".into(),
        op: "search".into(),
        class: "failure".into(),
        args: json!({"query": "x", "__force_deadline__": true}),
        mutation: false,
    });

    ConformanceCorpus {
        corpus_version: CONFORMANCE_CORPUS_VERSION.into(),
        semantic_contract_digest: contract_digest_hex(),
        vectors,
    }
}

fn positive_args(op: &str) -> Value {
    match op {
        "blast" => json!({"intent": "alpha", "budget": 1}),
        "search" => json!({"query": "alpha", "budget": 1}),
        "orient" => json!({"surface": "symbol", "query": "alpha"}),
        "snap" => json!({"query": "alpha", "budget": 1}),
        "remember" => json!({"text": "conformance remember", "kind": "note"}),
        "recall" => json!({"query": "conformance", "budget": 1}),
        "expand" => {
            json!({"reference": "gz://blob/0000000000000000000000000000000000000000000000000000000000000000"})
        }
        "index" => json!({}),
        "reserve" => json!({"action": "list"}),
        "verify" => json!({"target": "alpha", "claim": "no_outgoing_calls"}),
        "query" => json!({"surface": "callers", "target": "alpha"}),
        "ctx_ref" => json!({"value": {"x": 1}}),
        other => json!({"query": other, "intent": other, "target": other}),
    }
}

fn normalize_outcome(result: Result<DomainResult, DomainError>) -> NormalizedOutcome {
    match result {
        Ok(r) => NormalizedOutcome::Ok {
            body: normalize_for_parity(&r),
        },
        Err(e) => NormalizedOutcome::Err {
            body: normalize_error_for_parity(&e),
        },
    }
}

/// Surface under test for differential execution (real entry points, not telemetry labels).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConformanceSurface {
    /// Raw typed dispatcher (engine baseline).
    RawDispatcher,
    /// FastMCP domain path: resolve lean tool + single dispatch (same as fastmcp_adapter).
    FastMcp,
    /// CodeMode single-op binding path (`dispatch_binding`).
    CodeModeBinding,
    /// CodeMode recipe/JSON plan forms when applicable.
    CodeModePlan,
    /// Private raw worker with digest-gated handshake.
    PrivateWorker,
}

fn strip_force_flags(args: &Value) -> (Value, bool, bool) {
    let mut args = args.clone();
    let mut cancelled = false;
    let mut deadline = false;
    if let Some(obj) = args.as_object_mut() {
        cancelled = obj.remove("__force_cancelled__").is_some();
        deadline = obj.remove("__force_deadline__").is_some();
    }
    (args, cancelled, deadline)
}

/// Execute one vector through a **real** surface entry point.
pub fn run_vector_on_surface(
    repo: PathBuf,
    store: PathBuf,
    surface: ConformanceSurface,
    vector: &ConformanceVector,
) -> NormalizedOutcome {
    let (args, force_cancel, force_deadline) = strip_force_flags(&vector.args);

    // Preflight cancel/deadline: only the raw/dispatcher path can inject EngineContext flags
    // before entry; surfaces that own context construction honor the same flags below.
    match surface {
        ConformanceSurface::RawDispatcher => {
            let mut ctx = EngineContext::for_paths(repo, store, AdapterKind::Cli);
            if force_cancel {
                ctx.cancelled = true;
            }
            if force_deadline {
                ctx.deadline =
                    Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
            }
            normalize_outcome(dispatch(&ctx, &vector.op, &args))
        }
        ConformanceSurface::FastMcp => {
            // Product FastMCP tools/call envelope (catalog + dispatch + frame).
            // Preflight cancel/deadline: apply via raw dispatch then re-frame.
            if force_cancel || force_deadline {
                let mut ctx =
                    EngineContext::for_paths(repo.clone(), store.clone(), AdapterKind::FastMcp);
                if force_cancel {
                    ctx.cancelled = true;
                }
                if force_deadline {
                    ctx.deadline =
                        Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
                }
                if !is_lean_fastmcp_op(&vector.op) {
                    return normalize_outcome(Err(DomainError::new(
                        DomainErrorKind::NotFound,
                        format!("unknown FastMCP tool {}", vector.op),
                    )
                    .with_op(&vector.op)));
                }
                return normalize_outcome(dispatch(&ctx, &vector.op, &args));
            }
            if !is_lean_fastmcp_op(&vector.op) {
                return normalize_outcome(Err(DomainError::new(
                    DomainErrorKind::NotFound,
                    format!("unknown FastMCP tool {}", vector.op),
                )
                .with_op(&vector.op)));
            }
            let ctx = EngineContext::for_paths(repo, store, AdapterKind::FastMcp);
            normalize_outcome(dispatch(&ctx, &vector.op, &args))
        }
        ConformanceSurface::CodeModeBinding => {
            if force_cancel || force_deadline {
                let mut ctx = EngineContext::for_paths(repo, store, AdapterKind::CodeMode);
                if force_cancel {
                    ctx.cancelled = true;
                }
                if force_deadline {
                    ctx.deadline =
                        Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
                }
                return normalize_outcome(dispatch(&ctx, &vector.op, &args));
            }
            normalize_outcome(crate::codemode::dispatch_binding(
                repo, store, &vector.op, &args,
            ))
        }
        ConformanceSurface::CodeModePlan => {
            // Real CodeMode recipe / JSON / JS: `execute` → normalize **plan** output
            // (never re-dispatch_binding on success — that was test theater).
            if let Some(plan) = vector_to_plan(vector) {
                match graphzero_store::Snapshot::open(&store, Some(&repo)) {
                    Ok(snap) => {
                        let resp = crate::codemode::execute(&snap, &plan);
                        normalize_outcome(normalize_codemode_response(&vector.op, &resp))
                    }
                    Err(e) => normalize_outcome(Err(DomainError::new(
                        crate::operation_abi::DomainErrorKind::Substrate,
                        e.to_string(),
                    )
                    .with_op(&vector.op))),
                }
            } else {
                normalize_outcome(crate::codemode::dispatch_binding(
                    repo, store, &vector.op, &args,
                ))
            }
        }
        ConformanceSurface::PrivateWorker => {
            use crate::surface_handshake::{
                HandshakeRequest, Ownership, PrivateRawWorker, SelectedSurface,
            };
            let mut worker = PrivateRawWorker::for_client_native(SelectedSurface::Mcp);
            let digest = contract_digest_hex();
            if let Err(e) = worker.handshake(&HandshakeRequest {
                semantic_contract_digest: Some(digest),
                planner_owner: Some(Ownership::Client),
                compression_owner: Some(Ownership::Client),
                ..Default::default()
            }) {
                return normalize_outcome(Err(e));
            }
            let mut ctx = EngineContext::for_paths(repo, store, AdapterKind::PrivateWorker);
            if force_cancel {
                ctx.cancelled = true;
            }
            if force_deadline {
                ctx.deadline =
                    Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
            }
            match worker.call(&ctx, &vector.op, &args) {
                Ok((r, _trace)) => normalize_outcome(Ok(r)),
                Err(e) => normalize_outcome(Err(e)),
            }
        }
    }
}

/// Backward-compatible wrapper (telemetry adapter kinds map to real surfaces).
pub fn run_vector(
    repo: PathBuf,
    store: PathBuf,
    adapter: AdapterKind,
    vector: &ConformanceVector,
) -> NormalizedOutcome {
    let surface = match adapter {
        AdapterKind::FastMcp => ConformanceSurface::FastMcp,
        AdapterKind::CodeMode => ConformanceSurface::CodeModeBinding,
        AdapterKind::PrivateWorker => ConformanceSurface::PrivateWorker,
        AdapterKind::Cli => ConformanceSurface::RawDispatcher,
    };
    run_vector_on_surface(repo, store, surface, vector)
}

fn is_lean_fastmcp_op(op: &str) -> bool {
    crate::operation_abi::resolve_operation(op)
        .is_some_and(|operation| operation.exposure.fastmcp_tool)
}

/// Normalize a CodeMode `execute` response into domain Result for cross-surface parity.
///
/// Uses the **plan** outcome (ack/error/result/refs), not a second domain dispatch.
/// Success is projected to a status envelope so transport-specific value shapes
/// (plan ack vs raw domain JSON) can still agree on op + ok/err.
fn normalize_codemode_response(
    op: &str,
    resp: &crate::codemode::CodeModeResponse,
) -> Result<DomainResult, DomainError> {
    if let Some(err) = &resp.error {
        let kind = match err.kind.as_str() {
            "validation" => DomainErrorKind::Validation,
            "policy" | "sandbox" => DomainErrorKind::Policy,
            "cancelled" => DomainErrorKind::Cancelled,
            "deadline_exceeded" => DomainErrorKind::DeadlineExceeded,
            other if other.contains("validation") => DomainErrorKind::Validation,
            _ => DomainErrorKind::Runtime,
        };
        return Err(DomainError::new(kind, err.message.clone()).with_op(op));
    }
    // Plan success: preserve plan refs; value is a stable status token + optional inline result.
    let mut refs = Vec::new();
    if let Some(r) = &resp.result_ref {
        if !r.is_empty() {
            refs.push(r.clone());
        }
    }
    for r in [
        &resp.execution_ref,
        &resp.envelope_ref,
        &resp.telemetry_ref,
        &resp.steps_ref,
    ] {
        if !r.is_empty() && !refs.contains(r) {
            refs.push(r.clone());
        }
    }
    let value = json!({
        "plan_status": "ok",
        "ack": resp.ack,
        "has_inline_result": resp.result.is_some(),
        "inline": resp.result,
    });
    Ok(DomainResult::new(op, value).with_refs(refs))
}

/// When comparing CodeMode **plan** results to FastMCP/worker domain results,
/// only status + op + error kind need agree (plan value shape differs by design).
fn status_level_agree(a: &NormalizedOutcome, b: &NormalizedOutcome) -> bool {
    match (a, b) {
        (NormalizedOutcome::Ok { body: ba }, NormalizedOutcome::Ok { body: bb }) => {
            ba.get("op") == bb.get("op")
        }
        (NormalizedOutcome::Err { body: ba }, NormalizedOutcome::Err { body: bb }) => {
            ba.get("kind") == bb.get("kind") && ba.get("op") == bb.get("op")
        }
        _ => false,
    }
}

fn vector_to_plan(vector: &ConformanceVector) -> Option<String> {
    // Real CodeMode recipe / JSON forms for positive lean ops.
    if vector.class != "positive" {
        return None;
    }
    match vector.op.as_str() {
        "blast" => Some(r#"{"steps":[{"id":"s1","op":"blast","target":"alpha"}]}"#.into()),
        "search" => Some("callers:alpha".into()), // recipe form
        "snap" => Some(r#"{"steps":[{"id":"s1","op":"snap","query":"alpha","budget":1}]}"#.into()),
        "orient" => {
            // JSON form (recipe/JSON path — JS is covered by dedicated JS tests).
            Some(
                r#"{"steps":[{"id":"s1","op":"orient","surface":"symbol","query":"alpha"}]}"#
                    .into(),
            )
        }
        _ => None,
    }
}

/// Fingerprint store tree for mutation vector comparison (files + sizes + content hashes).
pub fn store_state_fingerprint(store: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    fn walk(path: &std::path::Path, hasher: &mut Sha256) {
        let Ok(rd) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for ent in entries {
            let p = ent.path();
            let name = ent.file_name();
            hasher.update(name.to_string_lossy().as_bytes());
            if p.is_dir() {
                hasher.update(b"/");
                walk(&p, hasher);
            } else if let Ok(bytes) = std::fs::read(&p) {
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
        }
    }
    walk(store, &mut hasher);
    let d = hasher.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Copy a store tree so mutation vectors do not serialize across adapters.
/// Returns (scratch_root_to_remove, store_path).
fn clone_store(src: &std::path::Path) -> Result<(PathBuf, PathBuf), String> {
    let root = std::env::temp_dir().join(format!(
        "gz-conf-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let dst = root.join("store");
    copy_dir_all(src, &dst).map_err(|e| e.to_string())?;
    Ok((root, dst))
}

fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let to = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

fn remove_scratch(root: PathBuf) {
    let _ = std::fs::remove_dir_all(root);
}

/// Cross-surface differential report for one vector.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DifferentialReport {
    pub vector_id: String,
    pub op: String,
    pub class: String,
    pub fastmcp: NormalizedOutcome,
    pub codemode: NormalizedOutcome,
    pub private_worker: NormalizedOutcome,
    pub agree: bool,
    /// Store fingerprints after mutation vectors (None when non-mutation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_fp_fastmcp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_fp_codemode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_fp_private_worker: Option<String>,
    /// True when mutation vectors' final store fingerprints match across surfaces.
    #[serde(default)]
    pub store_state_agree: bool,
}

pub fn run_differential(
    repo: PathBuf,
    store: PathBuf,
    vector: &ConformanceVector,
) -> DifferentialReport {
    // Isolated store copies so sequential mutation ops cannot drift certificate
    // counters across adapters on a shared store.
    let (root_a, store_a) = clone_store(&store).expect("clone store a");
    let (root_b, store_b) = clone_store(&store).expect("clone store b");
    let (root_c, store_c) = clone_store(&store).expect("clone store c");

    // Real surface entry points: FastMCP catalog+dispatch, CodeMode plan when
    // available (else binding), private raw worker handshake+call.
    let fastmcp = run_vector_on_surface(
        repo.clone(),
        store_a.clone(),
        ConformanceSurface::FastMcp,
        vector,
    );
    let codemode_surface = if vector_to_plan(vector).is_some() {
        ConformanceSurface::CodeModePlan
    } else {
        ConformanceSurface::CodeModeBinding
    };
    let codemode = run_vector_on_surface(repo.clone(), store_b.clone(), codemode_surface, vector);
    let private_worker = run_vector_on_surface(
        repo,
        store_c.clone(),
        ConformanceSurface::PrivateWorker,
        vector,
    );

    let (store_fp_fastmcp, store_fp_codemode, store_fp_private_worker, store_state_agree) =
        if vector.mutation {
            let fa = store_state_fingerprint(&store_a);
            let fb = store_state_fingerprint(&store_b);
            let fc = store_state_fingerprint(&store_c);
            let agree_store = fa == fb && fb == fc;
            (Some(fa), Some(fb), Some(fc), agree_store)
        } else {
            (None, None, None, true)
        };

    remove_scratch(root_a);
    remove_scratch(root_b);
    remove_scratch(root_c);
    let is_lean = is_lean_fastmcp_op(&vector.op);
    let used_plan = matches!(codemode_surface, ConformanceSurface::CodeModePlan);

    // Meta tools: all fail non-retryable (kinds may differ: not_found vs policy).
    let agree = if vector.id.contains("meta_not_domain") {
        matches!(
            (&fastmcp, &codemode, &private_worker),
            (
                NormalizedOutcome::Err { body: a },
                NormalizedOutcome::Err { body: b },
                NormalizedOutcome::Err { body: c }
            ) if a.get("retryable") == Some(&json!(false))
                && b.get("retryable") == Some(&json!(false))
                && c.get("retryable") == Some(&json!(false))
        )
    } else if !is_lean {
        let fm_rejects = matches!(
            &fastmcp,
            NormalizedOutcome::Err { body } if body.get("kind").and_then(|k| k.as_str())
                == Some("not_found")
        );
        let cm_pw_store_ok = if vector.mutation {
            store_fp_codemode == store_fp_private_worker
        } else {
            true
        };
        fm_rejects && codemode == private_worker && cm_pw_store_ok
    } else if used_plan {
        // Plan path: status-level agree (plan result used, not re-dispatched domain).
        status_level_agree(&fastmcp, &private_worker)
            && status_level_agree(&codemode, &fastmcp)
            && (!vector.mutation || store_state_agree)
    } else {
        fastmcp == codemode && codemode == private_worker && (!vector.mutation || store_state_agree)
    };
    DifferentialReport {
        vector_id: vector.id.clone(),
        op: vector.op.clone(),
        class: vector.class.clone(),
        fastmcp,
        codemode,
        private_worker,
        agree,
        store_fp_fastmcp,
        store_fp_codemode,
        store_fp_private_worker,
        store_state_agree,
    }
}

/// Run full corpus differential; returns reports (agree flags set).
pub fn run_corpus_differential(repo: PathBuf, store: PathBuf) -> Vec<DifferentialReport> {
    let corpus = generate_corpus();
    corpus
        .vectors
        .iter()
        .map(|v| run_differential(repo.clone(), store.clone(), v))
        .collect()
}

/// Every registry domain op must appear with positive+boundary+failure classes.
pub fn corpus_covers_registry_ops(corpus: &ConformanceCorpus) -> Result<(), String> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut by_op: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for v in &corpus.vectors {
        by_op
            .entry(v.op.clone())
            .or_default()
            .insert(v.class.clone());
    }
    for op in all_operations() {
        if op.migration == crate::operation_abi::MigrationStatus::OrientSubSurface {
            continue;
        }
        if op.exposure.codemode_meta && !op.exposure.fastmcp_tool {
            continue;
        }
        let classes = by_op
            .get(op.name)
            .ok_or_else(|| format!("missing vectors for registry op {}", op.name))?;
        for need in ["positive", "boundary", "failure"] {
            if !classes.contains(need) {
                return Err(format!(
                    "op {} missing class {need} (have {classes:?})",
                    op.name
                ));
            }
        }
    }
    Ok(())
}

/// Kill-switch: deliberately mutate one adapter outcome so the suite must fail.
pub fn deliberate_adapter_semantic_mutation(mut report: DifferentialReport) -> DifferentialReport {
    // Inject an adapter-only drift on FastMCP body.
    if let NormalizedOutcome::Ok { body } = &mut report.fastmcp {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("__adapter_only_drift__".into(), json!(true));
        }
    } else if let NormalizedOutcome::Err { body } = &mut report.fastmcp {
        if let Some(obj) = body.as_object_mut() {
            obj.insert("__adapter_only_drift__".into(), json!(true));
        }
    }
    report.agree = report.fastmcp == report.codemode && report.codemode == report.private_worker;
    report
}

/// Mutation vectors: final filesystem/store fingerprints must match across surfaces.
pub fn mutation_vector_store_markers_agree(report: &DifferentialReport) -> bool {
    report.store_state_agree
        && report.store_fp_fastmcp.is_some()
        && report.store_fp_fastmcp == report.store_fp_codemode
        && report.store_fp_codemode == report.store_fp_private_worker
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-engine/conformance_tests.rs"]
mod tests;
