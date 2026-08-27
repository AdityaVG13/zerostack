use super::*;
/// What a public archive path may not contain. `audit_public_member_name`
/// and `audit_public_link_target` share this walk and map each finding to
/// their own issue codes/details.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathClassification {
    PrivateToolState,
    NonPublicDotdir,
    Sensitive,
    LocalGenerated,
}

macro_rules! classification_issues {
    ($($member_code:literal, $member_detail:literal;
       $link_code:literal, $link_detail:literal;)*) => {
        [$( (($member_code, $member_detail), ($link_code, $link_detail)) ),*]
    };
}
type ClassificationIssue = ((&'static str, &'static str), (&'static str, &'static str));
const CLASSIFICATION_ISSUES: [ClassificationIssue; 4] = classification_issues! {
    "private_tool_state_member", "archive includes private local AI/tool state";
        "private_tool_state_link_target", "archive link target points at private local AI/tool state";
    "non_public_dotdir_member", "archive includes a non-allowlisted dot directory";
        "non_public_dotdir_link_target", "archive link target points at a non-allowlisted dot directory";
    "sensitive_member_name", "archive or artifact member name looks credential-bearing";
        "sensitive_link_target", "archive link target looks credential-bearing";
    "local_generated_member", "archive includes local database, backup, dump, or generated metadata";
        "local_generated_link_target", "archive link target points at local database, backup, dump, or generated metadata";
};
impl PathClassification {
    fn issue(self, link: bool) -> (&'static str, &'static str) {
        let pair = CLASSIFICATION_ISSUES[self as usize];
        if link { pair.1 } else { pair.0 }
    }
}

/// Normalize backslashes, split into non-empty parts, and lowercase the leaf.
fn split_normalized(normalized: &str) -> Vec<&str> {
    normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect()
}

