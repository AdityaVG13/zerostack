//! Budgets and truncation: deadline, bytes, match caps (fszero-sz0t).

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpBudgets {
    pub deadline: Option<Duration>,
    pub max_bytes: Option<usize>,
    pub max_matches: Option<usize>,
}

impl Default for OpBudgets {
    fn default() -> Self {
        Self {
            deadline: None,
            max_bytes: Some(1_048_576),
            max_matches: Some(10_000),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetHit {
    Deadline,
    Bytes { used: usize, cap: usize },
    Matches { used: usize, cap: usize },
}

pub struct BudgetTracker {
    budgets: OpBudgets,
    started: Instant,
    bytes: usize,
    matches: usize,
}

impl BudgetTracker {
    pub fn start(budgets: OpBudgets) -> Self {
        Self {
            budgets,
            started: Instant::now(),
            bytes: 0,
            matches: 0,
        }
    }

    pub fn add_bytes(&mut self, n: usize) -> Result<(), BudgetHit> {
        self.bytes = self.bytes.saturating_add(n);
        if let Some(cap) = self.budgets.max_bytes {
            if self.bytes > cap {
                return Err(BudgetHit::Bytes {
                    used: self.bytes,
                    cap,
                });
            }
        }
        self.check_deadline()
    }

    pub fn add_match(&mut self) -> Result<(), BudgetHit> {
        self.matches = self.matches.saturating_add(1);
        if let Some(cap) = self.budgets.max_matches {
            if self.matches > cap {
                return Err(BudgetHit::Matches {
                    used: self.matches,
                    cap,
                });
            }
        }
        self.check_deadline()
    }

    pub fn check_deadline(&self) -> Result<(), BudgetHit> {
        if let Some(d) = self.budgets.deadline {
            if self.started.elapsed() > d {
                return Err(BudgetHit::Deadline);
            }
        }
        Ok(())
    }
}

/// Truncate payload to max_bytes with a clear truncated flag.
pub fn truncate_bytes(data: &[u8], max_bytes: usize) -> (Vec<u8>, bool) {
    if data.len() <= max_bytes {
        (data.to_vec(), false)
    } else {
        (data[..max_bytes].to_vec(), true)
    }
}
