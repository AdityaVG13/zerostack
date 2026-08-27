# zerostack-conformance contract

## Purpose

`zerostack-conformance` is the shared conformance suite that every ZeroStack engine must pass. It encodes the hub contract in executable checks against `zero-abi` traits. Each engine provides a thin runner that calls `run_all` with its concrete implementation. A failure means the engine violates the shared contract, not an implementation detail.

Live conformance lives only in this crate.

## Public API

* `file_engine` and `token_engine` modules expose engine-specific suites built against `zero-abi` traits.
* `ConformanceWorkspace` creates a hermetic temporary workspace with its store directory pre-created.
* `conformance_invocation(root, session)` builds a standard `EngineInvocation` with a generous budget and `NoopProbe`.
* `NoopProbe` implements `CancellationProbe` that never cancels.
* `SuiteResult` collects named check results. Every check that fails is recorded with its name.

Adding a new engine means writing one thin runner per trait it implements. Fixing a contract bug means one fix here that all engines inherit.

## Invariants

* Engines never import one another. All shared behavior is proven through `zero-abi` and this crate.
* No engine maintains a second copy of contract semantics. The contract is the checked-in JSON in `contracts/` and the traits in `zero-abi`.
* Tests are deterministic. No hidden global state. Workspaces are hermetic and isolated per run.
* Unsafe code is forbidden.

## Determinism

* Suite execution is deterministic for a given engine implementation, contract digest, and fixture input.
* Fixtures in `contracts/` (ZeroRef vectors, filesystem contract, operation ABI schemas, surface matrix, and digest pin) are inputs to determinism. Changing them requires updating the conformance expectations and the pinned digest.
* No time, randomness, or machine topology may affect pass or fail except where the contract explicitly defines a budget or wall-clock bound.

## Cancellation

* Conformance uses `NoopProbe` for the standard path. Real engine cancellation is proven through the `CancellationProbe` trait and the hub frame token. Cancellation before publication must not publish partial state.
* Suites that exercise cancellation must prove that a cancelled operation returns a retryable or typed cancellation outcome without leaking a partial snapshot as current.

## Errors

* Engine operations return typed, stable error classes defined by the filesystem contract and the GraphZero operation ABI. Branches should match on class, not on prose.
* Corrupt, missing, or stale artifacts return explicit error variants before a claim is published. An empty result is a successful result only when the request was valid and coverage was sufficient.
* Conformance failures report the exact check name through `SuiteResult`, not a bare boolean.

## Tests

* Run the narrowest target from the repository root. Example: `cargo test -p zerostack-conformance --lib` or an engine-specific runner.
* Use `ConformanceWorkspace` for hermetic filesystem state. Do not share temporary directories across tests.
* The suite depends only on `zero-abi` and `tempfile`. It does not depend on any engine crate.

## No-claim boundaries

* This suite does not claim security isolation against a hostile operating system account. It does not claim availability under resource exhaustion beyond the explicit budgets.
* Token savings, projection, or graph freshness claims are not proven by this crate alone. Those require engine-specific evidence and the contracts in `contracts/`.
* Passing conformance does not replace the engine-specific focused tests in `tests/fszero`, `tests/graphzero`, and `tests/tokenzero`.
