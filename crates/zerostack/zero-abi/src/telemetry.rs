//! Shared prevented-read counters used by GraphZero accounting and ZeroStack ledgers.

use serde::{Deserialize, Serialize};

/// The shared telemetry schema version.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TelemetrySchema {
    #[serde(rename = "zero-telemetry")]
    Current,
}

/// Exact cross-engine prevented-read counter set.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZeroTelemetry {
    pub schema: TelemetrySchema,
    pub prevented_files: u64,
    pub prevented_bytes: u64,
}

impl Default for ZeroTelemetry {
    fn default() -> Self {
        Self {
            schema: TelemetrySchema::Current,
            prevented_files: 0,
            prevented_bytes: 0,
        }
    }
}

impl ZeroTelemetry {
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
