//! Native `fs.*` connector API catalog — discovery search/describe source of truth.

#[derive(Debug, Clone, Copy)]
pub struct MethodDef {
    pub path: &'static str,
    pub description: &'static str,
    pub signature: &'static str,
    pub keywords: &'static [&'static str],
}

const fn md(
    path: &'static str,
    description: &'static str,
    signature: &'static str,
    keywords: &'static [&'static str],
) -> MethodDef {
    MethodDef {
        path,
        description,
        signature,
        keywords,
    }
}

pub const METHODS: &[MethodDef] = &[
    md(
        "fs.ls",
        "List workspace files (depth, glob, budget); paths are session-root-relative, absolute paths are rejected",
        "fs.ls(args?: { arg?: string }): zero-result",
        &["list", "directory", "glob", "tree"],
    ),
    md(
        "fs.read",
        "Read file; large first-serve payloads emit capsule + total_bytes/capsule_bytes + next_offset/remaining when truncated (visible_budget_tokens=800); resume via start_line or expand(ref#L…)",
        "fs.read(args: { path: string }): zero-result",
        &["read", "file", "content", "bytes", "capsule", "pagination"],
    ),
    md(
        "fs.search",
        "Literal-substring, structural, or ast-sgrep search (query is NOT a regex; use fs.multiSearch with regex:true)",
        "fs.search(args: { query: string }): zero-result",
        &["search", "grep", "callers", "defs", "ast-sgrep", "asgrep"],
    ),
    md(
        "fs.multiRead",
        "Vectorized snapshot-consistent reads with ordered item errors",
        "fs.multiRead(args: { paths: (string | { path, range?, max_bytes? })[] }): zero-result",
        &["read", "batch", "many", "files", "multi_read", "multiRead"],
    ),
    md(
        "fs.multiList",
        "Vectorized directory-trie batch listings with depth and hidden filtering",
        "fs.multiList(args: { items: (string | { path, depth?, include_hidden? })[] }): zero-result",
        &[
            "list",
            "batch",
            "many",
            "directory",
            "dirent",
            "multi_list",
            "multiList",
        ],
    ),
    md(
        "fs.multiSearch",
        "Vectorized multi-query search with one indexed traversal",
        "fs.multiSearch(args: { queries: (string | { query, paths?, limit?, case?, regex? })[], limit? }): zero-result",
        &[
            "search",
            "batch",
            "many",
            "queries",
            "multi_search",
            "multiSearch",
        ],
    ),
    md(
        "fs.multiAstSearch",
        "Vectorized multi-pattern AST search: all patterns of a language are evaluated during ONE parse of each file",
        "fs.multiAstSearch(args: { items: { language, pattern, paths?, limit? }[] }): zero-result",
        &[
            "ast",
            "search",
            "batch",
            "many",
            "pattern",
            "structural",
            "sgrep",
            "multi_ast_search",
            "multiAstSearch",
        ],
    ),
    md(
        "fs.edit",
        "Guarded edit with preimage",
        "fs.edit(args: { spec: string, base? }): zero-result",
        &["edit", "patch", "replace"],
    ),
    md(
        "fs.write",
        "Create or overwrite a workspace file",
        "fs.write(args: { path: string, content: string, base? }): zero-result",
        &["write", "create", "overwrite", "put"],
    ),
    md(
        "fs.transact",
        "All-or-nothing multi-step edit/write with CAS base gates and journaled rollback",
        "fs.transact(args: { steps: { op: \"edit\"|\"write\", path, find?, replace?, content?, base? }[] }): zero-result",
        &["transact", "transaction", "atomic", "multi", "rollback"],
    ),
    md(
        "fs.compound",
        "Server-side multi-op bundle or verifiedEdit compound",
        "fs.compound(args: { intent: string } | { name: 'list', path?: workspace-relative string, depth?: number, pattern?: string, budget?: number } | { name: 'verifiedEdit', path: string, edits: {old,new}[], verify?: string }): zero-result",
        &["compound", "bundle", "batch", "verifiedEdit", "verify"],
    ),
    md(
        "fs.expand",
        "Exact bytes for a prior ref; ref#L<start>-<end> for line windows; large expands may return window_hint for paging",
        "fs.expand(args: { ref: string }): zero-result",
        &["expand", "exact", "bytes", "window", "blob", "pagination"],
    ),
    md(
        "fs.stat",
        "File metadata behind ref",
        "fs.stat(args: { path: string }): zero-result",
        &["stat", "metadata", "mtime"],
    ),
    md(
        "fs.multiStat",
        "Vectorized file metadata requests with ordered item errors",
        "fs.multiStat(args: { paths: (string | { path })[] }): zero-result",
        &["stat", "metadata", "batch", "many"],
    ),
    // Product decision (fszero-fyx0.4): PRESENT on CodeMode. Runtime/connector
    // already bind `resolve`; exclude would leave a ghost JS path. Catalog +
    // registry alias + schemas must stay lockstep with MCP fszero.resolve.
    md(
        "fs.resolve",
        "Resolve a natural-language intent to ranked workspace paths",
        "fs.resolve(args: { intent: string, engine?: 'lexical'|'semantic'|'hybrid', limit?: number }): zero-result",
        &["resolve", "find", "locate", "intent", "path"],
    ),
    md(
        "fs.world",
        "Access ledger queries (hot/recent/coaccess) or speculative world new/commit/drop. MCP splits this into fszero.world + fszero.world_query; CodeMode is one method (no fs.world_query).",
        "fs.world(args: { query?: 'hot'|'recent'|'coaccess', path?: string, limit?: number } | { action?: string, arg?: string, path?: string, ... }): zero-result",
        &[
            "world",
            "hot",
            "recent",
            "coaccess",
            "speculative",
            "commit",
            "world_query",
        ],
    ),
    md(
        "fs.history",
        "List recoverable history entries for a path",
        "fs.history(args?: { path?: string, limit?: number }): zero-result",
        &["history", "versions", "journal", "undo-list"],
    ),
    md(
        "fs.undo",
        "Restore a prior history revision for a path",
        "fs.undo(args?: { path?: string, seq?: number }): zero-result",
        &["undo", "restore", "rollback", "history"],
    ),
    md(
        "fs.memory.put",
        "Durable agent memory write (mem://) — ack + fz://blob ref; bytes stay server-side",
        "fs.memory.put(args: { path: string, content: string }): zero-result",
        &["memory", "mem", "put", "persist", "recall", "constraints"],
    ),
    md(
        "fs.memory.get",
        "Durable agent memory read — ack + fz://blob ref; expand for exact bytes",
        "fs.memory.get(args: { path: string }): zero-result",
        &["memory", "mem", "get", "recall", "read"],
    ),
    md(
        "fs.memory.ls",
        "List durable memory paths under an optional prefix",
        "fs.memory.ls(args?: { prefix?: string }): zero-result",
        &["memory", "mem", "list", "ls", "inventory"],
    ),
    md(
        "fs.memory.delete",
        "Delete a durable memory path",
        "fs.memory.delete(args: { path: string }): zero-result",
        &["memory", "mem", "delete", "rm", "remove"],
    ),
    md(
        "fs.memory.rename",
        "Rename a durable memory path",
        "fs.memory.rename(args: { from: string, to: string }): zero-result",
        &["memory", "mem", "rename", "mv", "move"],
    ),
];

