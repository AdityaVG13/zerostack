use graphzero_store::ContentHash;

/// Provenance for tier-B edges (FR-004).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TierBSource {
    Scip,
    Lsp,
}

impl TierBSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TierBSource::Scip => "scip",
            TierBSource::Lsp => "lsp",
        }
    }
}

/// Evidence that justified the tier-B confidence label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TierBResolution {
    /// The SCIP symbol resolved to a SymbolInformation entry in this document.
    SymbolWitness,
    /// SCIP supplied an occurrence, but its symbol had no resolution witness.
    UnresolvedOccurrence,
}

impl TierBResolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SymbolWitness => "symbol_witness",
            Self::UnresolvedOccurrence => "unresolved_occurrence",
        }
    }

    /// Stable SCIP label encoding in the shared `EdgeRecord.confidence` byte.
    ///
    /// The shared store schema stays authoritative: SCIP does not add a second
    /// edge format. Exact values let SCIP consumers recover the named label.
    pub const fn persisted_confidence(self) -> u8 {
        match self {
            Self::SymbolWitness => u8::MAX,
            Self::UnresolvedOccurrence => 191,
        }
    }

    pub const fn from_persisted_confidence(confidence: u8) -> Option<Self> {
        match confidence {
            u8::MAX => Some(Self::SymbolWitness),
            191 => Some(Self::UnresolvedOccurrence),
            _ => None,
        }
    }
}

/// Normalized tier-B edge prior to store merge.
#[derive(Clone, Debug, PartialEq)]
pub struct TierBEdge {
    pub src: String,
    pub dst: String,
    pub kind: u8,
    pub confidence: f64,
    pub resolution: TierBResolution,
    pub source: TierBSource,
    pub blob: ContentHash,
    pub start: u32,
    pub end: u32,
}

/// Decode summary for golden fixtures (FR-001).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScipDecoded {
    pub symbol_count: usize,
    pub relationship_count: usize,
    pub document_count: usize,
}
