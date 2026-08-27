//! FR-008: redact secret-like content before persistence.

use crate::schema::RedactionState;

const FAKE_SECRET_MARKERS: &[&str] = &[
    "sk-fake-secret-for-test",
    "AKIAFAKEKEY000000",
    "ghp_fake_token_",
    "BEGIN RSA PRIVATE KEY",
];

pub struct RedactionOutcome {
    pub text: String,
    pub state: RedactionState,
}

pub fn redact_text(input: &str) -> RedactionOutcome {
    let mut text = input.to_owned();
    let mut redacted = false;
    for marker in FAKE_SECRET_MARKERS {
        let (next, replaced) = replace_ascii_case_insensitive(&text, marker);
        text = next;
        redacted |= replaced;
    }
    if redacted {
        return RedactionOutcome {
            text,
            state: RedactionState::Redacted,
        };
    }
    if looks_like_bearer_token(input) {
        return RedactionOutcome {
            text: "[REDACTED_TOKEN]".into(),
            state: RedactionState::Blocked,
        };
    }
    RedactionOutcome {
        text,
        state: RedactionState::None,
    }
}

fn replace_ascii_case_insensitive(input: &str, needle: &str) -> (String, bool) {
    let input_bytes = input.as_bytes();
    let needle_bytes = needle.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut copied_until = 0;
    let mut search_from = 0;
    let mut replaced = false;

    while search_from + needle_bytes.len() <= input_bytes.len() {
        let relative = input_bytes[search_from..]
            .windows(needle_bytes.len())
            .position(|candidate| candidate.eq_ignore_ascii_case(needle_bytes));
        let Some(relative) = relative else {
            break;
        };
        let start = search_from + relative;
        let end = start + needle_bytes.len();
        output.push_str(&input[copied_until..start]);
        output.push_str("[REDACTED]");
        copied_until = end;
        search_from = end;
        replaced = true;
    }

    if !replaced {
        return (input.to_owned(), false);
    }
    output.push_str(&input[copied_until..]);
    (output, true)
}

fn looks_like_bearer_token(s: &str) -> bool {
    s.contains("Bearer eyJ") && s.len() > 40
}

#[cfg(test)]
#[path = "../../../../tests/graphzero/unit/graphzero-why/redaction_tests.rs"]
mod tests;
