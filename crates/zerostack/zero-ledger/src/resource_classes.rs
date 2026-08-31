//! Resource ledger classes and provider-bill reconciliation.
//! `ChargeClass` and `CausalWorkClass` account for model-visible tokens.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize, de};

use crate::{LedgerError, PPM_ONE};

/// The maximum length of a class string on the wire.
pub const MAX_RESOURCE_CLASS_STRING_BYTES: usize = 64;

/// Closed set of non-token resource coordinates a ledger row can charge. Unknown classes are
/// refused by [`ResourceClass::from_str`] and by deserialization: there is no catch-all bucket.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    /// Bytes of tool call arguments sent to a tool.
    ToolArgsBytes,
    /// Bytes of tool call returns received from a tool.
    ToolReturnsBytes,
    /// Bytes across the wire (request and response framing).
    WireBytes,
    /// Bytes written to or read from disk.
    DiskBytes,
    /// CPU time consumed, in nanoseconds.
    CpuNanoseconds,
    /// GPU time consumed, in nanoseconds.
    GpuNanoseconds,
    /// Bytes of storage retained.
    StorageBytes,
    /// Maintenance work units (refresh, revalidation, retention).
    Maintenance,
    /// Input tokens that were not served by any cache.
    UncachedInputTokens,
}

impl ResourceClass {
    /// Every resource class, in canonical order.
    pub const ALL: [Self; 9] = [
        Self::ToolArgsBytes,
        Self::ToolReturnsBytes,
        Self::WireBytes,
        Self::DiskBytes,
        Self::CpuNanoseconds,
        Self::GpuNanoseconds,
        Self::StorageBytes,
        Self::Maintenance,
        Self::UncachedInputTokens,
    ];

    /// Canonical lowercase class string.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ToolArgsBytes => "tool_args_bytes",
            Self::ToolReturnsBytes => "tool_returns_bytes",
            Self::WireBytes => "wire_bytes",
            Self::DiskBytes => "disk_bytes",
            Self::CpuNanoseconds => "cpu_nanoseconds",
            Self::GpuNanoseconds => "gpu_nanoseconds",
            Self::StorageBytes => "storage_bytes",
            Self::Maintenance => "maintenance",
            Self::UncachedInputTokens => "uncached_input_tokens",
        }
    }
}

impl fmt::Display for ResourceClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Failure to parse a resource class string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceClassParseError {
    /// The rejected class string.
    pub unknown_class: String,
}

impl fmt::Display for ResourceClassParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown resource class {:?}", self.unknown_class)
    }
}

impl std::error::Error for ResourceClassParseError {}

impl FromStr for ResourceClass {
    type Err = ResourceClassParseError;

    /// Refuses any string that is not a canonical class name.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "tool_args_bytes" => Ok(Self::ToolArgsBytes),
            "tool_returns_bytes" => Ok(Self::ToolReturnsBytes),
            "wire_bytes" => Ok(Self::WireBytes),
            "disk_bytes" => Ok(Self::DiskBytes),
            "cpu_nanoseconds" => Ok(Self::CpuNanoseconds),
            "gpu_nanoseconds" => Ok(Self::GpuNanoseconds),
            "storage_bytes" => Ok(Self::StorageBytes),
            "maintenance" => Ok(Self::Maintenance),
            "uncached_input_tokens" => Ok(Self::UncachedInputTokens),
            _ => Err(ResourceClassParseError {
                unknown_class: value.to_string(),
            }),
        }
    }
}

/// How a row's amount was obtained. Ordering is `Estimate < Bounded < Exact`; a derived
/// total takes the minimum (strongest honest label): Exact only when every input was Exact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementSource {
    /// The amount is an estimate with no measured bound.
    Estimate,
    /// The amount is a measured or declared bound, not an exact reading.
    Bounded,
    /// The amount is an exact reading of a system meter.
    Exact,
}

impl MeasurementSource {
    /// The honest derived label over a set of input sources. `Exact` only when
    /// every input is `Exact`; otherwise `Estimate` when any input is
    /// `Estimate`, else `Bounded`. The empty fold is `Exact`: an empty sum is exactly zero.
    pub fn derive<I>(sources: I) -> Self
    where
        I: IntoIterator<Item = MeasurementSource>,
    {
        sources
            .into_iter()
            .fold(Self::Exact, |label, source| label.min(source))
    }
}

