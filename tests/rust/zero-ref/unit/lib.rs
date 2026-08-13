    use super::*;

    #[test]
    fn verified_selection_rejects_stale_content_with_typed_digest_mismatch() {
        let authenticated = b"abcdef";
        let reference = ZeroRefV1::parse(&format!(
            "fz://blob/{}#B1-4",
            content_hash_hex(authenticated),
        ))
        .unwrap();
        let stale = b"aXYZef";

        assert_eq!(reference.unchecked_select(stale).unwrap(), b"XYZ");
        let error = reference.verify_and_select(stale).unwrap_err();
        assert_eq!(error.class, ZeroRefErrorClass::DigestMismatch);
        assert!(error.message.contains("ref claims"));
    }
