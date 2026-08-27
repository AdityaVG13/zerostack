//! Legacy short-ref migration to full SHA-256 canonical refs.
//!
//! TokenZero's original `id_for(prefix, text)` generates 17-character short IDs
//! (prefix char + 16 hex from the first 8 SHA-256 bytes). The ZeroRef v1 shared
//! CAS uses the full 64-hex SHA-256 digest. This module migrates legacy
//! short-ID blobs to the canonical shared CAS, builds an alias index for
//! backward-compatible reads, and supports idempotent re-runs with a versioned
//! manifest.
//!
//! ## Operations
//! - `migrate` (default): dry-run by default; `--apply` publishes to CAS and
//!   stores aliases.
//! - `verify`: checks every entry in the manifest against current CAS state,
//!   reports integrity.
//! - `rollback`: removes migration-created aliases and manifest, never CAS/source
//!   bytes.
//! - `cleanup`: after successful verification, removes legacy source payloads
//!   while preserving alias reads through CAS. Dry-run first; requires both
//!   `--apply` and `--confirm-cleanup`.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::shared_cas::{SharedCas, SharedCasError};

/// Manifest schema version. Bumped when the manifest format changes.
pub const MIGRATION_MANIFEST_VERSION: &str = "tokenzero.migration.v2";

/// Prefix for blob refs in the legacy store.
const BLOB_REF_PREFIX: &str = "tz://blob/";

/// Length of a legacy short ID: prefix char + 16 hex chars = 17.
const LEGACY_SHORT_ID_LEN: usize = 17;

/// Length of a full SHA-256 hex ID: 64 chars.
/// Tmp retry budget for atomic manifest saves.
const TMP_RETRIES: usize = 16;

/// Whether a blob ref ID is a legacy short-ID ref.
///
/// Legacy refs look like `tz://blob/b<16hex>` (17-char ID portion).
/// Full-hash refs look like `tz://blob/<64hex>` (64-char hex portion).
pub fn is_legacy_blob_ref(ref_id: &str) -> bool {
    let Some(rest) = ref_id.strip_prefix(BLOB_REF_PREFIX) else {
        return false;
    };
    rest.len() == LEGACY_SHORT_ID_LEN
        && rest.starts_with('b')
        && rest[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Extract the 16-hex short ID portion from a legacy blob ref.
fn short_id_hex(ref_id: &str) -> Option<&str> {
    let rest = ref_id.strip_prefix(BLOB_REF_PREFIX)?;
    if rest.len() == LEGACY_SHORT_ID_LEN && rest.starts_with('b') {
        Some(&rest[1..])
    } else {
        None
    }
}

/// Compute the full 64-hex SHA-256 of `bytes`.
pub fn full_sha256_hex(bytes: &[u8]) -> String {
    crate::shared_cas::content_sha256_hex(bytes)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ── Stable error codes ────────────────────────────────────────────────────

/// Stable error codes returned by migration operations.
/// Every failure path has a unique, stable code suitable for scripting.
macro_rules! migration_error_codes {
    ($($variant:ident => $code:literal),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "kebab-case")]
        pub enum MigrationErrorCode { $($variant),+ }
        impl std::fmt::Display for MigrationErrorCode {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(match self { $(Self::$variant => $code),+ })
            }
        }
    };
}
migration_error_codes! {
    Internal => "internal",
    ManifestNewerVersion => "manifest-newer-version",
    ManifestCorrupt => "manifest-corrupt",
    ManifestMissing => "manifest-missing",
    SourceMissing => "source-missing",
    SourceCorrupt => "source-corrupt",
    AmbiguousShortId => "ambiguous-short-id",
    AliasConflict => "alias-conflict",
    ManifestHashConflict => "manifest-hash-conflict",
    CasIo => "cas-io",
    CasCorruption => "cas-corruption",
    CasPolicy => "cas-policy",
    StorePersist => "store-persist",
    ManifestSave => "manifest-save",
    CasMissing => "cas-missing",
    AliasMissing => "alias-missing",
    RollbackSourceGone => "rollback-source-gone",
    CleanupConfirmationRequired => "cleanup-confirmation-required",
    CleanupNeedsVerification => "cleanup-needs-verification",
    InvalidFlags => "invalid-flags",
}
impl std::error::Error for MigrationErrorCode {}

// ── Per-entry manifest state ──────────────────────────────────────────────

/// State of an individual entry in the migration manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState {
    Migrated,
    Verified,
    CleanupEligible,
}

// ── Manifest entry ────────────────────────────────────────────────────────

/// One entry in the migration manifest.
/// Contains proofs (hash + size) but no payload or filesystem paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub short_ref: String,
    pub full_hash: String,
    pub size: u64,
    pub state: EntryState,
    pub migrated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<u64>,
    #[serde(default)]
    pub resumed: bool,
    /// Whether this entry's alias was created by migration (true) or
    /// existed before migration ran (false). Used by rollback to avoid
    /// removing aliases that migration did not create.
    #[serde(default)]
    pub owner_alias: bool,
}

