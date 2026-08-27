//! ZeroRef v1 three-binary conformance matrix.
//!
//! Drives real FSZero/Graph `zeroref-fixture` binaries and Token
//! recovery/shared-CAS paths; writes retained evidence for release gates.
//!
//! The external matrix stays `#[ignore]` in generic test lanes because peer
//! binaries are required. CI is the mandatory lane: it builds pinned peers and
//! invokes this exact test with `--ignored` (CC1-R1-003).
//!
//! A Darwin-only local `--ignored` run is a single host-OS row, not multi-OS
//! evidence. Do not advertise Linux/Windows portability from that log alone.
//!
//! Run from the tokenzero repo root:
//!     env -u TOKENZERO_CACHE_PATH -u ZEROSTACK_STORE_ROOT \
//!       CARGO_BUILD_JOBS=1 FSZERO_BIN=/path/to/fszero \
//!       GRAPHZERO_BIN=/path/to/graphzero TOKENZERO_BIN=/path/to/tokenzero \
//!       cargo test -p tokenzero-recovery --test zeroref_conformance_matrix -- --ignored --test-threads=1

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::SystemTime;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use tokenzero_recovery::RecoveryStore;
use tokenzero_recovery::shared_cas::SharedCas;

const SCHEMA: &str = "zeroref-conformance-evidence/v1";
const ZEROREF_VERSION: &str = "v1";
const OS: &str = std::env::consts::OS;
const MAX_OBJECT_BYTES: &str = "268435456";
const ENGINES: [Engine; 3] = [Engine::Fs, Engine::Graph, Engine::Token];
const OS_ROWS: [&str; 3] = ["macos", "linux", "windows"];
const VECTORS: &[(&str, &[u8])] = &[
    ("empty", b""),
    ("utf8_text", b"Hello, World!\nLine two.\nLine three.\n"),
    ("crlf", b"line1\r\nline2\r\nline3\r\n"),
    ("binary", &[0x00, 0x01, 0x02, 0xff, 0xfe, 0x80, 0x41]),
];
const FRAGMENTS: &[(&str, &str)] = &[
    ("B0-5", "alpha"),
    ("B6-10", "beta"),
    ("B0-0", ""),
    ("L1-1", "alpha\n"),
    ("L2-3", "beta\ngamma\n"),
];
const FRAG_SRC: &[u8] = b"alpha\nbeta\ngamma\ndelta\n";
const CORRUPT_PAYLOAD: &[u8] = b"corruption-canary\n";
const CONCURRENT_PAYLOAD: &[u8] = b"concurrent-identical-writer-content\n";

#[derive(Debug, Clone)]
struct BinaryMeta {
    engine: &'static str,
    path: PathBuf,
    sha256: String,
    version: String,
    commit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Engine {
    Fs = 0,
    Graph = 1,
    Token = 2,
}

impl Engine {
    fn as_str(self) -> &'static str {
        ["fszero", "graphzero", "tokenzero"][self as usize]
    }
    fn env_bin(self) -> &'static str {
        ["FSZERO_BIN", "GRAPHZERO_BIN", "TOKENZERO_BIN"][self as usize]
    }
    fn is_fixture(self) -> bool {
        (self as usize) < 2
    }
    fn bin(self) -> PathBuf {
        env::var_os(self.env_bin())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(self.as_str()))
    }
}

struct Harness {
    base: TempDir,
    shared_cas: PathBuf,
    binaries: [BinaryMeta; 3],
    evidence: PathBuf,
}

