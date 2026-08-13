    use super::*;
    use tempfile::tempdir;
    use zero_abi::ArtifactOwnerV1;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    #[test]
    fn durable_reopen_zbf_round_trip_across_handles() {
        let dir = tempdir().unwrap();
        let profile = DurableProfileV1::portable_strict();
        let object = ZbfObjectV1::new_leaf(
            ZbfArtifactKindV1::Plan,
            ArtifactOwnerV1::ZeroStack,
            digest(1),
            profile,
            digest(2),
            digest(3),
            b"canonical payload".to_vec(),
        )
        .unwrap();
        let first = SharedCas::open(dir.path());
        let outcome = first.put_zbf(&object, profile).unwrap();
        drop(first);

        let reopened = SharedCas::open(dir.path());
        assert_eq!(
            reopened.get_zbf(&outcome.hash, digest(1), profile).unwrap(),
            object
        );
        let independent_session = SharedCas::open(dir.path());
        assert_eq!(
            independent_session
                .get_zbf(&outcome.hash, digest(1), profile)
                .unwrap(),
            object
        );
    }
