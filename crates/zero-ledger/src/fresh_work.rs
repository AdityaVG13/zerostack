//! Fresh-work accounting vector and the eta_action efficiency metric.
//!
//! Normative prose: conformance/contracts/fresh-work-vector.md.
//!
//! Token savings alone cannot show that redundant work is disappearing: a
//! cheap action that re-derives information the session already paid for still
//! costs. This module decomposes the declared input of one action into the
//! components of the pay-once causal information law and exposes
//! `eta_action` = fresh work / total as an integer parts-per-million ratio.
//!
//! Invariants enforced here, at construction and again on the wire:
//!
//! - the four components sum exactly to `total_tokens` (no side component),
//! - `eta_action` is therefore always inside [0, 1] (0 ppm..=1_000_000 ppm),
//! - aggregation is checked integer addition, so a session-level eta is the
//!   same arithmetic applied to the summed vector.

use serde::de;
use serde::{Deserialize, Deserializer, Serialize};

use crate::{LedgerError, PPM_ONE, RetainedFractionPpm};

/// The exhaustive component set of the fresh-work vector.
///
/// Every token of an action's declared input belongs to exactly one component.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum FreshWorkComponent {
    /// Causally novel work: new instructions and changed objects the session
    /// has never paid for. This is the numerator of eta_action.
    FreshWork,
    /// Work served from prior information: cache hits, replayed evidence and
    /// re-exposed spans the session already holds.
    Replayed,
    /// Work spent recovering or re-expanding information that was already
    /// paid for once: verification, retries, re-expansion of compressed spans.
    Recovery,
    /// Structural cost that carries no repository information: schema,
    /// protocol framing and harness scaffolding.
    Overhead,
}

impl FreshWorkComponent {
    /// Every component, in canonical order.
    pub const ALL: [FreshWorkComponent; 4] = [
        FreshWorkComponent::FreshWork,
        FreshWorkComponent::Replayed,
        FreshWorkComponent::Recovery,
        FreshWorkComponent::Overhead,
    ];

    /// Name of the vector field this component accumulates into.
    pub fn field_name(self) -> &'static str {
        match self {
            Self::FreshWork => "fresh_work_tokens",
            Self::Replayed => "replayed_tokens",
            Self::Recovery => "recovery_tokens",
            Self::Overhead => "overhead_tokens",
        }
    }
}

impl std::fmt::Display for FreshWorkComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.field_name())
    }
}

/// One action's token cost decomposed across [`FreshWorkComponent`].
///
/// `total_tokens` is derived, never caller-supplied: it is recomputed by
/// [`FreshWorkVector::new`] and re-checked when a vector is deserialized, so a
/// wire value cannot understate a component or inflate the total to flatter
/// eta_action. The all-zero vector is the "not declared" case.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct FreshWorkVector {
    fresh_work_tokens: u64,
    replayed_tokens: u64,
    recovery_tokens: u64,
    overhead_tokens: u64,
    total_tokens: u64,
}

impl FreshWorkVector {
    /// Builds a vector from its components, deriving the total with checked
    /// arithmetic.
    pub fn new(
        fresh_work_tokens: u64,
        replayed_tokens: u64,
        recovery_tokens: u64,
        overhead_tokens: u64,
    ) -> Result<Self, LedgerError> {
        let mut total = 0u64;
        for (component, tokens) in [
            (FreshWorkComponent::FreshWork, fresh_work_tokens),
            (FreshWorkComponent::Replayed, replayed_tokens),
            (FreshWorkComponent::Recovery, recovery_tokens),
            (FreshWorkComponent::Overhead, overhead_tokens),
        ] {
            total = total
                .checked_add(tokens)
                .ok_or(LedgerError::CounterOverflow {
                    counter: component.field_name(),
                })?;
        }
        Ok(Self {
            fresh_work_tokens,
            replayed_tokens,
            recovery_tokens,
            overhead_tokens,
            total_tokens: total,
        })
    }

