//! Encode/decode identity for raw-worker frames. Seed contract: WithSource.

use proptest::prelude::*;
use proptest::test_runner::{Config, FileFailurePersistence};
use zero_abi::{
    DEFAULT_MAX_FRAME_BYTES, EngineIdentity, HandshakeRequest, RAW_WORKER_PROTOCOL_VERSION,
    ShutdownRequest, WorkerRequestFrame, decode_request_frame, encode_frame,
};

fn hex64() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[0-9a-f]{64}").unwrap()
}

fn nonempty_ident() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[A-Za-z0-9._/-]{1,32}").unwrap()
}

fn engine() -> impl Strategy<Value = EngineIdentity> {
    prop_oneof![
        Just(EngineIdentity::FsZero),
        Just(EngineIdentity::GraphZero),
        Just(EngineIdentity::TokenZero),
    ]
}

fn config() -> Config {
    Config {
        cases: if cfg!(miri) { 8 } else { 64 },
        failure_persistence: if cfg!(miri) {
            None
        } else {
            Some(Box::new(FileFailurePersistence::WithSource(
                "proptest-regressions",
            )))
        },
        ..Config::default()
    }
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn handshake_encode_decode_is_identity(
        root in nonempty_ident(),
        session in nonempty_ident(),
        digest in hex64(),
        expected_engine in engine(),
    ) {
        let frame = WorkerRequestFrame::Handshake {
            request: HandshakeRequest {
                protocol_version: RAW_WORKER_PROTOCOL_VERSION.to_string(),
                root,
                session_id: session,
                expected_engine,
                expected_worker_revision: None,
                expected_contract_digest: digest,
                expected_registry_digest: None,
            },
        };
        let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).expect("encode");
        let decoded = decode_request_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).expect("decode");
        prop_assert_eq!(decoded, frame);
    }

    #[test]
    fn shutdown_encode_decode_is_identity(reason in nonempty_ident()) {
        let frame = WorkerRequestFrame::Shutdown {
            request: ShutdownRequest { reason },
        };
        let encoded = encode_frame(&frame, DEFAULT_MAX_FRAME_BYTES).expect("encode");
        let decoded = decode_request_frame(&encoded, DEFAULT_MAX_FRAME_BYTES).expect("decode");
        prop_assert_eq!(decoded, frame);
    }
}
