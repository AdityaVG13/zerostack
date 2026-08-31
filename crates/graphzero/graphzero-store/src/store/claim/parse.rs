use serde::{Deserialize, Serialize};

/// Supported assertion kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    /// No remaining tier-A `calls` edges into `target` symbol.
    NoRemainingCallers,
    /// No remaining tier-A `calls` edges out of `target` symbol.
    NoOutgoingCalls,
    /// No remaining tier-A `refs` edges into `target` symbol.
    NoRemainingReferences,
    /// Target symbol is absent from the indexed graph (post-deletion check).
    SymbolRemoved,
    /// No remaining incoming `calls`, `refs`, or `imports` edges into `target`.
    NoRemainingDependencies,
}

impl ClaimKind {
    pub const ALL: &'static [Self] = &[
        Self::NoRemainingCallers,
        Self::NoOutgoingCalls,
        Self::NoRemainingReferences,
        Self::SymbolRemoved,
        Self::NoRemainingDependencies,
    ];

    pub fn parse_claim_kind(s: &str) -> Option<Self> {
        match s {
            "no_remaining_callers" => Some(Self::NoRemainingCallers),
            "no_outgoing_calls" => Some(Self::NoOutgoingCalls),
            "no_remaining_references" => Some(Self::NoRemainingReferences),
            "symbol_removed" => Some(Self::SymbolRemoved),
            "no_remaining_dependencies" => Some(Self::NoRemainingDependencies),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoRemainingCallers => "no_remaining_callers",
            Self::NoOutgoingCalls => "no_outgoing_calls",
            Self::NoRemainingReferences => "no_remaining_references",
            Self::SymbolRemoved => "symbol_removed",
            Self::NoRemainingDependencies => "no_remaining_dependencies",
        }
    }
}

pub fn supported_claim_kinds_csv() -> String {
    ClaimKind::ALL
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join("|")
}