impl ManifestEntry {
    fn full_ref(&self) -> String {
        format!("{BLOB_REF_PREFIX}{}", self.full_hash)
    }

    fn report(&self, report: &mut MigrationReport, short_ref: &str, status: AliasStatus) {
        report.alias(short_ref, self.full_ref(), self.size, status);
    }

    fn report_error(
        &self,
        report: &mut MigrationReport,
        short_ref: &str,
        status: AliasStatus,
        error: &str,
        code: Option<&str>,
    ) {
        self.report(report, short_ref, status);
        report.annotate_last_alias(status, error, code);
    }
}

// ── Manifest ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationManifest {
    pub version: String,
    pub entries: BTreeMap<String, ManifestEntry>,
    pub completed: bool,
}

impl MigrationManifest {
    fn entry(
        short_ref: String,
        full_hash: String,
        size: u64,
        migrated_at: u64,
        resumed: bool,
        owner_alias: bool,
    ) -> ManifestEntry {
        ManifestEntry {
            short_ref,
            full_hash,
            size,
            state: EntryState::Migrated,
            migrated_at,
            verified_at: None,
            resumed,
            owner_alias,
        }
    }

    /// Load a manifest from `path`, or return an empty one if missing.
    /// Returns an error if the file exists but is corrupt or has a newer version.
    pub fn load(path: &Path) -> Result<Self, MigrationErrorCode> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mf: Self =
                    serde_json::from_str(&text).map_err(|_| MigrationErrorCode::ManifestCorrupt)?;
                if mf.version.as_str() != MIGRATION_MANIFEST_VERSION {
                    return Err(MigrationErrorCode::ManifestNewerVersion);
                }
                Ok(mf)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Err(MigrationErrorCode::ManifestMissing)
            }
            Err(_) => Err(MigrationErrorCode::ManifestCorrupt),
        }
    }

    fn empty() -> Self {
        Self {
            version: MIGRATION_MANIFEST_VERSION.to_string(),
            entries: BTreeMap::new(),
            completed: false,
        }
    }

    /// Save the manifest to `path` atomically:
    /// write to a unique temp file, sync data, then rename.
    pub fn save(&self, path: &Path) -> Result<(), MigrationErrorCode> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| MigrationErrorCode::ManifestSave)?;
        }
        let text =
            serde_json::to_string_pretty(self).map_err(|_| MigrationErrorCode::ManifestSave)?;
        for attempt in 0..TMP_RETRIES {
            let tmp = tmp_manifest_path(path, attempt);
            if Self::write_tmp_sync(&tmp, &text)
                .and_then(|()| fs::rename(&tmp, path))
                .is_ok()
            {
                return Ok(());
            }
            let _ = fs::remove_file(tmp);
        }
        Err(MigrationErrorCode::ManifestSave)
    }

    fn write_tmp_sync(tmp: &Path, text: &str) -> Result<(), std::io::Error> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tmp)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        #[cfg(unix)]
        {
            let _ = file.sync_data();
        }
        #[cfg(not(unix))]
        {
            let _ = file.sync_all();
        }
        Ok(())
    }

    pub fn contains_hash(&self, short_ref: &str, full_hash: &str) -> bool {
        self.entries
            .get(short_ref)
            .is_some_and(|e| e.full_hash == full_hash)
    }

    pub fn contains_short(&self, short_ref: &str) -> bool {
        self.entries.contains_key(short_ref)
    }
}

fn tmp_manifest_path(manifest: &Path, attempt: usize) -> PathBuf {
    let parent = manifest.parent().unwrap_or_else(|| Path::new("."));
    let name = manifest
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("migration-manifest.json"));
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.{}.tmp", std::process::id(), attempt));
    parent.join(tmp_name)
}

// ── Alias entry (in report) ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AliasEntry {
    pub short_ref: String,
    pub full_ref: String,
    pub size: u64,
    pub status: AliasStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AliasStatus {
    Migrated,
    Skipped,
    Failed,
    Repaired,
    Verified,
}

// ── Operation report ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationReport {
    pub manifest_version: String,
    pub operation: String,
    pub dry_run: bool,
    pub total: usize,
    pub migrated: usize,
    pub skipped: usize,
    pub failed: usize,
    pub repaired: usize,
    pub verified: usize,
    pub aliases: Vec<AliasEntry>,
    pub errors: Vec<MigrationError>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short_ref: Option<String>,
}

impl MigrationReport {
    fn new(operation: &str, dry_run: bool) -> Self {
        Self {
            manifest_version: MIGRATION_MANIFEST_VERSION.to_string(),
            operation: operation.to_string(),
            dry_run,
            total: 0,
            migrated: 0,
            skipped: 0,
            failed: 0,
            repaired: 0,
            verified: 0,
            aliases: Vec::new(),
            errors: Vec::new(),
            timestamp: now_unix(),
        }
    }

