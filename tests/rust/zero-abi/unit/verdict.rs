    use super::*;

    fn premise(name: &str, established: Option<bool>) -> PremiseV1 {
        PremiseV1::new(name, established).expect("valid premise fixture")
    }

    /// ZS-KERNEL-004 acceptance: removing (or falsifying) any one required
    /// premise must never yield `Safe`.
    #[test]
    fn fault_injection_removing_one_premise_is_never_safe() {
        let names = ["premise_a", "premise_b", "premise_c", "premise_d"];
        let all_established: Vec<PremiseV1> = names
            .iter()
            .map(|name| premise(name, Some(true)))
            .collect();
        assert_eq!(
            SafetyVerdictV1::from_premises(&all_established),
            SafetyVerdictV1::Safe
        );

        // For EVERY index: dropping the premise (None) yields Unknown, and
        // falsifying it (Some(false)) yields Unsafe. Never Safe.
        for index in 0..names.len() {
            let mut missing = all_established.clone();
            missing[index].established = None;
            let verdict = SafetyVerdictV1::from_premises(&missing);
            assert!(
                matches!(verdict, SafetyVerdictV1::Unknown { .. }),
                "premise {index} missing must be Unknown, got {verdict:?}"
            );
            assert!(!verdict.grants_authority());

            let mut falsified = all_established.clone();
            falsified[index].established = Some(false);
            let verdict = SafetyVerdictV1::from_premises(&falsified);
            assert!(
                matches!(verdict, SafetyVerdictV1::Unsafe { .. }),
                "premise {index} falsified must be Unsafe, got {verdict:?}"
            );
            assert!(!verdict.grants_authority());
        }
    }

    #[test]
    fn meet_lattice_is_commutative_and_unsafe_dominates() {
        let safe = SafetyVerdictV1::Safe;
        let unknown = SafetyVerdictV1::Unknown {
            reasons: vec!["u1".into()],
        };
        let unsafe_v = SafetyVerdictV1::Unsafe {
            reasons: vec!["s1".into()],
        };
        let unsafe_other = SafetyVerdictV1::Unsafe {
            reasons: vec!["s2".into()],
        };

        // Unsafe dominates everything.
        for other in [safe.clone(), unknown.clone(), unsafe_v.clone()] {
            let merged = unsafe_v.clone().meet(other.clone());
            assert!(
                matches!(&merged, SafetyVerdictV1::Unsafe { .. }),
                "Unsafe.meet({other:?}) must be Unsafe, got {merged:?}"
            );
        }

        // Unknown dominates Safe.
        let merged = unknown.clone().meet(safe.clone());
        assert!(matches!(merged, SafetyVerdictV1::Unknown { .. }));
        let merged = safe.clone().meet(unknown.clone());
        assert!(matches!(merged, SafetyVerdictV1::Unknown { .. }));

        // Commutativity + idempotence over the full lattice.
        let all = [safe.clone(), unknown.clone(), unsafe_v.clone()];
        for a in &all {
            for b in &all {
                assert_eq!(a.clone().meet(b.clone()), b.clone().meet(a.clone()));
                assert_eq!(a.clone().meet(a.clone()), a.clone());
            }
        }

        // Associativity spot-check: (Unsafe.meet(Unknown)).meet(Safe) ==
        // Unsafe.meet(Unknown.meet(Safe)).
        let left = unsafe_v
            .clone()
            .meet(unknown.clone())
            .meet(safe.clone());
        let right = unsafe_v
            .clone()
            .meet(unknown.clone().meet(safe.clone()));
        assert_eq!(left, right);

        // meet_all folds equal to repeated meet.
        let folded = SafetyVerdictV1::meet_all([
            unsafe_v.clone(),
            unknown.clone(),
            safe.clone(),
            unsafe_other.clone(),
        ]);
        assert_eq!(
            folded,
            unsafe_v
                .clone()
                .meet(unknown.clone())
                .meet(safe.clone())
                .meet(unsafe_other.clone())
        );
    }

    #[test]
    fn empty_premises_are_unknown_not_safe() {
        let verdict = SafetyVerdictV1::from_premises(&[]);
        assert_eq!(
            verdict,
            SafetyVerdictV1::Unknown {
                reasons: vec!["no_premises".into()]
            }
        );
        assert!(!verdict.grants_authority());

        // A single Some(true) premise with a companion missing premise is
        // Unknown, and reasons are sorted/deduped through the meet.
        let verdict = SafetyVerdictV1::from_premises(&[
            premise("a", Some(true)),
            premise("b", None),
            premise("b", None),
        ]);
        assert_eq!(
            verdict,
            SafetyVerdictV1::Unknown {
                reasons: vec!["b".into()]
            }
        );
    }

    #[test]
    fn premise_validation_fails_closed() {
        assert!(PremiseV1::new("", Some(true)).is_err());
        assert!(PremiseV1::new("x".repeat(257), Some(true)).is_err());
        assert!(PremiseV1::new("has\0control", Some(true)).is_err());
        assert!(PremiseV1::new("ok_premise", Some(true)).is_ok());
    }
