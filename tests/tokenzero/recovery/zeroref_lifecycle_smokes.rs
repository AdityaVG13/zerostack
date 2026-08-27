//! ZeroRef lifecycle smokes for cqr.10 (TokenZero-local, no external bins).
//!
//! Covers: fresh, upgraded-legacy, explicit-shared, default-isolated,
//! incompatible-peer, corruption, disable (missing ref stays miss), and
//! migration rollback dry-run. Foreign-OS multi-engine cells remain in
//! `zeroref_conformance_matrix` (pending until multi-OS CI merges).

use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use tempfile::TempDir;
use tokenzero_core::ContentType;
use tokenzero_recovery::RecoveryStore;
use tokenzero_recovery::migration::{LegacyMigration, RecoveryStoreAdapter};
use tokenzero_recovery::shared_cas::{SharedCas, SharedCasError};

type Scenario = fn() -> Result<(), String>;

const SCENARIOS: [(&str, Scenario); 8] = [
    ("fresh", fresh_publish_expand),
    ("upgraded_legacy", upgraded_legacy_alias_path),
    ("explicit_shared", explicit_shared_mode),
    ("default_isolated", default_isolated_mode),
    ("incompatible_peer", incompatible_peer_missing_hash),
    ("corruption", corruption_digest_mismatch),
    ("disable", disable_ref_index_local_only),
    ("rollback", migration_rollback_smoke),
];

fn ensure(condition: bool, message: impl Into<String>) -> Result<(), String> {
    condition.then_some(()).ok_or_else(|| message.into())
}

fn recovery_store(cache: PathBuf) -> Result<RecoveryStore, String> {
    fs::create_dir_all(cache.parent().unwrap()).map_err(|e| e.to_string())?;
    Ok(RecoveryStore::new(Some(cache)))
}

fn temp_dir() -> Result<TempDir, String> {
    TempDir::new().map_err(|e| e.to_string())
}

fn put_ref(store: &mut RecoveryStore, text: &str) -> Result<String, String> {
    store
        .store_payload(text, ContentType::Unknown, None, None, None)
        .map(|stored| stored.blob_ref)
        .map_err(|e| e.to_string())
}

fn expand_ref(store: &mut RecoveryStore, reference: &str) -> tokenzero_recovery::ExpansionResult {
    store.expand(reference, None, None, None, None, None)
}

fn cell(name: &str, status: &str, notes: impl Into<String>) -> Value {
    json!({
        "test": name,
        "status": status,
        "notes": notes.into(),
        "os": std::env::consts::OS,
        "engine": "tokenzero",
    })
}

fn run_catch(name: &str, scenario: Scenario) -> Value {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(scenario)) {
        Ok(Ok(())) => cell(name, "pass", ""),
        Ok(Err(notes)) => cell(name, "fail", notes),
        Err(err) => cell(
            name,
            "fail",
            err.downcast_ref::<String>()
                .map(|s| format!("panic: {s}"))
                .or_else(|| err.downcast_ref::<&str>().map(|s| format!("panic: {s}")))
                .unwrap_or_else(|| "panic: unknown".to_string()),
        ),
    }
}

fn fresh_publish_expand() -> Result<(), String> {
    let dir = temp_dir()?;
    let cas_root = dir.path().join("shared-cas");
    fs::create_dir_all(&cas_root).map_err(|e| e.to_string())?;
    let cas = SharedCas::new(cas_root.clone());
    let bytes = b"fresh-install-smoke-payload\n";
    let hash = cas.publish(bytes).map_err(|e| e.to_string())?;
    ensure(
        cas.resolve(&hash).map_err(|e| e.to_string())? == bytes,
        "fresh CAS round-trip mismatch",
    )?;
    let mut store = recovery_store(cas_root.join("tokenzero/recovery-cache.json"))?;
    let text = std::str::from_utf8(bytes).unwrap();
    let put = put_ref(&mut store, text)?;
    let expanded = expand_ref(&mut store, &put);
    ensure(
        expanded.found,
        format!("fresh expand miss: {}", expanded.reason),
    )?;
    ensure(expanded.content == text, "fresh recovery expand mismatch")
}

fn upgraded_legacy_alias_path() -> Result<(), String> {
    let dir = temp_dir()?;
    let legacy = dir.path().join(".tokenzero/recovery-cache.json");
    let mut store = recovery_store(legacy.clone())?;
    let text = "upgraded-legacy-smoke\n";
    let put = put_ref(&mut store, text)?;
    let mut reopened = RecoveryStore::new(Some(legacy));
    let exp = expand_ref(&mut reopened, &put);
    ensure(exp.found, format!("legacy reopen miss: {}", exp.reason))?;
    ensure(exp.content == text, "legacy reopen byte mismatch")?;
    let via_fz = expand_ref(&mut reopened, &put.replacen("tz://blob/", "fz://blob/", 1));
    ensure(
        via_fz.found && via_fz.content == text,
        "legacy fz alias expand failed",
    )
}

