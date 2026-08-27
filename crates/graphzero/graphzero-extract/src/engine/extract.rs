//! Query lookup and edge extraction passes.

use tree_sitter::{QueryCursor, StreamingIterator, Tree};

use crate::confidence_band;
use crate::queries::LangQueries;
use crate::{
    BlobInput, Edge, EdgeKind, EvidenceRef, FILE_NODE_ID, Language, NodeKind, PATH_NODE_OFFSET,
    PathNode, Source, find_enclosing_caller,
};

use super::facts::ExtractionState;

pub(super) fn run_tier_a_extractors(
    tree: &Tree,
    input: &BlobInput,
    lang_queries: &LangQueries,
    lang: Language,
    state: &mut ExtractionState,
) {
    extract_defs(
        tree,
        input.content,
        &lang_queries.definitions,
        lang,
        input.hash,
        state,
    );
    extract_calls(tree, input.content, &lang_queries.calls, input.hash, state);
    if may_contain_imports(input.content, lang) {
        extract_imports(
            tree,
            input.content,
            &lang_queries.imports,
            input.hash,
            state,
        );
    }
    if may_contain_implements(input.content, lang) {
        extract_implements(
            tree,
            input.content,
            &lang_queries.implements,
            input.hash,
            state,
        );
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn may_contain_imports(source: &[u8], lang: Language) -> bool {
    match lang {
        Language::Rust => contains_bytes(source, b"use"),
        Language::TypeScript | Language::Python => contains_bytes(source, b"import"),
        Language::Unknown => false,
    }
}

fn may_contain_implements(source: &[u8], lang: Language) -> bool {
    match lang {
        Language::Rust => contains_bytes(source, b"impl"),
        Language::TypeScript => contains_bytes(source, b"implements"),
        Language::Python => contains_bytes(source, b"class"),
        Language::Unknown => false,
    }
}

fn find_capture_node<'a>(
    mat: &'a tree_sitter::QueryMatch<'_, '_>,
    query: &tree_sitter::Query,
    capture_name: &str,
) -> Option<tree_sitter::Node<'a>> {
    mat.captures.iter().find_map(|capture| {
        let cap = query.capture_names()[capture.index as usize];
        (cap == capture_name).then_some(capture.node)
    })
}

fn record_def_name(
    name_node: tree_sitter::Node,
    def_node: tree_sitter::Node,
    source: &[u8],
    lang: Language,
    blob_hash: crate::ContentHash,
    state: &mut ExtractionState,
) {
    let name = name_node.utf8_text(source).unwrap_or("").to_string();
    let start = name_node.start_byte() as u32;
    let end = name_node.end_byte() as u32;
    let block_start = def_node.start_byte() as u32;
    let block_end = def_node.end_byte() as u32;
    let kind = infer_def_kind_for_name(lang, def_node.kind(), &name, state);

    let id = state.nodes.len() as u32;
    state.nodes.push(crate::SymbolNode {
        id,
        name: name.clone(),
        kind,
        span_start: start,
        span_end: end,
        block_start,
        block_end,
    });
    state.name_to_ids.entry(name).or_default().push(id);

    state.edges.push(Edge {
        src: FILE_NODE_ID,
        dst: id,
        kind: EdgeKind::Contains,
        confidence: confidence_band::CONTAINS,
        source: Source::TreeSitter,
        evidence: EvidenceRef::new_unchecked(blob_hash, start, end),
    });
}

fn extract_defs(
    tree: &Tree,
    source: &[u8],
    query: &tree_sitter::Query,
    lang: Language,
    blob_hash: crate::ContentHash,
    state: &mut ExtractionState,
) {
    let mut cursor = QueryCursor::new();
    let mut stream = cursor.matches(query, tree.root_node(), source);
    while let Some(mat) = stream.next() {
        let Some(name_node) = find_capture_node(mat, query, "def_name") else {
            continue;
        };
        let def_node = find_capture_node(mat, query, "def_node").unwrap_or(name_node);
        record_def_name(name_node, def_node, source, lang, blob_hash, state);
    }
}

fn call_name_span(
    mat: &tree_sitter::QueryMatch<'_, '_>,
    query: &tree_sitter::Query,
    source: &[u8],
) -> Option<(String, u32, u32)> {
    for capture in mat.captures {
        let cap_name = query.capture_names()[capture.index as usize];
        if cap_name != "call_name" {
            continue;
        }
        let node = capture.node;
        return Some((
            node.utf8_text(source).unwrap_or("").to_string(),
            node.start_byte() as u32,
            node.end_byte() as u32,
        ));
    }
    None
}

fn emit_call_edges(
    state: &mut ExtractionState,
    cname: &str,
    call_start: u32,
    call_end: u32,
    blob_hash: crate::ContentHash,
) {
    let Some(callee_ids) = state.name_to_ids.get(cname) else {
        return;
    };
    let caller_id = find_enclosing_caller(&state.nodes, call_start);
    for &callee_id in callee_ids {
        state.edges.push(Edge {
            src: caller_id,
            dst: callee_id,
            kind: EdgeKind::Calls,
            confidence: confidence_band::LOCAL_CALL,
            source: Source::TreeSitter,
            evidence: EvidenceRef::new_unchecked(blob_hash, call_start, call_end),
        });
    }
}

fn extract_calls(
    tree: &Tree,
    source: &[u8],
    query: &tree_sitter::Query,
    blob_hash: crate::ContentHash,
    state: &mut ExtractionState,
) {
    let mut cursor = QueryCursor::new();
    let mut stream = cursor.matches(query, tree.root_node(), source);
    while let Some(mat) = stream.next() {
        let Some((cname, call_start, call_end)) = call_name_span(mat, query, source) else {
            continue;
        };
        emit_call_edges(state, &cname, call_start, call_end, blob_hash);
    }
}

