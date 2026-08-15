//! Bounded accounting for one server-side verdict loop.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zero_abi::{WorkerTokenAccountingV1, WorkerTokenCountKind};

pub const VERDICT_LOOP_RECEIPT_SCHEMA: &str = "zerostack.verdict_loop_receipt.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerdictLoopEnvelope {
    pub max_logical_dispatches: u64,
    pub max_raw_worker_input_bytes: u64,
    pub max_raw_worker_output_bytes: u64,
    pub max_raw_tokens: u64,
    pub max_visible_tokens: u64,
    pub max_recovery_tokens: u64,
    pub max_billed_tokens: u64,
    pub max_cached_tokens: u64,
}

impl VerdictLoopEnvelope {
    pub fn validate(&self) -> Result<(), String> {
        if [
            self.max_logical_dispatches,
            self.max_raw_worker_input_bytes,
            self.max_raw_worker_output_bytes,
            self.max_raw_tokens,
            self.max_visible_tokens,
            self.max_recovery_tokens,
            self.max_billed_tokens,
        ]
        .contains(&0)
        {
            return Err("verdict-loop non-cache bounds must be nonzero".into());
        }
        if self.max_cached_tokens > self.max_billed_tokens {
            return Err("verdict-loop cached-token bound exceeds billed-token bound".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerdictDecision {
    Pass,
    Fail,
}

impl VerdictDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
        }
    }

    fn from_value(value: &Value) -> Result<Self, String> {
        match value.as_str() {
            Some("pass") => Ok(Self::Pass),
            Some("fail") => Ok(Self::Fail),
            _ => Err("verdict-loop must return exactly the string pass or fail".into()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerdictLoopReceiptV1 {
    pub schema: String,
    pub logical_dispatches: u64,
    pub raw_worker_input_bytes: u64,
    pub raw_worker_output_bytes: u64,
    pub raw_tokens: u64,
    pub visible_tokens: u64,
    pub recovery_tokens: u64,
    pub billed_tokens: u64,
    pub cached_tokens: u64,
    pub exact_ref_tokens: Option<u64>,
    pub tokenizer_ids: Vec<String>,
    pub count_kinds: Vec<WorkerTokenCountKind>,
    pub final_atom_json_bytes: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VerdictLoopResult {
    pub decision: VerdictDecision,
    pub receipt: VerdictLoopReceiptV1,
}

pub(crate) struct VerdictMeter {
    envelope: VerdictLoopEnvelope,
    receipt: VerdictLoopReceiptV1,
    tokenizer_ids: BTreeSet<String>,
    count_kinds: BTreeSet<String>,
    failure: Option<String>,
}

impl VerdictMeter {
    pub(crate) fn new(envelope: VerdictLoopEnvelope) -> Result<Self, String> {
        envelope.validate()?;
        Ok(Self {
            envelope,
            receipt: VerdictLoopReceiptV1 {
                schema: VERDICT_LOOP_RECEIPT_SCHEMA.into(),
                logical_dispatches: 0,
                raw_worker_input_bytes: 0,
                raw_worker_output_bytes: 0,
                raw_tokens: 0,
                visible_tokens: 0,
                recovery_tokens: 0,
                billed_tokens: 0,
                cached_tokens: 0,
                exact_ref_tokens: Some(0),
                tokenizer_ids: Vec::new(),
                count_kinds: Vec::new(),
                final_atom_json_bytes: 0,
            },
            tokenizer_ids: BTreeSet::new(),
            count_kinds: BTreeSet::new(),
            failure: None,
        })
    }

    fn sticky(&mut self, detail: impl Into<String>) -> String {
        let detail = detail.into();
        if self.failure.is_none() {
            self.failure = Some(detail.clone());
        }
        self.failure.clone().unwrap_or(detail)
    }

    fn add_bounded(
        &mut self,
        field: &'static str,
        amount: u64,
        maximum: u64,
    ) -> Result<u64, String> {
        let current = match field {
            "logical_dispatches" => self.receipt.logical_dispatches,
            "raw_worker_input_bytes" => self.receipt.raw_worker_input_bytes,
            "raw_worker_output_bytes" => self.receipt.raw_worker_output_bytes,
            "raw_tokens" => self.receipt.raw_tokens,
            "visible_tokens" => self.receipt.visible_tokens,
            "recovery_tokens" => self.receipt.recovery_tokens,
            "billed_tokens" => self.receipt.billed_tokens,
            "cached_tokens" => self.receipt.cached_tokens,
            _ => return Err(self.sticky(format!("unknown verdict meter field {field}"))),
        };
        let next = current
            .checked_add(amount)
            .ok_or_else(|| self.sticky(format!("verdict-loop {field} overflowed")))?;
        if next > maximum {
            return Err(self.sticky(format!(
                "verdict-loop {field} budget exceeded: requested={next} maximum={maximum}"
            )));
        }
        Ok(next)
    }

    pub(crate) fn reserve_dispatch(&mut self, input_bytes: u64) -> Result<(), String> {
        if let Some(failure) = &self.failure {
            return Err(failure.clone());
        }
        self.receipt.logical_dispatches = self.add_bounded(
            "logical_dispatches",
            1,
            self.envelope.max_logical_dispatches,
        )?;
        self.receipt.raw_worker_input_bytes = self.add_bounded(
            "raw_worker_input_bytes",
            input_bytes,
            self.envelope.max_raw_worker_input_bytes,
        )?;
        Ok(())
    }

    pub(crate) fn record_response(
        &mut self,
        output_bytes: u64,
        accounting: Option<&WorkerTokenAccountingV1>,
    ) -> Result<(), String> {
        let accounting = accounting
            .ok_or_else(|| self.sticky("verdict-loop dispatch omitted worker token accounting"))?;
        if accounting.count_kind == WorkerTokenCountKind::Estimate {
            return Err(self.sticky("verdict-loop dispatch returned estimated token accounting"));
        }
        if accounting.visible_tokens > accounting.raw_tokens
            || accounting.recovery_tokens > accounting.raw_tokens
            || accounting.cached_tokens > accounting.billed_tokens
        {
            return Err(self.sticky(
                "verdict-loop dispatch returned internally inconsistent token accounting",
            ));
        }
        self.receipt.raw_worker_output_bytes = self.add_bounded(
            "raw_worker_output_bytes",
            output_bytes,
            self.envelope.max_raw_worker_output_bytes,
        )?;
        self.receipt.raw_tokens = self.add_bounded(
            "raw_tokens",
            accounting.raw_tokens,
            self.envelope.max_raw_tokens,
        )?;
        self.receipt.visible_tokens = self.add_bounded(
            "visible_tokens",
            accounting.visible_tokens,
            self.envelope.max_visible_tokens,
        )?;
        self.receipt.recovery_tokens = self.add_bounded(
            "recovery_tokens",
            accounting.recovery_tokens,
            self.envelope.max_recovery_tokens,
        )?;
        self.receipt.billed_tokens = self.add_bounded(
            "billed_tokens",
            accounting.billed_tokens,
            self.envelope.max_billed_tokens,
        )?;
        self.receipt.cached_tokens = self.add_bounded(
            "cached_tokens",
            accounting.cached_tokens,
            self.envelope.max_cached_tokens,
        )?;
        self.receipt.exact_ref_tokens =
            match (self.receipt.exact_ref_tokens, accounting.exact_ref_tokens) {
                (Some(total), Some(value)) => Some(
                    total
                        .checked_add(value)
                        .ok_or_else(|| self.sticky("verdict-loop exact_ref_tokens overflowed"))?,
                ),
                _ => None,
            };
        self.tokenizer_ids.insert(accounting.tokenizer_id.clone());
        self.count_kinds.insert(
            match accounting.count_kind {
                WorkerTokenCountKind::Exact => "exact",
                WorkerTokenCountKind::ConservativeUpperBound => "conservative_upper_bound",
                WorkerTokenCountKind::Estimate => "estimate",
            }
            .into(),
        );
        Ok(())
    }

    pub(crate) fn fail(&mut self, detail: impl Into<String>) {
        self.sticky(detail);
    }

    pub(crate) fn finish(mut self, value: &Value) -> Result<VerdictLoopResult, String> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        let decision = VerdictDecision::from_value(value)?;
        self.receipt.final_atom_json_bytes = serde_json::to_vec(value)
            .map_err(|error| format!("cannot encode verdict atom: {error}"))?
            .len()
            .try_into()
            .map_err(|_| "verdict atom byte count overflowed".to_string())?;
        self.receipt.tokenizer_ids = self.tokenizer_ids.into_iter().collect();
        self.receipt.count_kinds = self
            .count_kinds
            .into_iter()
            .map(|kind| match kind.as_str() {
                "exact" => Ok(WorkerTokenCountKind::Exact),
                "conservative_upper_bound" => Ok(WorkerTokenCountKind::ConservativeUpperBound),
                "estimate" => Ok(WorkerTokenCountKind::Estimate),
                other => Err(format!("verdict-loop unknown count_kind {other}")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(VerdictLoopResult {
            decision,
            receipt: self.receipt,
        })
    }
}

#[cfg(test)]
mod honesty_tests {
    use super::*;
    use serde_json::json;

    fn envelope() -> VerdictLoopEnvelope {
        VerdictLoopEnvelope {
            max_logical_dispatches: 8,
            max_raw_worker_input_bytes: 1024,
            max_raw_worker_output_bytes: 1024,
            max_raw_tokens: 1024,
            max_visible_tokens: 1024,
            max_recovery_tokens: 1024,
            max_billed_tokens: 1024,
            max_cached_tokens: 0,
        }
    }

    #[test]
    fn finish_preserves_estimate_count_kind() {
        let mut meter = VerdictMeter::new(envelope()).expect("envelope");
        meter.count_kinds.insert("estimate".into());
        let result = meter.finish(&json!("pass")).expect("finish");
        assert_eq!(
            result.receipt.count_kinds,
            vec![WorkerTokenCountKind::Estimate]
        );
    }

    #[test]
    fn finish_preserves_exact_and_conservative_kinds() {
        let mut meter = VerdictMeter::new(envelope()).expect("envelope");
        meter.count_kinds.insert("exact".into());
        meter.count_kinds.insert("conservative_upper_bound".into());
        let result = meter.finish(&json!("fail")).expect("finish");
        assert!(result.receipt.count_kinds.contains(&WorkerTokenCountKind::Exact));
        assert!(result
            .receipt
            .count_kinds
            .contains(&WorkerTokenCountKind::ConservativeUpperBound));
        assert!(!result
            .receipt
            .count_kinds
            .contains(&WorkerTokenCountKind::Estimate));
    }
}
