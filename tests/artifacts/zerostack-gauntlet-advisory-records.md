# Gauntlet advisory records (drain 2026-08-13)

Closed as **recorded findings** (no surprise installs, no full cargo, no EXECUTE).
Product remediations stay on the dedicated bug beads.

| bead | disposition |
|---|---|
| p0-macos-soak-tools | heaptrack/strace yellow on macOS; no install |
| p0-missing-skills | 2 helper skills missing; advisory |
| p0-oracle-preflight-yellow | doctor yellow; no greenfield class branch |
| p0-release-perf-drift | profile exists; keep-gate recipe not applied |
| p0-toolchain-reds | criterion/show-asm/afl/insta missing; no surprise install |
| p1-daemon-sidecar-conflict | spec conflict recorded; AGENTS daemonless wins |
| p1-dual-cli-zs-zsx | two CLIs by design until zs leak bead |
| p1-dual-harness | tests/ vs conformance/ split recorded |
| p1-fu-missing-families | FeatureUniverse incomplete |
| p1-fuzz-archived | no live fuzz_targets |
| p1-matrix-stale-rows | matrix rows stale |
| p1-miri-toolchain | miri component row missing |
| p1-node-ffi-unsafe | zsx-node FFI mapping incomplete |
| p1-proposal-inventory | public-surface-inventory is proposal |
| p1-worker-vs-inprocess | dual runtime recorded |
| p2-cargo-target-dir-split | AGENTS vs ADR 0001; keep `/tmp/rch_target_zerostack` |
| p2-oracle-pins-pending | UNPINNED remainder |
| p2-racc-r-payloads-unhashed | zip payloads unhashed |
| p2-spec-tags-synthesized | SPEC-ZS synthesized |
| p6-fault-kind-matrix | crash-at ≠ FaultKind |
| p8-agents-mandate | AGENTS gitignored; mandate not committed |
| p8-cass-blocker | cass CLI missing |
| p8-excluded-predicate | excluded rows not 8-form |
| p9-score-family-category | family≠category adapter |