fn explicit_shared_mode() -> Result<(), String> {
    let dir = temp_dir()?;
    let cas = SharedCas::new(dir.path().join("cas"));
    let bytes = b"explicit-shared-mode\n";
    let hash = cas.publish(bytes).map_err(|e| e.to_string())?;
    ensure(
        cas.resolve(&hash).map_err(|e| e.to_string())? == bytes,
        "shared resolve mismatch",
    )?;
    let cache = dir.path().join("unified/tokenzero/recovery-cache.json");
    let mut first = recovery_store(cache.clone())?;
    let text = std::str::from_utf8(bytes).unwrap();
    let put = put_ref(&mut first, text)?;
    let mut second = RecoveryStore::new(Some(cache));
    let exp = expand_ref(&mut second, &put);
    ensure(
        exp.found && exp.content == text,
        format!("shared second handle miss: {}", exp.reason),
    )?;
    Ok(())
}

fn default_isolated_mode() -> Result<(), String> {
    unsafe {
        std::env::set_var("TOKENZERO_REF_INDEX", "0");
        std::env::remove_var("TOKENZERO_SHARED_STORE");
        std::env::remove_var("ZEROSTACK_SHARED_STORE");
        std::env::remove_var("ZEROSTACK_STORE_ROOT");
    }
    let a = temp_dir()?;
    let b = temp_dir()?;
    let mut store_a = recovery_store(
        a.path()
            .join("proj-a/.zerostack/tokenzero/recovery-cache.json"),
    )?;
    let mut store_b = recovery_store(
        b.path()
            .join("proj-b/.zerostack/tokenzero/recovery-cache.json"),
    )?;
    let put = put_ref(&mut store_a, "isolated-only\n")?;
    let miss = expand_ref(&mut store_b, &put);
    ensure(
        !miss.found,
        format!(
            "default-isolated unexpectedly resolved foreign store ref (reason={})",
            miss.reason
        ),
    )
}

fn incompatible_peer_missing_hash() -> Result<(), String> {
    let dir = temp_dir()?;
    let mut store = recovery_store(dir.path().join(".tokenzero/recovery-cache.json"))?;
    let foreign = format!("fz://blob/{}", "ab".repeat(32));
    let exp = expand_ref(&mut store, &foreign);
    ensure(
        !exp.found,
        "incompatible-peer unexpectedly found foreign full hash",
    )?;
    ensure(
        !exp.reason.trim().is_empty(),
        "incompatible-peer missing structured reason",
    )
}

fn corruption_digest_mismatch() -> Result<(), String> {
    let dir = temp_dir()?;
    let cas = SharedCas::new(dir.path().join("cas"));
    let bytes = b"corruption-canary\n";
    let hash = cas.publish(bytes).map_err(|e| e.to_string())?;
    let path = cas
        .root()
        .join("blobs")
        .join("sha256")
        .join(&hash[..2])
        .join(&hash);
    fs::write(&path, b"tampered-bytes").map_err(|e| e.to_string())?;
    match cas.resolve(&hash) {
        Err(SharedCasError::Corruption) => Ok(()),
        Err(other) => Err(format!("expected Corruption, got {other}")),
        Ok(_) => Err("corruption resolve unexpectedly succeeded".into()),
    }
}

fn disable_ref_index_local_only() -> Result<(), String> {
    unsafe {
        std::env::set_var("TOKENZERO_REF_INDEX", "0");
    }
    let dir = temp_dir()?;
    let mut store = recovery_store(dir.path().join(".tokenzero/recovery-cache.json"))?;
    let missing = format!("tz://blob/{}", "cd".repeat(32));
    let exp = expand_ref(&mut store, &missing);
    ensure(!exp.found, "disable smoke resolved missing ref")?;
    ensure(
        !exp.reason.trim().is_empty(),
        "disable smoke missing reason",
    )
}

fn migration_rollback_smoke() -> Result<(), String> {
    let dir = temp_dir()?;
    let cas = SharedCas::new(dir.path().join("cas"));
    let mut store = recovery_store(dir.path().join("tokenzero/recovery-cache.json"))?;
    let manifest = dir.path().join("migration-manifest.json");
    let mut adapter = RecoveryStoreAdapter::new(&mut store);
    let mut migration = LegacyMigration::new(&mut adapter, &cas, Some(manifest.clone()));
    let report = migration.rollback(false);
    ensure(
        report.failed == 0,
        format!(
            "rollback dry-run failed={} errors={:?}",
            report.failed, report.errors
        ),
    )?;
    ensure(!manifest.exists(), "rollback dry-run created manifest")
}

/// Runnable cqr.10 lifecycle smokes (host OS only; no foreign multi-engine bins).
#[test]
fn zeroref_lifecycle_smokes_tokenzero_local() {
    let cells: Vec<_> = SCENARIOS
        .into_iter()
        .map(|(name, scenario)| run_catch(name, scenario))
        .collect();

    if let Some(path) = std::env::var_os("ZEROREF_LIFECYCLE_EVIDENCE_PATH") {
        let doc = json!({
            "schema": "zeroref-lifecycle-smokes/v1",
            "os": std::env::consts::OS,
            "cells": cells,
            "status": if cells.iter().all(|c| c["status"] == "pass") { "pass" } else { "fail" },
        });
        let path = PathBuf::from(path);
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        fs::write(path, serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    }

    let failed: Vec<_> = cells
        .iter()
        .filter(|c| c["status"] != "pass")
        .map(|c| {
            format!(
                "{}: {}",
                c["test"].as_str().unwrap_or("?"),
                c["notes"].as_str().unwrap_or("")
            )
        })
        .collect();
    assert!(
        failed.is_empty(),
        "lifecycle smokes failed: {}",
        failed.join("; ")
    );
}
