//! Production Program evidence assembler CLI.
//!
//! Reads a canonical evidence manifest naming the exact explicit source head,
//! the current hub head, the assembly manifest digest, and one evidence
//! artifact file per engine (fz/gz/tz) per evidence class (planner, codemode
//! raw-worker, MCP, lifecycle, applied-GC). Every artifact is validated
//! against its contract, digest, and provenance; the five separated reports
//! are assembled per engine, and the verified `AggregateProgramReceipt` is
//! written as canonical JSON. Missing, partial, stale, or digest-mismatched
//! evidence fails the run closed — there is no fixture fallback and no
//! synthesized success digest.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use zero_gate::{ProgramEvidenceManifest, assemble_program_evidence};

fn usage() -> String {
    "usage: zerostack-program-evidence --manifest <manifest.json> --out <receipt.json>".into()
}

fn parse_args() -> Result<(PathBuf, PathBuf), String> {
    let mut manifest = None;
    let mut out = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                manifest = Some(PathBuf::from(
                    args.next().ok_or("--manifest requires a path")?,
                ));
            }
            "--out" => {
                out = Some(PathBuf::from(args.next().ok_or("--out requires a path")?));
            }
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown argument {other:?}\n{}", usage())),
        }
    }
    let manifest = manifest.ok_or_else(|| format!("missing --manifest\n{}", usage()))?;
    let out = out.ok_or_else(|| format!("missing --out\n{}", usage()))?;
    Ok((manifest, out))
}

fn load(path: &Path) -> Result<Vec<u8>, zero_gate::ProgramEvidenceError> {
    std::fs::read(path).map_err(|error| {
        zero_gate::ProgramEvidenceError::io(format!("reading {}: {error}", path.display()))
    })
}

fn main() -> ExitCode {
    let (manifest_path, out_path) = match parse_args() {
        Ok(pair) => pair,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("cannot read manifest {}: {error}", manifest_path.display());
            return ExitCode::from(1);
        }
    };
    let manifest = match ProgramEvidenceManifest::from_canonical_bytes(&bytes) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    let receipt = match assemble_program_evidence(&manifest, load) {
        Ok(receipt) => receipt,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    let canonical = match receipt.canonical_bytes() {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(1);
        }
    };
    if let Some(parent) = out_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!("cannot create {}: {error}", parent.display());
            return ExitCode::from(1);
        }
    }
    let mut output = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&out_path)
    {
        Ok(output) => output,
        Err(error) => {
            eprintln!(
                "cannot create immutable receipt {}: {error}",
                out_path.display()
            );
            return ExitCode::from(1);
        }
    };
    if let Err(error) = output
        .write_all(&canonical)
        .and_then(|()| output.sync_all())
    {
        eprintln!("cannot write {}: {error}", out_path.display());
        return ExitCode::from(1);
    }
    println!("wrote {}", out_path.display());
    ExitCode::SUCCESS
}