    pub fn is_failure(&self) -> bool {
        self.failed > 0 || !self.errors.is_empty()
    }

    fn alias(&mut self, short_ref: &str, full_ref: String, size: u64, status: AliasStatus) {
        self.aliases.push(AliasEntry {
            short_ref: short_ref.to_string(),
            full_ref,
            size,
            status,
            error: None,
            error_code: None,
        });
    }

    fn annotate_last_alias(
        &mut self,
        status: AliasStatus,
        error: impl Into<String>,
        code: Option<&str>,
    ) {
        let entry = self.aliases.last_mut().unwrap();
        entry.status = status;
        entry.error = Some(error.into());
        entry.error_code = code.map(str::to_string);
    }

    fn alias_error(
        &mut self,
        short_ref: &str,
        full_ref: String,
        size: u64,
        status: AliasStatus,
        error: &str,
        code: &str,
    ) {
        self.alias(short_ref, full_ref, size, status);
        self.annotate_last_alias(status, error, Some(code));
    }

    fn record_error(
        &mut self,
        code: impl Into<String>,
        message: impl Into<String>,
        short_ref: Option<String>,
    ) {
        self.errors.push(MigrationError {
            code: code.into(),
            message: message.into(),
            short_ref,
        });
    }

    fn fail(&mut self, code: &str, message: impl Into<String>, short_ref: Option<String>) {
        self.failed += 1;
        self.record_error(code, message, short_ref);
    }

    fn fail_alias(
        &mut self,
        short_ref: &str,
        full_ref: String,
        size: u64,
        code: &str,
        message: impl Into<String>,
    ) {
        let message = message.into();
        self.failed += 1;
        self.alias_error(
            short_ref,
            full_ref,
            size,
            AliasStatus::Failed,
            &message,
            code,
        );
        self.record_error(code, message, Some(short_ref.to_string()));
    }

    fn fail_last_alias(
        &mut self,
        short_ref: &str,
        alias_error: impl Into<String>,
        code: &str,
        message: impl Into<String>,
    ) {
        self.failed += 1;
        self.annotate_last_alias(AliasStatus::Failed, alias_error, Some(code));
        self.record_error(code, message, Some(short_ref.to_string()));
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn to_text(&self) -> String {
        let mut out = format!(
            "Migration {} (dry_run={})\n         ─────────────────────────────────\n         total:    {}\n         migrated: {}\n         skipped:  {}\n         failed:   {}\n         repaired: {}\n         verified: {}\n",
            self.operation,
            self.dry_run,
            self.total,
            self.migrated,
            self.skipped,
            self.failed,
            self.repaired,
            self.verified,
        );
        if !self.aliases.is_empty() {
            out.push_str("\naliases:\n");
            for entry in &self.aliases {
                let st = match entry.status {
                    AliasStatus::Migrated => "migrated",
                    AliasStatus::Skipped => "skipped",
                    AliasStatus::Failed => "failed",
                    AliasStatus::Repaired => "repaired",
                    AliasStatus::Verified => "verified",
                };
                out.push_str(&format!(
                    "  {} → {}  [{}]\n",
                    entry.short_ref, entry.full_ref, st
                ));
                if let Some(err) = &entry.error {
                    out.push_str(&format!("    error: {err}\n"));
                }
            }
        }
        if !self.errors.is_empty() {
            out.push_str("\nerrors:\n");
            for err in &self.errors {
                out.push_str(&format!("  [{}] {}\n", err.code, err.message));
            }
        }
        out
    }
}

// ── Migration engine ──────────────────────────────────────────────────────

#[derive(Debug)]
pub enum BlobContentResult {
    Ok(Vec<u8>),
    Missing,
    Corrupt,
}

/// Trait abstracting the RecoveryStore operations needed by migration.
pub trait MigrationStore {
    fn blob_ref_ids(&self) -> Vec<String>;
    fn resolve_blob_bytes(&self, ref_id: &str) -> BlobContentResult;
    fn alias_target(&self, alias: &str) -> Option<String>;
    fn store_alias_deferred(&mut self, alias: &str, target: &str);
    fn remove_alias(&mut self, alias: &str);
    fn remove_blob(&mut self, ref_id: &str);
    fn mark_ambiguous(&mut self, short_ref: &str);
    fn is_ambiguous(&self, short_ref: &str) -> bool;
    fn persist_pending(&mut self) -> Result<(), String>;
}

#[derive(Clone, Copy)]
enum IntegrityIssue {
    SourceMismatch,
    SourceCorrupt,
    CasMissing,
    CasMismatch,
    CasRead,
    AliasConflict,
    AliasMissing,
}

impl IntegrityIssue {
    fn verify_details(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::SourceMismatch => (
                "legacy source hash/size mismatch with manifest",
                "source-corrupt",
                "legacy source hash/size mismatch",
            ),
            Self::SourceCorrupt => (
                "legacy source corrupt",
                "source-corrupt",
                "legacy source corrupt",
            ),
            Self::CasMissing => ("CAS object missing", "cas-missing", "CAS object missing"),
            Self::CasMismatch => (
                "CAS hash/size mismatch",
                "cas-corruption",
                "CAS hash/size mismatch",
            ),
            Self::CasRead => ("CAS read failure", "cas-corruption", "CAS read failure"),
            Self::AliasConflict => (
                "alias targets wrong ref",
                "alias-conflict",
                "alias mismatch",
            ),
            Self::AliasMissing => ("alias missing from store", "alias-missing", "alias missing"),
        }
    }

