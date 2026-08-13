    use super::*;
    use serde_json::json;

    #[test]
    fn digest_is_stable_across_key_order() {
        let a = json!({ "engine": "x", "ops": [{ "name": "read", "cost": "cheap" }] });
        let b = json!({ "ops": [{ "cost": "cheap", "name": "read" }], "engine": "x" });
        assert_eq!(contract_digest_hex(&a), contract_digest_hex(&b));
    }

    #[test]
    fn digest_changes_on_content_change() {
        let a = json!({ "engine": "x", "version": "1.0.0" });
        let b = json!({ "engine": "x", "version": "1.0.1" });
        assert_ne!(contract_digest_hex(&a), contract_digest_hex(&b));
    }

    #[test]
    fn sha256_hex_known_vector() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