fn classify_public_path(
    parts: &[&str],
    leaf_is_directory: bool,
    leaf: &str,
    mut found: impl FnMut(PathClassification),
) {
    for (index, part) in parts.iter().enumerate() {
        let is_leaf = index + 1 == parts.len();
        if is_private_tool_dotdir(part) {
            found(PathClassification::PrivateToolState);
            break;
        }
        if (!is_leaf || leaf_is_directory) && part.starts_with('.') && !is_public_dotdir(part) {
            found(PathClassification::NonPublicDotdir);
            break;
        }
    }
    if is_sensitive_member_leaf(leaf) {
        found(PathClassification::Sensitive);
    }
    if is_local_generated_member_leaf(leaf) {
        found(PathClassification::LocalGenerated);
    }
}
pub(crate) fn audit_public_member_name(
    artifact: &str,
    member: &str,
    check_path_escape: bool,
    member_is_directory: bool,
    issues: &mut Vec<serde_json::Value>,
) {
    if let Some(reason) = archive_path_control_reason(member) {
        push_archive_member_name_uninspectable(artifact, member, reason, issues);
    }
    let mut normalized = member.replace('\\', "/");
    normalized.make_ascii_lowercase();
    let parts = split_normalized(&normalized);
    let leaf = parts.last().copied().unwrap_or_default();
    if check_path_escape {
        if let Some(reason) = archive_path_escape_reason(&normalized, &parts) {
            issues.push(serde_json::json!({
                "code": "archive_member_path_escape",
                "path": artifact,
                "member": member,
                "reason": reason,
                "detail": "archive member path escapes the package root"
            }));
        }
    }
    if leaf.starts_with("._") {
        issues.push(serde_json::json!({
            "code": "appledouble_metadata",
            "path": artifact,
            "member": member,
            "detail": "archive contains macOS AppleDouble metadata"
        }));
    }
    classify_public_path(&parts, member_is_directory, leaf, |finding| {
        let (code, detail) = finding.issue(false);
        issues.push(serde_json::json!({
            "code": code,
            "path": artifact,
            "member": member,
            "detail": detail
        }));
    });
}
pub(crate) fn audit_public_link_target(
    artifact: &str,
    member: &str,
    target: &str,
    kind: ArchiveMemberKind,
    issues: &mut Vec<serde_json::Value>,
) {
    if let Some(reason) = archive_path_control_reason(target) {
        push_archive_link_target_uninspectable(artifact, member, target, kind, reason, issues);
    }
    let mut normalized = target.replace('\\', "/");
    normalized.make_ascii_lowercase();
    let target_is_directory = normalized.ends_with('/');
    let parts = split_normalized(&normalized);
    let leaf = parts.last().copied().unwrap_or_default();
    let link_kind = kind.as_str();
    if let Some(reason) = archive_path_escape_reason(&normalized, &parts) {
        issues.push(serde_json::json!({
            "code": "archive_link_target_escape",
            "path": artifact,
            "member": member,
            "link_target": target,
            "link_kind": link_kind,
            "reason": reason,
            "detail": "archive link target escapes the package root"
        }));
    }
    classify_public_path(&parts, target_is_directory, leaf, |finding| {
        let (code, detail) = finding.issue(true);
        issues.push(serde_json::json!({
            "code": code,
            "path": artifact,
            "member": member,
            "link_target": target,
            "link_kind": link_kind,
            "detail": detail
        }));
    });
}
pub(crate) fn archive_path_escape_reason(normalized: &str, parts: &[&str]) -> Option<&'static str> {
    if normalized.starts_with('/') {
        return Some("absolute_path");
    }
    if has_windows_drive_prefix(normalized) {
        return Some("windows_drive_path");
    }
    if parts.contains(&"..") {
        return Some("parent_directory");
    }
    None
}
pub(crate) fn archive_path_control_reason(path: &str) -> Option<&'static str> {
    path.chars().find(|ch| ch.is_control()).map(|ch| {
        if ch == '\0' {
            "nul_byte"
        } else {
            "control_character"
        }
    })
}
pub(crate) fn audit_tar_header_name_encoding(
    artifact: &str,
    member: &str,
    header: &[u8],
    issues: &mut Vec<serde_json::Value>,
) {
    if std::str::from_utf8(nul_terminated_bytes(&header[0..100])).is_err()
        || std::str::from_utf8(nul_terminated_bytes(&header[345..500])).is_err()
    {
        push_archive_member_name_uninspectable(artifact, member, "invalid_utf8", issues);
    }
}
pub(crate) fn audit_tar_header_link_encoding(
    artifact: &str,
    member: &str,
    header: &[u8],
    kind: ArchiveMemberKind,
    issues: &mut Vec<serde_json::Value>,
) {
    if std::str::from_utf8(nul_terminated_bytes(&header[157..257])).is_err() {
        let target =
            parse_tar_header_link_name(header).unwrap_or_else(|| "<invalid-utf8>".to_string());
        push_archive_link_target_uninspectable(
            artifact,
            member,
            &target,
            kind,
            "invalid_utf8",
            issues,
        );
    }
}
pub(crate) fn push_archive_member_name_uninspectable(
    artifact: &str,
    member: &str,
    reason: &'static str,
    issues: &mut Vec<serde_json::Value>,
) {
    issues.push(serde_json::json!({
        "code": "archive_member_name_uninspectable",
        "path": artifact,
        "member": member,
        "reason": reason,
        "detail": archive_uninspectable_detail(reason, "member name")
    }));
}
pub(crate) fn push_archive_link_target_uninspectable(
    artifact: &str,
    member: &str,
    target: &str,
    kind: ArchiveMemberKind,
    reason: &'static str,
    issues: &mut Vec<serde_json::Value>,
) {
    issues.push(serde_json::json!({
        "code": "archive_link_target_uninspectable",
        "path": artifact,
        "member": member,
        "link_target": target,
        "link_kind": kind.as_str(),
        "reason": reason,
        "detail": archive_uninspectable_detail(reason, "link target")
    }));
}
pub(crate) fn archive_uninspectable_detail(reason: &str, label: &str) -> String {
    match reason {
        "invalid_utf8" => format!("archive {label} is not valid UTF-8; package-audit fails closed"),
        _ => format!("archive {label} contains a control character; package-audit fails closed"),
    }
}
pub(crate) fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

