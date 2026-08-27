//! Tree-sitter query loading and compilation (FR-003, ADR-005).
//!
//! .scm files are embedded at build time via include_str!.

use std::error::Error;
use std::fmt;

use tree_sitter::{Query, QueryError};

use crate::Language;

/// Compiled query set for a single language.
pub struct LangQueries {
    pub definitions: Query,
    pub calls: Query,
    pub imports: Query,
    pub implements: Query,
}

/// Diagnostic returned when a tree-sitter query fails to compile.
#[derive(Debug)]
pub struct QueryCompileError {
    language: Language,
    source_name: &'static str,
    error: QueryError,
}

impl QueryCompileError {
    pub fn language(&self) -> Language {
        self.language
    }

    pub fn source_name(&self) -> &'static str {
        self.source_name
    }

    pub fn query_error(&self) -> &QueryError {
        &self.error
    }
}

impl fmt::Display for QueryCompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to compile {:?} tree-sitter query from {}: {}",
            self.language, self.source_name, self.error
        )
    }
}

impl Error for QueryCompileError {}

/// All compiled queries for all supported languages.
pub struct QuerySet {
    pub rust: LangQueries,
    pub typescript: LangQueries,
    pub python: LangQueries,
}

impl Default for QuerySet {
    fn default() -> Self {
        Self::new()
    }
}

impl QuerySet {
    /// Compile all embedded query files at crate init.
    ///
    /// The embedded query files are build-time invariants, so this compatibility
    /// constructor still aborts on failure. Runtime callers that need structured
    /// diagnostics should use QuerySet::try_new.
    pub fn new() -> Self {
        Self::try_new().unwrap_or_else(|err| panic!("{err}"))
    }

    /// Compile all embedded query files and return a structured diagnostic on
    /// query/grammar mismatch.
    pub fn try_new() -> Result<Self, QueryCompileError> {
        Ok(Self {
            rust: compile_rust_queries()?,
            typescript: compile_typescript_queries()?,
            python: compile_python_queries()?,
        })
    }

    pub fn for_language(&self, lang: Language) -> Option<&LangQueries> {
        match lang {
            Language::Rust => Some(&self.rust),
            Language::TypeScript => Some(&self.typescript),
            Language::Python => Some(&self.python),
            Language::Unknown => None,
        }
    }
}

fn compile_query(
    grammar: tree_sitter::Language,
    language: Language,
    source_name: &'static str,
    source: &str,
) -> Result<Query, QueryCompileError> {
    Query::new(&grammar, source).map_err(|error| QueryCompileError {
        language,
        source_name,
        error,
    })
}

fn compile_rust_queries() -> Result<LangQueries, QueryCompileError> {
    let all = include_str!("queries/rust.scm");
    let language = Language::Rust;
    let grammar: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    Ok(LangQueries {
        definitions: compile_query(grammar.clone(), language, "queries/rust.scm", all)?,
        calls: compile_query(grammar.clone(), language, "queries/rust.scm", all)?,
        imports: compile_query(grammar.clone(), language, "queries/rust.scm", all)?,
        implements: compile_query(grammar, language, "queries/rust.scm", all)?,
    })
}

fn compile_typescript_queries() -> Result<LangQueries, QueryCompileError> {
    let all = include_str!("queries/typescript.scm");
    let language = Language::TypeScript;
    let grammar: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
    Ok(LangQueries {
        definitions: compile_query(grammar.clone(), language, "queries/typescript.scm", all)?,
        calls: compile_query(grammar.clone(), language, "queries/typescript.scm", all)?,
        imports: compile_query(grammar.clone(), language, "queries/typescript.scm", all)?,
        implements: compile_query(grammar, language, "queries/typescript.scm", all)?,
    })
}

fn compile_python_queries() -> Result<LangQueries, QueryCompileError> {
    let all = include_str!("queries/python.scm");
    let language = Language::Python;
    let grammar: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();
    Ok(LangQueries {
        definitions: compile_query(grammar.clone(), language, "queries/python.scm", all)?,
        calls: compile_query(grammar.clone(), language, "queries/python.scm", all)?,
        imports: compile_query(grammar.clone(), language, "queries/python.scm", all)?,
        implements: compile_query(grammar, language, "queries/python.scm", all)?,
    })
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-extract/queries_tests.rs"]
mod tests;
