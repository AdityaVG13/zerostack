//! Definition and import line classification for grep hits.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineClass {
    Import,
    Definition,
    Other,
}

pub fn classify_line(line: &str) -> LineClass {
    let t = line.trim_start();
    if t.starts_with("use ")
        || t.starts_with("import ")
        || t.starts_with("from ")
        || t.starts_with("#include")
    {
        return LineClass::Import;
    }
    if t.starts_with("fn ")
        || t.starts_with("pub fn ")
        || t.starts_with("def ")
        || t.starts_with("class ")
        || t.starts_with("struct ")
        || t.starts_with("enum ")
        || t.starts_with("impl ")
    {
        return LineClass::Definition;
    }
    LineClass::Other
}
