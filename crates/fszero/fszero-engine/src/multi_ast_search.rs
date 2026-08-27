//! multi_ast_search: multi-pattern AST walk (fszero-lb9i).
//!
//! items:[{language, pattern, paths?, limit?}] -> per-item hits carrying
//! path#Lstart-Lend spans. ALL patterns of the same language are evaluated
//! during ONE parse of each file: the language comes from the file extension,
//! the file is parsed once into an ast-sgrep-lang ExtractionResult whose
//! pattern_nodes carry signature/span/excerpt for every candidate node, and
//! every interested item's compiled matcher is tested against that one node
//! list.
//!
//! The parsed forest is cached by (file_key, content hash) so an unchanged file
//! is never reparsed across calls; cold files are parsed on demand. parses /
//! cache_hits are observable so tests can pin one-parse-per-file for N
//! patterns.

use ast_sgrep_lang::{Language, ParserRegistry, PatternNode};
use std::collections::HashMap;

/// One compiled pattern. Matching is signature-based over the pattern_nodes of
/// a parsed file; a metavariable matches any single signature segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledPattern {
    pub language: Language,
    segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Meta,
}

/// Compile language + pattern into a matcher. Errors are per-item and never
/// fail the batch.
pub fn compile_pattern(language: &str, pattern: &str) -> Result<CompiledPattern, String> {
    let language = parse_language(language)?;
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return Err("ast pattern must not be empty".to_string());
    }
    let mut segments = Vec::new();
    for token in pattern.split_whitespace() {
        if let Some(name) = token.strip_prefix('$') {
            if name.is_empty() {
                return Err(format!(
                    "malformed metavariable in pattern {pattern:?}: bare sigil"
                ));
            }
            if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(format!(
                    "malformed metavariable {token:?}: name must be [A-Za-z0-9_]"
                ));
            }
            segments.push(Segment::Meta);
        } else {
            if token.contains('$') {
                return Err(format!(
                    "malformed metavariable in token {token:?}: sigil must start the token"
                ));
            }
            segments.push(Segment::Literal(token.to_string()));
        }
    }
    Ok(CompiledPattern { language, segments })
}

pub fn parse_language(language: &str) -> Result<Language, String> {
    match language.trim().to_ascii_lowercase().as_str() {
        "rust" | "rs" => Ok(Language::Rust),
        "typescript" | "ts" | "tsx" => Ok(Language::TypeScript),
        "javascript" | "js" | "jsx" => Ok(Language::JavaScript),
        "python" | "py" => Ok(Language::Python),
        "go" => Ok(Language::Go),
        "java" => Ok(Language::Java),
        "csharp" | "cs" => Ok(Language::CSharp),
        "ruby" | "rb" => Ok(Language::Ruby),
        other => Err(format!("unsupported ast language {other:?}")),
    }
}

impl CompiledPattern {
    /// Whitespace-delimited segments compared in order, a metavariable matching
    /// exactly one segment. Pattern node signatures are the shapes
    /// ast-sgrep-lang emits (a bare identifier, "fn NAME", "decl:fn:NAME",
    /// "kind:NODE", "call:TARGET", "call-name:NAME"), so "fn $NAME" matches
    /// every Rust function declaration.
    fn matches(&self, signature: &str) -> bool {
        let parts: Vec<&str> = signature.split_whitespace().collect();
        if parts.len() != self.segments.len() {
            return false;
        }
        self.segments
            .iter()
            .zip(parts)
            .all(|(segment, part)| match segment {
                Segment::Meta => true,
                Segment::Literal(literal) => literal == part,
            })
    }
}

/// One parsed file's candidate nodes, keyed by content hash in AstForest.
#[derive(Debug)]
struct ForestEntry {
    content_hash: u64,
    nodes: Vec<PatternNode>,
}

/// Syntax forest cache: file_key -> parsed pattern nodes for a content hash.
/// Reused across calls so unchanged files are never reparsed. The counters are
/// the single-walk instrumentation.
#[derive(Debug, Default)]
pub struct AstForest {
    entries: HashMap<String, ForestEntry>,
    parses: u64,
    cache_hits: u64,
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

impl AstForest {
    pub fn parses(&self) -> u64 {
        self.parses
    }
    pub fn cache_hits(&self) -> u64 {
        self.cache_hits
    }

