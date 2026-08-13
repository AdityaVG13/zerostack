    use super::*;
    use serde_json::{Value, json};

    fn canonical() -> SharedCapability {
        SharedCapability::zeroref_v1(
            HashAlgorithm::Sha256,
            CasLayout::BlobsSha256Hh,
            LayoutVersion::V1,
        )
    }

    #[test]
    fn shared_capability_accepts_every_observed_alias() {
        let canonical: SharedCapability = serde_json::from_value(json!({
            "schema": "zeroref-capability/v1",
            "hash": {"algorithm": "sha256"},
            "shared_cas": {"layout": "blobs/sha256/<hh>/<hash>", "layout_version": 1},
            "fragments": {"byte": "strict", "line_start": "strict", "line_end": "clamp_end"}
        }))
        .unwrap();
        let legacy: SharedCapability = serde_json::from_value(json!({
            "schema": "zeroref-capability/v1",
            "hash": {"algo": "sha256"},
            "shared_cas": {"layout": "blobs/sha256/<hh>/<hash>", "version": 1},
            "fragments": {"byte": "strict", "line_start": "strict", "line_end": "clamp_end"}
        }))
        .unwrap();
        assert_eq!(canonical, legacy);
    }

    #[test]
    fn shared_capability_serializes_only_canonical_spellings() {
        let value = serde_json::to_value(canonical()).unwrap();
        assert_eq!(value["hash"], json!({"algorithm": "sha256"}));
        assert_eq!(
            value["shared_cas"],
            json!({
                "layout": "blobs/sha256/<hh>/<hash>", "layout_version": 1
            })
        );
        assert!(value["hash"].get("algo").is_none());
        assert!(value["shared_cas"].get("version").is_none());
        assert_eq!(
            serde_json::to_string(&canonical()).unwrap(),
            serde_json::to_string(&canonical()).unwrap()
        );
    }

    #[test]
    fn shared_capability_rejects_unknown_missing_and_zero_version() {
        let mut value = serde_json::to_value(canonical()).unwrap();
        value["extra"] = Value::Bool(true);
        assert!(serde_json::from_value::<SharedCapability>(value).is_err());

        let mut missing = serde_json::to_value(canonical()).unwrap();
        missing["fragments"]
            .as_object_mut()
            .unwrap()
            .remove("line_end");
        assert!(serde_json::from_value::<SharedCapability>(missing).is_err());

        let mut zero = serde_json::to_value(canonical()).unwrap();
        zero["shared_cas"]["layout_version"] = json!(0);
        assert!(serde_json::from_value::<SharedCapability>(zero).is_err());
    }

    #[test]
    fn shared_capability_reports_each_mismatch_in_deterministic_order() {
        let local = canonical();
        let peer = SharedCapability {
            hash: HashCapability {
                algorithm: HashAlgorithm::Sha1,
            },
            shared_cas: SharedCasCapability {
                layout: CasLayout::BlobsSha256Xx,
                layout_version: LayoutVersion::new(NonZeroU64::new(2).unwrap()),
            },
            fragments: FragmentPolicy {
                byte: FragmentBehavior::ClampEnd,
                line_start: FragmentBehavior::ClampEnd,
                line_end: FragmentBehavior::Strict,
            },
            ..local
        };
        assert_eq!(
            local.compatibility_mismatches(&peer),
            vec![
                CapabilityMismatch::HashAlgorithm {
                    expected: HashAlgorithm::Sha256,
                    actual: HashAlgorithm::Sha1
                },
                CapabilityMismatch::CasLayout {
                    expected: CasLayout::BlobsSha256Hh,
                    actual: CasLayout::BlobsSha256Xx
                },
                CapabilityMismatch::LayoutVersion {
                    expected: LayoutVersion::V1,
                    actual: peer.shared_cas.layout_version
                },
                CapabilityMismatch::FragmentByte {
                    expected: FragmentBehavior::Strict,
                    actual: FragmentBehavior::ClampEnd
                },
                CapabilityMismatch::FragmentLineStart {
                    expected: FragmentBehavior::Strict,
                    actual: FragmentBehavior::ClampEnd
                },
                CapabilityMismatch::FragmentLineEnd {
                    expected: FragmentBehavior::ClampEnd,
                    actual: FragmentBehavior::Strict
                },
            ]
        );
    }