    fn cleanup_details(self) -> (&'static str, &'static str) {
        match self {
            Self::SourceMismatch | Self::SourceCorrupt => {
                ("source-corrupt", "source hash/size mismatch")
            }
            Self::CasMissing => ("cas-missing", "CAS object missing"),
            Self::CasMismatch | Self::CasRead => ("cas-corruption", "CAS hash/size mismatch"),
            Self::AliasConflict | Self::AliasMissing => {
                ("alias-missing", "alias missing or mismatch")
            }
        }
    }
}

struct MigrationCandidate {
    content: Vec<u8>,
    full_hash: String,
    full_ref: String,
    size: u64,
}

impl MigrationCandidate {
    fn new(content: Vec<u8>) -> Self {
        let size = content.len() as u64;
        let full_hash = full_sha256_hex(&content);
        let full_ref = format!("{BLOB_REF_PREFIX}{full_hash}");
        Self {
            content,
            full_hash,
            full_ref,
            size,
        }
    }

    fn alias(&self, report: &mut MigrationReport, short_ref: &str, status: AliasStatus) {
        report.alias(short_ref, self.full_ref.clone(), self.size, status);
    }

    fn alias_error(
        &self,
        report: &mut MigrationReport,
        short_ref: &str,
        status: AliasStatus,
        error: &str,
        code: &str,
    ) {
        report.alias_error(
            short_ref,
            self.full_ref.clone(),
            self.size,
            status,
            error,
            code,
        );
    }

    fn fail(&self, report: &mut MigrationReport, short_ref: &str, code: &str, message: String) {
        report.fail_alias(short_ref, self.full_ref.clone(), self.size, code, message);
    }

    fn manifest_entry(&self, short_ref: &str, resumed: bool) -> ManifestEntry {
        MigrationManifest::entry(
            short_ref.to_string(),
            self.full_hash.clone(),
            self.size,
            now_unix(),
            resumed,
            true,
        )
    }
}

pub struct LegacyMigration<'a> {
    store: &'a mut dyn MigrationStore,
    cas: &'a SharedCas,
    manifest_path: Option<PathBuf>,
}

impl<'a> LegacyMigration<'a> {
    pub fn new(
        store: &'a mut dyn MigrationStore,
        cas: &'a SharedCas,
        manifest_path: Option<PathBuf>,
    ) -> Self {
        Self {
            store,
            cas,
            manifest_path,
        }
    }

