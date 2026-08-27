//! GraphZero structural truth authority (RACC-R adoption).
//!
//! Owns source-anchored truth-classed claims, coverage/negative knowledge,
//! and certified incremental invalidation. Composition with other engines is
//! the ZeroStack hub's job -- this crate never imports peer engines.
#![forbid(unsafe_code)]

pub mod adapters;
pub mod atlas;
pub mod cognitive_work;
pub mod conflict;
pub mod conformance;
pub mod decision;
pub mod derivability;
pub mod dirty;
pub mod effect_map;
pub mod grades;
pub mod graph;
pub mod invalidation;
pub mod omission;
pub mod refinement;
pub mod truth;
pub mod world_fiber;

pub use adapters::{
    AdapterContractError, AdapterIngestReport, DomainAdapter, ReplayTraceAdapter, ingest_adapter,
    is_dynamic_truth_class,
};
pub use atlas::{AddressAtlas, AtlasError, LocusRank, SnapLevel, TaskFingerprint};
pub use cognitive_work::{
    CognitiveNodeClass, CompilerEdgeKind, CompilerImpact, CompilerSemanticEdge, EdgeDelta,
    InterruptProposal, MechanicalGraphVerdict, MechanicalRegionInput, ProofSupportHyperedge,
    SemanticExtractionReport, TemporalEdge, TypedObligation, TypedObligationKind,
    classify_mechanical_region, compiler_reverse_impact, fuse_edge_overlay,
};
pub use conflict::{ConflictHyperedge, ConflictHypergraph, ConflictKind};
pub use decision::{ClosureClass, DecisionClosure, DecisionEvidence, DecisionGap, EvidenceKind};
pub use derivability::{DerivabilityAnswer, DerivabilityPredicate, UnknownReason};
pub use dirty::{
    Bookmark, DirtyReport, JournalEvent, JournalEventKind, dirty_since, dirty_since_closures_only,
};
pub use effect_map::{
    ConsequenceClass, EffectConsequenceMap, ObligationKind, VerifierObligation,
    VerifierObligationMap,
};
pub use grades::{
    ClaimKind, GRADES_CROSS_REPO_SCHEMA, GradeConformanceFixture, GradeConformanceVector,
    GradeError, GradeEvidence, GradeLedger, GradeLedgerEntry, GradeName, GradeRevocation,
    GradeUpgradeRecord, HubGradeName, grade_from_hub, hub_equivalent,
};
pub use graph::{
    CoverageClass, EdgeId, GraphEdge, GraphError, GraphNode, NegativeKnowledgeCertificate, NodeId,
    ProjectGraph, Relation, SourceAnchor,
};
pub use invalidation::{
    ArtifactId, CutoffReport, DependencyClosureRecord, DependencyGraph, InfluenceClass,
    InvalidationCertificate, InvalidationError, RecomputeEngine, RecomputeResult,
};
pub use omission::{OmissionImpact, OmissionKind, RecoveryTrigger};
pub use refinement::{
    EdgeProvenance, FixedPointReport, ObservedInfluence, RefinementLoop, RefinementOutcome,
    RetainedCounterexample, RetainedCounterexampleStore,
};
pub use truth::TruthClass;
pub use world_fiber::{FiberClass, WorldAlternative, WorldFiber};