/// Whether `call` is a registered native `fs.*` kernel method.
pub fn is_kernel_method(call: &str) -> bool {
    METHODS.iter().any(|m| m.path == call)
}

pub const RECIPES: &[(&str, &str, &str)] = &[
    (
        "recipe.explore",
        "Workspace overview — ls + fn scan + imports + defs",
        "program: explore | explore:<scope>",
    ),
    (
        "recipe.impact",
        "Symbol impact — defs + callers + read primary file",
        "program: impact:<symbol>",
    ),
    (
        "recipe.refactor",
        "Refactor prep — impact + compound bundle",
        "program: refactor:<symbol>",
    ),
    (
        "recipe.compound",
        "Server-side compound bundle by intent",
        "program: compound:<intent>",
    ),
    (
        "recipe.structural",
        "Structural search shorthand (callers, defs, imports)",
        "program: structural:<query> | structural:<query>:<symbol>",
    ),
    (
        "recipe.ast-sgrep",
        "AST structural search via ast-sgrep query",
        "program: ast-sgrep:<query> | asgrep:<query>",
    ),
    (
        "recipe.memory",
        "Durable memory put/get/ls — path-keyed mem:// with fz:// refs",
        "program: memory:put:<path>|<content> | memory:get:<path> | memory:ls[:prefix]",
    ),
];

