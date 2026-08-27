//! Fuzz SCIP index protobuf decoding (graphzero-la0n).
//!
//! Invariant: decode_scip_bytes must never panic on arbitrary bytes.
//! Decode errors are normal and are discarded.

#![no_main]

use libfuzzer_sys::fuzz_target;

use graphzero_scip::decode_scip_bytes;

fuzz_target!(|data: &[u8]| {
    let _ = decode_scip_bytes(data);
});
