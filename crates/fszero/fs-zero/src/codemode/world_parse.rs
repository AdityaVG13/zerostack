//! Parse world ids from kernel messages (shared by connector + parallel runtime).

pub fn world_id_from_kernel_message(message: &str) -> Option<String> {
    message
        .split_whitespace()
        .find(|token| {
            token.starts_with('W')
                && token.len() > 1
                && token[1..].chars().all(|c| c.is_ascii_digit())
        })
        .map(str::to_string)
}
