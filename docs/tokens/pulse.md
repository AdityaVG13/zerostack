# Pulse accounting ledger

Pulse is a ZeroStack hub component implemented by `crates/zerostack/zero-pulse`. TokenZero supplies token measurement and certification. Pulse records the resulting facts. It is not a TokenZero product or a model-facing tool surface.

## Ledger

The primary store is an append-only JSONL ledger under the ZeroStack workspace state root. Each event binds the session, operation class, measured byte and token counts, tokenizer identity, exact recovery refs when present, and accounting attribution.

Pulse may maintain a SQLite sidecar for bounded aggregate queries. The JSONL ledger remains authoritative. A missing or corrupt sidecar can be rebuilt from valid ledger records.

## Accounting rules

- Every count names its tokenizer or estimator.
- Certified counts and heuristic estimates remain distinct.
- Raw, visible, recovered, and charged counts retain separate fields.
- Missing measurements remain unknown. They never become zero.
- Savings require comparable numerator and denominator observations.
- Pulse records facts after the operation boundary. It does not change dispatch, ranking, admission, or filesystem behavior.

## Concurrency and durability

Writers use a bounded lock and append complete newline-delimited records. Lock contention returns a typed retryable error with the lock path. Import, export, and sidecar rebuild use verified inputs and atomic replacement. Malformed trailing JSONL data is reported rather than accepted silently.

## Privacy

Pulse stores accounting facts, not prompts or source content. It does not record raw command output, file contents, queries, credentials, network identity, or hidden tracking fields. Reference identifiers may be stored only when needed to connect accounting with an exact ZeroStack recovery object.

Pulse has no network exporter. Embedding applications may read bounded aggregate reports through typed Rust APIs. ZeroKernel does not expose Pulse as a seventh operation.

## Public boundary

Users interact with Pulse through ZeroStack responses and diagnostics. FSZero and GraphZero emit domain facts to the hub but do not import Pulse. TokenZero may certify counts, but it does not own the ledger lifecycle.
