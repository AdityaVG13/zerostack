#[derive(Debug)]
pub struct IndexedFile {
    pub file_key: String,
    pub fns: Vec<IndexedFn>,
    pub imports: Vec<IndexedImport>,
    pub calls: Vec<(String, String, usize)>,
}

#[derive(Debug, Clone)]
pub struct IndexedFn {
    pub span_start: usize,
    pub span_end: usize,
    pub name: String,
    pub kind: SymbolNodeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolNodeKind {
    Fn,
    Method,
    Type,
    Enum,
    Interface,
    Class,
}

impl SymbolNodeKind {
    pub fn as_db_kind(self) -> &'static str {
        match self {
            SymbolNodeKind::Fn => "fn",
            SymbolNodeKind::Method => "method",
            SymbolNodeKind::Type => "type",
            SymbolNodeKind::Enum => "enum",
            SymbolNodeKind::Interface => "interface",
            SymbolNodeKind::Class => "class",
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexedImport {
    pub span_start: usize,
    pub span_end: usize,
    pub name: String,
}
