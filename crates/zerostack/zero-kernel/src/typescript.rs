//! Bounded TypeScript syntax erasure for ZeroKernel cells. The evaluator executes JavaScript
//! semantics. Type-only syntax is replaced with spaces while preserving newlines and byte offsets.

use tree_sitter::{Node, Parser};
use tree_sitter_typescript::LANGUAGE_TYPESCRIPT;

#[derive(Debug, thiserror::Error)]
pub enum TypeScriptError {
    #[error("TypeScript parser setup failed: {0}")]
    Parser(String),
    #[error("TypeScript syntax error")]
    Syntax,
    #[error("unsupported runtime TypeScript construct: {0}")]
    Unsupported(String),
}

pub fn erase_typescript(source: &str) -> Result<String, TypeScriptError> {
    let mut parser = Parser::new();
    parser
        .set_language(&LANGUAGE_TYPESCRIPT.into())
        .map_err(|error| TypeScriptError::Parser(error.to_string()))?;
    const PREFIX: &str = "async function __zero_kernel_cell() {\n";
    const SUFFIX: &str = "\n}";
    let wrapped = format!("{PREFIX}{source}{SUFFIX}");
    let tree = parser
        .parse(&wrapped, None)
        .ok_or(TypeScriptError::Syntax)?;
    if tree.root_node().has_error() {
        return Err(TypeScriptError::Syntax);
    }
    let mut wrapped_ranges = Vec::new();
    collect_erased_ranges(tree.root_node(), &mut wrapped_ranges)?;
    let source_start = PREFIX.len();
    let source_end = source_start + source.len();
    let mut ranges = wrapped_ranges
        .into_iter()
        .filter_map(|(start, end)| {
            let start = start.max(source_start);
            let end = end.min(source_end);
            (start < end).then_some((start - source_start, end - source_start))
        })
        .collect::<Vec<_>>();
    ranges.sort_unstable();
    merge_ranges(&mut ranges);
    let mut bytes = source.as_bytes().to_vec();
    for (start, end) in ranges {
        for byte in &mut bytes[start..end] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(bytes).map_err(|_| TypeScriptError::Syntax)
}

fn collect_erased_ranges(
    node: Node<'_>,
    ranges: &mut Vec<(usize, usize)>,
) -> Result<(), TypeScriptError> {
    match node.kind() {
        "enum_declaration" | "namespace_declaration" | "module_declaration" => {
            return Err(TypeScriptError::Unsupported(node.kind().into()));
        }
        "interface_declaration"
        | "type_alias_declaration"
        | "type_annotation"
        | "type_parameters"
        | "type_arguments"
        | "accessibility_modifier"
        | "abstract_modifier"
        | "declare" => {
            ranges.push((node.start_byte(), node.end_byte()));
            return Ok(());
        }
        "as_expression" | "satisfies_expression" => {
            if let Some(expression) = node.named_child(0) {
                ranges.push((expression.end_byte(), node.end_byte()));
                collect_erased_ranges(expression, ranges)?;
                return Ok(());
            }
        }
        "non_null_expression" => {
            if let Some(expression) = node.named_child(0) {
                ranges.push((expression.end_byte(), node.end_byte()));
                collect_erased_ranges(expression, ranges)?;
                return Ok(());
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_erased_ranges(child, ranges)?;
    }
    Ok(())
}

fn merge_ranges(ranges: &mut Vec<(usize, usize)>) {
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges.drain(..) {
        if start >= end {
            continue;
        }
        match merged.last_mut() {
            Some((_, previous_end)) if start <= *previous_end => {
                *previous_end = (*previous_end).max(end);
            }
            _ => merged.push((start, end)),
        }
    }
    *ranges = merged;
}
