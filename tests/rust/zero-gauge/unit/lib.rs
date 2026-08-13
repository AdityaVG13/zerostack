    use super::*;
    fn lock() -> ProviderLock {
        ProviderLock {
            provider: "runtime".into(),
            model: "caller-supplied".into(),
            tokenizer_revision_digest:
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
        }
    }
    fn fixture(rendered: &[&str]) -> AtomFixture {
        AtomFixture {
            schema: "zerostack.zero_gauge.complete_atoms.v1".into(),
            provider_lock: lock(),
            instances: rendered
                .iter()
                .map(|rendered| FixtureInstance {
                    rendered: (*rendered).into(),
                    expected_token_count: None,
                })
                .collect(),
        }
    }

    #[test]
    fn zero_gauge_grammar_canonicality_and_tamper() {
        for value in ["fz://o/0/0", "gz://o/1/9", "tz://o/18446744073709551615/2"] {
            let parsed: OrdinalRef = value.parse().unwrap();
            assert_eq!(parsed.to_string(), value);
            let json = serde_json::to_string(&parsed).unwrap();
            assert_eq!(serde_json::from_str::<OrdinalRef>(&json).unwrap(), parsed);
        }
        for invalid in [
            "xz://o/1/1",
            "TZ://o/1/1",
            "tz://o/+1/1",
            "tz://o/-1/1",
            "tz://o/01/1",
            "tz://o/1/00",
            "tz://o/1/1/2",
            "tz://o/1/1?x",
            "tz://o/1/1#x",
            "tz://o/18446744073709551616/1",
            "tz://o/１/1",
        ] {
            assert!(invalid.parse::<OrdinalRef>().is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn zero_gauge_arithmetic_roundtrip_bounds_and_overflow() {
        assert_eq!(Gauge::new(0), Err(GaugeError::ZeroCapacity));
        let gauge = Gauge::new(3).unwrap();
        for allocation in [0, 1, 2, 3, 4, u64::MAX] {
            let reference = gauge.allocate(EngineScheme::Gz, allocation).unwrap();
            assert_eq!(gauge.allocation(reference).unwrap(), allocation);
        }
        assert_eq!(
            gauge.allocation(OrdinalRef::new(EngineScheme::Fz, 1, 4)),
            Err(GaugeError::OrdinalOutOfRange {
                ordinal: 4,
                capacity: 3
            })
        );
        assert_eq!(
            gauge.allocation(OrdinalRef::new(EngineScheme::Tz, 0, 1)),
            Err(GaugeError::ZeroCoordinate)
        );
        assert_eq!(
            Gauge::new(1).unwrap().allocate(EngineScheme::Tz, u64::MAX),
            Err(GaugeError::ArithmeticOverflow)
        );
        assert_eq!(
            Gauge::new(u64::MAX).unwrap().allocation(OrdinalRef::new(
                EngineScheme::Tz,
                u64::MAX,
                u64::MAX
            )),
            Err(GaugeError::ArithmeticOverflow)
        );
    }

    #[test]
    fn zero_gauge_fixture_lock_and_callback_one_token_pass_fail() {
        let bundled = parse_fixture(BUNDLED_ATOM_FIXTURE_JSON).unwrap();
        let proof = certify_fixture(&bundled, &lock(), |seen, rendered| {
            assert_eq!(seen, &lock());
            assert!(rendered.contains("://o/"));
            Ok(1)
        })
        .unwrap();
        assert_eq!(proof.certified_instances(), 3);
        assert!(matches!(
            certify_fixture(&bundled, &lock(), |_, _| Ok(2)),
            Err(CertificationError::RuntimeCountNotOne { .. })
        ));
        let mut wrong_lock = lock();
        wrong_lock.model = "other".into();
        assert_eq!(
            certify_fixture(&bundled, &wrong_lock, |_, _| Ok(1)),
            Err(CertificationError::ProviderLockMismatch)
        );
        assert!(matches!(
            certify_fixture(&fixture(&["tz://o/1/1", "tz://o/1/1"]), &lock(), |_, _| Ok(
                1
            )),
            Err(CertificationError::DuplicateInstance(_))
        ));
    }

    #[test]
    fn zero_gauge_fixture_tamper_requires_explicit_count() {
        let missing = "{\"schema\":\"zerostack.zero_gauge.complete_atoms.v1\",\"provider_lock\":{\"provider\":\"p\",\"model\":\"m\",\"tokenizer_revision_digest\":\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"},\"instances\":[{\"rendered\":\"tz://o/1/1\"}]}";
        assert!(parse_fixture(missing).is_err());
        for rendered in ["", "tz://o/01/1", "tz://o/1/1#fragment"] {
            assert!(certify_fixture(&fixture(&[rendered]), &lock(), |_, _| Ok(1)).is_err());
        }
    }

    #[test]
    fn zero_gauge_one_token_pieces_do_not_certify_untested_composed_ref() {
        let pieces = fixture(&["tz://o/", "1", "1"]);
        let mut callbacks = 0;
        let result = certify_fixture(&pieces, &lock(), |_, _| {
            callbacks += 1;
            Ok(1)
        });
        assert!(matches!(
            result,
            Err(CertificationError::NoncanonicalInstance(_))
        ));
        assert_eq!(callbacks, 0);
    }