const PRIVATE_TOOL_DOTDIRS: &[&str] = &[
    ".aider",
    ".anthropic",
    ".browser-harness",
    ".claude",
    ".cline",
    ".codex",
    ".continue",
    ".cursor",
    ".dev-browser",
    ".devin",
    ".droid",
    ".factory",
    ".gemini",
    ".grok",
    ".mcp",
    ".openai",
    ".opencode",
    ".playwright-mcp",
    ".tokenzero",
    ".windsurf",
];
const PUBLIC_DOTDIRS: &[&str] = &[
    ".azuredevops",
    ".buildkite",
    ".cargo",
    ".changeset",
    ".circleci",
    ".devcontainer",
    ".forgejo",
    ".gitea",
    ".github",
    ".gitlab",
    ".husky",
    ".storybook",
    ".vscode",
    ".well-known",
    ".yarn",
];
const SENSITIVE_LEAVES: &[&str] = &[
    ".env",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "auth.json",
    "credentials",
    "credentials.json",
];
const SENSITIVE_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx", ".ppk", ".ovpn", ".kdbx"];
const LOCAL_GENERATED_SUFFIXES: &[&str] = &[
    ".sqlite", ".sqlite3", ".db", ".bak", ".backup", ".dump", ".dmp",
];
const LOCAL_GENERATED_NEEDLES: &[&str] = &[
    "transcript",
    "chat-export",
    "chat_export",
    "conversation-export",
    "conversation_export",
    "debug-report",
    "debug_report",
    "screenshot",
    "screen-shot",
    "screen_shot",
    "local-output",
    "local_output",
    "agent-output",
    "agent_output",
];
pub(crate) fn is_private_tool_dotdir(part: &str) -> bool {
    PRIVATE_TOOL_DOTDIRS.contains(&part)
}
pub(crate) fn is_public_dotdir(part: &str) -> bool {
    PUBLIC_DOTDIRS.contains(&part)
}
pub(crate) fn is_sensitive_member_leaf(leaf: &str) -> bool {
    SENSITIVE_LEAVES.contains(&leaf)
        || leaf.starts_with(".env.")
        || SENSITIVE_SUFFIXES
            .iter()
            .any(|suffix| leaf.ends_with(suffix))
}
pub(crate) fn is_local_generated_member_leaf(leaf: &str) -> bool {
    LOCAL_GENERATED_SUFFIXES
        .iter()
        .any(|suffix| leaf.ends_with(suffix))
        || LOCAL_GENERATED_NEEDLES
            .iter()
            .any(|needle| leaf.contains(needle))
}

const EXECUTABLE_LEAVES: &[&str] = &[
    "tokenzero",
    "tokenzero.exe",
    "tokenzero.cmd",
    "tokenzero.js",
];
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "bat", "cmd", "com", "cjs", "dll", "dylib", "exe", "fish", "jar", "js", "mjs", "node", "php",
    "pl", "ps1", "psm1", "py", "rb", "sh", "so", "wasm", "zsh",
];
/// Format-agnostic executable/script payload audit (tar members and zip files).
pub(crate) fn audit_archive_executable_payload(
    artifact: &str,
    member: &str,
    payload: &[u8],
    issues: &mut Vec<serde_json::Value>,
) {
    if !is_executable_or_script_member_name(member) {
        return;
    }
    let Ok(text) = std::str::from_utf8(payload) else {
        return;
    };
    audit_release_text(artifact, Some(member), text, member, issues);
}
pub(crate) fn is_executable_or_script_member_name(name: &str) -> bool {
    let mut normalized = name.replace('\\', "/");
    normalized.make_ascii_lowercase();
    let mut parts = normalized.split('/').filter(|part| !part.is_empty());
    let Some(leaf) = parts.clone().next_back() else {
        return false;
    };
    if EXECUTABLE_LEAVES.contains(&leaf) || leaf.starts_with("tokenzero-runtime-") {
        return true;
    }
    if parts.any(|part| part == "bin") && !leaf.contains('.') {
        return true;
    }
    let ext = leaf.rsplit_once('.').map(|(_, e)| e).unwrap_or_default();
    EXECUTABLE_EXTENSIONS.contains(&ext)
}
