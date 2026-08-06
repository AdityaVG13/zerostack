//! Shared counters emitted by GraphZero prevented-read accounting and consumed by ledgers.
//!
//! Source anchor: GraphZero `crates/graphzero-query/src/accounting.rs`
//! (`PreventedReadAccounting::{prevented_files, prevented_bytes}`).

use serde::{Deserialize, Serialize};

/// The shared telemetry schema version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetrySchema {
    #[serde(rename = "zero-telemetry/v1")]
    V1,
}

/// Exact cross-engine prevented-read counter set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroTelemetryV1 {
    pub schema: TelemetrySchema,
    pub prevented_files: u64,
    pub prevented_bytes: u64,
}

impl Default for ZeroTelemetryV1 {
    fn default() -> Self {
        Self {
            schema: TelemetrySchema::V1,
            prevented_files: 0,
            prevented_bytes: 0,
        }
    }
}

impl ZeroTelemetryV1 {
    /// Adds one typed counter without wrapping.
    pub fn checked_accumulate(
        &mut self,
        field: TelemetryCounter,
        amount: u64,
    ) -> Result<(), TelemetryOverflow> {
        let counter = match field {
            TelemetryCounter::PreventedFiles => &mut self.prevented_files,
            TelemetryCounter::PreventedBytes => &mut self.prevented_bytes,
        };
        *counter = counter
            .checked_add(amount)
            .ok_or(TelemetryOverflow { field })?;
        Ok(())
    }

    /// Transactionally merges another counter set without wrapping or partial mutation.
    pub fn checked_merge(&mut self, other: Self) -> Result<(), TelemetryOverflow> {
        let prevented_files = self
            .prevented_files
            .checked_add(other.prevented_files)
            .ok_or(TelemetryOverflow {
                field: TelemetryCounter::PreventedFiles,
            })?;
        let prevented_bytes = self
            .prevented_bytes
            .checked_add(other.prevented_bytes)
            .ok_or(TelemetryOverflow {
                field: TelemetryCounter::PreventedBytes,
            })?;
        self.prevented_files = prevented_files;
        self.prevented_bytes = prevented_bytes;
        Ok(())
    }
}

/// A telemetry counter dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TelemetryCounter {
    PreventedFiles,
    PreventedBytes,
}

/// Checked accumulation failure with the exact overflowing field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TelemetryOverflow {
    pub field: TelemetryCounter,
}

#[cfg(test)]
mod tests {
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
}
