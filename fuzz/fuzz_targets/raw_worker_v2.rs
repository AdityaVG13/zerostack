#![no_main]

use libfuzzer_sys::fuzz_target;
use zero_abi::{
    decode_request_frame, decode_response_frame, FrameCodecError, DEFAULT_MAX_FRAME_BYTES,
};

const SMALL_MAX_FRAME_BYTES: usize = 16 * 1024;

fn trimmed_frame_len(data: &[u8]) -> usize {
    let line = data.strip_suffix(b"\n").unwrap_or(data);
    line.strip_suffix(b"\r").unwrap_or(line).len()
}

fn assert_too_large<T>(result: Result<T, FrameCodecError>, actual: usize, maximum: usize) {
    assert!(
        matches!(
            result,
            Err(FrameCodecError::TooLarge { actual: got_actual, maximum: got_maximum })
                if got_actual == actual && got_maximum == maximum
        ),
        "bounded decode must report exact frame size and limit"
    );
}

fuzz_target!(|data: &[u8]| {
    const _: () = assert!(SMALL_MAX_FRAME_BYTES <= DEFAULT_MAX_FRAME_BYTES);

    let selector = usize::from(data.first().copied().unwrap_or(0))
        | (usize::from(data.get(1).copied().unwrap_or(0)) << 8);
    let selected_max = selector % (SMALL_MAX_FRAME_BYTES + 1);

    let _ = decode_request_frame(data, selected_max);
    let _ = decode_response_frame(data, selected_max);

    let actual = trimmed_frame_len(data);
    if actual > 0 {
        let smaller_max = actual.saturating_sub(1).min(SMALL_MAX_FRAME_BYTES);
        assert_too_large(decode_request_frame(data, smaller_max), actual, smaller_max);
        assert_too_large(decode_response_frame(data, smaller_max), actual, smaller_max);
    }
});