fn record_import_path(
    node: tree_sitter::Node,
    source: &[u8],
    blob_hash: crate::ContentHash,
    state: &mut ExtractionState,
) {
    let path = node.utf8_text(source).unwrap_or("").to_string();
    let start = node.start_byte() as u32;
    let end = node.end_byte() as u32;

    let path_id = PATH_NODE_OFFSET + state.path_nodes.len() as u32;
    state.path_nodes.push(PathNode {
        id: path_id,
        path: path.clone(),
    });

    state.edges.push(Edge {
        src: FILE_NODE_ID,
        dst: path_id,
        kind: EdgeKind::Imports,
        confidence: confidence_band::IMPORTS,
        source: Source::TreeSitter,
        evidence: EvidenceRef::new_unchecked(blob_hash, start, end),
    });
}

fn extract_imports(
    tree: &Tree,
    source: &[u8],
    query: &tree_sitter::Query,
    blob_hash: crate::ContentHash,
    state: &mut ExtractionState,
) {
    let mut cursor = QueryCursor::new();
    let mut stream = cursor.matches(query, tree.root_node(), source);
    while let Some(mat) = stream.next() {
        let Some(node) = find_capture_node(mat, query, "import_path") else {
            continue;
        };
        record_import_path(node, source, blob_hash, state);
    }
}

fn record_trait_impl_edges(
    tname: &str,
    implementer_name: Option<&str>,
    source: &[u8],
    exclude_impl_defs: bool,
    start: u32,
    end: u32,
    blob_hash: crate::ContentHash,
    state: &mut ExtractionState,
) {
    let src = implementer_name
        .and_then(|name| state.name_to_ids.get(name))
        .and_then(|ids| {
            ids.iter().copied().find(|id| {
                state.nodes.get(*id as usize).is_some_and(|node| {
                    !exclude_impl_defs || !source[node.block_start as usize..].starts_with(b"impl")
                })
            })
        })
        .unwrap_or(FILE_NODE_ID);
    let Some(trait_ids) = state.name_to_ids.get(tname) else {
        return;
    };
    for &trait_id in trait_ids {
        state.edges.push(Edge {
            src,
            dst: trait_id,
            kind: EdgeKind::Implements,
            confidence: confidence_band::IMPLEMENTS_LOCAL,
            source: Source::TreeSitter,
            evidence: EvidenceRef::new_unchecked(blob_hash, start, end),
        });
    }
}

fn extract_implements(
    tree: &Tree,
    source: &[u8],
    query: &tree_sitter::Query,
    blob_hash: crate::ContentHash,
    state: &mut ExtractionState,
) {
    let mut cursor = QueryCursor::new();
    let mut stream = cursor.matches(query, tree.root_node(), source);
    while let Some(mat) = stream.next() {
        let Some(node) = find_capture_node(mat, query, "trait_name") else {
            continue;
        };
        let tname = node.utf8_text(source).unwrap_or("").to_string();
        let impl_node = find_capture_node(mat, query, "impl_node").unwrap_or(node);
        let implementer_node = match impl_node.kind() {
            "impl_item" => impl_node.child_by_field_name("type"),
            "class_definition" => impl_node.child_by_field_name("name"),
            "implements_clause" => impl_node
                .parent()
                .and_then(|parent| parent.child_by_field_name("name")),
            _ => None,
        };
        let implementer_name = implementer_node.and_then(|node| node.utf8_text(source).ok());
        let exclude_impl_defs = impl_node.kind() == "impl_item";
        let start = node.start_byte() as u32;
        let end = node.end_byte() as u32;
        record_trait_impl_edges(
            &tname,
            implementer_name,
            source,
            exclude_impl_defs,
            start,
            end,
            blob_hash,
            state,
        );
    }
}

fn infer_def_kind_for_name(
    lang: Language,
    kind_str: &str,
    name: &str,
    state: &ExtractionState,
) -> NodeKind {
    match lang {
        Language::Rust => infer_def_kind_rust(kind_str, name, state),
        Language::TypeScript => infer_def_kind_typescript(kind_str),
        Language::Python => infer_def_kind_python(kind_str),
        Language::Unknown => NodeKind::Variable,
    }
}

fn infer_def_kind_rust(kind_str: &str, name: &str, state: &ExtractionState) -> NodeKind {
    match kind_str {
        "function_item" => NodeKind::Function,
        "struct_item" => NodeKind::Struct,
        "enum_item" => NodeKind::Enum,
        "trait_item" => NodeKind::Trait,
        "type_item" => NodeKind::Type,
        "mod_item" => NodeKind::Module,
        "const_item" | "static_item" => NodeKind::Variable,
        "impl_item" => state
            .name_to_ids
            .get(name)
            .and_then(|ids| ids.first())
            .and_then(|id| state.nodes.get(*id as usize))
            .map(|node| node.kind)
            .unwrap_or(NodeKind::Type),
        _ => NodeKind::Variable,
    }
}

fn infer_def_kind_typescript(kind_str: &str) -> NodeKind {
    match kind_str {
        "function_declaration" | "generator_function_declaration" => NodeKind::Function,
        "class_declaration" => NodeKind::Class,
        "interface_declaration" => NodeKind::Interface,
        "type_alias_declaration" => NodeKind::Type,
        "method_definition" => NodeKind::Method,
        "variable_declarator" | "lexical_declaration" => NodeKind::Variable,
        _ => NodeKind::Variable,
    }
}

fn infer_def_kind_python(kind_str: &str) -> NodeKind {
    match kind_str {
        "function_definition" => NodeKind::Function,
        "class_definition" => NodeKind::Class,
        _ => NodeKind::Variable,
    }
}
