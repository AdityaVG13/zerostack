//! Live proof matrix: GraphZero vs ripgrep on identical tasks — no recorded estimates.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use graphzero_engine::blast::{blast_radius, blast_to_json_budget};
use graphzero_engine::query_surface::{QuerySurfaceRequest, QuerySurfaceRouter};
use graphzero_store::Snapshot;
use graphzero_store::store::expand::ExpandResolver;
use graphzero_store::store::refs::GzRef;
use serde_json::json;

use crate::gates::token_accounting::TokenCounts;
use crate::gates::token_by_task::{ScaledRepo, index_scaled_repo};

pub const PROOF_SCALE_SMALL: usize = 50;
pub const PROOF_SCALE_LARGE: usize = 500;

pub const MAX_COMPACT_SHELL_CLAUDE: usize = 15;
pub const MAX_COMPACT_SHELL_O200K: usize = 16;

#[derive(Clone, Debug)]
pub struct ProofTask {
    pub id: &'static str,
    pub surface: &'static str,
    pub query: &'static str,
    pub path: Option<&'static str>,
    pub blast: bool,
    pub ripgrep_args: Option<&'static [&'static str]>,
}

pub const PROOF_TASKS: &[ProofTask] = &[
    ProofTask {
        id: "find_path",
        surface: "locate",
        query: "",
        path: None,
        blast: false,
        ripgrep_args: Some(&["--files", "-g"]),
    },
    ProofTask {
        id: "find_symbol",
        surface: "locate",
        query: "",
        path: None,
        blast: false,
        ripgrep_args: Some(&["-l"]),
    },
    ProofTask {
        id: "callers",
        surface: "callers",
        query: "",
        path: None,
        blast: false,
        ripgrep_args: Some(&["-l"]),
    },
    ProofTask {
        id: "impact",
        surface: "blast",
        query: "",
        path: None,
        blast: true,
        ripgrep_args: None,
    },
    ProofTask {
        id: "search_needle",
        surface: "search",
        query: "",
        path: None,
        blast: false,
        ripgrep_args: Some(&[]),
    },
];

pub struct ProofFixture {
    pub fx: ScaledRepo,
    pub path_query: String,
    pub symbol: String,
    pub blast_intent: String,
}

pub fn proof_fixture(file_count: usize) -> ProofFixture {
    let fx = index_scaled_repo(file_count);
    let mid = file_count / 2;
    let symbol = format!("sym_{mid}");
    let path_query = format!("src/m_{mid:04}.rs");
    let blast_intent = format!("change signature of {symbol}");
    ProofFixture {
        fx,
        path_query,
        symbol,
        blast_intent,
    }
}

fn task_query<'a>(task: &ProofTask, fixture: &'a ProofFixture) -> (&'a str, Option<&'a str>) {
    match task.id {
        "find_path" => (
            fixture.path_query.as_str(),
            Some(fixture.path_query.as_str()),
        ),
        "impact" => (fixture.blast_intent.as_str(), None),
        _ => (fixture.symbol.as_str(), None),
    }
}

pub fn run_graphzero(task: &ProofTask, fixture: &ProofFixture) -> (String, f64) {
    let snapshot =
        Snapshot::open(&fixture.fx.store_root, Some(&fixture.fx.repo_root)).expect("open");
    let (query, path) = task_query(task, fixture);
    let start = Instant::now();

    let body = if task.blast {
        let capsule = blast_radius(&snapshot, query, 1).expect("blast");
        blast_to_json_budget(&capsule, 1, Some(&fixture.fx.store_root))
            .expect("serialize blast capsule")
    } else {
        let mut req = QuerySurfaceRequest {
            surface: task.surface.into(),
            query: Some(query.into()),
            budget: Some(1),
            ..Default::default()
        };
        if let Some(p) = path {
            req.path = Some(p.into());
        }
        if matches!(task.id, "find_symbol" | "callers" | "search_needle") {
            req.name = Some(query.into());
        }
        let resp = QuerySurfaceRouter::execute(&snapshot, &req).expect(task.id);
        QuerySurfaceRouter::to_json_string_with_budget(&resp, 1, Some(&fixture.fx.store_root))
    };

    (body, start.elapsed().as_secs_f64() * 1000.0)
}

/// Typed ripgrep outcome so callers can distinguish a missing `rg` binary
/// from a normal "no matches" (exit 1) or a hard execution failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RipgrepError {
    /// `rg` executable could not be spawned (usually `NotFound`).
    MissingBinary(String),
    /// Spawn succeeded but `rg` reported a non-success exit that is not the
    /// conventional "no matches" (`exit 1`) case.
    NonZero { code: Option<i32>, stderr: String },
    /// I/O error other than “not found”.
    Io(String),
}

