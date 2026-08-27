//! ZeroRef v1 conformance fixture surface for the three-binary matrix
//! (bead fszero-c6q.6; counterparts: tokenzero-itl, graphzero-zeroref-v1-shared-cas-1ghi.7).
//!
//! Deterministic and non-interactive: "put" emits one machine-readable JSON document
//! on stdout; "expand" writes exact object bytes to stdout (or "--out") and keeps
//! machine-readable diagnostics strictly on stderr. Failures exit with the stable
//! per-class codes in [exit_code_for_class].
//!
//! Anti-cheating: this surface uses the production FSZero recovery store and CAS;
//! it never retags a scheme, mocks a sibling store, or injects bytes directly into
//! private stores.

use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::cas::CasStore;
use super::session::FSZeroSession;
use super::zeroref::{EMITTED_SCHEME, LineEndPolicy, ZeroRef, ZeroRefError};

pub const FIXTURE_SCHEMA: &str = "zeroref-fixture/v1";

/// Stable exit codes per ZeroRef v1 error class. 0 = success, 1 = other.
pub fn exit_code_for_class(class: &str) -> i32 {
    match class {
        "malformed" => 2,
        "unsupported" => 3,
        "range_out_of_bounds" => 4,
        "not_utf8" => 5,
        "missing" => 6,
        "io" => 7,
        "digest_mismatch" => 8,
        "policy_denied" => 9,
        "incompatible_version" => 10,
        "legacy_ambiguity" => 11,
        _ => 1,
    }
}

#[derive(Debug)]
pub struct FixtureError {
    pub class: &'static str,
    pub message: String,
    pub reference: Option<String>,
}

impl fmt::Display for FixtureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for FixtureError {}

fn fail(
    class: &'static str,
    message: impl Into<String>,
    reference: Option<String>,
) -> FixtureError {
    FixtureError {
        class,
        message: message.into(),
        reference,
    }
}

fn class_from_zeroref(err: &ZeroRefError, reference: &str) -> FixtureError {
    fail(
        err.class.as_str(),
        err.to_string(),
        Some(reference.to_string()),
    )
}

pub fn binary_identity() -> serde_json::Value {
    json!({ "engine": "fszero", "version": env!("CARGO_PKG_VERSION"), "commit": option_env!("FSZERO_COMMIT"), })
}

/// Non-secret identity for a CAS root: hash of the canonicalized path, so the
/// matrix can prove two engines used the same root without leaking it.
fn root_identity(root: &Path) -> String {
    let canonical = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned();
    crate::hexutil::sha256_hex_of(Sha256::digest(canonical.as_bytes()).into())[..16].to_string()
}

/// Open a session rooted at `store_root` and, when requested, attach the shared
/// CAS at `shared_root`. Returns the effective CAS root used for `shared_root_identity`.
fn open_session(store_root: &Path, shared_root: Option<&Path>) -> (FSZeroSession, Option<PathBuf>) {
    // Use a private durable store under the given store root so the fixture is
    // deterministic and isolated from any global ZEROSTACK_STORE_ROOT setting.
    let fszero_dir = store_root.join(".fszero");
    let _ = std::fs::create_dir_all(&fszero_dir);
    let db_path = fszero_dir.join("store.sqlite3");
    let mut sess = FSZeroSession::with_durable_root(store_root, db_path);
    sess.recovery.disable_ref_index();
    let effective = if let Some(shared) = shared_root {
        let blobs = shared.join("blobs");
        let _ = std::fs::create_dir_all(&blobs);
        sess.recovery.attach_cas(CasStore::for_store_root(shared));
        Some(blobs)
    } else {
        let blobs = store_root.join("blobs");
        let _ = std::fs::create_dir_all(&blobs);
        sess.recovery
            .attach_cas(CasStore::for_store_root(store_root));
        Some(blobs)
    };
    (sess, effective)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZerorefFixtureAction {
    Descriptor {
        store_root: PathBuf,
    },
    Put {
        store_root: PathBuf,
        shared_root: Option<PathBuf>,
        input: Option<PathBuf>,
        max_object_bytes: Option<u64>,
    },
    Expand {
        store_root: PathBuf,
        shared_root: Option<PathBuf>,
        reference: String,
        out: Option<PathBuf>,
    },
}

/// Parse the argv tail after the `fszero zeroref-fixture` subcommand.
/// Does not assume any particular argv layout; every option is explicit.
/// Consume `--name value` / `--name=value` at `args[*i]`; advances `i` over value when spaced.
fn consume_flag<'a>(
    args: &'a [String],
    i: &mut usize,
    name: &str,
    what: &str,
) -> Result<Option<&'a str>, String> {
    let a = args[*i].as_str();
    if a == name {
        *i += 1;
        let v = args
            .get(*i)
            .map(String::as_str)
            .ok_or_else(|| format!("{name} requires a {what}"))?;
        return Ok(Some(v));
    }
    if let Some(rest) = a.strip_prefix(name).and_then(|r| r.strip_prefix('=')) {
        return Ok(Some(rest));
    }
    Ok(None)
}

