//! Structural file detection + polyglot extract (`fszero-ast-sgrep`) with heuristic fallback.

use super::types::{IndexedFn, IndexedImport, SymbolNodeKind};
use std::path::Path;

pub fn is_structural_extension(ext: &str) -> bool {
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "pyi"
            | "go"
            | "java"
            | "cs"
            | "rb"
    )
}

pub fn is_structural_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| is_structural_extension(&e.to_ascii_lowercase()))
        .unwrap_or(false)
}

pub fn is_structural_file_key(file_key: &str) -> bool {
    file_key
        .rsplit_once('.')
        .map(|(_, ext)| is_structural_extension(ext))
        .unwrap_or(false)
}

pub struct ExtractedFile {
    pub fns: Vec<IndexedFn>,
    pub imports: Vec<IndexedImport>,
    pub calls: Vec<(String, String, usize)>,
}

pub fn extract_structural(path: &Path, txt: &str) -> ExtractedFile {
    #[cfg(feature = "fszero-ast-sgrep")]
    {
        if let Some(ext) = extract_via_ast_sgrep_lang(path, txt) {
            return ext;
        }
    }
    ExtractedFile {
        fns: super::parse::parse_file_fns_heuristic(txt),
        imports: super::parse::parse_file_imports_heuristic(txt),
        calls: Vec::new(),
    }
}

#[cfg(feature = "fszero-ast-sgrep")]
fn extract_via_ast_sgrep_lang(path: &Path, txt: &str) -> Option<ExtractedFile> {
    use ast_sgrep_lang::{ParserRegistry, SymbolKind, detect_language};

    let lang = detect_language(path, Some(txt))?;
    let reg = ParserRegistry::new();
    let result = reg.parse(lang, txt).ok()?;
    let mut fns = Vec::new();
    for sym in result.symbols {
        let kind = match sym.kind {
            SymbolKind::Function => SymbolNodeKind::Fn,
            SymbolKind::Method => SymbolNodeKind::Method,
            SymbolKind::Type => SymbolNodeKind::Type,
            SymbolKind::Enum => SymbolNodeKind::Enum,
            SymbolKind::Interface => SymbolNodeKind::Interface,
            SymbolKind::Class => SymbolNodeKind::Class,
            SymbolKind::Doc => continue,
        };
        fns.push(IndexedFn {
            span_start: sym.byte_start,
            span_end: sym.byte_end,
            name: sym.name,
            kind,
        });
    }
    let imports = result
        .imports
        .into_iter()
        .map(|imp| IndexedImport {
            span_start: 0,
            span_end: 0,
            name: imp.module_path,
        })
        .collect();
    let calls = result
        .calls
        .into_iter()
        .map(|c| (c.caller, c.callee, c.line as usize))
        .collect();
    Some(ExtractedFile {
        fns,
        imports,
        calls,
    })
}
