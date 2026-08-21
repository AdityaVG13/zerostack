use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{AsgrepMode, TokenAccounting, ZeroHandle};

pub const SNAP_WORKSPACE_SCHEMA: &str = "zerostack.snap.workspace";
pub const EXPAND_RESULT_SCHEMA: &str = "zerostack.expand";
pub const EFFECT_RESULT_SCHEMA: &str = "zerostack.effect";
pub const EFFECT_TARGET_LIMIT: usize = 32;
pub const EFFECT_CHANGE_LIMIT: usize = 128;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapViewMode {
    #[default]
    Decision,
    Structure,
    Full,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapViewRequest {
    #[serde(default)]
    pub mode: SnapViewMode,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapSearchRequest {
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<AsgrepMode>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SnapTargetRequest {
    Path { path: PathBuf },
    Search { search: SnapSearchRequest },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapLineSelectionRequest {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapByteSelectionRequest {
    pub start: u64,
    pub end: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapSelectionRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<SnapLineSelectionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<SnapByteSelectionRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exact_text: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapRequest {
    pub target: SnapTargetRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cardinality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SnapSelectionRequest>,
    #[serde(default)]
    pub view: SnapViewRequest,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapNewline {
    Lf,
    Crlf,
    Mixed,
    None,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapByteRange {
    pub byte_start: u64,
    pub byte_end: u64,
}

impl SnapRequest {
    pub fn validate(&self) -> Result<(), String> {
        match &self.target {
            SnapTargetRequest::Path { path } => validate_confined_path(path)?,
            SnapTargetRequest::Search { search } => {
                if search.query.trim().is_empty() {
                    return Err("z.snap search query must not be empty".into());
                }
                if let Some(under) = &search.under {
                    validate_confined_path(under)?;
                }
            }
        }
        if self
            .cardinality
            .as_deref()
            .is_some_and(|cardinality| cardinality != "exactly_one")
        {
            return Err("mutation-grade z.snap supports only cardinality exactly_one".into());
        }
        if let Some(selection) = &self.selection {
            let count = usize::from(selection.lines.is_some())
                + usize::from(selection.bytes.is_some())
                + usize::from(selection.symbol.is_some())
                + usize::from(selection.exact_text.is_some());
            if count != 1 {
                return Err(
                    "z.snap selection requires exactly one lines, bytes, symbol, or exactText"
                        .into(),
                );
            }
            if selection
                .lines
                .as_ref()
                .is_some_and(|lines| lines.start == 0 || lines.end < lines.start)
            {
                return Err("z.snap lines must be one-based and inclusive".into());
            }
            if selection
                .bytes
                .as_ref()
                .is_some_and(|bytes| bytes.end <= bytes.start)
            {
                return Err("z.snap byte end must exceed byte start".into());
            }
            if selection
                .symbol
                .as_deref()
                .is_some_and(|symbol| symbol.trim().is_empty())
            {
                return Err("z.snap symbol must not be empty".into());
            }
            if selection.exact_text.as_deref().is_some_and(str::is_empty) {
                return Err("z.snap exactText must not be empty".into());
            }
        }
        Ok(())
    }
}

fn validate_confined_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err("path must be workspace-relative or an absolute external path".into());
    }
    if path.is_absolute() {
        // Absolute external paths are byte-authority only: no ParentDir components.
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err("path must be workspace-relative or an absolute external path".into());
        }
        return Ok(());
    }
    // Relative paths remain root-confined: no ParentDir, RootDir, or Prefix.
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err("path must be workspace-relative or an absolute external path".into());
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapSource {
    pub exact: ZeroHandle,
    pub content_digest: String,
    pub byte_length: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_count: Option<u64>,
    pub encoding: String,
    pub newline: SnapNewline,
    pub bom: bool,
    pub mode: u32,
    pub modified_unix_ns: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapSelection {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    pub byte_start: u64,
    pub byte_end: u64,
    pub selected_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapView {
    pub mode: SnapViewMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    pub full_file_visible: bool,
    pub visible_bytes: u64,
    pub omitted_bytes: u64,
    pub visible_ranges: Vec<SnapByteRange>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapRecovery {
    pub manifest: ZeroHandle,
    pub exact: ZeroHandle,
    pub complete: bool,
    pub recoverable_bytes: u64,
    pub unrecoverable_bytes: u64,
    pub retained: bool,
    pub retention_policy: String,
    pub selectors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapStructuralEvidence {
    pub index_digest: String,
    pub complete: bool,
    pub source: ZeroHandle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ZeroHandle>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapAccounting {
    pub tokenizer: String,
    pub certified: bool,
    pub source_tokens: u64,
    pub visible_tokens: u64,
    pub omitted_tokens: u64,
    pub recovered_tokens: u64,
    pub saved_tokens_now: u64,
    pub cached_tokens: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SnapResult {
    pub schema: String,
    pub snapshot: ZeroHandle,
    pub path: PathBuf,
    pub source: SnapSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SnapSelection>,
    pub view: SnapView,
    pub recovery: SnapRecovery,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structural: Option<SnapStructuralEvidence>,
    pub accounting: SnapAccounting,
}

impl SnapResult {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != SNAP_WORKSPACE_SCHEMA {
            return Err("z.snap result schema mismatch".into());
        }
        validate_confined_path(&self.path)?;
        if self.source.content_digest != self.source.exact.digest()
            || self.source.content_digest.len() != 64
        {
            return Err("z.snap source digest does not match its exact handle".into());
        }
        if self.source.modified_unix_ns.parse::<u128>().is_err() {
            return Err("z.snap modifiedUnixNs must be an unsigned decimal string".into());
        }
        if !matches!(self.source.encoding.as_str(), "utf8" | "binary") {
            return Err("z.snap source encoding is unsupported".into());
        }
        if let Some(selection) = &self.selection {
            if selection.byte_start >= selection.byte_end
                || selection.byte_end > self.source.byte_length
                || selection.selected_digest.len() != 64
            {
                return Err(
                    "z.snap selection is outside its source or has an invalid digest".into(),
                );
            }
        }
        let mut covered = 0_u64;
        let mut previous_end = 0_u64;
        for range in &self.view.visible_ranges {
            if range.byte_start >= range.byte_end
                || range.byte_start < previous_end
                || range.byte_end > self.source.byte_length
            {
                return Err("z.snap visible ranges are invalid or overlap".into());
            }
            covered = covered.saturating_add(range.byte_end - range.byte_start);
            previous_end = range.byte_end;
        }
        if covered.saturating_add(self.view.omitted_bytes) != self.source.byte_length {
            return Err("z.snap visible and omitted source coverage is not exact".into());
        }
        if self.view.full_file_visible {
            if self.view.omitted_bytes != 0
                || covered != self.source.byte_length
                || self
                    .view
                    .text
                    .as_ref()
                    .is_none_or(|text| text.len() as u64 != self.source.byte_length)
            {
                return Err("z.snap full view is not byte-for-byte complete".into());
            }
        }
        if self.source.encoding == "binary"
            && (self.view.text.is_some() || !self.view.visible_ranges.is_empty())
        {
            return Err("z.snap binary source must not claim a text view".into());
        }
        if self.recovery.exact != self.source.exact
            || !self.recovery.complete
            || self.recovery.recoverable_bytes != self.source.byte_length
            || self.recovery.unrecoverable_bytes != 0
        {
            return Err("z.snap recovery does not cover the exact source".into());
        }
        if self
            .structural
            .as_ref()
            .is_some_and(|structural| structural.source != self.source.exact)
        {
            return Err("z.snap structural evidence is bound to another source".into());
        }
        if self.accounting.visible_tokens > self.accounting.source_tokens
            || self.accounting.omitted_tokens
                != self
                    .accounting
                    .source_tokens
                    .saturating_sub(self.accounting.visible_tokens)
            || self.accounting.saved_tokens_now != self.accounting.omitted_tokens
            || self.accounting.recovered_tokens != 0
        {
            return Err("z.snap token accounting is inconsistent".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ExpandResult {
    pub schema: String,
    pub source: ZeroHandle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<String>,
    pub encoding: String,
    pub byte_start: u64,
    pub byte_end: u64,
    pub byte_length: u64,
    pub exact_digest: String,
    pub complete: bool,
    pub recovered_tokens: u64,
    pub accounting: TokenAccounting,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectTargetRequest {
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectChangeKind {
    ReplaceExact,
    ReplaceFile,
    InsertBefore,
    InsertAfter,
    CreateFile,
    RemoveFile,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectAnchorRequest {
    pub exact_text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectChangeRequest {
    pub target: String,
    pub kind: EffectChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<EffectAnchorRequest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectCommandRequest {
    pub argv: Vec<String>,
    pub timeout_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectVerificationRequest {
    #[serde(default)]
    pub parse: bool,
    #[serde(default)]
    pub changed_targets_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<EffectCommandRequest>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectRequest {
    pub targets: BTreeMap<String, EffectTargetRequest>,
    pub changes: Vec<EffectChangeRequest>,
    #[serde(default)]
    pub verify: EffectVerificationRequest,
}

impl EffectRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.targets.is_empty() || self.targets.len() > EFFECT_TARGET_LIMIT {
            return Err(format!(
                "z.effect requires 1..={EFFECT_TARGET_LIMIT} targets"
            ));
        }
        if self.changes.is_empty() || self.changes.len() > EFFECT_CHANGE_LIMIT {
            return Err(format!(
                "z.effect requires 1..={EFFECT_CHANGE_LIMIT} changes"
            ));
        }
        let mut paths = BTreeSet::new();
        for (name, target) in &self.targets {
            if name.trim().is_empty() || name.trim() != name {
                return Err("z.effect target names must be non-empty and trimmed".into());
            }
            validate_confined_path(&target.path)?;
            if !paths.insert(target.path.clone()) {
                return Err("z.effect target paths must be unique".into());
            }
            if target
                .expect
                .as_deref()
                .is_some_and(|expect| !matches!(expect, "exists" | "absent"))
            {
                return Err("z.effect expect must be exists or absent".into());
            }
        }
        for change in &self.changes {
            if !self.targets.contains_key(&change.target) {
                return Err(format!(
                    "z.effect change names unknown target {:?}",
                    change.target
                ));
            }
            match change.kind {
                EffectChangeKind::ReplaceExact => {
                    if change.expected_count != Some(1)
                        || change.old.as_deref().is_none_or(str::is_empty)
                        || change.replacement.is_none()
                        || change.content.is_some()
                        || change.anchor.is_some()
                    {
                        return Err(
                            "replace_exact requires only non-empty old, replacement, and expectedCount 1"
                                .into(),
                        );
                    }
                }
                EffectChangeKind::ReplaceFile | EffectChangeKind::CreateFile => {
                    if change.content.is_none()
                        || change.old.is_some()
                        || change.replacement.is_some()
                        || change.expected_count.is_some()
                        || change.anchor.is_some()
                    {
                        return Err("replace_file/create_file accepts only content".into());
                    }
                }
                EffectChangeKind::InsertBefore | EffectChangeKind::InsertAfter => {
                    if change.content.is_none()
                        || change
                            .anchor
                            .as_ref()
                            .is_none_or(|anchor| anchor.exact_text.is_empty())
                        || change.old.is_some()
                        || change.replacement.is_some()
                        || change.expected_count.is_some()
                    {
                        return Err(
                            "insert_before/insert_after accepts only content and anchor.exactText"
                                .into(),
                        );
                    }
                }
                EffectChangeKind::RemoveFile => {
                    if change.old.is_some()
                        || change.replacement.is_some()
                        || change.content.is_some()
                        || change.expected_count.is_some()
                        || change.anchor.is_some()
                    {
                        return Err("remove_file accepts no change payload fields".into());
                    }
                }
            }
        }
        for name in self.targets.keys() {
            if !self.changes.iter().any(|change| &change.target == name) {
                return Err(format!("z.effect target {name:?} has no change"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectTargetResult {
    pub name: String,
    pub path: PathBuf,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<ZeroHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<ZeroHandle>,
    pub journal: ZeroHandle,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectVerificationResult {
    pub parse: String,
    pub command: String,
    pub changed_targets_only: bool,
}

impl EffectResult {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != EFFECT_RESULT_SCHEMA || self.outcome != "staged" {
            return Err("z.effect result schema or outcome is invalid".into());
        }
        if self.changed_files as usize != self.targets.len() || self.targets.is_empty() {
            return Err("z.effect changedFiles does not match its targets".into());
        }
        let mut names = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for target in &self.targets {
            if !names.insert(target.name.as_str()) || !paths.insert(&target.path) {
                return Err("z.effect result contains duplicate targets".into());
            }
            validate_confined_path(&target.path)?;
            if !matches!(target.kind.as_str(), "edit" | "create" | "remove") {
                return Err("z.effect result target kind is invalid".into());
            }
            match target.kind.as_str() {
                "edit" if target.before.is_none() || target.after.is_none() => {
                    return Err("z.effect edit target requires before and after handles".into());
                }
                "create" if target.before.is_some() || target.after.is_none() => {
                    return Err("z.effect create target has invalid handles".into());
                }
                "remove" if target.before.is_none() || target.after.is_some() => {
                    return Err("z.effect remove target has invalid handles".into());
                }
                _ => {}
            }
        }
        if !self.verification.changed_targets_only {
            return Err("z.effect result must prove changedTargetsOnly".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EffectResult {
    pub schema: String,
    pub outcome: String,
    pub delta: ZeroHandle,
    pub targets: Vec<EffectTargetResult>,
    pub changed_files: u32,
    pub verification: EffectVerificationResult,
}
