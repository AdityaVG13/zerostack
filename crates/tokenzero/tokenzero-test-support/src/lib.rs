//! TokenZero-specific tests plus the shared ZeroStack test contract.

pub mod certification;
pub mod conformal;
pub mod gauntlet;
pub mod invariant_catalog;
pub mod parity_taxonomy;
pub mod ratchet;
pub use certification::{
    CERTIFICATION_MAX_HIGH_SEVERITY_COUNTEREXAMPLES, CERTIFICATION_MIN_VERIFICATION_PCT,
    CERTIFICATION_REQUIRED_SUITE_PASS_RATE_PCT, CERTIFICATION_SCHEMA,
    CONFORMAL_LOWER_ONE_UNREACHABLE, CertificationAssessment, CertificationVerdict,
    assess_certification, catalog_verification_pct,
};
pub use conformal::{
    BetaParams, CategoryEvidence, CategoryScore, ConformalInterval, ConformalStatus,
    DEFAULT_CONFIDENCE, MIN_CALIBRATION_RESIDUALS, ParityScorecard, ReleaseBlock, ReleaseVerdict,
    SCORECARD_SCHEMA, UNIFORM_ALPHA_PRIOR, UNIFORM_BETA_PRIOR, apply_conformal_residuals,
    release_pass_on_point_estimate, residual_quantile, score_categories, score_passes_trials,
};
pub use gauntlet::{
    CanonicalizationRules, CrashBoundary, CrashWindowDriver, CrashWindowKind, EngineVersions,
    ExecutionEnvelope, FAILURE_BUNDLE_SCHEMA, FAILURE_FIRST_DIVERGENCE_JSONPTR,
    FORBIDDEN_MCP_ENGINE_IDENTITY, FORBIDDEN_MCP_REGISTRY_ENGINE, FailureBody, FailureBundle,
    FailureProvenance, FailureType, FirstDivergence, GauntletEngineIdentity, GauntletIdentityPair,
    GauntletOracle, SPEC_TAG_WIRES, SUBJECT_IDENTITY, ScenarioAgreement, SpecTagClass, SpecTagWire,
    assert_distinct, compare_bytes, fragment_reason_class_matches, is_forbidden_gauntlet_identity,
    scenario,
};
pub use invariant_catalog::{
    ArtifactRef, BaseGate, CATALOG_SCHEMA_VERSION, CatalogViolation, CloseDecision, ContractStatus,
    InvariantCatalog, InvariantId, ParityInvariant, ProofKind, ProofObligation, ProofStatus,
    VERIFICATION_CONTRACT_SCHEMA, close_decision, seal_satisfied_hashes, unique_invariant_ids,
};
pub use parity_taxonomy::{
    Feature, FeatureId, FeatureUniverse, LoaderError, ParityStatus, Stats, truncate_score,
};
pub use ratchet::{
    CATEGORY_QUARANTINE_THRESHOLD, RATCHET_STATE_SCHEMA, RatchetState, RatchetVerdict,
    RatchetWaiver, apply_ratchet, apply_ratchet_with_waiver,
};
