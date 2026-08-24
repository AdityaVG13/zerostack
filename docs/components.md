# Components

## FSZero

[FSZero](https://github.com/AdityaVG13/FSZero) owns exact bytes and filesystem effects: reads, directories, snapshots, guarded edits, atomic multi-file application, receipts, and restoration. It does not own graph relationships or output projection.

## GraphZero

[GraphZero](https://github.com/AdityaVG13/GraphZero) owns structural evidence: syntax-aware search, symbols, definitions, references, callers, imports, impact, freshness, and coverage. It does not own source bytes or file mutation.

## TokenZero

[TokenZero](https://github.com/AdityaVG13/TokenZero) owns output economics: tokenizer identity, measurement, bounded projection, compression, recovery refs, exact expansion, and recovery-aware accounting. It does not own process creation or workspace effects.

## ZeroStack

ZeroStack owns composition. It binds the three engine contracts to one host lifecycle and one terminal response. Normal JavaScript connects operations; the model does not select an engine or transport.

The engines are the released products. ZeroStack remains source-only. Once coordinated releases begin, FSZero, GraphZero, and TokenZero will publish the same version to signal compatible contract parity.
