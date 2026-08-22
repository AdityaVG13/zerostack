use flate2::read::{DeflateDecoder, MultiGzDecoder};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::{borrow::Cow, collections::HashSet, fs};
const MAX_ARCHIVE_MEMBERS: usize = 4096;
const MAX_NESTED_ARCHIVE_DEPTH: usize = 3;
pub const MAX_TOP_LEVEL_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_NESTED_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;
const MAX_GZIP_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES: usize = 256 * 1024 * 1024;
pub const ZIP_FLAG_ENCRYPTED: u16 = 0x0001;
pub const ZIP_FLAG_DATA_DESCRIPTOR: u16 = 0x0008;
const ZIP_FLAG_STRONG_ENCRYPTION: u16 = 0x0040;
const ZIP_FLAG_MASKED_LOCAL_HEADER_VALUES: u16 = 0x2000;
pub const ZIP_DATA_DESCRIPTOR_SIGNATURE: u32 = 0x0807_4b50;
pub const ZIP64_EOCD_RECORD_SIGNATURE: u32 = 0x0606_4b50;
pub const ZIP64_EOCD_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
pub const ZIP64_EXTENDED_INFORMATION_EXTRA: u16 = 0x0001;
type Issues = Vec<serde_json::Value>;
pub fn package_audit(root: &Path, artifacts: &[PathBuf]) -> serde_json::Value {
    let defaults = [
        root.join("Cargo.toml"),
        root.join("package/npm/package.json"),
        root.join("packaging/homebrew/tokenzero.rb"),
    ];
    let candidates: &[PathBuf] = if artifacts.is_empty() {
        &defaults
    } else {
        artifacts
    };
    let mut issues = Vec::new();
    let mut checked = 0;
    for path in candidates.iter().filter(|path| path.exists()) {
        checked += 1;
        audit_artifact(path, &mut issues);
    }
    serde_json::json!({
        "schema_version": "tokenzero.package_audit.v1",
        "status": if issues.is_empty() { "ok" } else { "blocked" },
        "ok": issues.is_empty(),
        "archives_checked": checked,
        "issue_count": issues.len(),
        "issues": issues,
        "external_runtime_required_for_core": false
    })
}
fn audit_artifact(path: &Path, issues: &mut Issues) {
    let display = path.display().to_string();
    audit_public_member_name(&display, &display, false, false, issues);
    if let Some(members) = archive_members(path, issues) {
        audit_archive_members(&display, members, 0, issues);
        return;
    }
    if !is_supported_archive_name(
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default(),
    ) && let Ok(text) = fs::read_to_string(path)
    {
        audit_release_text(
            &path.display().to_string(),
            None,
            &text,
            path.to_string_lossy().as_ref(),
            issues,
        );
    }
}
fn audit_release_text(
    artifact: &str,
    member: Option<&str>,
    text: &str,
    public_path: &str,
    issues: &mut Issues,
) {
    let lower = text.to_ascii_lowercase();
    let normalized_text = lower.replace('\\', "/");
    let normalized_path = public_path.to_ascii_lowercase().replace('\\', "/");
    let leaf = normalized_path.rsplit('/').next().unwrap_or_default();
    let findings = [
        (
            [["py", "thon "], ["uv", " run"], ["pip", " install"]]
                .into_iter()
                .any(|parts| lower.contains(&parts.concat())),
            "external_runtime_dependency",
            "artifact references a non-Rust runtime",
            "archive executable/script member references a non-Rust runtime",
        ),
        (
            (lower.starts_with("@echo off")
                || lower.starts_with("#!/bin/sh")
                || leaf == "tokenzero.cmd"
                || normalized_path.ends_with("/.tokenzero/bin/tokenzero"))
                && normalized_text.contains("target/release/tokenzero"),
            "dev_runtime_launcher",
            "launcher points at a development target/release binary",
            "archive executable/script member points at a development target/release binary",
        ),
        (
            ["raw_traces", "lab_notes", "local_only"]
                .iter()
                .any(|marker| lower.contains(marker)),
            "non_release_artifact_reference",
            "artifact references non-release material",
            "archive executable/script member references non-release material",
        ),
    ];
    for (_, code, artifact_detail, member_detail) in findings.into_iter().filter(|f| f.0) {
        let mut issue = serde_json::json!({
            "code": code,
            "path": artifact,
            "detail": if member.is_some() { member_detail } else { artifact_detail }
        });
        if let Some(member) = member {
            issue["member"] = member.into();
        }
        issues.push(issue);
    }
}
fn push_archive_issue(issues: &mut Issues, code: &str, path: &str, detail: impl Into<String>) {
    issues.push(serde_json::json!({ "code": code, "path": path, "detail": detail.into() }));
}
fn audit_archive_members(
    artifact: &str,
    members: Vec<ArchiveMember>,
    depth: usize,
    issues: &mut Issues,
) {
    for member in members {
        audit_public_member_name(
            artifact,
            &member.name,
            true,
            matches!(member.kind, ArchiveMemberKind::Directory),
            issues,
        );
        if let Some(target) = member.link_target.as_deref() {
            audit_public_link_target(artifact, &member.name, target, member.kind, issues);
        }
        let Some(bytes) = member.nested_archive.as_deref() else {
            continue;
        };
        let violation = if bytes.len() > MAX_NESTED_ARCHIVE_BYTES {
            Some((
                "nested_archive_too_large",
                "nested archive exceeds the package-audit in-memory inspection limit",
            ))
        } else if depth >= MAX_NESTED_ARCHIVE_DEPTH {
            Some((
                "nested_archive_depth_exceeded",
                "nested archive exceeds the package-audit recursion limit",
            ))
        } else {
            None
        };
        if let Some((code, detail)) = violation {
            issues.push(serde_json::json!({ "code": code, "path": artifact, "member": member.name, "detail": detail }));
            continue;
        }
        let nested_artifact = format!("{artifact}!{}", member.name);
        if let Some(nested) =
            archive_members_from_bytes(&member.name, bytes, &nested_artifact, issues)
        {
            audit_archive_members(&nested_artifact, nested, depth + 1, issues);
        }
    }
}
#[derive(Clone, Copy)]
enum ArchiveMemberKind {
    Path,
    Directory,
    Hardlink,
    Symlink,
}
impl ArchiveMemberKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Directory => "directory",
            Self::Hardlink => "hardlink",
            Self::Symlink => "symlink",
        }
    }
}
struct ArchiveMember {
    name: String,
    kind: ArchiveMemberKind,
    link_target: Option<String>,
    nested_archive: Option<Vec<u8>>,
}
fn archive_members(path: &Path, issues: &mut Issues) -> Option<Vec<ArchiveMember>> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !is_supported_archive_name(name) {
        return None;
    }
    let artifact = path.display().to_string();
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            push_archive_issue(
                issues,
                "archive_member_listing_failed",
                &artifact,
                format!("failed to stat archive: {error}"),
            );
            return None;
        }
    };
    if metadata.len() > MAX_TOP_LEVEL_ARCHIVE_BYTES {
        issues.push(serde_json::json!({ "code": "archive_file_too_large", "path": artifact, "size": metadata.len(), "limit": MAX_TOP_LEVEL_ARCHIVE_BYTES, "detail": "top-level archive exceeds the package-audit read budget; package-audit fails closed before loading it into memory" }));
        return None;
    }
    match fs::read(path) {
        Ok(bytes) => archive_members_from_bytes(name, &bytes, &artifact, issues),
        Err(error) => {
            push_archive_issue(
                issues,
                "archive_member_listing_failed",
                &artifact,
                format!("failed to read archive: {error}"),
            );
            None
        }
    }
}
fn archive_members_from_bytes(
    name: &str,
    bytes: &[u8],
    artifact: &str,
    issues: &mut Issues,
) -> Option<Vec<ArchiveMember>> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".tar") {
        return Some(parse_tar_members(bytes, artifact, issues));
    }
    if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".crate") {
        return match gzip_decompress_bytes(bytes) {
            Ok(data) => Some(parse_tar_members(&data, artifact, issues)),
            Err(ArchivePayloadError::TooLarge) => {
                push_archive_issue(
                    issues,
                    "archive_member_listing_too_large",
                    artifact,
                    "gzip archive expands beyond the package-audit decompression limit",
                );
                None
            }
            Err(ArchivePayloadError::Malformed(error)) => {
                push_archive_issue(
                    issues,
                    "archive_member_listing_unavailable",
                    artifact,
                    format!("gzip archive member listing failed: {error}"),
                );
                None
            }
        };
    }
    if lower.ends_with(".zip") {
        return match parse_zip_members(bytes, artifact, issues) {
            Ok(members) => Some(members),
            Err(error) => {
                push_archive_issue(issues, "archive_member_listing_failed", artifact, error);
                None
            }
        };
    }
    None
}
fn is_supported_archive_name(name: &str) -> bool {
    [".tar", ".tar.gz", ".tgz", ".crate", ".zip"]
        .iter()
        .any(|suffix| name.to_ascii_lowercase().ends_with(suffix))
}
pub enum ArchivePayloadError {
    TooLarge,
    Malformed(String),
}
struct ZipPayloadBudget {
    remaining: usize,
    exhausted: bool,
}
impl ZipPayloadBudget {
    fn new() -> Self {
        Self {
            remaining: MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES,
            exhausted: false,
        }
    }
    fn consume(&mut self, artifact: &str, member: &str, size: usize, issues: &mut Issues) -> bool {
        if self.exhausted {
            return false;
        }
        if let Some(remaining) = self.remaining.checked_sub(size) {
            self.remaining = remaining;
            return true;
        }
        self.exhausted = true;
        issues.push(serde_json::json!({ "code": "zip_total_payload_size_exceeded", "path": artifact, "member": member, "uncompressed_size": size, "limit": MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES, "detail": "zip archive aggregate uncompressed payload size exceeds the package-audit budget; package-audit fails closed" }));
        false
    }
}
fn gzip_decompress_bytes(bytes: &[u8]) -> Result<Vec<u8>, ArchivePayloadError> {
    read_bounded_decoder(MultiGzDecoder::new(bytes), MAX_GZIP_DECOMPRESSED_BYTES)
}
pub fn deflate_decompress_bytes(bytes: &[u8]) -> Result<Vec<u8>, ArchivePayloadError> {
    read_bounded_decoder(DeflateDecoder::new(bytes), MAX_NESTED_ARCHIVE_BYTES)
}
fn read_bounded_decoder<R: Read>(decoder: R, max: usize) -> Result<Vec<u8>, ArchivePayloadError> {
    let mut output = Vec::new();
    decoder
        .take(max.saturating_add(1) as u64)
        .read_to_end(&mut output)
        .map_err(|error| ArchivePayloadError::Malformed(error.to_string()))?;
    (output.len() <= max)
        .then_some(output)
        .ok_or(ArchivePayloadError::TooLarge)
}
mod paths;
mod tar;
pub mod zip;
use paths::*;
use tar::*;
use zip::*;