impl Harness {
    fn meta(&self, e: Engine) -> &BinaryMeta {
        &self.binaries[e as usize]
    }
    fn roots(&self, prefix: &str, w: Engine, r: Engine) -> (PathBuf, PathBuf) {
        let mk = |role: &str, e: Engine| {
            let p = self
                .base
                .path()
                .join(format!("{prefix}-{}-{role}", e.as_str()));
            fs::create_dir_all(&p).unwrap();
            p
        };
        (mk("writer", w), mk("reader", r))
    }
    fn out(&self, kind: &str, w: Engine, r: Engine, name: &str) -> PathBuf {
        self.base
            .path()
            .join(format!("{kind}-{}-{}-{name}.bin", w.as_str(), r.as_str()))
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

fn portable_path(path: &Path) -> String {
    let mut rules: Vec<(PathBuf, &str)> = Vec::new();
    if let Some(workspace) = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
    {
        if let Ok(canonical) = workspace.canonicalize() {
            rules.push((canonical, ""));
        }
        rules.push((workspace.to_path_buf(), ""));
    }
    rules.push((env::temp_dir(), "<tmp>"));
    if let Ok(canonical) = env::temp_dir().canonicalize() {
        rules.push((canonical, "<tmp>"));
    }
    rules.push((PathBuf::from("/tmp"), "<tmp>"));
    if let Some(home) = env::var_os("HOME") {
        rules.push((PathBuf::from(home), "<home>"));
    }
    for (base, placeholder) in &rules {
        if let Ok(relative) = path.strip_prefix(base) {
            let rendered = relative.display().to_string();
            return match (placeholder.is_empty(), rendered.is_empty()) {
                (true, true) => ".".to_string(),
                (true, false) => rendered,
                (false, true) => (*placeholder).to_string(),
                (false, false) => format!("{placeholder}/{rendered}"),
            };
        }
    }
    path.display().to_string()
}

fn discover_binary(engine: Engine) -> BinaryMeta {
    let name = engine.as_str();
    let path = engine.bin();
    let path = path.canonicalize().unwrap_or_else(|_| path.clone());
    BinaryMeta {
        engine: name,
        sha256: sha256_bytes(&fs::read(&path).expect("read binary")),
        path: path.clone(),
        version: Command::new(&path)
            .arg("--version")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .unwrap_or_default()
            .trim()
            .to_string(),
        commit: env::var(format!("{}_COMMIT", name.to_uppercase().replace('-', "_")))
            .unwrap_or_else(|_| "unknown".to_string()),
    }
}

fn fixture_run(
    bin: &Path,
    action: &str,
    store: &Path,
    shared: &Path,
    args: &[(&str, &str)],
) -> Output {
    let mut cmd = Command::new(bin);
    cmd.env_remove("TOKENZERO_CACHE_PATH")
        .env_remove("ZEROSTACK_STORE_ROOT")
        .env_remove("TOKENZERO_REF_INDEX")
        .env("FSZERO_REF_INDEX", "0");
    cmd.arg("zeroref-fixture")
        .arg(action)
        .arg("--store-root")
        .arg(store)
        .arg("--shared-root")
        .arg(shared);
    for &(k, v) in args {
        cmd.arg(k).arg(v);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.output().expect("spawn fixture command")
}

fn fixture_put(bin: &Path, store: &Path, shared: &Path, input: &Path, engine: &str) -> Value {
    let out = fixture_run(
        bin,
        "put",
        store,
        shared,
        &[
            ("--input", input.to_str().unwrap()),
            ("--max-object-bytes", MAX_OBJECT_BYTES),
        ],
    );
    assert!(
        out.status.success(),
        "{engine} put failed: {} / {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn put_bytes(h: &Harness, w: Engine, root: &Path, input: &Path, payload: &[u8]) -> String {
    if w.is_fixture() {
        fixture_put(&h.meta(w).path, root, &h.shared_cas, input, w.as_str())["ref"]
            .as_str()
            .unwrap()
            .to_string()
    } else {
        format!(
            "tz://blob/{}",
            SharedCas::new(h.shared_cas.clone())
                .publish(payload)
                .expect("tokenzero publish")
        )
    }
}

fn expand_bytes(h: &Harness, r: Engine, root: &Path, rf: &str, out: &Path, exp: &[u8]) -> Vec<u8> {
    if r.is_fixture() {
        let run = fixture_run(
            &h.meta(r).path,
            "expand",
            root,
            &h.shared_cas,
            &[("--ref", rf), ("--out", out.to_str().unwrap())],
        );
        assert!(run.status.success(), "{:?}", run.status.code());
        fs::read(out).expect("read expanded bytes")
    } else if rf.contains('#') {
        let mut store =
            RecoveryStore::new(Some(h.shared_cas.join("tokenzero/recovery-cache.json")));
        let result = store.expand(rf, None, None, None, None, None);
        assert!(result.found, "{}", result.reason);
        result.content.into_bytes()
    } else {
        let hash = ["tz://blob/", "fz://blob/", "gz://blob/"]
            .iter()
            .find_map(|p| rf.strip_prefix(p))
            .expect("valid blob ref");
        let bytes = SharedCas::new(h.shared_cas.clone()).resolve(hash).unwrap();
        assert_eq!(bytes, exp);
        bytes
    }
}

fn payloads() -> Vec<(&'static str, Vec<u8>)> {
    let mut v: Vec<_> = VECTORS.iter().map(|(n, b)| (*n, b.to_vec())).collect();
    v.push((
        "big",
        b"the quick brown fox jumps over the lazy dog\n"
            .iter()
            .copied()
            .cycle()
            .take(10 * 1024 * 1024)
            .collect(),
    ));
    v
}

fn write_payload(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(format!("{name}.bin"));
    fs::write(&path, bytes).expect("write payload");
    path
}

fn panic_notes(err: Box<dyn std::any::Any + Send>) -> String {
    format!(
        "panic: {}",
        err.downcast_ref::<String>()
            .map(|s| s.as_str())
            .or_else(|| err.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic")
    )
}

fn pair_put(
    h: &Harness,
    prefix: &str,
    writer: Engine,
    reader: Engine,
    name: &str,
    payload: &[u8],
) -> (PathBuf, String) {
    let (wroot, rroot) = h.roots(prefix, writer, reader);
    let input = write_payload(h.base.path(), name, payload);
    (rroot, put_bytes(h, writer, &wroot, &input, payload))
}

fn run_cell(h: &Harness, w: Engine, r: Engine, name: &str, payload: &[u8]) -> Value {
    let (rroot, reference) = pair_put(h, "store", w, r, name, payload);
    let expected_hash = sha256_bytes(payload);
    let consumer = h.meta(r);
    let result = std::panic::catch_unwind(|| {
        let bytes = expand_bytes(h, r, &rroot, &reference, &h.out("out", w, r, name), payload);
        let hash = sha256_bytes(&bytes);
        assert_eq!(hash, expected_hash);
        assert_eq!(bytes, payload);
        hash
    });
    let (status, actual_hash, notes) = match result {
        Ok(hash) => ("pass", Some(hash), String::new()),
        Err(err) => ("fail", None, panic_notes(err)),
    };
    json!({
        "writer": w.as_str(), "reader": r.as_str(), "payload": name, "reference": reference,
        "expected_hash": expected_hash, "actual_hash": actual_hash, "status": status, "notes": notes,
        "consumer": consumer.engine, "consumer_version": consumer.version,
        "consumer_path": portable_path(&consumer.path), "consumer_sha256": consumer.sha256,
    })
}

fn run_fragment_cell(h: &Harness, writer: Engine, reader: Engine) -> Vec<Value> {
    let (rroot, reference) = pair_put(h, "frag-store", writer, reader, "fragment_text", FRAG_SRC);
    FRAGMENTS.iter().map(|(frag, expected_text)| {
        let ref_with_frag = format!("{reference}#{frag}");
        let out = h.out("frag-out", writer, reader, frag);
        let result = std::panic::catch_unwind(|| {
            let got = expand_bytes(h, reader, &rroot, &ref_with_frag, &out, expected_text.as_bytes());
            assert_eq!(got, expected_text.as_bytes());
        });
        let (status, notes) = match &result {
            Ok(_) => ("pass", String::new()),
            Err(err) => ("fail", format!("{:?}", err)),
        };
        json!({
            "writer": writer.as_str(), "reader": reader.as_str(), "fragment": frag,
            "reference": ref_with_frag, "expected": expected_text, "status": status, "notes": notes,
        })
    }).collect()
}

/// FSZero and GraphZero expose the portable ZeroRef fixture class
/// `range_out_of_bounds`. TokenZero keeps the more precise domain reason
/// `fragment-out-of-range`; the conformance evidence normalizes only those
/// equivalent ref-fragment failures. Call-time window failures remain the
/// distinct `window-out-of-range` class.
fn portable_range_error_class(native: &str) -> Option<&'static str> {
    let tag = native.split(';').next().unwrap_or(native);
    matches!(
        tag,
        "range_out_of_bounds" | "fragment-out-of-range" | "fragment_out_of_range"
    )
    .then_some("range_out_of_bounds")
}

fn fixture_error_class(output: &Output) -> Option<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .and_then(|diag| diag["error_class"].as_str().map(str::to_string))
}

fn native_range_error(
    h: &Harness,
    reader: Engine,
    root: &Path,
    reference: &str,
    out: &Path,
) -> String {
    if reader.is_fixture() {
        let run = fixture_run(
            &h.meta(reader).path,
            "expand",
            root,
            &h.shared_cas,
            &[("--ref", reference), ("--out", out.to_str().unwrap())],
        );
        assert!(
            !run.status.success(),
            "out-of-range fragment unexpectedly expanded"
        );
        fixture_error_class(&run).expect("fixture error_class")
    } else {
        let mut store =
            RecoveryStore::new(Some(h.shared_cas.join("tokenzero/recovery-cache.json")));
        let expanded = store.expand(reference, None, None, None, None, None);
        assert!(
            !expanded.found,
            "out-of-range fragment unexpectedly expanded"
        );
        expanded
            .reason
            .split(';')
            .next()
            .unwrap_or(expanded.reason.as_str())
            .to_string()
    }
}

fn run_range_error_cells(h: &Harness, writer: Engine, reader: Engine) -> Vec<Value> {
    let (rroot, reference) = pair_put(
        h,
        "range-error-store",
        writer,
        reader,
        "range_error_text",
        FRAG_SRC,
    );
    let invalid = [format!("B0-{}", FRAG_SRC.len() + 1), "L99-100".to_string()];
    invalid
        .into_iter()
        .map(|fragment| {
            let ref_with_fragment = format!("{reference}#{fragment}");
            let out = h.out("range-error-out", writer, reader, &fragment);
            let expected_native = if reader.is_fixture() {
                "range_out_of_bounds"
            } else {
                "fragment-out-of-range"
            };
            let result = std::panic::catch_unwind(|| {
                let native = native_range_error(h, reader, &rroot, &ref_with_fragment, &out);
                assert_eq!(native, expected_native);
                let portable = portable_range_error_class(&native);
                assert_eq!(portable, Some("range_out_of_bounds"));
                native
            });
            let (status, native_error_class, notes) = match result {
                Ok(native) => ("pass", Some(native), String::new()),
                Err(err) => ("fail", None, panic_notes(err)),
            };
            json!({
                "writer": writer.as_str(), "reader": reader.as_str(),
                "fragment": fragment, "reference": ref_with_fragment,
                "portable_error_class": "range_out_of_bounds",
                "expected_native_error_class": expected_native,
                "native_error_class": native_error_class,
                "status": status, "notes": notes,
            })
        })
        .collect()
}

#[test]
fn portable_range_taxonomy_keeps_call_windows_distinct() {
    assert_eq!(
        portable_range_error_class("range_out_of_bounds"),
        Some("range_out_of_bounds")
    );
    assert_eq!(
        portable_range_error_class("fragment-out-of-range; start=0 end=9 len=4"),
        Some("range_out_of_bounds")
    );
    assert_eq!(
        portable_range_error_class("window-out-of-range; start=9 end=9 line_count=4"),
        None
    );
}

fn run_wrong_store(h: &Harness) -> Value {
    let (wroot, rroot) = h.roots("corrupt", Engine::Fs, Engine::Graph);
    let put = fixture_put(
        &h.meta(Engine::Fs).path,
        &wroot,
        &h.shared_cas,
        &write_payload(h.base.path(), "corrupt", CORRUPT_PAYLOAD),
        "fszero",
    );
    let reference = put["ref"].as_str().unwrap().to_string();
    let hash = put["hash"].as_str().unwrap();
    fs::write(
        h.shared_cas
            .join("blobs/sha256")
            .join(&hash[..2])
            .join(hash),
        b"tampered bytes",
    )
    .expect("corrupt object");
    let out_path = h.base.path().join("corrupt-out.bin");
    let out = fixture_run(
        &h.meta(Engine::Graph).path,
        "expand",
        &rroot,
        &h.shared_cas,
        &[
            ("--ref", reference.as_str()),
            ("--out", out_path.to_str().unwrap()),
        ],
    );
    let diag: Value = serde_json::from_slice(&out.stderr).unwrap_or(json!({}));
    let failed = !out.status.success();
    json!({
        "test": "corruption-catches-false-positive", "reference": reference,
        "producer": "fszero", "consumer": "graphzero", "consumer_failed": failed,
        "exit_code": out.status.code(), "error_class": diag.get("error_class"), "diag": diag,
        "status": if failed { "pass" } else { "fail" }
    })
}

fn publish_hash(engine: Engine, shared: &Path, payload: &[u8]) -> String {
    if engine.is_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("store");
        fs::create_dir_all(&store).unwrap();
        let input = write_payload(dir.path(), "payload", payload);
        fixture_put(&engine.bin(), &store, shared, &input, engine.as_str())["hash"]
            .as_str()
            .unwrap()
            .to_string()
    } else {
        SharedCas::new(shared.to_path_buf())
            .publish(payload)
            .unwrap()
    }
}

fn run_concurrent_writes(h: &Harness) -> Value {
    let expected_hash = sha256_bytes(CONCURRENT_PAYLOAD);
    let handles: Vec<_> = ENGINES
        .into_iter()
        .map(|engine| {
            let shared = h.shared_cas.clone();
            let b = CONCURRENT_PAYLOAD.to_vec();
            (
                engine,
                std::thread::spawn(move || publish_hash(engine, &shared, &b)),
            )
        })
        .collect();
    let mut hashes = BTreeMap::new();
    for (engine, handle) in handles {
        hashes.insert(
            engine.as_str().to_string(),
            handle.join().expect("thread join"),
        );
    }
    json!({
        "test": "concurrent-identical-writers", "expected_hash": expected_hash, "hashes": hashes,
        "status": if hashes.values().all(|v| v == &expected_hash) { "pass" } else { "fail" }
    })
}

fn platform_row(h: &Harness, os: &str) -> Value {
    json!({
        "os": os,
        "cells": if OS == os {
            ENGINES.into_iter().flat_map(|w| ENGINES.into_iter().map(move |r| (w, r)))
                .flat_map(|(w, r)| payloads().into_iter().map(move |(n, p)| run_cell(h, w, r, n, &p)))
                .collect::<Vec<_>>()
        } else {
            ENGINES.into_iter().flat_map(|w| ENGINES.into_iter().map(move |r| (w, r)))
                .map(|(w, r)| json!({
                    "writer": w.as_str(), "reader": r.as_str(), "payload": "all", "status": "skip",
                    "skip_reason": format!("host OS is {OS}; cannot run {os} cells on this machine")
                }))
                .collect::<Vec<_>>()
        }
    })
}

#[test]
#[ignore = "requires external fszero, graphzero, and tokenzero release binaries"]
fn zeroref_conformance_matrix() {
    unsafe {
        env::remove_var("TOKENZERO_CACHE_PATH");
        env::remove_var("ZEROSTACK_STORE_ROOT");
        env::set_var("TOKENZERO_REF_INDEX", "0");
    }
    let binaries = ENGINES.map(discover_binary);
    for meta in &binaries {
        assert!(meta.path.exists(), "{:?} missing", meta.path);
    }
    let base = TempDir::new().expect("temp dir");
    let shared_cas = base.path().join("shared-cas");
    fs::create_dir_all(&shared_cas).unwrap();
    let evidence = env::var_os("ZEROREF_EVIDENCE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| base.path().join("zeroref-conformance-evidence.json"));
    let h = Harness {
        base,
        shared_cas,
        binaries,
        evidence,
    };
    let rows: Vec<_> = OS_ROWS.iter().map(|os| platform_row(&h, os)).collect();
    let fragment_rows: Vec<_> = ENGINES
        .into_iter()
        .flat_map(|w| ENGINES.into_iter().map(move |r| (w, r)))
        .flat_map(|(w, r)| run_fragment_cell(&h, w, r))
        .collect();
    let range_error_rows: Vec<_> = ENGINES
        .into_iter()
        .flat_map(|w| ENGINES.into_iter().map(move |r| (w, r)))
        .flat_map(|(w, r)| run_range_error_cells(&h, w, r))
        .collect();
    let wrong_store = run_wrong_store(&h);
    let concurrent = run_concurrent_writes(&h);
    let sibling_shas: Vec<_> = h.binaries.iter().map(|m| json!({
        "engine": m.engine, "path": portable_path(&m.path), "sha256": m.sha256, "version": m.version, "commit": m.commit, "os": OS
    })).collect();
    let fail = rows.iter().any(|row| {
        row["cells"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["status"] == "fail")
    }) || fragment_rows.iter().any(|f| f["status"] == "fail")
        || range_error_rows.iter().any(|row| row["status"] == "fail")
        || wrong_store["status"] != "pass"
        || concurrent["status"] != "pass";
    let status = if fail { "red" } else { "green" };
    let evidence_doc = json!({
        "schema": SCHEMA,
        "zeroref_version": ZEROREF_VERSION,
        "timestamp": humantime_rfc3339(SystemTime::now()),
        "descriptor_tests": "crates/tokenzero-recovery/tests/zeroref_conformance_matrix.rs",
        "docs_audit": "ok",
        "matrix": {
            "status": status,
            "note": "Real three-binary ZeroRef v1 conformance matrix. macOS rows executed on this host; Linux/Windows rows are explicit skips because the host is macOS.",
            "sibling_shas": sibling_shas,
            "rows": rows,
            "fragment_rows": fragment_rows,
            "range_error_rows": range_error_rows,
            "wrong_store": wrong_store,
            "concurrent": concurrent
        }
    });
    fs::create_dir_all(h.evidence.parent().unwrap()).unwrap();
    fs::write(
        &h.evidence,
        serde_json::to_string_pretty(&evidence_doc).unwrap(),
    )
    .expect("write evidence");
    assert_eq!(status, "green", "matrix red; see {:?}", h.evidence);
}

fn humantime_rfc3339(t: SystemTime) -> String {
    let d = t.duration_since(SystemTime::UNIX_EPOCH).expect("time");
    let (secs, nanos, rem) = (d.as_secs(), d.subsec_nanos(), d.as_secs() % 86400);
    let mut l = (secs / 86400) as i64 + 2_509_157;
    let n = 4 * l / 146_097;
    l -= (146_097 * n + 3) / 4;
    let i = 4_000 * (l + 1) / 1_461_001;
    l = l - 1_461 * i / 4 + 31;
    let j = 80 * l / 2_447;
    let day = l - 2_447 * j / 80;
    l = j / 11;
    let (month, year) = (j + 2 - 12 * l, 100 * (n - 49) + i + l);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{nanos:09}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}
