//! Subprocess worker for surface_bench (graphzero-o2uq.8).
//! Args: <repo> <store> <n> <surface>
//! Surfaces: cli_raw | fastmcp | codemode_fused | codemode_plan | private_worker

use graphzero_engine::surface_bench::{BenchSurface, run_n_ops_surface};
use serde_json::json;
use std::env;
use std::path::PathBuf;

fn main() {
    let mut args = env::args().skip(1);
    let repo = PathBuf::from(args.next().expect("repo"));
    let store = PathBuf::from(args.next().expect("store"));
    let n: usize = args.next().expect("n").parse().expect("n");
    let surface = match args.next().as_deref() {
        Some("cli_raw") | Some("cli") => BenchSurface::CliRaw,
        Some("fastmcp") => BenchSurface::FastMcp,
        Some("codemode_fused") | Some("codemode") => BenchSurface::CodeModeFused,
        Some("codemode_plan") => BenchSurface::CodeModePlan,
        Some("private_worker") => BenchSurface::PrivateWorker,
        other => panic!("unknown surface {other:?}"),
    };
    let trial = run_n_ops_surface(repo, store, surface, n, true);
    println!(
        "{}",
        json!({
            "wall_ns": trial.wall_ns,
            "dispatcher_wall_ns_sum": trial.dispatcher_wall_ns_sum,
            "serialize_ns": trial.serialize_ns,
            "handshake_ns": trial.handshake_ns,
            "op_count": trial.op_count,
            "ok": true,
        })
    );
}
