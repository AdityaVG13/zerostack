//! Built-in recipes — compile to native `fs.*` program steps (no special runtime).

use super::program::{
    Program, Step, bound_read_step, call_step, named_call_step, parallel_branch, parallel_step,
};
use crate::core::{FSZeroSession, decode_wire_path};
use serde_json::json;

pub fn try_recipe_with_session(plan: &str, session: &FSZeroSession) -> Option<Program> {
    if plan == "explore" {
        return Some(explore_program(None));
    }
    if let Some(scope) = plan.strip_prefix("explore:") {
        return Some(explore_program(Some(scope.trim())));
    }
    if let Some(sym) = plan.strip_prefix("impact:") {
        let sym = sym.trim();
        let read_when_indexed = file_for_symbol(session, sym).is_some();
        return Some(impact_program(sym, read_when_indexed));
    }
    if let Some(sym) = plan.strip_prefix("refactor:") {
        return Some(refactor_program(sym.trim(), session));
    }
    if let Some(intent) = plan.strip_prefix("compound:") {
        return Some(Program::single(
            "fs.compound",
            json!({ "intent": intent.trim() }),
            "compound",
        ));
    }
    if let Some(rest) = plan.strip_prefix("structural:") {
        return Some(structural_program(rest.trim()));
    }
    if let Some(query) = plan
        .strip_prefix("ast-sgrep:")
        .or_else(|| plan.strip_prefix("asgrep:"))
    {
        return Some(Program::single(
            "fs.search",
            json!({ "query": format!("ast-sgrep:{}", query.trim()) }),
            "ast-sgrep",
        ));
    }
    if let Some(rest) = plan.strip_prefix("memory:") {
        return Some(memory_program(rest.trim()));
    }
    None
}

fn memory_program(rest: &str) -> Program {
    if let Some(body) = rest.strip_prefix("put:") {
        let (path_enc, content) = body.split_once('|').unwrap_or((body, ""));
        let path = decode_wire_path(path_enc.trim());
        return Program::single(
            "fs.memory.put",
            json!({ "path": path, "content": content }),
            "memory-put",
        );
    }
    if let Some(path) = rest.strip_prefix("get:") {
        return Program::single(
            "fs.memory.get",
            json!({ "path": path.trim() }),
            "memory-get",
        );
    }
    if rest == "ls" || rest.is_empty() {
        return Program::single("fs.memory.ls", json!({}), "memory-ls");
    }
    if let Some(prefix) = rest.strip_prefix("ls:") {
        return Program::single(
            "fs.memory.ls",
            json!({ "prefix": prefix.trim() }),
            "memory-ls",
        );
    }
    if let Some(path) = rest.strip_prefix("delete:") {
        return Program::single(
            "fs.memory.delete",
            json!({ "path": path.trim() }),
            "memory-delete",
        );
    }
    if let Some(body) = rest.strip_prefix("rename:") {
        let (from_enc, to_enc) = body.split_once('|').unwrap_or((body, ""));
        let from = decode_wire_path(from_enc.trim());
        let to = decode_wire_path(to_enc.trim());
        return Program::single(
            "fs.memory.rename",
            json!({ "from": from, "to": to }),
            "memory-rename",
        );
    }
    // Default: treat as get path.
    Program::single("fs.memory.get", json!({ "path": rest }), "memory-get")
}

fn step_ls(arg: Option<&str>) -> Step {
    Step {
        call: "fs.ls".to_string(),
        args: match arg {
            Some(a) => json!({ "arg": a }),
            None => json!({}),
        },
    }
}

pub fn explore_program(scope: Option<&str>) -> Program {
    let ls_arg = scope.map(|s| format!("--depth=2 {s}"));
    Program {
        label: format!("explore:{}", scope.unwrap_or("workspace")),
        steps: vec![
            step_ls(ls_arg.as_deref()).into_plan_step(),
            // Width bounded by MAX_PARALLEL_WIDTH (=2): the defs probe runs as a
            // named sequential step so the built-in recipe always validates.
            parallel_step(vec![
                parallel_branch("fn", "fs.search", json!({ "query": "fn " })),
                parallel_branch("imports", "fs.search", json!({ "query": "imports" })),
            ]),
            named_call_step("defs", "fs.search", json!({ "query": "defs:main" })),
            bound_read_step("$defs.path", vec!["defs".to_string()]),
        ],
        transaction: Default::default(),
    }
}

pub fn impact_program(symbol: &str, read_when_indexed: bool) -> Program {
    let sym = symbol.trim();
    let mut steps = vec![parallel_step(vec![
        parallel_branch(
            "defs",
            "fs.search",
            json!({ "query": format!("defs:{sym}") }),
        ),
        parallel_branch(
            "callers",
            "fs.search",
            json!({ "query": format!("callers:{sym}") }),
        ),
    ])];
    if read_when_indexed {
        steps.push(bound_read_step("$defs.path", vec!["defs".to_string()]));
    }
    Program {
        label: format!("impact:{sym}"),
        steps,
        transaction: Default::default(),
    }
}

pub fn refactor_program(symbol: &str, session: &FSZeroSession) -> Program {
    let sym = symbol.trim();
    let read_when_indexed = file_for_symbol(session, sym).is_some();
    let mut steps = impact_program(sym, read_when_indexed).steps;
    steps.push(call_step(
        "fs.compound",
        json!({ "intent": format!("refactor-{sym}") }),
    ));
    Program {
        label: format!("refactor:{sym}"),
        steps,
        transaction: Default::default(),
    }
}

fn structural_program(rest: &str) -> Program {
    let (q, sym) = if let Some((q, sym)) = rest.split_once(':') {
        (q.trim(), Some(sym.trim()))
    } else {
        (rest, None)
    };
    let query = normalize_structural(q, sym);
    Program::single(
        "fs.search",
        json!({ "query": query }),
        format!("structural:{query}"),
    )
}

fn normalize_structural(q: &str, target: Option<&str>) -> String {
    let sym = target
        .filter(|s| !s.is_empty() && *s != "m" && *s != "mod")
        .unwrap_or("main");
    if q.starts_with("callers:") || q.starts_with("defs:") {
        return q.to_string();
    }
    match q {
        "callers" | "impact" => format!("callers:{sym}"),
        "tests" | "test" => format!("callers:{sym}"),
        "defs" | "def" => format!("defs:{sym}"),
        "imports" | "import" => "imports".to_string(),
        _ => q.to_string(),
    }
}

pub fn file_for_symbol(session: &FSZeroSession, sym: &str) -> Option<String> {
    session
        .index
        .symbols
        .iter()
        .find(|(name, _)| name == sym)
        .map(|(_, file)| file.clone())
}
