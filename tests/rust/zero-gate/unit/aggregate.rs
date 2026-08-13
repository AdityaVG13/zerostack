    use super::*;
    use serde_json::Value;

    fn digest(value: u8) -> DigestV1 {
        DigestV1::from_bytes([value; 32])
    }

    fn slot(class: AggregateEvidenceClassV1, value: u8) -> EvidenceSlotV1 {
        EvidenceSlotV1 {
            class,
            evidence_digest: digest(value),
        }
    }

    fn engine(engine: ArtifactOwnerV1, base: u8) -> EngineEvidenceV1 {
        EngineEvidenceV1 {
            engine,
            slots: AGGREGATE_PROGRAM_EVIDENCE_CLASSES
                .iter()
                .enumerate()
                .map(|(index, class)| slot(*class, base + index as u8))
                .collect(),
        }
    }

    fn source_heads() -> Vec<AggregateSourceHeadV1> {
        vec![AggregateSourceHeadV1 {
            repository: "zerostack".to_owned(),
            head: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
        }]
    }

    fn complete() -> Vec<EngineEvidenceV1> {
        vec![
            engine(ArtifactOwnerV1::FsZero, 10),
            engine(ArtifactOwnerV1::GraphZero, 20),
            engine(ArtifactOwnerV1::TokenZero, 30),
        ]
    }

    fn receipt() -> AggregateProgramReceiptV1 {
        AggregateProgramReceiptV1::new(digest(1), digest(2), source_heads(), complete()).unwrap()
    }

    fn raw(
        engines: Vec<EngineEvidenceV1>,
        heads: Vec<AggregateSourceHeadV1>,
    ) -> AggregateProgramReceiptV1 {
        AggregateProgramReceiptV1 {
            schema_version: AGGREGATE_PROGRAM_SCHEMA_VERSION,
            program_digest: digest(1),
            assembly_manifest_digest: digest(2),
            source_repository_heads: heads,
            engines,
            receipt_head: DigestV1::ZERO,
        }
    }

    #[test]
    fn valid_aggregate_verifies_and_round_trips_canonically() {
        let receipt = receipt();
        receipt.verify().unwrap();
        let bytes = receipt.canonical_bytes().unwrap();
        let decoded = AggregateProgramReceiptV1::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(decoded, receipt);
        decoded.verify().unwrap();
        // receipt_head is not part of the body digest
        let mut forged = receipt.clone();
        forged.receipt_head = digest(0xff);
        assert_eq!(
            forged.verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::ReceiptHeadMismatch
        );
    }

    #[test]
    fn missing_engine_can_never_report_program_success() {
        for missing in AGGREGATE_PROGRAM_REQUIRED_ENGINES {
            let engines: Vec<EngineEvidenceV1> = complete()
                .into_iter()
                .filter(|entry| entry.engine != missing)
                .collect();
            let receipt = raw(engines, source_heads());
            assert_eq!(
                receipt.verify().unwrap_err().failure_code(),
                AggregateProgramFailureCodeV1::MissingEngine
            );
        }
    }

    #[test]
    fn unknown_engine_is_rejected() {
        let mut engines = complete();
        engines[0].engine = ArtifactOwnerV1::ZeroStack;
        let receipt = raw(engines, source_heads());
        assert_eq!(
            receipt.verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::UnknownEngine
        );
    }

    #[test]
    fn duplicate_engine_is_rejected() {
        let mut engines = complete();
        engines.push(engines[0].clone());
        let receipt = raw(engines, source_heads());
        assert_eq!(
            receipt.verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::DuplicateEngine
        );
    }

    #[test]
    fn missing_surface_can_never_report_program_success() {
        for missing in AGGREGATE_PROGRAM_EVIDENCE_CLASSES {
            let mut engines = complete();
            engines[0].slots.retain(|entry| entry.class != missing);
            let receipt = raw(engines, source_heads());
            assert_eq!(
                receipt.verify().unwrap_err().failure_code(),
                AggregateProgramFailureCodeV1::MissingEvidenceClass
            );
        }
    }

    #[test]
    fn duplicate_slot_is_rejected() {
        let mut engines = complete();
        let duplicate = engines[0].slots[0].clone();
        engines[0].slots.push(duplicate);
        let receipt = raw(engines, source_heads());
        assert_eq!(
            receipt.verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::DuplicateEvidenceSlot
        );
    }

    #[test]
    fn noncanonical_slot_order_is_rejected() {
        let mut engines = complete();
        engines[1].slots.swap(0, 1);
        let receipt = raw(engines, source_heads());
        assert_eq!(
            receipt.verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn zero_digests_are_rejected() {
        let mut engines = complete();
        engines[0].slots[2].evidence_digest = DigestV1::ZERO;
        let receipt = raw(engines, source_heads());
        assert_eq!(
            receipt.verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::ZeroDigest
        );
    }

    #[test]
    fn schema_version_mismatch_is_rejected() {
        let mut receipt = receipt();
        receipt.schema_version = 99;
        assert_eq!(
            receipt.verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::SchemaVersionMismatch
        );
    }

    #[test]
    fn invalid_source_identity_is_rejected() {
        let mut heads = source_heads();
        heads[0].head = "not-a-valid-head".to_owned();
        assert_eq!(
            raw(complete(), heads).verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::InvalidSourceIdentity
        );
    }

    #[test]
    fn empty_source_heads_are_rejected() {
        assert_eq!(
            raw(complete(), vec![]).verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::InvalidSourceIdentity
        );
    }

    #[test]
    fn duplicate_source_heads_are_rejected() {
        let heads = vec![source_heads()[0].clone(), source_heads()[0].clone()];
        assert_eq!(
            raw(complete(), heads).verify().unwrap_err().failure_code(),
            AggregateProgramFailureCodeV1::InvalidSourceIdentity
        );
    }

    #[test]
    fn noncanonical_bytes_are_rejected() {
        let bytes = receipt().canonical_bytes().unwrap();
        // Emit the same object with keys in reverse alphabetical order: valid
        // JSON, noncanonical encoding.
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let Value::Object(map) = value else {
            panic!("expected object");
        };
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort_by(|left, right| right.cmp(left));
        let mut text = String::from("{");
        for (index, key) in keys.iter().enumerate() {
            if index > 0 {
                text.push(',');
            }
            text.push_str(&serde_json::to_string(key).unwrap());
            text.push(':');
            text.push_str(&serde_json::to_string(&map[*key]).unwrap());
        }
        text.push('}');
        assert_eq!(
            AggregateProgramReceiptV1::from_canonical_bytes(text.as_bytes())
                .unwrap_err()
                .failure_code(),
            AggregateProgramFailureCodeV1::NonCanonicalEncoding
        );
    }

    #[test]
    fn contract_digest_is_stable_and_bound() {
        let manifest = aggregate_program_contract_manifest_v1();
        let digest_a = aggregate_program_contract_digest_v1();
        let mut bytes = Vec::with_capacity(
            AGGREGATE_PROGRAM_CONTRACT_DOMAIN_V1.len() + manifest.to_string().len(),
        );
        bytes.extend_from_slice(AGGREGATE_PROGRAM_CONTRACT_DOMAIN_V1);
        bytes.extend_from_slice(canonical_json(&manifest).as_bytes());
        assert_eq!(digest_a, DigestV1::from_bytes(sha256(&bytes)));
    }
