# Contributing to ZeroStack

Thank you for contributing. ZeroStack is the composition hub for three
independent engines (FSZero, GraphZero, TokenZero); changes here affect the
shared contract all four depend on.

## Repository topology

The engines are sibling checkouts, referenced by relative path:

```text
AI/            (or any parent directory)
├── ZeroStack/
├── FSZero/
├── GraphZero/
└── TokenZero/
```

`cargo build --workspace` requires all four siblings side by side.

## Ground rules

1. **Engines never import one another.** ZeroStack owns shared contracts
   (`zero-abi`), storage (`zero-store`), and the kernel that composes them.
   Domain logic belongs in its engine.
2. **One canonical execution surface.** Model-facing capability enters through
   `ZeroKernel` direct methods only. No second catalog, no compatibility shims.
3. **Tests live in `tests/unit/<crate>/`.** Every fix ships with a regression
   test that fails without it. Run targeted suites (`cargo test -p <crate>`);
   the full four-engine matrix needs all siblings present.
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

Anything touching `crates/zero-abi` changes the contract digest consumed by
all engines. Coordinate across repos in the same change window and update
`docs/zero-kernel.md` plus the conformance fixtures together.

## Reporting issues

Open a GitHub issue with: expected behavior, actual behavior, minimal repro,
and platform. Security-sensitive findings follow [SECURITY.md](SECURITY.md).
