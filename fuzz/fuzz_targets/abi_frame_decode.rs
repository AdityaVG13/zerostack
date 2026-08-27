#![no_main]

use libfuzzer_sys::fuzz_target;
use zero_abi::{DEFAULT_MAX_FRAME_BYTES, decode_request_frame, decode_response_frame};

fuzz_target!(|data: &[u8]| {
    let _ = decode_request_frame(data, DEFAULT_MAX_FRAME_BYTES);
    let _ = decode_response_frame(data, DEFAULT_MAX_FRAME_BYTES);
    if data.len() > 8 {
        let _ = decode_request_frame(data, 8);
        let _ = decode_response_frame(data, 8);
    }
});
