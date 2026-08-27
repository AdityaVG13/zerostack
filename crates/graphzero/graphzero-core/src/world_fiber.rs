//! Sound structural world / effect alternatives (feeds hub aggregate fiber).

use std::collections::BTreeSet;

use graphzero_types::ContentHash;

use crate::truth::TruthClass;

/// Classification of a world alternative.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum FiberClass {
    Exact,
    SoundOverapproximation,
    /// Explicit underapproximation -- must never be labeled complete/strict.
    Underapproximation,
    Unknown,
}

impl FiberClass {
    #[must_use]
    pub const fn admissible_as_strict(self) -> bool {
        matches!(self, Self::Exact | Self::SoundOverapproximation)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorldAlternative {
    pub id: ContentHash,
    pub class: FiberClass,
    pub truth: TruthClass,
    pub effects: BTreeSet<ContentHash>,
    pub premises: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorldFiber {
    pub source_root: ContentHash,
    pub alternatives: Vec<WorldAlternative>,
}

impl WorldFiber {
    #[must_use]
    pub fn new(source_root: ContentHash) -> Self {
        Self {
            source_root,
            alternatives: Vec::new(),
        }
    }

    pub fn push(&mut self, alt: WorldAlternative) {
        self.alternatives.push(alt);
    }

    /// Strict fiber may only contain exact or sound-overapprox alternatives.
    #[must_use]
    pub fn is_strict_admissible(&self) -> bool {
        !self.alternatives.is_empty()
            && self
                .alternatives
                .iter()
                .all(|a| a.class.admissible_as_strict() && a.truth.admissible_in_strict_fiber())
    }

    /// Detect hidden underapproximation labeled as complete/strict.
    #[must_use]
    pub fn has_hidden_underapproximation(&self) -> bool {
        self.alternatives
            .iter()
            .any(|a| a.class == FiberClass::Underapproximation)
    }

    /// Common effect intersection across alternatives (empty if any Unknown).
    #[must_use]
    pub fn common_effects(&self) -> Option<BTreeSet<ContentHash>> {
        if self.alternatives.is_empty() {
            return Some(BTreeSet::new());
        }
        if self
            .alternatives
            .iter()
            .any(|a| a.class == FiberClass::Unknown)
        {
            return None;
        }
        let mut iter = self.alternatives.iter();
        let mut acc = iter.next().unwrap().effects.clone();
        for a in iter {
            acc = acc.intersection(&a.effects).copied().collect();
        }
        Some(acc)
    }
}
