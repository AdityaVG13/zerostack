    use super::*;

    #[test]
    fn fixture_binding_and_echo_are_typed() {
        let adapter = FixtureAdapter::new(EngineIdentity::TokenZero, "/tmp", "session-fixture");
        assert_eq!(adapter.engine(), EngineIdentity::TokenZero);
        assert_eq!(adapter.binding().ref_scheme, "tz://");
        assert_eq!(adapter.binding().semantic_contract_digest.len(), 64);
    }
