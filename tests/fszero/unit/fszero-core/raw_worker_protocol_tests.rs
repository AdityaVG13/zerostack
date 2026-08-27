
use super::*;
use fszero_test_support::decode_worker_transcript;

#[test]
fn protocol_digest_is_the_current_hub_digest() {
    assert_eq!(
        raw_worker_protocol_digest_hex(),
        "2bd658957370bd40a26f87a2dd218677f7fa3106f99dd1ec99f6ce8d2e77dd86"
    );
}

#[test]
fn shared_decoder_rejects_shutdown_ack_unknown_fields() {
    let transcript = concat!(
        r#"{"kind":"shutdown_ack"}"#,
        "\n",
        r#"{"kind":"shutdown_ack","extra":true}"#,
        "\n",
    );
    let error = decode_worker_transcript(transcript.as_bytes())
        .expect_err("unknown shutdown_ack field must fail closed");
    assert_eq!(error.kind(), "invalid_frame");
}