    pub fn run(&mut self, dry_run: bool) -> MigrationReport {
        let mut report = MigrationReport::new("migrate", dry_run);
        let manifest = match self.manifest_path.as_ref() {
            Some(p) => match MigrationManifest::load(p) {
                Ok(mf) => mf,
                Err(MigrationErrorCode::ManifestMissing) => MigrationManifest::empty(),
                Err(MigrationErrorCode::ManifestNewerVersion) => {
                    report.record_error(
                        "manifest-newer-version",
                        format!(
                            "manifest version is newer than supported ({})",
                            MIGRATION_MANIFEST_VERSION
                        ),
                        None,
                    );
                    return report;
                }
                Err(_) => {
                    report.record_error(
                        "manifest-corrupt",
                        "manifest file is corrupt, cannot continue",
                        None,
                    );
                    return report;
                }
            },
            None => MigrationManifest::empty(),
        };

        let blob_refs = self.store.blob_ref_ids();
        let legacy_refs: Vec<String> = blob_refs
            .into_iter()
            .filter(|r| is_legacy_blob_ref(r))
            .collect();

        report.total = legacy_refs.len();

        let mut updated_manifest = manifest.clone();
        let mut marked_ambiguous = false;

        // First pass: detect genuine ambiguous prefixes (two different blobs
        // with the same short ref).
        let mut short_to_hashes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for short_ref in &legacy_refs {
            if !self.store.is_ambiguous(short_ref)
                && let BlobContentResult::Ok(bytes) = self.store.resolve_blob_bytes(short_ref)
            {
                short_to_hashes
                    .entry(short_ref.clone())
                    .or_default()
                    .insert(full_sha256_hex(&bytes));
            }
        }

        // Detect true collisions: same short_ref, different full hashes.
        for (short_ref, hashes) in &short_to_hashes {
            if hashes.len() > 1 {
                if !dry_run {
                    self.store.mark_ambiguous(short_ref);
                    marked_ambiguous = true;
                }
                report.record_error(
                    "ambiguous-short-id",
                    format!(
                        "{short_ref}: short prefix maps to {} distinct full hashes",
                        hashes.len()
                    ),
                    Some(short_ref.clone()),
                );
            }
        }

        for short_ref in &legacy_refs {
            self.migrate_one(
                short_ref,
                &mut report,
                &manifest,
                &mut updated_manifest,
                dry_run,
            );
        }

        // Persist store changes and manifest.
        if !dry_run && (report.migrated > 0 || report.repaired > 0 || marked_ambiguous) {
            let mut write_failed = false;

            if let Err(err) = self.store.persist_pending() {
                write_failed = true;
                for entry in &mut report.aliases {
                    if entry.status == AliasStatus::Migrated
                        || entry.status == AliasStatus::Repaired
                    {
                        entry.status = AliasStatus::Failed;
                        entry.error = Some(format!("store persist failed: {err}"));
                        entry.error_code = Some("store-persist".to_string());
                    }
                }
                report.failed += report.migrated + report.repaired;
                report.migrated = 0;
                report.repaired = 0;
                report.record_error(
                    "store-persist",
                    format!("store persist failed: {err}"),
                    None,
                );
            }

            if let Some(path) = &self.manifest_path {
                updated_manifest.completed = report.failed == 0 && !write_failed;
                if let Err(err) = updated_manifest.save(path) {
                    report.record_error(
                        "manifest-save",
                        format!("manifest save failed: {err}"),
                        None,
                    );
                }
            }
        }

        report
    }

    fn cas_ok(&self, full_hash: &str, size: u64) -> bool {
        self.cas.contains(full_hash)
            && matches!(
                self.cas.resolve(full_hash),
                Ok(b) if full_sha256_hex(&b) == full_hash && b.len() as u64 == size
            )
    }

    fn fail_cas_io(report: &mut MigrationReport, short_ref: &str, item: &MigrationCandidate) {
        item.fail(
            report,
            short_ref,
            "cas-io",
            format!("{short_ref}: CAS republish failed"),
        );
        report.aliases.last_mut().unwrap().error = Some("CAS republish failed".to_string());
    }

    fn resume_entry(existing: &ManifestEntry, short_ref: &str, updated: &mut MigrationManifest) {
        let mut entry = existing.clone();
        entry.resumed = true;
        entry.owner_alias = true;
        updated.entries.insert(short_ref.to_string(), entry);
    }

    fn republish_repair(
        &mut self,
        report: &mut MigrationReport,
        short_ref: &str,
        item: &MigrationCandidate,
        existing: &ManifestEntry,
        updated: &mut MigrationManifest,
        realias: bool,
    ) {
        if self.cas.publish(&item.content).is_err() {
            Self::fail_cas_io(report, short_ref, item);
            return;
        }
        if realias {
            self.store.store_alias_deferred(short_ref, &item.full_ref);
        }
        report.repaired += 1;
        item.alias(report, short_ref, AliasStatus::Repaired);
        Self::resume_entry(existing, short_ref, updated);
    }