impl std::fmt::Display for RipgrepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingBinary(msg) => write!(f, "rg missing: {msg}"),
            Self::NonZero { code, stderr } => {
                write!(f, "rg failed with code {code:?}: {stderr}")
            }
            Self::Io(msg) => write!(f, "rg I/O error: {msg}"),
        }
    }
}

impl std::error::Error for RipgrepError {}

/// Pure helper: map a spawn `io::Error` to a typed `RipgrepError` without
/// touching PATH or spawning. `NotFound` -> `MissingBinary`, else `Io`.
fn classify_spawn_error(err: std::io::Error) -> RipgrepError {
    if err.kind() == std::io::ErrorKind::NotFound {
        RipgrepError::MissingBinary(err.to_string())
    } else {
        RipgrepError::Io(err.to_string())
    }
}

/// Pure helper: classify a finished `rg` exit.
/// `success` mirrors `ExitStatus::success()` (exit 0).
/// Contract: exit 0 => success with stdout, exit 1 => no matches (empty
/// stdout => Ok(None)), exit >=2 or missing code => NonZero. Unlike the
/// prior workaround, exit 1 with non-empty stdout is still NonZero because
/// rg exit 1 means no matches per rg contract.
fn classify_rg_exit(
    success: bool,
    code: Option<i32>,
    stdout_is_empty: bool,
    stderr: String,
) -> Result<Option<()>, RipgrepError> {
    if success {
        return Ok(Some(()));
    }
    if code == Some(1) && stdout_is_empty {
        return Ok(None);
    }
    Err(RipgrepError::NonZero { code, stderr })
}

/// Injectable `rg` executable path. `rg_exe` is the binary to invoke (e.g.
/// `"rg"` in production, `"false"` in tests). Returns:
/// - `Ok(Some((stdout, ms)))` on success (exit 0),
/// - `Ok(None)` on the conventional ripgrep "no matches" (`exit 1` with
///   empty stdout) or when `task.ripgrep_args` is `None` (not-applicable),
/// - `Err(MissingBinary)` when the executable cannot be found,
/// - `Err(NonZero)` / `Err(Io)` for hard failures (exit >=2).
fn run_ripgrep_with_executable(
    task: &ProofTask,
    fixture: &ProofFixture,
    rg_exe: &str,
) -> Result<Option<(String, f64)>, RipgrepError> {
    let Some(args) = task.ripgrep_args else {
        return Ok(None);
    };
    let (query, _) = task_query(task, fixture);
    let start = Instant::now();

    let output_res = match task.id {
        "find_path" => {
            let file = fixture
                .path_query
                .strip_prefix("src/")
                .unwrap_or(&fixture.path_query);
            Command::new(rg_exe)
                .args(args)
                .arg(format!("**/{file}"))
                .current_dir(&fixture.fx.repo_root)
                .output()
        }
        "callers" => Command::new(rg_exe)
            .args(args)
            .arg(format!("{query}\\("))
            .current_dir(&fixture.fx.repo_root)
            .output(),
        "search_needle" => {
            let mut cmd = Command::new(rg_exe);
            cmd.arg(query).current_dir(&fixture.fx.repo_root);
            cmd.output()
        }
        _ => Command::new(rg_exe)
            .args(args)
            .arg(query)
            .current_dir(&fixture.fx.repo_root)
            .output(),
    };

    let output = match output_res {
        Ok(o) => o,
        Err(e) => return Err(classify_spawn_error(e)),
    };

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    match classify_rg_exit(
        output.status.success(),
        output.status.code(),
        output.stdout.is_empty(),
        stderr,
    )? {
        Some(()) => Ok(Some((
            String::from_utf8_lossy(&output.stdout).into_owned(),
            elapsed,
        ))),
        None => Ok(None),
    }
}

/// Typed entry point: distinguishes missing `rg` / hard failures from
/// normal "no matches". Tasks with `ripgrep_args == None` return
/// `Ok(None)` (not-applicable, not an error).
pub fn run_ripgrep(
    task: &ProofTask,
    fixture: &ProofFixture,
) -> Result<Option<(String, f64)>, RipgrepError> {
    run_ripgrep_with_executable(task, fixture, "rg")
}

