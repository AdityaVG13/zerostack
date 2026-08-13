    use super::*;
    use serde_json::json;

    #[test]
    fn request_defaults_and_canonical_wire_are_stable() {
        let request: TokenJobPollRequestV1 =
            serde_json::from_value(json!({"id":"tzjob-7"})).unwrap();
        assert_eq!(request.wait_ms, TOKEN_JOB_DEFAULT_WAIT_MS_V1);
        assert_eq!(request.since, 0);
        assert_eq!(request.tail_bytes, TOKEN_JOB_DEFAULT_TAIL_BYTES_V1);
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            json!({"id":"tzjob-7","waitMs":30000,"since":0,"tailBytes":8192})
        );
    }

    #[test]
    fn request_rejects_unknown_and_out_of_range_fields() {
        for mutant in [
            json!({"id":"tzjob-7","extra":true}),
            json!({"id":""}),
            json!({"id":"tzjob-7","waitMs":30001}),
            json!({"id":"tzjob-7","tailBytes":0}),
            json!({"id":"tzjob-7","tailBytes":65537}),
        ] {
            assert!(serde_json::from_value::<TokenJobPollRequestV1>(mutant).is_err());
        }
    }

    #[test]
    fn result_round_trips_and_revalidates_public_values() {
        let result = TokenJobPollResultV1::new(
            "tzjob-7",
            TokenJobStatusV1::Running,
            Some(42),
            None,
            "ok\n",
            true,
            3,
            3,
            3,
            2,
            true,
            Some(20_000),
        )
        .unwrap();
        let encoded = serde_json::to_value(&result).unwrap();
        assert!(encoded.get("exitCode").is_none());
        assert_eq!(encoded["tailUtf8Lossless"], true);
        assert_eq!(
            serde_json::from_value::<TokenJobPollResultV1>(encoded).unwrap(),
            result
        );

        let exited = TokenJobPollResultV1::new(
            "tzjob-7",
            TokenJobStatusV1::Exited,
            Some(42),
            Some(0),
            "",
            true,
            0,
            3,
            3,
            3,
            true,
            None,
        )
        .unwrap();
        assert_eq!(exited.status, TokenJobStatusV1::Exited);

        let invalid = TokenJobPollResultV1 {
            id: "tzjob-7".into(),
            status: TokenJobStatusV1::Running,
            pid: None,
            exit_code: Some(0),
            tail: String::new(),
            tail_utf8_lossless: true,
            tail_bytes: 0,
            log_bytes: 0,
            cursor: 0,
            version: 0,
            changed: false,
            next_poll_ms: None,
        };
        assert_eq!(
            invalid.validate(),
            Err(TokenJobContractError::InvalidExitCode)
        );
    }

    #[test]
    fn result_rejects_unknown_and_inconsistent_fields() {
        let base = json!({
            "id":"tzjob-7","status":"running","tail":"","tailUtf8Lossless":true,"tailBytes":0,
            "logBytes":0,"cursor":0,"version":0,"changed":false,"nextPollMs":20000
        });
        let mut unknown = base.clone();
        unknown["log"] = json!("/private/session.log");
        assert!(serde_json::from_value::<TokenJobPollResultV1>(unknown).is_err());

        let mut cursor = base.clone();
        cursor["cursor"] = json!(2);
        assert!(serde_json::from_value::<TokenJobPollResultV1>(cursor).is_err());

        let mut changed = base.clone();
        changed["tail"] = json!("hidden");
        assert!(serde_json::from_value::<TokenJobPollResultV1>(changed).is_err());

        let mut byte_mismatch = base;
        byte_mismatch["tail"] = json!("x");
        byte_mismatch["tailBytes"] = json!(2);
        byte_mismatch["cursor"] = json!(2);
        byte_mismatch["logBytes"] = json!(2);
        byte_mismatch["changed"] = json!(true);
        assert!(serde_json::from_value::<TokenJobPollResultV1>(byte_mismatch).is_err());
    }

    #[test]
    fn contract_digest_is_frozen() {
        assert_eq!(token_job_contract_manifest_v1()["operation"], "job");
        assert_eq!(
            token_job_contract_digest_v1(),
            "d9b15de5be5a4c5a2d80ffd409eb04fc796b16b377a67254016fc4f285b7a597"
        );
    }