    fn migrate_one(
        &mut self,
        short_ref: &str,
        report: &mut MigrationReport,
        manifest: &MigrationManifest,
        updated_manifest: &mut MigrationManifest,
        dry_run: bool,
    ) {
        if self.store.is_ambiguous(short_ref) {
            // Alias-row failure only (no top-level errors entry — historical contract).
            report.failed += 1;
            report.alias_error(
                short_ref,
                String::new(),
                0,
                AliasStatus::Failed,
                "short ref is ambiguous (maps to multiple full hashes)",
                "ambiguous-short-id",
            );
            return;
        }
        let content = match self.store.resolve_blob_bytes(short_ref) {
            BlobContentResult::Ok(bytes) => bytes,
            BlobContentResult::Missing => {
                report.fail_alias(
                    short_ref,
                    String::new(),
                    0,
                    "source-missing",
                    format!("{short_ref}: could not resolve blob content"),
                );
                return;
            }
            BlobContentResult::Corrupt => {
                report.fail_alias(
                    short_ref,
                    String::new(),
                    0,
                    "source-corrupt",
                    format!("{short_ref}: blob content is empty or corrupt"),
                );
                return;
            }
        };
        let item = MigrationCandidate::new(content);
        if let Some(short_hex) = short_id_hex(short_ref)
            && &item.full_hash[..16] != short_hex
        {
            item.fail(
                    report, short_ref, "ambiguous-short-id",
                    format!(
                        "{short_ref}: ambiguous short ID — short prefix {short_hex} does not match full hash prefix {}",
                        &item.full_hash[..16],
                    ),
                );
            return;
        }

        if let Some(existing) = manifest.entries.get(short_ref) {
            if existing.full_hash != item.full_hash || existing.size != item.size {
                item.fail(
                    report, short_ref, "manifest-hash-conflict",
                    format!("{short_ref}: manifest hash conflict — manifest entry differs from computed hash/size"),
                );
                return;
            }
            let cas_ok = self.cas_ok(&item.full_hash, item.size);
            let needs_alias = self.store.alias_target(short_ref).is_none();
            if !cas_ok {
                if dry_run {
                    report.repaired += 1;
                    let (message, code) = if needs_alias {
                        ("alias missing — would repair", "alias-missing")
                    } else {
                        ("CAS missing — would republish", "cas-missing")
                    };
                    item.alias_error(report, short_ref, AliasStatus::Repaired, message, code);
                } else {
                    self.republish_repair(
                        report,
                        short_ref,
                        &item,
                        existing,
                        updated_manifest,
                        needs_alias,
                    );
                }
                return;
            }
            if needs_alias {
                if dry_run {
                    report.repaired += 1;
                    item.alias_error(
                        report,
                        short_ref,
                        AliasStatus::Repaired,
                        "alias missing — would repair",
                        "alias-missing",
                    );
                } else {
                    self.store.store_alias_deferred(short_ref, &item.full_ref);
                    report.repaired += 1;
                    item.alias(report, short_ref, AliasStatus::Repaired);
                    Self::resume_entry(existing, short_ref, updated_manifest);
                }
                return;
            }
            report.skipped += 1;
            item.alias(report, short_ref, AliasStatus::Skipped);
            if !dry_run && !updated_manifest.entries.contains_key(short_ref) {
                updated_manifest
                    .entries
                    .insert(short_ref.to_string(), existing.clone());
            }
            return;
        }

        if let Some(existing_target) = self.store.alias_target(short_ref) {
            if existing_target != item.full_ref {
                item.fail(
                    report, short_ref, "alias-conflict",
                    format!(
                        "{short_ref}: alias conflict — existing alias targets {existing_target}, but content hashes to {}",
                        item.full_ref,
                    ),
                );
                return;
            }
            if !self.cas_ok(&item.full_hash, item.size) {
                if dry_run {
                    report.repaired += 1;
                    item.alias_error(
                        report,
                        short_ref,
                        AliasStatus::Repaired,
                        "CAS missing — would republish",
                        "cas-missing",
                    );
                    return;
                }
                if self.cas.publish(&item.content).is_err() {
                    Self::fail_cas_io(report, short_ref, &item);
                    return;
                }
            }
            if !manifest.contains_hash(short_ref, &item.full_hash) && !dry_run {
                updated_manifest
                    .entries
                    .insert(short_ref.to_string(), item.manifest_entry(short_ref, true));
                report.repaired += 1;
                item.alias(report, short_ref, AliasStatus::Repaired);
            } else {
                report.skipped += 1;
                item.alias(report, short_ref, AliasStatus::Skipped);
            }
            return;
        }

        if dry_run {
            report.migrated += 1;
            item.alias(report, short_ref, AliasStatus::Migrated);
            return;
        }
        match self.cas.publish(&item.content) {
            Ok(published_hash) => debug_assert_eq!(published_hash, item.full_hash),
            Err(SharedCasError::Corruption) => {
                item.fail(
                    report,
                    short_ref,
                    "cas-corruption",
                    format!(
                        "{short_ref}: CAS corruption — object {} exists with different bytes",
                        item.full_hash,
                    ),
                );
                return;
            }
            Err(SharedCasError::Policy) => {
                item.fail(
                    report,
                    short_ref,
                    "cas-policy",
                    format!("{short_ref}: CAS policy violation"),
                );
                return;
            }
            Err(_) => {
                item.fail(
                    report,
                    short_ref,
                    "cas-io",
                    format!("{short_ref}: CAS publish failed"),
                );
                return;
            }
        }
        self.store.store_alias_deferred(short_ref, &item.full_ref);
        updated_manifest
            .entries
            .insert(short_ref.to_string(), item.manifest_entry(short_ref, false));
        report.migrated += 1;
        item.alias(report, short_ref, AliasStatus::Migrated);
    }

    /// Verify migration integrity: checks every manifest entry's CAS object
    /// hash+size against the manifest and the exact alias target in the store.
    /// Also hash/size-checks the legacy source blob when present, ensuring the
    fn load_manifest(&self, report: &mut MigrationReport) -> Option<MigrationManifest> {
        match self.manifest_path.as_ref() {
            Some(p) => match MigrationManifest::load(p) {
                Ok(mf) => Some(mf),
                Err(MigrationErrorCode::ManifestMissing) => {
                    report.record_error(
                        "manifest-missing",
                        "migration manifest does not exist",
                        None,
                    );
                    None
                }
                Err(MigrationErrorCode::ManifestNewerVersion) => {
                    report.record_error(
                        "manifest-newer-version",
                        "manifest version is newer than supported",
                        None,
                    );
                    None
                }
                Err(_) => {
                    report.record_error("manifest-corrupt", "manifest is corrupt", None);
                    None
                }
            },
            None => {
                report.record_error("manifest-missing", "no manifest path configured", None);
                None
            }
        }
    }

