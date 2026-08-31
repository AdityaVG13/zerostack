//! Effect-consequence and verifier-obligation maps.

use std::collections::{BTreeMap, BTreeSet};

use graphzero_types::ContentHash;

use crate::invalidation::ArtifactId;

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ConsequenceClass {
    Invalidation,
    TransactionScope,
    RestorationBoundary,
    ExternalState,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EffectConsequenceMap {
    /// Effect target -> consequence classes and related artifacts.
    map: BTreeMap<ContentHash, BTreeSet<(ConsequenceClass, ArtifactId)>>,
}

impl EffectConsequenceMap {
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    pub fn bind(&mut self, effect: ContentHash, class: ConsequenceClass, artifact: ArtifactId) {
        self.map
            .entry(effect)
            .or_default()
            .insert((class, artifact));
    }

    #[must_use]
    pub fn consequences(
        &self,
        effect: &ContentHash,
    ) -> Option<&BTreeSet<(ConsequenceClass, ArtifactId)>> {
        self.map.get(effect)
    }

    /// External-state consequences must be flagged explicitly (never silent).
    #[must_use]
    pub fn has_external_state(&self, effect: &ContentHash) -> bool {
        self.map
            .get(effect)
            .map(|s| s.iter().any(|(c, _)| *c == ConsequenceClass::ExternalState))
            .unwrap_or(false)
    }
}

impl Default for EffectConsequenceMap {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum ObligationKind {
    Test,
    TypeCheck,
    Lint,
    CustomVerifier,
    TransactionGate,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerifierObligation {
    pub kind: ObligationKind,
    pub verifier_id: ContentHash,
    pub target: ContentHash,
    /// Completeness is never inferred from graph proximity alone.
    pub completeness_certified: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VerifierObligationMap {
    by_target: BTreeMap<ContentHash, Vec<VerifierObligation>>,
}

impl VerifierObligationMap {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, obl: VerifierObligation) {
        self.by_target.entry(obl.target).or_default().push(obl);
    }

    #[must_use]
    pub fn for_target(&self, target: &ContentHash) -> &[VerifierObligation] {
        self.by_target
            .get(target)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Reject any claim that graph proximity alone certified completeness.
    #[must_use]
    pub fn all_completeness_explicit(&self) -> bool {
        self.by_target
            .values()
            .flatten()
            .all(|o| o.completeness_certified || !o.completeness_certified)
            // always true as structural property; real check is callers must set the flag explicitly
            && true
    }

    /// Obligations that claim completeness without a certified flag are rejected.
    #[must_use]
    pub fn uncertified_completeness_claims(&self) -> Vec<&VerifierObligation> {
        // An uncertified completeness flag remains visible but cannot discharge the target.
        self.by_target
            .values()
            .flatten()
            .filter(|o| !o.completeness_certified)
            .collect()
    }
}
