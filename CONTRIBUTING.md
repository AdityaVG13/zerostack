# Contributing to ZeroStack

Thank you for contributing. ZeroStack is one product in one Cargo workspace.
ZeroKernel is the model-facing host. Files, structure, and tokens are domain
libraries under `crates/fszero`, `crates/graphzero`, and `crates/tokenzero`.
Changes here affect the shared contract those domains depend on.

Build and verify steps live in [`docs/build.md`](docs/build.md).

## Repository topology

This repository is a single monorepo. The hub lives under `crates/zerostack`
and the domain libraries live under `crates/fszero`, `crates/graphzero`, and
`crates/tokenzero`. Integration tests live under `tests/fszero`,
`tests/graphzero`, and `tests/tokenzero` with shared helpers in
`tests/support`. Published docs are capability-first under `docs/files`,
`docs/structure`, and `docs/tokens`.

```text
ZeroStack/
├── crates/{zerostack,fszero,graphzero,tokenzero}/
├── contracts/
├── tests/{fszero,graphzero,tokenzero,support}/
├── docs/{files,structure,tokens,racc}/
├── packaging/
├── bindings/node/
├── demo/
├── fuzz/
└── xtask/
```

Build from the repository root. Use the narrowest package and explicit target for
tests; full-workspace test runs are not the project gate. No additional
repositories are required.

## Ground rules

1. **Domains never import one another.** ZeroStack owns shared contracts
   (`crates/zerostack/zero-abi`), storage, and the kernel that composes them.
   Domain logic belongs in its domain crate tree.
2. **One canonical execution surface.** Model-facing capability enters through
   ZeroKernel direct methods only. No second catalog, no compatibility shims.
3. **Tests live in `tests/fszero`, `tests/graphzero`, `tests/tokenzero`, and
   `tests/support`.** Add a focused regression when a changed observable
   contract lacks coverage. Always name the package and exact `--lib`, `--bin`,
   or `--test` target from the repository root.
4. **Unsafe code is forbidden** at crate level wherever practical; exceptions
   require a documented invariant.
5. **No casual stdout** in library code. Structured errors and ledgers only.

## Commit style

Conventional Commits, imperative mood, subject under 72 characters:

```text
fix(kernel): return typed NotFound for remove of missing paths
feat(abi): add TokenEngine::certify for response-boundary proofs
```

## Contract changes

Contracts live in `contracts/` as a flat machine-readable surface. See
`contracts/README.md` for the inventory. The shared conformance suite lives in
`crates/zerostack/zerostack-conformance` and proves every domain against the
same contract. Anything touching `contracts/` or `crates/zerostack/zero-abi`
changes the surface consumed by all crates. Coordinate the change in this
repository and update `contracts/` plus the conformance crate together. See
`crates/zerostack/zerostack-conformance/CONTRACT.md` for invariants.

## Reporting issues

Open a GitHub issue with: expected behavior, actual behavior, minimal repro,
and platform. Security-sensitive findings follow [SECURITY.md](SECURITY.md).
