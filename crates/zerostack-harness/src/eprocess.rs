//! Ville-bounded e-processes. Hardware vs software calibration is fixed.

use crate::repo::repo_root;
use crate::spec_oracle::verify_spec_comp_001;

pub const HARDWARE_P0: f64 = 1e-9;
pub const HARDWARE_LAMBDA: f64 = 0.999;
pub const HARDWARE_ALPHA: f64 = 1e-6;
pub const SOFTWARE_P0: f64 = 1e-6;
pub const SOFTWARE_LAMBDA: f64 = 0.9;
pub const SOFTWARE_ALPHA: f64 = 0.001;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Calibration {
    Hardware,
    Software,
}

impl Calibration {
    pub const fn params(self) -> (f64, f64, f64) {
        match self {
            Self::Hardware => (HARDWARE_P0, HARDWARE_LAMBDA, HARDWARE_ALPHA),
            Self::Software => (SOFTWARE_P0, SOFTWARE_LAMBDA, SOFTWARE_ALPHA),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MonitoredInvariant {
    CasDigestMatch,
    AtomicRenameVisibility,
    EnginesDoNotImportEachOther,
}

impl MonitoredInvariant {
    pub const ALL: [Self; 3] = [
        Self::CasDigestMatch,
        Self::AtomicRenameVisibility,
        Self::EnginesDoNotImportEachOther,
    ];

    pub const fn calibration(self) -> Calibration {
        match self {
            Self::CasDigestMatch | Self::AtomicRenameVisibility => Calibration::Hardware,
            Self::EnginesDoNotImportEachOther => Calibration::Software,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CasDigestMatch => "CasDigestMatch",
            Self::AtomicRenameVisibility => "AtomicRenameVisibility",
            Self::EnginesDoNotImportEachOther => "EnginesDoNotImportEachOther",
        }
    }
}

#[derive(Clone, Debug)]
pub struct EProcess {
    pub invariant: MonitoredInvariant,
    pub p0: f64,
    pub lambda: f64,
    pub alpha: f64,
    pub e_value: f64,
    pub observations: u64,
}

impl EProcess {
    pub fn new(invariant: MonitoredInvariant) -> Self {
        let (p0, lambda, alpha) = invariant.calibration().params();
        Self {
            invariant,
            p0,
            lambda,
            alpha,
            e_value: 1.0,
            observations: 0,
        }
    }

    /// x = 0 held, x = 1 violated. Mixture likelihood ratio from Pattern 70.
    pub fn update(&mut self, violated: bool) {
        let increment = if violated {
            self.lambda / self.p0
        } else {
            (1.0 - self.lambda) / (1.0 - self.p0)
        };
        self.e_value *= increment;
        self.observations += 1;
    }

    pub fn rejected(&self) -> bool {
        self.e_value >= 1.0 / self.alpha
    }
}

pub fn global_e_value(per_invariant: &[EProcess]) -> f64 {
    if per_invariant.is_empty() {
        return 1.0;
    }
    per_invariant.iter().map(|e| e.e_value).sum::<f64>() / per_invariant.len() as f64
}

pub fn global_rejected(per_invariant: &[EProcess]) -> bool {
    if per_invariant.is_empty() {
        return false;
    }
    let alpha = per_invariant
        .iter()
        .map(|e| e.alpha)
        .fold(f64::INFINITY, f64::min);
    global_e_value(per_invariant) >= 1.0 / alpha
}

pub fn observe_engines_do_not_import() -> bool {
    verify_spec_comp_001(&repo_root()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h0_stays_below_ville() {
        let mut proc = EProcess::new(MonitoredInvariant::CasDigestMatch);
        for _ in 0..10_000 {
            proc.update(false);
        }
        assert!(!proc.rejected());
        assert!(proc.e_value < 1.0);
    }

    #[test]
    fn hardware_violation_crosses_immediately() {
        let mut proc = EProcess::new(MonitoredInvariant::CasDigestMatch);
        proc.update(true);
        assert!(proc.rejected());
        assert!(proc.e_value >= 1.0 / HARDWARE_ALPHA);
    }

    #[test]
    fn software_violation_crosses() {
        let mut proc = EProcess::new(MonitoredInvariant::EnginesDoNotImportEachOther);
        proc.update(true);
        assert!(proc.rejected());
    }

    #[test]
    fn global_mean_is_eprocess() {
        let mut hardware = EProcess::new(MonitoredInvariant::CasDigestMatch);
        let software = EProcess::new(MonitoredInvariant::EnginesDoNotImportEachOther);
        hardware.update(false);
        let mean = global_e_value(&[hardware.clone(), software]);
        assert!((mean - (hardware.e_value + 1.0) / 2.0).abs() < 1e-12);
        assert!(!global_rejected(&[hardware]));
    }

    #[test]
    fn engines_do_not_import_holds() {
        assert!(observe_engines_do_not_import());
    }
}