    /// source, CAS, and alias are all consistent. Redacts underlying storage
    /// errors from report messages.
    fn entry_bytes_match(bytes: &[u8], entry: &ManifestEntry) -> bool {
        full_sha256_hex(bytes) == entry.full_hash && bytes.len() as u64 == entry.size
    }

    fn integrity(
        &self,
        short_ref: &str,
        entry: &ManifestEntry,
        require_source: bool,
    ) -> Result<(), IntegrityIssue> {
        match self.store.resolve_blob_bytes(short_ref) {
            BlobContentResult::Ok(bytes) if Self::entry_bytes_match(&bytes, entry) => {}
            BlobContentResult::Ok(_) => return Err(IntegrityIssue::SourceMismatch),
            BlobContentResult::Missing if !require_source => {}
            BlobContentResult::Missing | BlobContentResult::Corrupt => {
                return Err(IntegrityIssue::SourceCorrupt);
            }
        }
        if !self.cas.contains(&entry.full_hash) {
            return Err(IntegrityIssue::CasMissing);
        }
        match self.cas.resolve(&entry.full_hash) {
            Ok(bytes) if Self::entry_bytes_match(&bytes, entry) => {}
            Ok(_) => return Err(IntegrityIssue::CasMismatch),
            Err(_) => return Err(IntegrityIssue::CasRead),
        }
        match self.store.alias_target(short_ref) {
            Some(target) if target == format!("{BLOB_REF_PREFIX}{}", entry.full_hash) => Ok(()),
            Some(_) => Err(IntegrityIssue::AliasConflict),
            None => Err(IntegrityIssue::AliasMissing),
        }
    }

    fn report_integrity(report: &mut MigrationReport, short_ref: &str, issue: IntegrityIssue) {
        let (alias_error, code, message) = issue.verify_details();
        report.fail_last_alias(
            short_ref,
            alias_error,
            code,
            format!("{short_ref}: {message}"),
        );
    }

    pub fn verify(&self) -> MigrationReport {
        let mut report = MigrationReport::new("verify", false);
        let Some(manifest) = self.load_manifest(&mut report) else {
            return report;
        };
        report.total = manifest.entries.len();
        for (short_ref, entry) in &manifest.entries {
            entry.report(&mut report, short_ref, AliasStatus::Verified);
            if let Err(issue) = self.integrity(short_ref, entry, false) {
                Self::report_integrity(&mut report, short_ref, issue);
                continue;
            }
            report.verified += 1;
        }
        report
    }

    /// Rollback: remove migration-created aliases and manifest file.
    /// Never touches CAS bytes or source blobs.
    pub fn rollback(&mut self, apply: bool) -> MigrationReport {
        let dry_run = !apply;
        let mut report = MigrationReport::new("rollback", dry_run);
        let Some(manifest) = self.load_manifest(&mut report) else {
            return report;
        };

        report.total = manifest.entries.len();

        for (short_ref, entry) in &manifest.entries {
            // Verify legacy source hash+size match manifest before removing alias.
            // Only remove aliases that were created by migration (owner_alias).
            let source_verified = matches!(
                self.store.resolve_blob_bytes(short_ref),
                BlobContentResult::Ok(bytes) if Self::entry_bytes_match(&bytes, entry)
            );

            if !source_verified {
                report.fail_alias(
                    short_ref,
                    format!("{BLOB_REF_PREFIX}{}", entry.full_hash),
                    entry.size,
                    "rollback-source-gone",
                    format!("{short_ref}: legacy source hash/size mismatch, cannot verify rollback safety"),
                );
                continue;
            }

            // Only remove aliases known to have been created by migration.
            if !entry.owner_alias {
                report.skipped += 1;
                entry.report_error(
                    &mut report,
                    short_ref,
                    AliasStatus::Skipped,
                    "alias was not created by migration, skipping",
                    None,
                );
                continue;
            }

            if apply {
                self.store.remove_alias(short_ref);
            }
            report.migrated += 1;
            entry.report(&mut report, short_ref, AliasStatus::Migrated);
        }

        // Persist alias removals successfully before deleting manifest.
        if apply && report.failed == 0 {
            if let Err(err) = self.store.persist_pending() {
                report.record_error(
                    "store-persist",
                    format!("persist failed: rollback incomplete: {err}"),
                    None,
                );
                // Don't delete manifest if persist failed
                return report;
            }
            if let Some(path) = &self.manifest_path
                && path.exists()
            {
                let _ = fs::remove_file(path);
            }
        }

        report
    }