pub fn parse_args(args: &[String]) -> Result<ZerorefFixtureAction, String> {
    let mut cmd: Option<String> = None;
    let mut store_root: Option<PathBuf> = None;
    let mut shared_root: Option<PathBuf> = None;
    let mut input: Option<PathBuf> = None;
    let mut max_object_bytes: Option<u64> = None;
    let mut reference: Option<String> = None;
    let mut out: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        if let Some(v) = consume_flag(args, &mut i, "--store-root", "path")? {
            store_root = Some(PathBuf::from(v));
        } else if let Some(v) = consume_flag(args, &mut i, "--shared-root", "path")? {
            shared_root = Some(PathBuf::from(v));
        } else if let Some(v) = consume_flag(args, &mut i, "--input", "path")? {
            input = Some(PathBuf::from(v));
        } else if let Some(v) = consume_flag(args, &mut i, "--out", "path")? {
            out = Some(PathBuf::from(v));
        } else if let Some(v) = consume_flag(args, &mut i, "--ref", "reference")? {
            reference = Some(v.to_string());
        } else if let Some(v) = consume_flag(args, &mut i, "--max-object-bytes", "number")? {
            max_object_bytes = Some(
                v.parse()
                    .map_err(|e| format!("bad max-object-bytes: {e}"))?,
            );
        } else {
            let v = args[i].as_str();
            if v.starts_with('-') {
                return Err(format!("unknown option: {v}"));
            }
            if cmd.is_some() {
                return Err(format!("unexpected positional argument: {v}"));
            }
            cmd = Some(v.to_string());
        }
        i += 1;
    }

    let cmd = cmd.ok_or_else(|| "missing subcommand: descriptor, put, or expand".to_string())?;
    let store_root = store_root.unwrap_or_else(|| PathBuf::from("."));
    match cmd.as_str() {
        "descriptor" => Ok(ZerorefFixtureAction::Descriptor { store_root }),
        "put" => Ok(ZerorefFixtureAction::Put {
            store_root,
            shared_root,
            input,
            max_object_bytes,
        }),
        "expand" => Ok(ZerorefFixtureAction::Expand {
            store_root,
            shared_root,
            reference: reference.ok_or_else(|| "expand requires --ref".to_string())?,
            out,
        }),
        other => Err(format!("unknown subcommand: {other}")),
    }
}

fn read_fixture_bytes(input: Option<&Path>) -> Result<Vec<u8>, FixtureError> {
    match input {
        Some(path) => std::fs::read(path).map_err(|e| {
            fail(
                "io",
                format!("read fixture input {}: {e}", path.display()),
                None,
            )
        }),
        None => {
            let mut buf = Vec::new();
            std::io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| fail("io", format!("read fixture bytes from stdin: {e}"), None))?;
            Ok(buf)
        }
    }
}

/// Produce a capability descriptor for the effective store configuration.
pub fn run_descriptor(store_root: &Path) -> Result<serde_json::Value, FixtureError> {
    let (sess, _) = open_session(store_root, None);
    Ok(sess.capability_descriptor())
}

/// Store `bytes` through the production FSZero store and return the fixture JSON.
pub fn run_put_bytes(
    store_root: &Path,
    shared_root: Option<&Path>,
    bytes: &[u8],
    max_object_bytes: Option<u64>,
) -> Result<serde_json::Value, FixtureError> {
    if let Some(limit) = max_object_bytes {
        if bytes.len() as u64 > limit {
            return Err(fail(
                "policy_denied",
                format!("payload {} bytes exceeds limit {limit}", bytes.len()),
                None,
            ));
        }
    }
    let (mut sess, effective_root) = open_session(store_root, shared_root);
    let reference = sess
        .recovery
        .try_put_content_ref(bytes)
        .map_err(|e| fail("io", format!("put failed: {e}"), None))?;
    if !reference.starts_with("fz://blob/") {
        return Err(fail(
            "io",
            format!("unexpected reference: {reference}"),
            None,
        ));
    }
    let hash = reference.strip_prefix("fz://blob/").unwrap().to_string();
    let size = bytes.len() as u64;
    let line_example = (std::str::from_utf8(bytes).is_ok() && !bytes.is_empty())
        .then(|| format!("{reference}#L1-1"));
    let shared_root_identity = effective_root.as_deref().map(root_identity);

    Ok(json!({
        "schema": FIXTURE_SCHEMA, "ok": true,
        "binary": binary_identity(), "capability": sess.capability_descriptor(),
        "ref": reference, "hash": hash, "size": size, "shared_root_identity": shared_root_identity,
        "fragments": {
            "whole": reference,
            "empty_bytes": format!("{reference}#B0-0"),
            "all_bytes": format!("{reference}#B0-{size}"),
            "first_line": line_example, },
        "os": std::env::consts::OS,
    }))
}

