//! Legacy `gz-raw-worker` compatibility entrypoint.
//!
//! The canonical artifact is `graphzero-codemode` from package
//! `graphzero-worker`. Both entrypoints call one shared planner-free adapter.

use std::io::{self, Write};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h" | "help" | "status"))
    {
        let banner = concat!(
            "gz-raw-worker — legacy GraphZero raw-worker alias\n",
            "canonical artifact: graphzero-codemode (package graphzero-worker)\n",
            "Protocol: line-delimited JSON on stdin/stdout (zerostack.surface / zerostack.raw_worker).\n",
            "No interactive REPL: feed handshake/call frames on stdin.\n",
            "Env: GRAPHZERO_REPO, GRAPHZERO_STORE, GRAPHZERO_SURFACE, ZEROSTACK_SESSION_ID, ZEROSTACK_WORKER_REVISION.\n",
        );
        let _ = writeln!(io::stderr(), "{banner}");
        std::process::exit(2);
    }
    if !args.is_empty() {
        eprintln!("gz-raw-worker: unsupported argument");
        std::process::exit(2);
    }
    std::process::exit(graphzero_engine::raw_worker_stdio::run_stdio(None));
}
