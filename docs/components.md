# Components

## FSZero

FSZero owns exact bytes and filesystem effects: reads, directories, snapshots,
guarded edits, atomic multi-file application, receipts, and restoration. It
does not own graph relationships or output projection. Source lives in
[crates/fszero](../crates/fszero) and is documented in
[docs/fszero/README.md](fszero/README.md).

## GraphZero

GraphZero owns structural evidence: syntax-aware search, symbols, definitions,
references, callers, imports, impact, freshness, and coverage. It does not own
source bytes or file mutation. Source lives in
[crates/graphzero](../crates/graphzero) and is documented in
[docs/graphzero/README.md](graphzero/README.md).

## TokenZero

TokenZero owns output economics: tokenizer identity, measurement, bounded
projection, compression, recovery refs, exact expansion, and recovery-aware
accounting. It does not own process creation or workspace effects. Source lives
in [crates/tokenzero](../crates/tokenzero) and is documented in
[docs/tokenzero/README.md](tokenzero/README.md).

## ZeroStack

ZeroStack owns composition. It binds the three engine contracts to one host
lifecycle and one terminal response. Normal JavaScript connects operations; the
model does not select an engine or transport. The hub lives in
[crates/zerostack](../crates/zerostack) and is documented in
[architecture.md](architecture.md). All crates build from this workspace with
`cargo build --workspace` from the repository root. No additional repositories
are required. The engines never import one another.
