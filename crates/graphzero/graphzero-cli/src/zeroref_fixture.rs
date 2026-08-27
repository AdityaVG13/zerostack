//! ZeroRef v1 conformance fixture surface for the three-binary matrix
//! (docs/contracts/zeroref-fixture-cli.md, bead 1ghi.7).
//!
//! Deterministic and non-interactive: `put` prints one machine-readable JSON
//! document on stdout; `expand` writes exact object bytes to stdout (or
//! `--out`) and keeps machine-readable diagnostics strictly on stderr.
//! Failures exit with the stable per-class codes in [`exit_code_for_class`].
//! Diagnostics never include blob contents or raw filesystem paths — roots
//! are reported as content-hashed identities.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use graphzero_store::store::zeroref::{ZeroRef, ZeroRefError};
use graphzero_store::{ContentHash, ExternalResolveError, SharedCas, ZeroRefDescriptor};
use serde_json::json;

use crate::cli_args::ZerorefFixtureAction;

pub(crate) const FIXTURE_SCHEMA: &str = "zeroref-fixture/v1";

/// Stable exit codes per ZeroRef v1 error class. 0 = success, 1 = other.
pub(crate) fn exit_code_for_class(class: &str) -> i32 {
    match class {
        "malformed" => 2,
        "unsupported" => 3,
        "range_out_of_bounds" => 4,
        "not_utf8" => 5,
        "missing" | "not_found" => 6,
        "io" => 7,
        "digest_mismatch" => 8,
        "policy_denied" => 9,
        "incompatible_version" => 10,
        "legacy_ambiguity" => 11,
        _ => 1,
    }
}

/// Non-secret identity for a CAS root: hash of the canonicalized path, so
/// the matrix can prove two engines used the same root without leaking it.
fn root_identity(root: &Path) -> String {
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    ContentHash::of(canonical.as_bytes()).to_hex()[..16].to_string()
}

fn binary_identity() -> serde_json::Value {
    json!({
        "engine": "graphzero",
        "version": env!("CARGO_PKG_VERSION"),
        "commit": option_env!("GRAPHZERO_COMMIT"),
    })
}

fn fail(class: &str, message: &str, reference: Option<&str>) -> ! {
    let diag = json!({
        "schema": FIXTURE_SCHEMA,
        "ok": false,
        "binary": binary_identity(),
        "error_class": class,
        "exit_code": exit_code_for_class(class),
        "message": message,
        "ref": reference,
        "os": std::env::consts::OS,
    });
    eprintln!("{diag}");
    std::process::exit(exit_code_for_class(class));
}

fn fail_external(err: &ExternalResolveError, reference: Option<&str>) -> ! {
    fail(err.class(), &err.detail(), reference)
}

fn fail_zeroref(err: &ZeroRefError, reference: Option<&str>) -> ! {
    fail(err.class.as_str(), &err.message, reference)
}

fn cas_for(store_root: &Path, shared_root: Option<&Path>) -> SharedCas {
    match shared_root {
        Some(root) => SharedCas::open_labeled(root, "cas-shared"),
        None => SharedCas::open_labeled(store_root, "cas-local"),
    }
}

pub(crate) fn run(action: ZerorefFixtureAction) -> Result<()> {
    match action {
        ZerorefFixtureAction::Descriptor => {
            println!(
                "{}",
                serde_json::to_string_pretty(&ZeroRefDescriptor::from_env().to_json())?
            );
            Ok(())
        }
        ZerorefFixtureAction::Put {
            store_root,
            shared_root,
            input,
            max_object_bytes,
        } => run_put(&store_root, shared_root.as_deref(), input, max_object_bytes),
        ZerorefFixtureAction::Expand {
            store_root,
            shared_root,
            reference,
            out,
        } => run_expand(&store_root, shared_root.as_deref(), &reference, out),
    }
}

fn run_put(
    store_root: &Path,
    shared_root: Option<&Path>,
    input: Option<PathBuf>,
    max_object_bytes: Option<u64>,
) -> Result<()> {
    let bytes = match input {
        Some(path) => std::fs::read(&path)
            .with_context(|| format!("read fixture input {}", path.display()))?,
        None => {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .context("read fixture bytes from stdin")?;
            buf
        }
    };
    let cas = cas_for(store_root, shared_root);
    let put = match max_object_bytes {
        Some(limit) => cas.put_limited(&bytes, limit),
        None => cas.put(&bytes),
    };
    let hash = match put {
        Ok(hash) => hash,
        Err(err) => fail_external(&err, None),
    };
    let reference = format!("gz://blob/{hash}");
    let size = bytes.len() as u64;
    let line_example = (std::str::from_utf8(&bytes).is_ok() && !bytes.is_empty())
        .then(|| format!("{reference}#L1-1"));
    let doc = json!({
        "schema": FIXTURE_SCHEMA,
        "ok": true,
        "binary": binary_identity(),
        "capability": ZeroRefDescriptor::from_env().to_json(),
        "ref": reference,
        "hash": hash,
        "size": size,
        "shared_root_identity": root_identity(cas.root()),
        "fragments": {
            "whole": reference,
            "empty_bytes": format!("{reference}#B0-0"),
            "all_bytes": format!("{reference}#B0-{size}"),
            "first_line": line_example,
        },
        "os": std::env::consts::OS,
    });
    println!("{}", serde_json::to_string_pretty(&doc)?);
    Ok(())
}

fn run_expand(
    store_root: &Path,
    shared_root: Option<&Path>,
    reference: &str,
    out: Option<PathBuf>,
) -> Result<()> {
    let parsed = match ZeroRef::parse(reference) {
        Ok(parsed) => parsed,
        Err(err) => fail_zeroref(&err, Some(reference)),
    };
    let cas = cas_for(store_root, shared_root);
    let object = match cas.get_verified(&parsed.hash) {
        Ok(bytes) => bytes,
        Err(ExternalResolveError::NotFound) => fail(
            "missing",
            "object not present in the selected root",
            Some(reference),
        ),
        Err(err) => fail_external(&err, Some(reference)),
    };
    let selected = match parsed.select(&object) {
        Ok(selected) => selected,
        Err(err) => fail_zeroref(&err, Some(reference)),
    };
    match out {
        Some(path) => std::fs::write(&path, selected)
            .with_context(|| format!("write fixture bytes to {}", path.display()))?,
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(selected).context("write fixture bytes")?;
            stdout.flush().context("flush fixture bytes")?;
        }
    }
    let diag = json!({
        "schema": FIXTURE_SCHEMA,
        "ok": true,
        "binary": binary_identity(),
        "ref": reference,
        "hash": parsed.hash,
        "object_size": object.len(),
        "selected_size": selected.len(),
        "shared_root_identity": root_identity(cas.root()),
        "os": std::env::consts::OS,
    });
    eprintln!("{diag}");
    Ok(())
}
