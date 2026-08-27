//! `graphzero` CLI: index, snap, expand, daemon, serve (MCP stdio), stats.
//! Mutually exclusive package surfaces: see `docs/install.md` (graphzero-o2uq.3).

use std::process::ExitCode;

use graphzero::cli_args::agent_json_mode;
use graphzero::dispatch::run;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let msg = e.to_string();
            if agent_json_mode() {
                eprintln!(
                    "{}",
                    graphzero::agent_errors::enrich_cli_error_message(&msg, ".")
                );
            } else {
                eprintln!("{e:#}");
                eprintln!("hint: graphzero agent-triage or graphzero --help");
            }
            ExitCode::FAILURE
        }
    }
}