    /// A vector whose whole cost is causally novel work.
    pub fn all_fresh(tokens: u64) -> Self {
        Self {
            fresh_work_tokens: tokens,
            replayed_tokens: 0,
            recovery_tokens: 0,
            overhead_tokens: 0,
            total_tokens: tokens,
        }
    }

    /// Tokens attributed to one component.
    pub fn component_tokens(&self, component: FreshWorkComponent) -> u64 {
        match component {
            FreshWorkComponent::FreshWork => self.fresh_work_tokens,
            FreshWorkComponent::Replayed => self.replayed_tokens,
            FreshWorkComponent::Recovery => self.recovery_tokens,
            FreshWorkComponent::Overhead => self.overhead_tokens,
        }
    }

    /// Causally novel tokens: the numerator of eta_action.
    pub fn fresh_work_tokens(&self) -> u64 {
        self.fresh_work_tokens
    }

    /// Tokens served from information the session already paid for.
    pub fn replayed_tokens(&self) -> u64 {
        self.replayed_tokens
    }

    /// Tokens spent recovering or re-expanding already-paid information.
    pub fn recovery_tokens(&self) -> u64 {
        self.recovery_tokens
    }

    /// Structural tokens that carry no repository information.
    pub fn overhead_tokens(&self) -> u64 {
        self.overhead_tokens
    }

    /// Sum over every component. Always equal to [`Self::component_sum`].
    pub fn total_tokens(&self) -> u64 {
        self.total_tokens
    }

    /// Recomputes the component sum from the components alone.
    pub fn component_sum(&self) -> Result<u64, LedgerError> {
        let mut total = 0u64;
        for component in FreshWorkComponent::ALL {
            total = total.checked_add(self.component_tokens(component)).ok_or(
                LedgerError::CounterOverflow {
                    counter: component.field_name(),
                },
            )?;
        }
        Ok(total)
    }

    /// True when this action declared a decomposition at all.
    pub fn is_declared(&self) -> bool {
        self.total_tokens > 0
    }

    /// eta_action: the fraction of this action's cost that was causally novel,
    /// floored to parts per million. `None` for an undeclared (all-zero)
    /// vector, where the ratio has no denominator.
    ///
    /// Because the components sum to the total, the result is always inside
    /// [0, 1]. eta_action -> 0 is the target: transformations grow while the
    /// novel delta stays structurally describable.
    pub fn eta_action_ppm(&self) -> Option<RetainedFractionPpm> {
        if self.total_tokens == 0 {
            return None;
        }
        let ppm = u128::from(self.fresh_work_tokens) * u128::from(PPM_ONE)
            / u128::from(self.total_tokens);
        let ppm =
            u32::try_from(ppm).expect("fresh work never exceeds the total, so ppm <= PPM_ONE");
        Some(RetainedFractionPpm::new(ppm).expect("ppm <= PPM_ONE by the component-sum invariant"))
    }

    /// Component-wise checked addition, used to aggregate actions.
    pub fn merge(&self, other: &Self) -> Result<Self, LedgerError> {
        let mut merged = *self;
        for component in FreshWorkComponent::ALL {
            let sum = self
                .component_tokens(component)
                .checked_add(other.component_tokens(component))
                .ok_or(LedgerError::CounterOverflow {
                    counter: component.field_name(),
                })?;
            merged.set_component(component, sum);
        }
        merged.total_tokens = merged.component_sum()?;
        Ok(merged)
    }

    fn set_component(&mut self, component: FreshWorkComponent, tokens: u64) {
        match component {
            FreshWorkComponent::FreshWork => self.fresh_work_tokens = tokens,
            FreshWorkComponent::Replayed => self.replayed_tokens = tokens,
            FreshWorkComponent::Recovery => self.recovery_tokens = tokens,
            FreshWorkComponent::Overhead => self.overhead_tokens = tokens,
        }
    }
}

