//! Query paths: cold (spawn + mmap + lazy freshness check, FR-009) and warm
//! (snapshot held open in-process, FR-008). Readers never lock; they mmap
//! the published snapshot named by the manifest (FR-012).

mod budget;
mod capsule_json;
mod delta_codec;
mod engine;
pub(crate) mod file_target;
mod freshness;
mod legacy;
pub(crate) mod lexical;
mod locate;
mod name_bigram;
mod snap;
mod snap_edit;
mod snapshot;
mod spans;
mod types;

pub use budget::{
    append_accounting, compact_truncated_budgeted, persist_query_json, savings_tokens_after_expand,
    tokens_for_str, tokens_for_utf8,
};
pub use delta_codec::{
    decode_edge, decode_symbol, encode_edge, encode_edge_with_meta, encode_symbol,
};
pub use engine::QueryEngine;
pub use file_target::{
    FileTargetHit, TARGET_CONTEXT_LINES, TARGET_INLINE_TOP_HITS, file_target_for_evidence,
    render_target,
};
pub use lexical::{
    LEXICAL_SEMANTIC_MAGIC, LEXICAL_SEMANTIC_VERSION, LexicalDocSource, LexicalHit,
    LexicalIndexBuilder, LexicalSemanticIndex, graph_proximity_boost, lexical_semantic_file_name,
    tokenize_into,
};
pub use locate::{
    LocateCapsule, LocateHit, LocateIndex, LocateKind, canonical_ref_for_loc, locate, locate_shell,
    locate_shell_for_name, locate_shell_tokens, query_shell,
};
pub use name_bigram::{
    NAME_BIGRAM_MAGIC, NAME_BIGRAM_VERSION, NameBigramIndex, SEARCH_BIGRAM_ENV,
    name_bigram_file_name, search_bigram_enabled,
};
pub use snap::{
    clear_snap_session_cache, export_capsule, normalize_snap_query, probe_snap_route, snap,
};
pub use snap_edit::{EditAnchor, EditByteSpan, SnapEditIndex, SnapEditResult, snap_to_edit};

pub use freshness::{
    StalenessVerdict, blob_staleness_verdict, indexed_path_stale_vs_disk, path_record_for_rel,
};
pub use snapshot::{
    OpenPhaseTimings, Snapshot, open_phase_timing_enabled, take_open_phase_timings,
};
pub use spans::span_range;
pub use types::{
    BudgetLedger, Capsule, CapsuleDef, CapsuleEdge, CapsuleMatch, CoverageCertificate,
    DestinationRef, ExportArtifact, ExportFormat, FreshnessDiagnostics, PathRecord, PendingDef,
    PendingEdge, PendingFacts, QueryCapsule, RouteDiagnostics, SnapRoute,
};
