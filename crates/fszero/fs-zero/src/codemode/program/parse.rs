use super::types::{ParallelBranch, ParallelOnError, PlanStep, Program, Step, TransactionMode};
use super::validate::validate_program;
use serde_json::Value;

pub fn parse_program(plan: &str) -> Result<Program, String> {
    let plan = plan.trim();
    if plan.is_empty() {
        return Err("empty plan".to_string());
    }
    if plan.starts_with('{') {
        return parse_json_program(plan);
    }
    parse_script_program(plan)
}
fn parse_needs(step: &Value, index: usize) -> Result<Vec<String>, String> {
    match step.get("needs") {
        None => Ok(Vec::new()),
        Some(Value::Array(arr)) => {
            let mut needs = Vec::with_capacity(arr.len());
            for (ni, item) in arr.iter().enumerate() {
                let s = item
                    .as_str()
                    .ok_or_else(|| format!("step {index} needs[{ni}]: must be string"))?;
                needs.push(s.to_string());
            }
            Ok(needs)
        }
        Some(_) => Err(format!("step {index}: needs must be array")),
    }
}

fn parse_json_program(raw: &str) -> Result<Program, String> {
    let v: Value = serde_json::from_str(raw).map_err(|e| format!("json plan: {e}"))?;
    let label = v
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("json")
        .to_string();
    let transaction = TransactionMode::parse(v.get("transaction"));
    let steps_v = v
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("json plan: missing steps array")?;
    let mut steps = Vec::with_capacity(steps_v.len());
    for (i, step) in steps_v.iter().enumerate() {
        steps.push(parse_json_step(step, i)?);
    }
    if steps.is_empty() {
        return Err("json plan: no steps".to_string());
    }
    let program = Program {
        steps,
        label,
        transaction,
    };
    validate_program(&program)?;
    Ok(program)
}

fn parse_json_step(step: &Value, index: usize) -> Result<PlanStep, String> {
    let has_call = step.get("call").is_some();
    let has_parallel = step.get("parallel").is_some();
    if has_call && has_parallel {
        return Err(format!(
            "step {index}: cannot specify both call and parallel"
        ));
    }
    if let Some(parallel) = step.get("parallel").and_then(Value::as_array) {
        let id = step.get("id").and_then(Value::as_str).map(str::to_string);
        let on_error = ParallelOnError::parse(step.get("on_error").and_then(Value::as_str));
        let needs = parse_needs(step, index)?;
        let mut branches = Vec::with_capacity(parallel.len());
        for (bi, branch) in parallel.iter().enumerate() {
            let call = branch
                .get("call")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("step {index} parallel[{bi}]: missing call"))?
                .to_string();
            let args = branch
                .get("args")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let id = branch
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("p{bi}"));
            branches.push(ParallelBranch { id, call, args });
        }
        return Ok(PlanStep::Parallel {
            id,
            branches,
            on_error,
            needs,
        });
    }
    let call = step
        .get("call")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("step {index}: missing call or parallel"))?
        .to_string();
    let args = step
        .get("args")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let id = step.get("id").and_then(Value::as_str).map(str::to_string);
    let needs = parse_needs(step, index)?;
    Ok(PlanStep::Call {
        id,
        call,
        args,
        needs,
    })
}

fn parse_script_program(plan: &str) -> Result<Program, String> {
    let statements: Vec<&str> = plan
        .split([';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "{" && *s != "}")
        .collect();
    if statements.is_empty() {
        return Err("script plan: no statements".to_string());
    }
    let mut steps = Vec::new();
    for stmt in statements {
        if stmt.starts_with("return") {
            break;
        }
        if let Some(step) = parse_script_statement(stmt) {
            steps.push(step.into_plan_step());
        } else if stmt.contains("zero.graph") || stmt.contains("zero.token") {
            continue;
        } else {
            return Err(format!("script plan: unrecognized statement: {stmt}"));
        }
    }
    if steps.is_empty() {
        return Err("script plan: no fs calls".to_string());
    }
    let program = Program {
        steps,
        label: "script".to_string(),
        transaction: TransactionMode::Auto,
    };
    validate_program(&program)?;
    Ok(program)
}

fn parse_script_statement(stmt: &str) -> Option<Step> {
    if stmt.starts_with("compound:") {
        let intent = stmt.strip_prefix("compound:").unwrap_or("").trim();
        return Some(Step {
            call: "fs.compound".to_string(),
            args: serde_json::json!({ "intent": intent }),
        });
    }
    if stmt.contains("zero.fs.plan") || stmt.contains("fs.plan(") || stmt.contains(".plan(") {
        let goal = extract_quoted_after(stmt, "plan('")
            .or_else(|| extract_quoted_after(stmt, "plan(\""))
            .unwrap_or_else(|| "load module".to_string());
        // Lower to the named plan recipe so the connector enforces the goal's
        // terminal action instead of silently succeeding on a listing.
        return Some(Step {
            call: "fs.compound".to_string(),
            args: serde_json::json!({ "name": "plan", "goal": goal }),
        });
    }
    let mappings: [(&str, &str, &str); 7] = [
        ("fs.ls(", "fs.ls", "arg"),
        ("fs.read(", "fs.read", "path"),
        ("fs.search(", "fs.search", "query"),
        ("fs.edit(", "fs.edit", "spec"),
        ("fs.compound(", "fs.compound", "intent"),
        ("fs.stat(", "fs.stat", "path"),
        ("fs.world(", "fs.world", "arg"),
    ];
    for (needle, call, key) in mappings {
        if stmt.contains(needle) || stmt.contains(&call.replace('.', ".fs.")) {
            let short = call.split('.').nth(1).unwrap_or("");
            let arg = extract_quoted_after(stmt, &format!("{short}('"))
                .or_else(|| extract_quoted_after(stmt, &format!("{short}(\"")))
                .or_else(|| extract_first_quoted(stmt));
            let mut map = serde_json::Map::new();
            if let Some(a) = arg {
                map.insert(key.to_string(), Value::String(a));
            }
            return Some(Step {
                call: call.to_string(),
                args: Value::Object(map),
            });
        }
    }
    None
}

fn extract_first_quoted(stmt: &str) -> Option<String> {
    extract_quoted_after(stmt, "('").or_else(|| extract_quoted_after(stmt, "(\""))
}

fn extract_quoted_after(hay: &str, needle: &str) -> Option<String> {
    let start = hay.find(needle)? + needle.len();
    let rest = &hay[start..];
    let close = if needle.contains('\'') { '\'' } else { '"' };
    let mut out = String::new();
    for ch in rest.chars() {
        if ch == close {
            break;
        }
        out.push(ch);
    }
    Some(out)
}
