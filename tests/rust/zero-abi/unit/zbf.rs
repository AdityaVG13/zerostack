    use super::*;

    fn digest(byte: u8) -> DigestV1 {
        DigestV1::from_bytes([byte; 32])
    }

    fn leaf(profile: DurableProfileV1) -> ZbfObjectV1 {
        ZbfObjectV1::new_leaf(
            ZbfArtifactKindV1::Plan,
            ArtifactOwnerV1::ZeroStack,
            digest(1),
            profile,
            digest(2),
            digest(3),
            b"canonical payload".to_vec(),
        )
        .unwrap()
    }

    #[test]
    fn zbf_canonical_round_trip_and_digest_are_stable() {
        let profile = DurableProfileV1::portable_strict();
        let child = leaf(profile);
        let object = ZbfObjectV1::new_container(
            ZbfArtifactKindV1::Snapshot,
            ArtifactOwnerV1::ZeroStack,
            digest(1),
            profile,
            digest(4),
            digest(5),
            vec![child],
        )
        .unwrap();
        let bytes = object.to_bytes(profile).unwrap();
        assert_eq!(
            ZbfObjectV1::from_bytes(&bytes, digest(1), profile).unwrap(),
            object
        );
        assert_eq!(bytes.len(), 413);
        assert_eq!(
            object.identity(profile).unwrap().to_hex(),
            "025ca5465d0ebf7bb086f896775880b746c0197a5ce428d7036a27d5341fd559"
        );
        assert_eq!(
            zbf_contract_digest_v1().to_hex(),
            "c33216eac0bb9e45b5a8d6337c71df2d4a439582a8a4e03d66b0a6b9e9a16670"
        );
    }

    #[test]
    fn zbf_profile_digests_are_stable_and_distinct() {
        let profiles = [
            DurableProfileIdV1::PortableStrict,
            DurableProfileIdV1::ApfsStrict,
            DurableProfileIdV1::Ext4XfsStrict,
            DurableProfileIdV1::NtfsStrict,
        ]
        .map(|id| DurableProfileV1::new(id).digest());
        assert_eq!(
            profiles[0].to_hex(),
            "c8bf0ccc2c25dcd2f222a137c612e6daae00c2f4509c75eedc3b87592d0c7c9c"
        );
        for pair in profiles.iter().enumerate() {
            for other in profiles.iter().skip(pair.0 + 1) {
                assert_ne!(pair.1, other);
            }
        }
    }

    #[test]
    fn zbf_oversize_and_torn_inputs_fail_closed() {
        let profile = DurableProfileV1::portable_strict();
        let mut bytes = leaf(profile).to_bytes(profile).unwrap();
        bytes[16..24].copy_from_slice(&(profile.max_payload_bytes() + 1).to_be_bytes());
        assert_eq!(
            ZbfObjectV1::from_bytes(&bytes, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::PayloadTooLarge
        );

        let mut torn = leaf(profile).to_bytes(profile).unwrap();
        torn.pop();
        assert_eq!(
            ZbfObjectV1::from_bytes(&torn, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::UnexpectedEof
        );
    }

    #[test]
    fn zbf_trailing_reserved_and_payload_mutants_fail_closed() {
        let profile = DurableProfileV1::portable_strict();
        let bytes = leaf(profile).to_bytes(profile).unwrap();

        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            ZbfObjectV1::from_bytes(&trailing, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::TrailingBytes
        );

        let mut reserved = bytes.clone();
        reserved[191] = 1;
        assert_eq!(
            ZbfObjectV1::from_bytes(&reserved, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::ReservedNonZero
        );

        let mut payload = bytes;
        payload[ZBF_HEADER_LEN_V1] ^= 1;
        assert_eq!(
            ZbfObjectV1::from_bytes(&payload, digest(1), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::DigestMismatch
        );
    }

    #[test]
    fn zbf_assembly_and_profile_swaps_fail_before_payload() {
        let profile = DurableProfileV1::portable_strict();
        let bytes = leaf(profile).to_bytes(profile).unwrap();
        assert_eq!(
            ZbfObjectV1::from_bytes(&bytes, digest(9), profile)
                .unwrap_err()
                .code(),
            ZbfFailureCodeV1::AssemblyMismatch
        );
        assert_eq!(
            ZbfObjectV1::from_bytes(
                &bytes,
                digest(1),
                DurableProfileV1::new(DurableProfileIdV1::ApfsStrict),
            )
            .unwrap_err()
            .code(),
            ZbfFailureCodeV1::ProfileMismatch
        );
    }

    #[test]
    fn zbf_deep_nesting_is_bounded() {
        let profile = DurableProfileV1::portable_strict();
        let mut object = leaf(profile);
        let mut rejection = None;
        for _ in 0..=profile.max_depth() {
            match ZbfObjectV1::new_container(
                ZbfArtifactKindV1::Snapshot,
                ArtifactOwnerV1::ZeroStack,
                digest(1),
                profile,
                digest(2),
                digest(3),
                vec![object.clone()],
            ) {
                Ok(next) => object = next,
                Err(error) => {
                    rejection = Some(error);
                    break;
                }
            }
        }
        assert_eq!(
            rejection.expect("depth limit must reject").code(),
            ZbfFailureCodeV1::DepthExceeded
        );
    }
