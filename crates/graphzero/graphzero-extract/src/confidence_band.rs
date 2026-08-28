//! Confidence band constants (DEC-001, ADR-004).
//!
//! Contains=1.0, rust-analyzer exact calls=0.99, typed inferred calls outrank local calls, imports=0.8, implements-local=0.85, default=0.7.

/// File-to-symbol containment is certain: the definition is in the blob.
pub const CONTAINS: f64 = 1.0;

/// Exact rust-analyzer call resolution: symbol target resolved by Rust types.
pub const RUST_ANALYZER_EXACT_CALL: f64 = 0.99;

/// Type-qualified rust-analyzer call resolution: target resolved through path/type context.
pub const RUST_ANALYZER_TYPE_QUALIFIED_CALL: f64 = 0.97;

/// Trait-dispatch rust-analyzer call resolution: target resolved through trait dispatch.
pub const RUST_ANALYZER_TRAIT_DISPATCH_CALL: f64 = 0.95;

/// Inferred rust-analyzer call resolution: target resolved, but adapter reports lower certainty.
pub const RUST_ANALYZER_INFERRED_CALL: f64 = 0.91;

/// Exact tsserver call/import resolution: symbol target resolved by TypeScript types.
pub const TSSERVER_EXACT: f64 = 0.98;

/// Interface-dispatch tsserver resolution.
pub const TSSERVER_INTERFACE: f64 = 0.94;

/// Re-export/import-chain tsserver resolution.
pub const TSSERVER_REEXPORT: f64 = 0.92;

/// Inferred tsserver resolution with lower certainty.
pub const TSSERVER_INFERRED: f64 = 0.90;

/// Intra-blob call resolution: name-match in same blob, below typed inferred calls.
pub const LOCAL_CALL: f64 = 0.89;

/// Import statement: syntactic but the target is not verified.
pub const IMPORTS: f64 = 0.8;

/// Local impl block: trait and implementor are both in the blob.
pub const IMPLEMENTS_LOCAL: f64 = 0.85;

// Compile-time ordering invariants (DEC-001): typed inferred calls outrank local calls.
const _: () = assert!(RUST_ANALYZER_INFERRED_CALL > LOCAL_CALL);
const _: () = assert!(TSSERVER_INFERRED > LOCAL_CALL);

/// Ambiguous or default confidence for any edge not in the band.
pub const DEFAULT: f64 = 0.7;
