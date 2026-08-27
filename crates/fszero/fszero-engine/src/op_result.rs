use super::session::OpCode;

/// Shared `{op}:0 ({detail})` failure string for kernel ops.
#[inline]
pub fn op0(op: &str, detail: impl std::fmt::Display) -> String {
    format!("{op}:0 ({detail})")
}

#[inline]
pub fn metadata_failed(e: impl std::fmt::Display) -> String {
    format!("metadata failed: {e}")
}

#[inline]
pub fn bad_path(e: impl std::fmt::Display) -> String {
    format!("bad path: {e}")
}

#[inline]
pub fn read_failed(e: impl std::fmt::Display) -> String {
    format!("read failed: {e}")
}

#[inline]
pub fn journal_err(e: impl std::fmt::Display) -> String {
    format!("journal: {e}")
}

#[derive(Debug, Clone)]
pub struct OpDetail {
    pub ok: bool,
    pub message: String,
}

impl OpDetail {
    pub fn visible_error(&self, op: OpCode) -> String {
        if self.ok {
            return visible_ack(op, None);
        }
        let _ = &self.message;
        match op {
            OpCode::Edit => "E0".to_string(),
            _ => "X0".to_string(),
        }
    }
}

pub fn classify_op_result(res_str: &str) -> OpDetail {
    let ok = !res_str.starts_with("edit:0")
        && !res_str.starts_with("write:0")
        && !res_str.starts_with("read:0 (")
        && !res_str.starts_with("stat:0")
        && !res_str.starts_with("budget:0")
        && !res_str.starts_with("world:0")
        && !res_str.starts_with("compound:0")
        && !res_str.starts_with("mem:0")
        && !res_str.starts_with("bad ")
        && !res_str.starts_with("expand:0")
        && !res_str.starts_with("history:0")
        && !res_str.starts_with("undo:0")
        && !res_str.starts_with("transact:0");
    // Success path never consults `message` (visible_error only on failure).
    OpDetail {
        ok,
        message: if ok {
            String::new()
        } else {
            res_str.to_string()
        },
    }
}

pub fn visible_ack(op: OpCode, op_count: Option<u32>) -> String {
    // Compound is the only bare letter (no session counter suffix).
    if matches!(op, OpCode::Compound) {
        return op.as_letter().into();
    }
    let suffix = op_count
        .map(|ctr| ((ctr % 999) + 1).to_string())
        .unwrap_or_default();
    format!("{}{suffix}", op.as_char())
}