/// One ledger row: a typed class, an amount, and an honest measurement
/// source.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceRow {
    /// The resource coordinate this row charges.
    pub class: ResourceClass,
    /// Amount charged to that coordinate.
    pub amount: u64,
    /// How the amount was obtained.
    pub source: MeasurementSource,
}

impl ResourceRow {
    /// One typed, labeled row.
    pub const fn new(class: ResourceClass, amount: u64, source: MeasurementSource) -> Self {
        Self {
            class,
            amount,
            source,
        }
    }
}

/// A derived per-class or grand total. The fields are private and there is no public constructor:
/// totals are computed by the ledger from rows, and the derived source honors the labeling law
/// (Exact only when every input row was Exact).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ResourceTotal {
    amount: u128,
    source: MeasurementSource,
}

impl ResourceTotal {
    /// The summed amount.
    pub fn amount(self) -> u128 {
        self.amount
    }

    /// The honest derived label.
    pub fn source(self) -> MeasurementSource {
        self.source
    }

    pub(crate) fn derived(amount: u128, source: MeasurementSource) -> Self {
        Self { amount, source }
    }
}

/// Append-only ledger of typed resource rows.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLedger {
    rows: Vec<ResourceRow>,
}

impl ResourceLedger {
    /// An empty ledger.
    pub fn new() -> Self {
        Self { rows: Vec::new() }
    }

    /// Appends one typed row. Rows are never mutated or removed.
    pub fn record(&mut self, row: ResourceRow) {
        self.rows.push(row);
    }

    /// All recorded rows, in record order.
    pub fn rows(&self) -> &[ResourceRow] {
        &self.rows
    }

    /// Derived total for one class; `None` when no row charged that class. A class with no rows is
    /// absent, not zero: absence is how a hidden uncharged coordinate stays visible to reconciliation.
    pub fn total_for(&self, class: ResourceClass) -> Option<ResourceTotal> {
        let mut amount = 0u128;
        let mut sources = Vec::new();
        for row in self.rows.iter().filter(|row| row.class == class) {
            amount += u128::from(row.amount);
            sources.push(row.source);
        }
        if sources.is_empty() {
            return None;
        }
        Some(ResourceTotal::derived(
            amount,
            MeasurementSource::derive(sources),
        ))
    }

    /// Derived grand total over every row.
    pub fn grand_total(&self) -> ResourceTotal {
        let mut amount = 0u128;
        let mut sources = Vec::new();
        for row in &self.rows {
            amount += u128::from(row.amount);
            sources.push(row.source);
        }
        ResourceTotal::derived(amount, MeasurementSource::derive(sources))
    }
}

/// One provider bill line with its declared tolerance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBillLine {
    /// Billing provider identity.
    pub provider: String,
    /// Billed resource coordinate.
    pub class: ResourceClass,
    /// Amount the provider billed.
    pub billed_amount: u64,
    /// Declared tolerance in ppm of the billed amount.
    pub tolerance_ppm: u32,
}

impl ProviderBillLine {
    /// Builds a bill line, refusing an empty provider and a tolerance above
    /// 1_000_000 ppm.
    pub fn new(
        provider: impl Into<String>,
        class: ResourceClass,
        billed_amount: u64,
        tolerance_ppm: u32,
    ) -> Result<Self, LedgerError> {
        let provider = provider.into();
        if provider.is_empty() {
            return Err(LedgerError::EmptyBillProvider);
        }
        if tolerance_ppm > PPM_ONE {
            return Err(LedgerError::PpmOutOfRange { ppm: tolerance_ppm });
        }
        Ok(Self {
            provider,
            class,
            billed_amount,
            tolerance_ppm,
        })
    }
}

/// Per-line reconciliation outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BillLineStatus {
    /// Ledger total equals the billed amount and the tolerance is zero.
    ReconcilesExactly,
    /// Ledger total deviates within the declared tolerance.
    ReconcilesWithinTolerance,
}