#[derive(Deserialize)]
struct FreshWorkVectorWire {
    fresh_work_tokens: u64,
    replayed_tokens: u64,
    recovery_tokens: u64,
    overhead_tokens: u64,
    total_tokens: u64,
}

impl<'de> Deserialize<'de> for FreshWorkVector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = FreshWorkVectorWire::deserialize(deserializer)?;
        let vector = Self::new(
            wire.fresh_work_tokens,
            wire.replayed_tokens,
            wire.recovery_tokens,
            wire.overhead_tokens,
        )
        .map_err(de::Error::custom)?;
        if vector.total_tokens != wire.total_tokens {
            return Err(de::Error::custom(LedgerError::FreshWorkTotalMismatch {
                declared: wire.total_tokens,
                decomposed: vector.total_tokens,
            }));
        }
        Ok(vector)
    }
}

/// One emitted action record: the action's identity plus its fresh-work vector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActionFreshWork {
    action_id: String,
    vector: FreshWorkVector,
}

impl ActionFreshWork {
    /// Records one action. An empty action id is not an identity.
    pub fn new(action_id: impl Into<String>, vector: FreshWorkVector) -> Result<Self, LedgerError> {
        let action_id = action_id.into();
        if action_id.is_empty() {
            return Err(LedgerError::EmptyActionId);
        }
        Ok(Self { action_id, vector })
    }

    /// Identity of the action this vector accounts for.
    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    /// The action's fresh-work decomposition.
    pub fn vector(&self) -> &FreshWorkVector {
        &self.vector
    }
}

#[derive(Deserialize)]
struct ActionFreshWorkWire {
    action_id: String,
    vector: FreshWorkVector,
}

impl<'de> Deserialize<'de> for ActionFreshWork {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ActionFreshWorkWire::deserialize(deserializer)?;
        Self::new(wire.action_id, wire.vector).map_err(de::Error::custom)
    }
}

/// Session-level aggregate of per-action fresh-work vectors.
///
/// The aggregate is the component-wise sum, so the session eta is the same
/// ratio computed over the summed vector: the novelty fraction of everything
/// the session paid for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SessionFreshWork {
    actions: u64,
    aggregate: FreshWorkVector,
}

impl SessionFreshWork {
    /// Aggregates action records into one session vector.
    pub fn from_actions<'a, I>(actions: I) -> Result<Self, LedgerError>
    where
        I: IntoIterator<Item = &'a ActionFreshWork>,
    {
        let mut session = Self::default();
        for action in actions {
            session = session.with_action(action.vector())?;
        }
        Ok(session)
    }

    /// Folds one more action's vector into the session aggregate.
    pub fn with_action(&self, vector: &FreshWorkVector) -> Result<Self, LedgerError> {
        Ok(Self {
            actions: self
                .actions
                .checked_add(1)
                .ok_or(LedgerError::CounterOverflow { counter: "actions" })?,
            aggregate: self.aggregate.merge(vector)?,
        })
    }

    /// Number of actions folded in.
    pub fn actions(&self) -> u64 {
        self.actions
    }

    /// The component-wise session aggregate.
    pub fn aggregate(&self) -> &FreshWorkVector {
        &self.aggregate
    }

    /// Session-level eta: aggregate fresh work over aggregate total.
    pub fn eta_session_ppm(&self) -> Option<RetainedFractionPpm> {
        self.aggregate.eta_action_ppm()
    }
}

#[derive(Deserialize)]
struct SessionFreshWorkWire {
    actions: u64,
    aggregate: FreshWorkVector,
}

impl<'de> Deserialize<'de> for SessionFreshWork {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = SessionFreshWorkWire::deserialize(deserializer)?;
        if wire.actions == 0 && wire.aggregate.is_declared() {
            return Err(de::Error::custom(LedgerError::FreshWorkTotalMismatch {
                declared: 0,
                decomposed: wire.aggregate.total_tokens(),
            }));
        }
        Ok(Self {
            actions: wire.actions,
            aggregate: wire.aggregate,
        })
    }
}
