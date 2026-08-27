//! Parser setup for Tier-A extraction.

use tree_sitter::{Parser, Tree};

use crate::{BlobInput, Language};

pub(super) fn grammar_for_language(lang: Language) -> tree_sitter::Language {
    match lang {
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::Unknown => unreachable!(),
    }
}

pub(super) fn parse_blob_tree(input: &BlobInput, lang: Language) -> Option<Tree> {
    thread_local! {
        static PARSER: std::cell::RefCell<Parser> = std::cell::RefCell::new(Parser::new());
    }
    let grammar_lang = grammar_for_language(lang);
    PARSER.with(|p| {
        let mut parser = p.borrow_mut();
        parser.set_language(&grammar_lang).ok()?;
        parser.parse(input.content, None)
    })
}
