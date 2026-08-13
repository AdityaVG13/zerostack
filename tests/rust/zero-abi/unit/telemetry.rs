    use super::*;
    use serde_json::json;

    #[test]
    fn shared_telemetry_serialization_is_canonical_and_deterministic() {
        let telemetry = ZeroTelemetryV1 {
            schema: TelemetrySchema::V1,
            prevented_files: 2,
            prevented_bytes: 9,
        };
        assert_eq!(
            serde_json::to_string(&telemetry).unwrap(),
            r#"{"schema":"zero-telemetry/v1","prevented_files":2,"prevented_bytes":9}"#
        );
        assert_eq!(
            serde_json::to_value(TelemetrySchema::V1).unwrap(),
            json!("zero-telemetry/v1")
        );
    }

    #[test]
    fn shared_telemetry_rejects_unknown_and_missing_fields() {
        assert!(serde_json::from_value::<ZeroTelemetryV1>(json!({
            "schema": "zero-telemetry/v1", "prevented_files": 1, "prevented_bytes": 2, "sink": "engine-specific"
        })).is_err());
        assert!(
            serde_json::from_value::<ZeroTelemetryV1>(json!({
                "schema": "zero-telemetry/v1", "prevented_files": 1
            }))
            .is_err()
        );
    }

    #[test]
    fn shared_telemetry_checked_accumulation_reports_typed_overflow() {
        let mut telemetry = ZeroTelemetryV1 {
            schema: TelemetrySchema::V1,
            prevented_files: u64::MAX,
            prevented_bytes: 0,
        };
        assert_eq!(
            telemetry.checked_accumulate(TelemetryCounter::PreventedFiles, 1),
            Err(TelemetryOverflow {
                field: TelemetryCounter::PreventedFiles
            })
        );
        assert_eq!(telemetry.prevented_files, u64::MAX);
    }

    #[test]
    fn shared_telemetry_merge_is_transactional_on_overflow() {
        let mut telemetry = ZeroTelemetryV1 {
            schema: TelemetrySchema::V1,
            prevented_files: 4,
            prevented_bytes: u64::MAX,
        };
        let original = telemetry;
        assert_eq!(
            telemetry.checked_merge(ZeroTelemetryV1 {
                schema: TelemetrySchema::V1,
                prevented_files: 1,
                prevented_bytes: 1
            }),
            Err(TelemetryOverflow {
                field: TelemetryCounter::PreventedBytes
            })
        );
        assert_eq!(telemetry, original);
    }

    #[test]
    fn shared_telemetry_merge_is_deterministic() {
        let rows = [
            ZeroTelemetryV1 {
                schema: TelemetrySchema::V1,
                prevented_files: 1,
                prevented_bytes: 5,
            },
            ZeroTelemetryV1 {
                schema: TelemetrySchema::V1,
                prevented_files: 2,
                prevented_bytes: 7,
            },
            ZeroTelemetryV1 {
                schema: TelemetrySchema::V1,
                prevented_files: 3,
                prevented_bytes: 11,
            },
        ];
        let mut forward = ZeroTelemetryV1::default();
        for row in rows {
            forward.checked_merge(row).unwrap();
        }
        let mut reverse = ZeroTelemetryV1::default();
        for row in rows.into_iter().rev() {
            reverse.checked_merge(row).unwrap();
        }
        assert_eq!(forward, reverse);
        assert_eq!(
            forward,
            ZeroTelemetryV1 {
                schema: TelemetrySchema::V1,
                prevented_files: 6,
                prevented_bytes: 23
            }
        );
    }