/// One reconciled bill line.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillLineReconciliation {
    /// Billing provider.
    pub provider: String,
    /// Billed resource coordinate.
    pub class: ResourceClass,
    /// Amount the provider billed.
    pub billed_amount: u64,
    /// Declared tolerance in ppm.
    pub tolerance_ppm: u32,
    /// Ledger-derived total for the coordinate.
    pub ledger_total: u128,
    /// Absolute deviation between ledger total and billed amount.
    pub deviation: u128,
    /// Per-line outcome.
    pub status: BillLineStatus,
}

/// Overall reconciliation state. `Exact` is reserved for the strongest case: every line reconciles
/// exactly and every contributing ledger row was measured `Exact`. Any inexact row source or any
/// nonzero tolerance demotes the overall state, so the label law holds for the whole report.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationState {
    /// Every line exact and every contributing row source Exact.
    Exact,
    /// All lines reconcile within their declared tolerances.
    WithinTolerance,
}

/// Complete provider-bill reconciliation report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderBillReconciliation {
    /// Per-line outcomes, in bill order.
    pub lines: Vec<BillLineReconciliation>,
    /// Overall state, honoring the exactness label law.
    pub overall: ReconciliationState,
}

impl ResourceLedger {
    /// Reconciles every bill line against this ledger within each line's declared tolerance.
    pub fn reconcile(
        &self,
        bills: &[ProviderBillLine],
    ) -> Result<ProviderBillReconciliation, LedgerError> {
        let mut lines = Vec::with_capacity(bills.len());
        let mut all_exact = true;
        let mut any_inexact_source = false;
        for bill in bills {
            if bill.provider.is_empty() {
                return Err(LedgerError::EmptyBillProvider);
            }
            if bill.tolerance_ppm > PPM_ONE {
                return Err(LedgerError::PpmOutOfRange {
                    ppm: bill.tolerance_ppm,
                });
            }
            let total = self.total_for(bill.class);
            let (ledger_total, deviation, status) = match total {
                None => {
                    if bill.billed_amount > 0 {
                        return Err(LedgerError::HiddenUnchargedWork {
                            provider: bill.provider.clone(),
                            class: bill.class.as_str(),
                            billed: bill.billed_amount,
                        });
                    }
                    (0, 0, BillLineStatus::ReconcilesExactly)
                }
                Some(total) => {
                    let billed = u128::from(bill.billed_amount);
                    let deviation = total.amount().abs_diff(billed);
                    let within = deviation
                        .checked_mul(u128::from(PPM_ONE))
                        .is_some_and(|scaled| scaled <= billed * u128::from(bill.tolerance_ppm));
                    if !within {
                        return Err(LedgerError::OutOfTolerance {
                            provider: bill.provider.clone(),
                            class: bill.class.as_str(),
                            billed: bill.billed_amount,
                            ledger: total.amount(),
                            tolerance_ppm: bill.tolerance_ppm,
                        });
                    }
                    let status = if deviation == 0 && bill.tolerance_ppm == 0 {
                        BillLineStatus::ReconcilesExactly
                    } else {
                        BillLineStatus::ReconcilesWithinTolerance
                    };
                    if total.source() != MeasurementSource::Exact {
                        any_inexact_source = true;
                    }
                    (total.amount(), deviation, status)
                }
            };
            if status != BillLineStatus::ReconcilesExactly {
                all_exact = false;
            }
            lines.push(BillLineReconciliation {
                provider: bill.provider.clone(),
                class: bill.class,
                billed_amount: bill.billed_amount,
                tolerance_ppm: bill.tolerance_ppm,
                ledger_total,
                deviation,
                status,
            });
        }
        let overall = if all_exact && !any_inexact_source {
            ReconciliationState::Exact
        } else {
            ReconciliationState::WithinTolerance
        };
        Ok(ProviderBillReconciliation { lines, overall })
    }
}

/// Wire decoding goes through the validated constructor.
impl<'de> Deserialize<'de> for ProviderBillLine {
    /// Wire decoding goes through the validated constructor.
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            provider: String,
            class: ResourceClass,
            billed_amount: u64,
            tolerance_ppm: u32,
        }
        let wire = Wire::deserialize(deserializer)?;
        Self::new(
            wire.provider,
            wire.class,
            wire.billed_amount,
            wire.tolerance_ppm,
        )
        .map_err(de::Error::custom)
    }
}