    /// Cleanup: remove legacy source payloads after successful verification.
    /// Dry-run by default; requires --apply and --confirm-cleanup.
    /// Verifies source+CAS+alias exactly before removing. Marks blob tombstones.
    /// Treats persist failure as failure. Never deletes CAS.
    pub fn cleanup(&mut self, apply: bool, confirmed: bool) -> MigrationReport {
        let dry_run = !apply;
        let mut report = MigrationReport::new("cleanup", dry_run);

        // Dry-run (`!apply`) does not require --confirm-cleanup. Apply without
        // confirm still refuses before verify / manifest load.
        if apply && !confirmed {
            report.record_error(
                "cleanup-confirmation-required",
                "cleanup requires --confirm-cleanup flag",
                None,
            );
            return report;
        }

        let verify_report = self.verify();
        if verify_report.is_failure() {
            report.record_error(
                "cleanup-needs-verification",
                "cleanup requires successful verification first",
                None,
            );
            if apply {
                report.errors.extend(verify_report.errors);
            }
            return report;
        }

        // Do not reuse `load_manifest`: cleanup maps every load Err (including
        // missing/newer-version) to `manifest-corrupt` when a path is set.
        let Some(path) = self.manifest_path.as_ref() else {
            report.record_error("manifest-missing", "no manifest path configured", None);
            return report;
        };
        let manifest = match MigrationManifest::load(path) {
            Ok(mf) => mf,
            Err(_) => {
                report.record_error("manifest-corrupt", "manifest is corrupt", None);
                return report;
            }
        };

        report.total = manifest.entries.len();

        for (short_ref, entry) in &manifest.entries {
            if !apply {
                // Dry-run: report planned removals without mutation.
                report.migrated += 1;
                entry.report(&mut report, short_ref, AliasStatus::Migrated);
                continue;
            }

            if let Err(issue) = self.integrity(short_ref, entry, true) {
                let (code, message) = issue.cleanup_details();
                report.fail(
                    code,
                    format!("{short_ref}: {message}"),
                    Some(short_ref.clone()),
                );
                continue;
            }
            self.store.remove_blob(short_ref);
            report.migrated += 1;
            entry.report(&mut report, short_ref, AliasStatus::Migrated);
        }

        // Treat persist failure as failure. Never delete CAS.
        if apply
            && report.migrated > 0
            && let Err(err) = self.store.persist_pending()
        {
            report.failed += report.migrated;
            report.migrated = 0;
            report.record_error(
                "store-persist",
                format!("persist failed: cleanup incomplete: {err}"),
                None,
            );
        }

        report
    }
} // ── RecoveryStore adapter ─────────────────────────────────────────────────

/// Adapter that wraps a `RecoveryStore` to implement `MigrationStore`.
pub struct RecoveryStoreAdapter<'a> {
    store: &'a mut crate::RecoveryStore,
}

impl<'a> RecoveryStoreAdapter<'a> {
    pub fn new(store: &'a mut crate::RecoveryStore) -> Self {
        Self { store }
    }
}

impl MigrationStore for RecoveryStoreAdapter<'_> {
    fn blob_ref_ids(&self) -> Vec<String> {
        crate::RecoveryStore::blob_ref_ids(self.store)
    }

    fn resolve_blob_bytes(&self, ref_id: &str) -> BlobContentResult {
        match crate::RecoveryStore::resolve_blob_content(self.store, ref_id) {
            Some(text) => {
                let bytes = text.into_bytes();
                if bytes.is_empty() {
                    BlobContentResult::Corrupt
                } else {
                    BlobContentResult::Ok(bytes)
                }
            }
            None => BlobContentResult::Missing,
        }
    }

    fn alias_target(&self, alias: &str) -> Option<String> {
        crate::RecoveryStore::alias_target(self.store, alias)
    }

    fn store_alias_deferred(&mut self, alias: &str, target: &str) {
        crate::RecoveryStore::store_alias_deferred(self.store, alias, target);
    }

    fn remove_alias(&mut self, alias: &str) {
        crate::RecoveryStore::remove_alias(self.store, alias);
    }

    fn remove_blob(&mut self, ref_id: &str) {
        crate::RecoveryStore::remove_blob(self.store, ref_id);
    }

    fn mark_ambiguous(&mut self, short_ref: &str) {
        crate::RecoveryStore::mark_ambiguous(self.store, short_ref);
    }

    fn is_ambiguous(&self, short_ref: &str) -> bool {
        crate::RecoveryStore::is_alias_ambiguous(self.store, short_ref)
    }

    fn persist_pending(&mut self) -> Result<(), String> {
        crate::RecoveryStore::persist_pending(self.store).map_err(|e| e.to_string())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────
