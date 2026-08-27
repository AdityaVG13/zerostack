use super::*;

macro_rules! capsule_response {
    ($tool:expr, $mode:expr, $capsule:expr, $refs:expr, $recovery_tokens:expr) => {{
        let refs = $refs;
        let capsule = $capsule;
        let exact_ref_tokens = exact_ref_token_count(&refs);
        success_response(
            $tool,
            capsule.mode,
            capsule.text,
            refs,
            (
                capsule.raw_tokens,
                capsule.visible_tokens,
                $recovery_tokens,
                Some(exact_ref_tokens),
            ),
        )
    }};
}
pub(super) use capsule_response;

pub(super) fn capsule_error_response(tool: &str, error: String) -> ToolResponse {
    failure_response(
        tool,
        "capsule_omission_invalid",
        format!("capsule omission validation failed: {error}"),
        None,
    )
}

pub(super) fn joined_bytes(parts: &[String]) -> usize {
    parts.iter().map(String::len).sum::<usize>() + parts.len().saturating_sub(1) * 2
}

/// Number of leading equal elements of two slices (longest-common-prefix
/// length). Shared by cache_meter (consecutive request texts) and exposure
/// (successive provider-visible message histories).
pub(super) fn common_prefix_len<T: PartialEq>(left: &[T], right: &[T]) -> usize {
    left.iter().zip(right).take_while(|(a, b)| a == b).count()
}

