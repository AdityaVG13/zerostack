//! Paired comparable observations for savings measurement. Measurement-only, off the authority
//! path. A `PairedObservations` binds one native baseline observation and one model-visible Zero
//! observation that share the same and machine fingerprint.

#![forbid(unsafe_code)]

use crate::observation::{Observation, ObservationError, ObservationKind};
use std::error::Error;
use std::fmt;

/// One paired measurement: the native baseline and the model-visible Zero
/// observation for the same task on the same machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PairedObservations {
    pub native: Observation,
    pub zero: Observation,
}

impl PairedObservations {
    /// Bind a native baseline and a Zero observation into a comparable pair.
    pub fn new(native: Observation, zero: Observation) -> Result<Self, PairError> {
        native.validate().map_err(PairError::InvalidObservation)?;
        zero.validate().map_err(PairError::InvalidObservation)?;
        if native.kind != ObservationKind::NativeBaseline {
            return Err(PairError::KindMismatch {
                expected: ObservationKind::NativeBaseline,
                actual: native.kind,
                side: PairSide::Native,
            });
        }
        if zero.kind != ObservationKind::ZeroDirect {
            return Err(PairError::KindMismatch {
                expected: ObservationKind::ZeroDirect,
                actual: zero.kind,
                side: PairSide::Zero,
            });
        }
        if native.task != zero.task {
            return Err(PairError::TaskMismatch);
        }
        if native.machine != zero.machine {
            return Err(PairError::MachineMismatch);
        }
        Ok(Self { native, zero })
    }

    /// Borrowed constructor when observations are already owned elsewhere.
    pub fn try_from_refs(native: &Observation, zero: &Observation) -> Result<Self, PairError> {
        Self::new(native.clone(), zero.clone())
    }

    pub fn native(&self) -> &Observation {
        &self.native
    }

    pub fn zero(&self) -> &Observation {
        &self.zero
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairSide {
    Native,
    Zero,
}

/// Typed, fail-closed pairing failure. Never a weaker claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PairError {
    InvalidObservation(ObservationError),
    KindMismatch {
        expected: ObservationKind,
        actual: ObservationKind,
        side: PairSide,
    },
    TaskMismatch,
    MachineMismatch,
}

impl fmt::Display for PairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidObservation(err) => write!(f, "paired observation invalid: {err}"),
            Self::KindMismatch {
                expected,
                actual,
                side,
            } => write!(
                f,
                "pair kind mismatch on {side:?}: expected {expected:?}, actual {actual:?}"
            ),
            Self::TaskMismatch => write!(f, "paired observations have different task identity"),
            Self::MachineMismatch => {
                write!(f, "paired observations have different machine fingerprint")
            }
        }
    }
}

impl Error for PairError {}
