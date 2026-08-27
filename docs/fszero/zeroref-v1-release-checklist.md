# ZeroRef v1 release checklist (FSZero)

This checklist is the human-readable companion to `scripts/zeroref_conformance_gate.py`.
Every FSZero release that advertises ZeroRef v1 interoperability MUST satisfy
all items below.

## Evidence

- [ ] `docs/contracts/zeroref-conformance-evidence.json` exists and is valid JSON.
- [ ] Evidence schema is `zeroref-conformance-evidence/v1`.
- [ ] Evidence contains `descriptor_tests`, `docs_audit`, and `timestamp`.
- [ ] `matrix.status` is `green`.
- [ ] Matrix contains rows for macOS, Linux, and Windows.
- [ ] Matrix contains `wrong_store` row with `expect_fail: true`.
- [ ] `matrix.sibling_shas` pins the exact release binary path, SHA-256, and commit for FSZero, TokenZero, and GraphZero.

## Capability fixtures

- [ ] `docs/contracts/zeroref-capability-fixtures.json` exists and is valid JSON.
- [ ] Fixtures include `compatible_enabled`, `compatible_but_peer_disabled`, and `compatible_but_peer_misconfigured` peers.
- [ ] Each fixture carries `expect`, `error_class`, and `shared_interop` fields.

## Tests

- [ ] `cargo test --test zeroref_fixture -- --test-threads=1` passes.
- [ ] `cargo test --test cross_engine_claims -- --test-threads=1` passes.
- [ ] `cargo test --test zeroref_v1_contract -- --test-threads=1` passes.
- [ ] `python3 scripts/zeroref_conformance_gate.py` exits `0`.

## Binaries

- [ ] Release binary is built from committed HEAD (no uncommitted WIP).
- [ ] Binary path and SHA-256 are recorded in the evidence file.
- [ ] Build command used only `CARGO_BUILD_JOBS=2` and a single crate target.

## Doctor / report

- [ ] `fszero doctor` prints `root_mode`, `layout_version`, `store_health`, `migration_legacy`, and `peer_incompatibility`.
- [ ] Doctor emits no absolute private paths.
- [ ] `fszero doctor` exits `0` for a healthy store and `1` for a degraded store.

## Embeddable store handle

- [ ] `FsZeroStore` is exported from `fs_zero::core`.
- [ ] Embedded handle descriptor matches standalone CLI descriptor.
- [ ] Two handles with different roots share the same CAS but isolate non-shared roots.

## Final

- [ ] `cargo fmt` and `cargo clippy` are clean for touched crates.
- [ ] Commit is direct to `main` and pushed.
- [ ] Beads `fszero-c6q.7` and `fszero-c6q.8` are closed with evidence.