    /// Parsed nodes for one file; parses at most once per (file_key, content).
    fn nodes(
        &mut self,
        registry: &ParserRegistry,
        language: Language,
        file_key: &str,
        text: &str,
    ) -> &[PatternNode] {
        let hash = content_hash(text.as_bytes());
        let fresh = self
            .entries
            .get(file_key)
            .is_some_and(|entry| entry.content_hash == hash);
        if fresh {
            self.cache_hits += 1;
        } else {
            let nodes = registry
                .parse(language, text)
                .map(|result| result.pattern_nodes)
                .unwrap_or_default();
            self.parses += 1;
            self.entries.insert(
                file_key.to_string(),
                ForestEntry {
                    content_hash: hash,
                    nodes,
                },
            );
        }
        &self.entries[file_key].nodes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstHit {
    pub path: String,
    pub span: String,
    pub preview: String,
}

/// One request item after parsing/compiling. Items whose pattern failed to
/// compile still occupy a slot, so result order matches input order exactly.
#[derive(Debug)]
pub struct AstItem {
    pub language: String,
    pub pattern: String,
    pub paths: Vec<String>,
    pub limit: usize,
    pub compiled: Result<CompiledPattern, String>,
}

impl AstItem {
    pub fn new(language: &str, pattern: &str, paths: Vec<String>, limit: usize) -> Self {
        Self {
            language: language.to_string(),
            pattern: pattern.to_string(),
            paths,
            limit,
            compiled: compile_pattern(language, pattern),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct AstItemResult {
    pub hits: Vec<AstHit>,
    pub truncated: bool,
    pub error: Option<String>,
}

/// A file offered to the walk: repo-relative key plus UTF-8 text.
pub struct AstFile<'a> {
    pub file_key: &'a str,
    pub text: &'a str,
}

/// Evaluate every item against every file with ONE parse per file per call.
///
/// Compiled matchers are deduped by (language, pattern) so a repeated pattern
/// compiles once. Files are visited in the caller's order; for each file only
/// items of that file's language and path scope are tested, all against one
/// node list. A file is parsed only when at least one item still has room for
/// hits; already-full items are still checked against the node list of a file
/// that was parsed anyway, so truncation is reported honestly.
pub fn multi_ast_search(
    forest: &mut AstForest,
    items: &[AstItem],
    files: &[AstFile<'_>],
) -> Vec<AstItemResult> {
    let registry = ParserRegistry::new();
    let mut results: Vec<AstItemResult> = items
        .iter()
        .map(|item| AstItemResult {
            hits: Vec::new(),
            truncated: false,
            error: item.compiled.as_ref().err().cloned(),
        })
        .collect();

    let mut matchers: HashMap<(&str, &str), &CompiledPattern> = HashMap::new();
    for item in items {
        if let Ok(compiled) = &item.compiled {
            matchers
                .entry((item.language.as_str(), item.pattern.as_str()))
                .or_insert(compiled);
        }
    }

    for file in files {
        let Some(language) = language_for_key(file.file_key) else {
            continue;
        };
        let candidates: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                item.compiled
                    .as_ref()
                    .is_ok_and(|compiled| compiled.language == language)
                    && matches_paths(&item.paths, file.file_key)
            })
            .map(|(index, _)| index)
            .collect();
        if candidates
            .iter()
            .all(|index| results[*index].hits.len() >= items[*index].limit)
        {
            continue;
        }
        let nodes = forest
            .nodes(&registry, language, file.file_key, file.text)
            .to_vec();
        for index in candidates {
            let compiled = matchers[&(
                items[index].language.as_str(),
                items[index].pattern.as_str(),
            )];
            for node in &nodes {
                if !compiled.matches(&node.signature) {
                    continue;
                }
                if results[index].hits.len() >= items[index].limit {
                    results[index].truncated = true;
                    break;
                }
                results[index].hits.push(AstHit {
                    path: file.file_key.to_string(),
                    span: format!("{}#L{}-L{}", file.file_key, node.line_start, node.line_end),
                    preview: node.excerpt.clone(),
                });
            }
        }
    }
    results
}

fn matches_paths(paths: &[String], file_key: &str) -> bool {
    paths.is_empty()
        || paths
            .iter()
            .any(|prefix| file_key == prefix || file_key.starts_with(&format!("{prefix}/")))
}

fn language_for_key(file_key: &str) -> Option<Language> {
    let (_, ext) = file_key.rsplit_once('.')?;
    parse_language(ext).ok()
}
