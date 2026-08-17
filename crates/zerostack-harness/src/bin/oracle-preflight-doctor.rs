use std::process::ExitCode;

use zerostack_harness::oracle_preflight_doctor::{discover_root, run};

fn main() -> ExitCode {
    let json = std::env::args().any(|arg| arg == "--json");
    let root = discover_root();
    let report = run(&root);
    if json {
        println!("{}", report.to_json());
    } else {
        println!(
            "oracle-preflight-doctor aggregate={} certifying={} verifiers={}",
            report.aggregate_outcome, report.certifying, report.verifier_count
        );
        for check in &report.checks {
            println!("  [{}] {} -- {}", check.outcome, check.name, check.detail);
        }
        if let Some(diag) = &report.first_failure_diagnosis {
            println!("first_failure_diagnosis: {diag}");
        }
    }
    if report.aggregate_outcome == "red" {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