pub fn assert_compact_shell(body: &str, task_id: &str) {
    assert!(
        body.starts_with("g:") || body.starts_with("q:"),
        "{task_id}: budget=1 must return compact g:/q: shell, got: {body}"
    );
    let shell = TokenCounts::for_text(body);
    assert!(
        shell.claude <= MAX_COMPACT_SHELL_CLAUDE,
        "{task_id}: claude shell {} > {MAX_COMPACT_SHELL_CLAUDE}: {body}",
        shell.claude
    );
    assert!(
        shell.o200k <= MAX_COMPACT_SHELL_O200K,
        "{task_id}: o200k shell {} > {MAX_COMPACT_SHELL_O200K}: {body}",
        shell.o200k
    );
}

pub fn assert_lossless_expand(store: &Path, repo: &Path, compact: &str, task_id: &str) {
    let resolver = ExpandResolver::new(store, Some(repo)).expect("resolver");
    let gz = GzRef::parse(compact).expect("parse compact ref");
    let expanded = resolver.resolve(&gz, compact).expect("expand compact");
    assert!(
        !expanded.bytes.is_empty(),
        "{task_id}: expand({compact}) must return full context"
    );

    let canonical = match &gz {
        GzRef::Loc { id } => {
            let snapshot = Snapshot::open(store, Some(repo)).expect("open");
            graphzero_store::store::query::canonical_ref_for_loc(&snapshot, *id)
                .expect("canonical loc")
        }
        GzRef::Query { id } => format!("gz://query/{id}"),
        other => panic!("{task_id}: unexpected compact ref {other:?}"),
    };
    let gz_canon = GzRef::parse(&canonical).expect("canonical ref");
    let via_canonical = resolver
        .resolve(&gz_canon, &canonical)
        .expect("expand canonical");

    if matches!(gz, GzRef::Loc { .. }) {
        assert_eq!(
            expanded.bytes, via_canonical.bytes,
            "{task_id}: g: expand must match gz:// blob bytes"
        );
    } else {
        assert_eq!(
            expanded.bytes, via_canonical.bytes,
            "{task_id}: q: expand must match gz://query bytes"
        );
    }
}

pub fn build_proof_report(file_count: usize) -> serde_json::Value {
    let fixture = proof_fixture(file_count);
    let mut rows = Vec::new();

    for task in PROOF_TASKS {
        let (gz_body, gz_ms) = run_graphzero(task, &fixture);
        let gz_shell = TokenCounts::for_text(&gz_body);
        let rg_res = run_ripgrep(task, &fixture);
        let ripgrep = match rg_res {
            Ok(opt) => opt.map(|(body, ms)| {
                let shell = TokenCounts::for_text(&body);
                json!({
                    "body_bytes": body.len(),
                    "shell_claude": shell.claude,
                    "shell_o200k": shell.o200k,
                    "latency_ms": ms,
                })
            }),
            Err(e) => panic!(
                "proof report: ripgrep task '{}' failed: {e}; missing rg or broken rg must fail loudly, not collapse to null",
                task.id
            ),
        };

        rows.push(json!({
            "task_id": task.id,
            "graphzero": {
                "body": gz_body,
                "body_bytes": gz_body.len(),
                "shell_claude": gz_shell.claude,
                "shell_o200k": gz_shell.o200k,
                "latency_ms": gz_ms,
            },
            "ripgrep": ripgrep,
        }));
    }

    json!({
        "schema_version": 1,
        "methodology": "Live GraphZero and ripgrep on identical chain-call fixture. Token counts use real tokenizers (claude via ah-ah-ah, o200k via tiktoken). No recorded competitor estimates.",
        "file_count": file_count,
        "target_symbol": fixture.symbol,
        "tasks": rows,
    })
}

pub fn assert_beats_ripgrep_at_scale(report: &serde_json::Value) {
    let file_count = report["file_count"].as_u64().unwrap_or(0) as usize;
    if file_count < PROOF_SCALE_LARGE {
        return;
    }
    for row in report["tasks"].as_array().expect("tasks") {
        let task_id = row["task_id"]
            .as_str()
            .expect("proof report task_id must be a string");
        let Some(rg) = row.get("ripgrep").filter(|v| v.is_object()) else {
            continue;
        };
        let gz_claude = row["graphzero"]["shell_claude"]
            .as_u64()
            .expect("GraphZero shell_claude count must be an unsigned integer")
            as usize;
        let rg_claude = rg["shell_claude"]
            .as_u64()
            .expect("ripgrep shell_claude count must be an unsigned integer")
            as usize;
        assert!(
            gz_claude < rg_claude,
            "{task_id} at {file_count} files: graphzero claude {gz_claude} must beat ripgrep {rg_claude}"
        );
    }
}
