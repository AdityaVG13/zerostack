//! Truth classes for every graph fact (RACC-R Batch 3 truth discipline).

/// Epistemic status of a structural claim. Strict world fibers may be exact or
/// sound overapproximation; underapproximation must never be labeled complete.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum TruthClass {
    CompilerExact,
    LspExactScoped,
    SyntaxDerived,
    SoundOverapproximation,
    RuntimeObserved,
    Historical,
    Heuristic,
    /// First-class unknown -- never silently coerced to absence or certainty.
    Unknown,
}

impl TruthClass {
    /// Whether this class may authorize a strict exact claim.
    #[must_use]
    pub const fn is_exact(self) -> bool {
        matches!(self, Self::CompilerExact | Self::LspExactScoped)
    }

    /// Whether this class is admissible in a strict sound world fiber.
    #[must_use]
    pub const fn admissible_in_strict_fiber(self) -> bool {
        matches!(
            self,
            Self::CompilerExact
                | Self::LspExactScoped
                | Self::SyntaxDerived
                | Self::SoundOverapproximation
        )
    }

    /// Heuristic / empirical classes never upgrade to exact by proximity alone.
    #[must_use]
    pub const fn may_upgrade_to_exact(self) -> bool {
        false
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CompilerExact => "compiler_exact",
            Self::LspExactScoped => "lsp_exact_scoped",
            Self::SyntaxDerived => "syntax_derived",
            Self::SoundOverapproximation => "sound_overapproximation",
            Self::RuntimeObserved => "runtime_observed",
            Self::Historical => "historical",
            Self::Heuristic => "heuristic",
            Self::Unknown => "unknown",
        }
    }
}