/// Read bytes from `--input` or stdin and store them.
pub fn run_put(
    store_root: &Path,
    shared_root: Option<&Path>,
    input: Option<&Path>,
    max_object_bytes: Option<u64>,
) -> Result<serde_json::Value, FixtureError> {
    let bytes = read_fixture_bytes(input)?;
    run_put_bytes(store_root, shared_root, &bytes, max_object_bytes)
}

#[derive(Debug)]
pub struct ExpandResult {
    pub payload: Vec<u8>,
    pub diag: serde_json::Value,
}

/// Expand a strict ZeroRef v1 ref (with optional `#B`/`#L` fragment), write the
/// selected bytes to `out` or stdout, and return diagnostics for stderr.
pub fn run_expand(
    store_root: &Path,
    shared_root: Option<&Path>,
    reference: &str,
    out: Option<&Path>,
) -> Result<ExpandResult, FixtureError> {
    let parsed = ZeroRef::parse(reference).map_err(|e| class_from_zeroref(&e, reference))?;
    let (sess, effective_root) = open_session(store_root, shared_root);

    // Resolve the whole object first so diagnostics can report its size.
    let whole_ref = format!("{}://blob/{}", EMITTED_SCHEME.as_str(), parsed.hash);
    let whole = sess
        .recovery
        .expand_zeroref(&whole_ref)
        .map_err(|e| class_from_zeroref(&e, reference))?;
    let selected = parsed
        .verify_and_select_with_policy(&whole, LineEndPolicy::Strict)
        .map_err(|e| class_from_zeroref(&e, reference))?;

    match out {
        Some(path) => std::fs::write(path, selected).map_err(|e| {
            fail(
                "io",
                format!("write fixture bytes to {}: {e}", path.display()),
                Some(reference.to_string()),
            )
        })?,
        None => {
            let mut stdout = std::io::stdout().lock();
            stdout.write_all(selected).map_err(|e| {
                fail(
                    "io",
                    format!("write fixture bytes: {e}"),
                    Some(reference.to_string()),
                )
            })?;
            stdout.flush().map_err(|e| {
                fail(
                    "io",
                    format!("flush fixture bytes: {e}"),
                    Some(reference.to_string()),
                )
            })?;
        }
    }

    let diag = json!({
        "schema": FIXTURE_SCHEMA, "ok": true, "binary": binary_identity(), "ref": reference,
        "hash": parsed.hash, "object_size": whole.len(),
        "selected_size": selected.len(), "shared_root_identity": effective_root.as_deref().map(root_identity),
        "os": std::env::consts::OS,
    });
    Ok(ExpandResult {
        payload: selected.to_vec(),
        diag,
    })
}

/// Render a fixture error as the JSON diagnostic a harness expects.
pub fn error_diag(err: &FixtureError) -> serde_json::Value {
    json!({
        "schema": FIXTURE_SCHEMA, "ok": false,
        "binary": binary_identity(), "error_class": err.class,
        "exit_code": exit_code_for_class(err.class), "message": err.message,
        "ref": err.reference, "os": std::env::consts::OS,
    })
}

/// Class names published by the ZeroRef fixture family (fszero-8n7a.6).
/// Codes stay single-sourced in \`exit_code_for_class\`.
pub const ZEROREF_EXIT_CLASSES: &[&str] = &[
    "malformed",
    "unsupported",
    "range_out_of_bounds",
    "not_utf8",
    "missing",
    "io",
    "digest_mismatch",
    "policy_denied",
    "incompatible_version",
    "legacy_ambiguity",
];

/// Exit-code dictionary for the ZeroRef fixture family, keyed by exit code.
/// \`capabilities --json\` merges this into its packaging exit codes so a single
/// surface answers both families.
pub fn exit_code_dictionary() -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for class in ZEROREF_EXIT_CLASSES {
        map.insert(
            exit_code_for_class(class).to_string(),
            serde_json::Value::String((*class).to_string()),
        );
    }
    serde_json::Value::Object(map)
}
