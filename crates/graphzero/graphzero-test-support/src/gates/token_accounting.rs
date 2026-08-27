//! Dual-metric token accounting: GraphZero byte heuristic vs real tokenizer counts.
//!
//! Separates **capsule shell** (payload text) from **MCP turn** (tool input + host envelope + result).

use std::sync::OnceLock;

use ah_ah_ah::{Backend, count_tokens};
use graphzero_store::store::query::tokens_for_str;
use serde_json::json;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenizerFamily {
    Heuristic,
    Cl100k,
    O200k,
    Claude,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct TokenCounts {
    pub heuristic: usize,
    pub cl100k: usize,
    pub o200k: usize,
    pub claude: usize,
}

impl TokenCounts {
    pub fn for_text(text: &str) -> Self {
        Self {
            heuristic: tokens_for_str(text),
            cl100k: count_cl100k(text),
            o200k: count_o200k(text),
            claude: count_claude(text),
        }
    }

    pub fn get(&self, family: TokenizerFamily) -> usize {
        match family {
            TokenizerFamily::Heuristic => self.heuristic,
            TokenizerFamily::Cl100k => self.cl100k,
            TokenizerFamily::O200k => self.o200k,
            TokenizerFamily::Claude => self.claude,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct McpTurnEstimate {
    /// Payload returned by GraphZero (`g:26`, spilled JSON shell, etc.).
    pub capsule_shell: TokenCounts,
    /// `mcp_text_result` JSON envelope around the payload (GraphZero MCP stdio).
    pub mcp_stdio_result: TokenCounts,
    /// Anthropic-style assistant `tool_use` block (serialized JSON).
    pub anthropic_tool_use: TokenCounts,
    /// Anthropic-style `tool_result` user message with text content only.
    pub anthropic_tool_result: TokenCounts,
    /// OpenAI-style function call arguments + function output string.
    pub openai_function_turn: TokenCounts,
    /// Sum: anthropic tool_use + anthropic tool_result (approximate billed turn).
    pub anthropic_turn_total: TokenCounts,
}

fn cl100k() -> &'static tiktoken_rs::CoreBPE {
    static BPE: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::cl100k_base().expect("cl100k_base"))
}

fn o200k() -> &'static tiktoken_rs::CoreBPE {
    static BPE: OnceLock<tiktoken_rs::CoreBPE> = OnceLock::new();
    BPE.get_or_init(|| tiktoken_rs::o200k_base().expect("o200k_base"))
}

pub fn count_cl100k(text: &str) -> usize {
    cl100k().encode_ordinary(text).len()
}

pub fn count_o200k(text: &str) -> usize {
    o200k().encode_ordinary(text).len()
}

pub fn count_claude(text: &str) -> usize {
    count_tokens(text, None, Backend::Claude, None).count
}

/// Mirrors `mcp_text_result` in `graphzero-cli/src/mcp.rs`.
pub fn mcp_stdio_envelope(payload: &str) -> String {
    serde_json::to_string(&json!({
        "content": [{ "type": "text", "text": payload }],
        "isError": false,
    }))
    .expect("mcp envelope json")
}

pub fn anthropic_tool_use_json(tool_name: &str, input: &serde_json::Value) -> String {
    serde_json::to_string(&json!({
        "type": "tool_use",
        "id": "toolu_gate_fixture",
        "name": tool_name,
        "input": input,
    }))
    .expect("anthropic tool_use json")
}

pub fn anthropic_tool_result_json(payload: &str) -> String {
    serde_json::to_string(&json!({
        "type": "tool_result",
        "tool_use_id": "toolu_gate_fixture",
        "content": payload,
    }))
    .expect("anthropic tool_result json")
}

pub fn openai_function_turn_json(
    tool_name: &str,
    input: &serde_json::Value,
    output: &str,
) -> String {
    let args = serde_json::to_string(input).expect("openai args");
    serde_json::to_string(&json!({
        "function_call": {
            "name": tool_name,
            "arguments": args,
        },
        "function_result": output,
    }))
    .expect("openai function turn json")
}

pub fn estimate_orient_turn(
    tool_name: &str,
    surface: &str,
    query: &str,
    payload: &str,
) -> McpTurnEstimate {
    let input = json!({
        "surface": surface,
        "query": query,
        "budget": 1,
        "repo": ".",
    });
    let capsule_shell = TokenCounts::for_text(payload);
    let mcp_stdio = TokenCounts::for_text(&mcp_stdio_envelope(payload));
    let tool_use = TokenCounts::for_text(&anthropic_tool_use_json(tool_name, &input));
    let tool_result = TokenCounts::for_text(&anthropic_tool_result_json(payload));
    let openai_turn = TokenCounts::for_text(&openai_function_turn_json(tool_name, &input, payload));
    let anthropic_turn_total = TokenCounts {
        heuristic: tool_use.heuristic + tool_result.heuristic,
        cl100k: tool_use.cl100k + tool_result.cl100k,
        o200k: tool_use.o200k + tool_result.o200k,
        claude: tool_use.claude + tool_result.claude,
    };
    McpTurnEstimate {
        capsule_shell,
        mcp_stdio_result: mcp_stdio,
        anthropic_tool_use: tool_use,
        anthropic_tool_result: tool_result,
        openai_function_turn: openai_turn,
        anthropic_turn_total,
    }
}

pub fn record_token_counts(step: &mut serde_json::Value, counts: &TokenCounts, prefix: &str) {
    if let Some(obj) = step.as_object_mut() {
        obj.insert(format!("{prefix}_heuristic"), counts.heuristic.into());
        obj.insert(format!("{prefix}_cl100k"), counts.cl100k.into());
        obj.insert(format!("{prefix}_o200k"), counts.o200k.into());
        obj.insert(format!("{prefix}_claude"), counts.claude.into());
    }
}

pub fn record_mcp_turn(step: &mut serde_json::Value, turn: &McpTurnEstimate) {
    record_token_counts(step, &turn.capsule_shell, "shell");
    record_token_counts(step, &turn.mcp_stdio_result, "mcp_stdio");
    record_token_counts(step, &turn.anthropic_turn_total, "anthropic_turn");
    record_token_counts(step, &turn.openai_function_turn, "openai_turn");
}