/// Whether `describe` resolves to a known method, recipe, or alias.
pub fn is_known_target(path: &str) -> bool {
    if METHODS.iter().any(|m| m.path == path) {
        return true;
    }
    if RECIPES.iter().any(|(p, _, _)| *p == path) {
        return true;
    }
    // Short aliases only — full `recipe.*` paths already hit RECIPES above.
    matches!(
        path,
        "explore"
            | "impact"
            | "refactor"
            | "compound"
            | "structural"
            | "ast-sgrep"
            | "asgrep"
            | "memory"
            | "codemode.search"
            | "codemode.describe"
    )
}

fn score(query: &str, path: &str, desc: &str, keywords: &[&str]) -> i32 {
    let q = query.to_lowercase();
    if q.is_empty() {
        return 0;
    }
    let mut s = 0i32;
    let path_l = path.to_lowercase();
    let desc_l = desc.to_lowercase();
    if path_l.contains(&q) {
        s += 10;
    }
    if desc_l.contains(&q) {
        s += 5;
    }
    for word in q.split_whitespace() {
        if path_l.contains(word) {
            s += 3;
        }
        if desc_l.contains(word) {
            s += 1;
        }
        for kw in keywords {
            if kw.contains(word) || word.contains(kw) {
                s += 2;
            }
        }
    }
    s
}

pub fn search_methods(query: &str) -> Vec<(&'static MethodDef, i32)> {
    let mut ranked: Vec<(&'static MethodDef, i32)> = METHODS
        .iter()
        .map(|m| (m, score(query, m.path, m.description, m.keywords)))
        .filter(|(_, s)| *s > 0 || query.trim().is_empty())
        .collect();
    ranked.sort_by_key(|(_, s)| std::cmp::Reverse(*s));
    ranked
}

pub fn search_all(query: &str) -> String {
    let mut lines: Vec<String> = search_methods(query)
        .into_iter()
        .take(12)
        .map(|(m, sc)| format!("{}\t{}\t{}\tscore={sc}", m.path, m.description, m.signature))
        .collect();
    for (path, desc, sig) in RECIPES {
        let sc = score(query, path, desc, &[]);
        if sc > 0 || query.trim().is_empty() {
            lines.push(format!("{path}\t{desc}\t{sig}\tscore={sc}"));
        }
    }
    lines.join("\n")
}

pub fn describe(path: &str) -> String {
    if let Some(m) = METHODS.iter().find(|m| m.path == path) {
        return m.signature.to_string();
    }
    if let Some((_, desc, sig)) = RECIPES.iter().find(|(p, _, _)| *p == path) {
        return format!("{path}: {desc} ({sig})");
    }
    // Full `recipe.*` paths already returned from RECIPES above; only short
    // aliases and codemode meta remain here.
    match path {
        "explore" => "program: explore | explore:<scope> — compiles to fs.ls + fs.search steps".to_string(),
        "impact" => "program: impact:<symbol> — fs.search defs/callers + fs.read".to_string(),
        "refactor" => "program: refactor:<symbol> — impact recipe + fs.compound".to_string(),
        "compound" => "program: compound:<intent> — fs.compound server-side bundle".to_string(),
        "structural" => "program: structural:<query> — compiles to fs.search with structural shorthand".to_string(),
        "ast-sgrep" | "asgrep" => "program: ast-sgrep:<query> — fs.search with ast-sgrep: prefix".to_string(),
        "memory" => { "program: memory:put:<path>|<content> | memory:get:<path> | memory:ls[:prefix] | memory:delete:<path> | memory:rename:<from>|<to> — durable mem:// with fz:// refs".to_string() }
        "codemode.search" => "codemode.search(query): ranked fs.* methods and recipes".to_string(),
        "codemode.describe" => "codemode.describe(target): TypeScript-style signature".to_string(),
        _ => format!("unknown '{path}'; use codemode.search first"), }
}
