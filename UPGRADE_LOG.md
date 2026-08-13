# Dependency Upgrade Log

**Date:** 2026-08-13
**Mission:** Discovery census only -- no version bumps, no `cargo update`, no install, no commit
**Projects:** ZeroStack (hub) + FSZero + GraphZero + TokenZero
**Language:** Rust workspaces (edition 2024, toolchain `nightly-2026-05-31` in all four). Python/Node manifests exist but carry almost no third-party runtime deps.

---

## Summary

| Metric | Count |
|--------|-------|
| **Updated this pass** | 0 (discovery only) |
| **Skipped / PRESERVE** | git pins, path deps, nightly toolchain, empty Python/npm product manifests |
| **Failed (rolled back)** | 0 |
| **Direct third-party crates surveyed** | 62 unique names across four repos |
| **Candidates for later passes** | see per-repo tables and [Handoff](#handoff-for-pass-2) |

**Method**
- Manifest inventory via `find` (CodeMode `zs` failed: `invalid_frame` / missing field `kind`).
- Declared versions parsed from every in-scope `Cargo.toml`.
- Resolved versions read from each `Cargo.lock` (no `cargo` invocation).
- Latest stable from crates.io `GET /api/v1/crates/{name}` → `crate.max_stable_version` (User-Agent `ZeroStack-library-updater-census/1.0`).
- npm `hyperframes` latest queried for the non-product video helper only.

**Explicitly not run:** workspace `cargo`, `cargo outdated`, `cargo update`, tests, audit, version edits.

---

## Manifest inventory

No `go.mod`, `Gemfile`, or `requirements*.txt` in product trees.

| Repo | Manifests (in scope) |
|------|----------------------|
| **ZeroStack** | `Cargo.toml` + `Cargo.lock` (workspace); `conformance/Cargo.toml` + `Cargo.lock` (excluded workspace); 16 member `Cargo.toml` under `crates/` + `tests/`; `bindings/node/package.json` (no npm deps); `rust-toolchain.toml` |
| **FSZero** | `Cargo.toml` + `Cargo.lock`; 8 crate + `tests/` members; `xtask/Cargo.toml` + empty `xtask/Cargo.lock`; `rust-toolchain.toml` |
| **GraphZero** | `Cargo.toml` + `Cargo.lock`; 15 crate + `tests/` members; `fuzz/Cargo.toml` + `Cargo.lock`; `crates/graphzero-scip/tools_gen/Cargo.toml` + `Cargo.lock`; `pyproject.toml` + `uv.lock` (zero third-party Python deps); `rust-toolchain.toml` |
| **TokenZero** | `Cargo.toml` + `Cargo.lock`; 11 crate + `tests/` members; `fuzz/Cargo.toml` + `Cargo.lock`; `package/npm/package.json` (no npm deps); `rust-toolchain.toml` |

**Out of product scope (logged, not upgrade targets):**
- `ZeroStack/archive/**` (pruned Cargo/npm)
- `ZeroStack/videos/tokenzero-v120/package.json` -- `npx hyperframes@0.7.36` (latest npm **0.7.107**)
- `GraphZero/benchmarks/foreign_corpora/fixtures/{rust-mini,ts-mini}` -- empty fixtures
- audit / Pareto / beads dumps

---

## ZeroStack

**Workspace:** resolver 2; members `zero-abi`, `zero-cert`, `zero-codemode`, `zero-gate`, `zero-gauge`, `zero-ledger`, `zero-mcp`, `zero-process`, `zero-ref`, `zero-store`, `zerostack-machine-permit`, `zsx`, `zsx-core`, `zsx-node`, `tests`, `tests/zero-testkit`. Excludes `conformance`.
**Package:** edition 2024, `rust-version = "1.85"`.
**Workspace.dependencies:** `serde = 1.0.229`, `serde_json = 1.0.151`, `sha2 = 0.11`.

### Current vs latest (direct third-party)

| Crate | Declared | Locked (main) | Locked (conformance) | Latest stable | Status |
|-------|----------|---------------|----------------------|---------------|--------|
| serde | 1 / 1.0.229 | 1.0.229 | 1.0.229 | 1.0.229 | current |
| serde_json | 1 / 1.0.151 | 1.0.151 | 1.0.151 | 1.0.151 | current |
| sha2 | 0.11 | 0.11.0 (+ transitive 0.10.9) | 0.11.0 | 0.11.0 | current (direct) |
| anyhow | 1 | 1.0.104 | 1.0.103 | 1.0.104 | conformance lock stale |
| chrono | 0.4 | 0.4.45 | -- | 0.4.45 | current |
| clap | 4 | 4.6.6 | 4.6.1 | 4.6.6 | conformance lock stale |
| jsonschema | 0.26 | 0.26.2 | 0.26.2 | **0.49.9** | candidate (0.x major) |
| libc | 0.2 | 0.2.189 | 0.2.186 | 0.2.189 | conformance lock stale |
| napi | 3.12.0 | 3.12.1 | -- | 3.12.1 | pin 1 patch behind (lock already latest) |
| napi-build | 2.4.0 | 2.4.1 | -- | 2.4.1 | pin 1 patch behind (lock already latest) |
| napi-derive | 3.6.2 | 3.6.3 | -- | 3.6.3 | pin 1 patch behind (lock already latest) |
| proptest | 1 | 1.11.0 | -- | 1.11.0 | current |
| regex | 1 | 1.13.1 | 1.12.4 | 1.13.1 | conformance lock stale |
| tempfile | 3 | 3.27.0 | -- | 3.27.0 | current |
| thiserror | 2 | 2.0.20 | -- | 2.0.20 | current |
| tree-sitter | 0.26.12 | 0.26.12 | -- | 0.26.12 | current |
| tree-sitter-javascript | 0.25.0 | 0.25.0 | -- | 0.25.0 | current |
| windows-sys | 0.61 | 0.61.2 (+ transitive 0.60.2) | 0.61.2 | 0.61.2 | current (direct) |
| allocation-counter | 0.8 (dev) | 0.8.1 | -- | 0.8.1 | current |
| criterion | 0.5 (dev) | 0.5.1 | -- | **0.8.2** | candidate (0.x major) |
| stats_alloc | 0.1 | 0.1.10 | -- | 0.1.10 | current |

`bindings/node/package.json` (`@zerostack/zsx-native` 0.1.0): no npm dependencies.

### PRESERVE

| Item | Why |
|------|-----|
| Path deps among `zero-*`, `zsx*`, `zero-testkit` | workspace members |
| `zsx-core` path deps on `../../../FSZero`, `../../../GraphZero/crates/graphzero-store`, `../../../TokenZero/crates/tokenzero-*` | sibling-engine composition; not crates.io |
| `zsx-core` path `../../../GraphZero/crates/graphzero-query` | **path missing on disk** (no `graphzero-query` crate; GraphZero has `graphzero-engine`). Do not "upgrade"; resolve rename/path in a later pass |
| `fastmcp-rust` git `https://github.com/Dicklesworthstone/fastmcp_rust` rev `1c66097b15dea9550e833ee9e433beeb66eefece` | git pin; nightly-related history in GraphZero toolchain comments |
| `rust-toolchain.toml` `nightly-2026-05-31` | nightly channel; PRESERVE |
| `[patch."https://github.com/AdityaVG13/zerostack"]` | local hub patch for sibling git pins |

### Candidates for later passes

1. **jsonschema** `0.26.2` → `0.49.9` (`conformance/`, `tests/`) -- large 0.x jump; research first.
2. **criterion** `0.5.1` → `0.8.2` (dev; `zero-cert`, `zero-gate`, `zero-ref`, `zero-ledger`) -- GraphZero already on 0.8.
3. **napi / napi-derive / napi-build** declared pins → already-locked latest (`3.12.0`→`3.12.1`, `3.6.2`→`3.6.3`, `2.4.0`→`2.4.1`).
4. **conformance `Cargo.lock` refresh** within existing ranges: anyhow `1.0.103`→`1.0.104`, clap `4.6.1`→`4.6.6`, libc `0.2.186`→`0.2.189`, regex `1.12.4`→`1.13.1`.

---

## FSZero

**Workspace:** resolver 3; members `fszero-core`, `fszero-store`, `fszero-engine`, `fs-zero`, `fszero`, `fszero-codemode`, `fszero-mcp`, `fszero-test-support`, `tests`.
**Package:** version 0.1.0, edition 2024.
**Workspace.dependencies:** hub git pins + local path crates only (no third-party workspace pins).

### Current vs latest (direct third-party)

| Crate | Declared | Locked | Latest stable | Status |
|-------|----------|--------|---------------|--------|
| fsqlite | 0.1.19 | 0.1.19 | **0.3.0** | candidate (0.x major) |
| fsqlite-core | 0.1.19 | 0.1.19 | **0.3.0** | candidate (0.x major) |
| frankensearch | 0.3.2 | 0.3.2 | 0.3.2 | current |
| ignore | 0.4 | 0.4.30 | 0.4.33 | lock refresh |
| memchr | 2 | 2.8.2 | 2.8.3 | lock refresh |
| notify | 8.2.0 | 8.2.0 | 8.2.0 | current |
| proptest | 1 | 1.11.0 | 1.11.0 | current |
| rayon | 1.10 | 1.12.0 | 1.12.0 | current in lock; declared floor stale |
| regex | 1 | 1.12.4 | 1.13.1 | lock refresh |
| rusqlite | 0.40 | 0.40.1 | 0.40.2 | lock refresh |
| serde | 1 | 1.0.229 | 1.0.229 | current |
| serde_json | 1 / 1.0.150 | 1.0.151 | 1.0.151 | current in lock |
| sha2 | 0.11.0 | 0.11.0 (+ transitive 0.10.9) | 0.11.0 | current (direct) |
| tempfile | 3.27.0 | 3.27.0 | 3.27.0 | current |
| unicode-normalization | 0.1 | 0.1.25 | 0.1.25 | current |
| xattr | 1 | 1.6.1 | 1.6.1 | current |

`xtask/` is an empty standalone workspace (`zerostack-xtask`) with no dependencies.

### PRESERVE

| Item | Why |
|------|-----|
| Hub crates `zero-abi`, `zero-ref`, `zero-cert`, `zero-store`, `zero-codemode`, `zero-mcp`, `zero-testkit` | git `https://github.com/AdityaVG13/zerostack` rev **`bd721f7fc4866b24dec0c552da3d96bd8d816fbc`** (AGENTS.md: pin hub by pushed `origin/main`) |
| `ast-sgrep-lang`, `ast-sgrep-core` | git `https://github.com/AdityaVG13/ast-sgrep.git` rev `beb70be6e536abc27ca3f6626b95f25356795f1a` |
| Path members `fs-zero`, `fszero-*` | workspace |
| `nightly-2026-05-31` | nightly pin |

### Candidates for later passes

1. **fsqlite + fsqlite-core** `0.1.19` → `0.3.0` -- high risk (store/pager; repo already tracks fsqlite page-leak work). Research before any bump.
2. **Lock refresh (ranges already allow):** ignore `0.4.30`→`0.4.33`, memchr `2.8.2`→`2.8.3`, regex `1.12.4`→`1.13.1`, rusqlite `0.40.1`→`0.40.2`. Also transitive-stale in this lock: anyhow `1.0.102`, libc `0.2.186`.
3. Optional: raise declared `rayon = "1.10"` to `1.12` to match lock/latest (no functional change if lock stays).

---

## GraphZero

**Workspace:** resolver 3; members `graphzero-types`, `graphzero-core`, `graphzero-store`, `graphzero-engine`, `graphzero`, `graphzero-codemode`, `graphzero-mcp-compat`, `graphzero-test-support`, `graphzero-coverage`, `graphzero-extract`, `graphzero-pack`, `graphzero-reserve`, `graphzero-scip`, `graphzero-semantic`, `graphzero-why`, `tests`.
**Package:** version 0.1.0, edition 2024, license MIT OR Apache-2.0.

**Python:** `pyproject.toml` `graphzero-python-tools` 0.0.0, `requires-python = ">=3.13,<3.14"`, `dependencies = []`. `.python-version` = `3.13`. `uv.lock` locks only the virtual project -- **already correct, nothing to upgrade**.

### Current vs latest (direct third-party)

| Crate | Declared | Locked (main) | Latest stable | Status |
|-------|----------|---------------|---------------|--------|
| ah-ah-ah | 0.1 | 0.1.0 | 0.1.0 | current |
| anyhow | 1 | 1.0.102 | 1.0.104 | lock refresh |
| bytemuck | 1.25 | 1.25.0 | 1.25.2 | lock refresh |
| clap | 4 | 4.6.1 | 4.6.6 | lock refresh |
| crc32fast | 1 | 1.5.0 | 1.5.0 | current |
| criterion | 0.8 | 0.8.2 | 0.8.2 | current |
| crossbeam-queue | 0.3 | 0.3.12 | 0.3.13 | lock refresh |
| ed25519-dalek | 2 | 2.2.0 | **3.0.0** | candidate (major) |
| git2 | 0.21 | 0.21.0 | 0.21.0 | current |
| hdrhistogram | 7 | 7.6.0 | 7.6.0 | current |
| hex | 0.4 | 0.4.3 | 0.4.3 | current |
| libc | 0.2 | 0.2.186 | 0.2.189 | lock refresh |
| libfuzzer-sys | 0.4 (fuzz) | 0.4.13 | 0.4.13 | current |
| memmap2 | >=0.9.11 | 0.9.11 | 0.9.11 | current (at floor) |
| notify | 8.2.0 | 8.2.0 | 8.2.0 | current |
| parking_lot | 0.12 | 0.12.5 | 0.12.5 | current |
| proptest | 1 | 1.11.0 | 1.11.0 | current |
| protobuf | 3.7 | 3.7.2 | 3.7.2 | current |
| rayon | 1.12 | 1.12.0 | 1.12.0 | current |
| scip | 0.8.1 | 0.8.1 | **0.9.0** | candidate (0.x) |
| serde | 1 | 1.0.229 | 1.0.229 | current |
| serde_json | 1 | 1.0.151 | 1.0.151 | current |
| serial_test | 4 | 4.0.1 | 4.0.1 | current |
| sha2 | **0.10** (workspace) | 0.10.9 (+ 0.11.0 elsewhere) | **0.11.0** | candidate (minor line; align with hub/FS/Token) |
| tempfile | 3.27 | 3.27.0 | 3.27.0 | current |
| tiktoken-rs | 0.12 | 0.12.0 | 0.12.0 | current |
| tracing | 0.1 | 0.1.44 | 0.1.44 | current |
| tree-sitter | 0.26.12 | 0.26.12 | 0.26.12 | current |
| tree-sitter-python | 0.25 | 0.25.0 | 0.25.0 | current |
| tree-sitter-rust | 0.24 | 0.24.2 | 0.24.2 | current |
| tree-sitter-typescript | 0.23 | 0.23.2 | 0.23.2 | current |
| zstd | 0.13 | 0.13.3 | 0.13.3 | current |

Fuzz lock is newer than main on several crates (anyhow 1.0.104, libc 0.2.189, memchr 2.8.3). `tools_gen` lock is older (serde 1.0.228, serde_json 1.0.150, scip 0.8.1).

### PRESERVE

| Item | Why |
|------|-----|
| Hub `zero-abi`, `zero-mcp`, `zero-gauge`, `zero-ref`, `zero-store`, `zero-testkit` | git zerostack rev **`bd721f7fc4866b24dec0c552da3d96bd8d816fbc`** |
| `zero-process` | same git rev (`crates/graphzero-types`) |
| Path members `graphzero-*` | workspace |
| `nightly-2026-05-31` | nightly pin (comment: retained after fastmcp `try_trait_v2` history) |
| `pyproject.toml` / `uv.lock` | no third-party Python packages by design |

### Candidates for later passes

1. **ed25519-dalek** `2.2.0` → `3.0.0` (`graphzero-pack`) -- major; signing API research required.
2. **scip** `0.8.1` → `0.9.0` (`graphzero-scip` + `tools_gen`) -- 0.x; protobuf/schema risk.
3. **sha2** workspace `0.10` → `0.11.0` -- align with ZeroStack/FSZero/TokenZero (already 0.11).
4. **Main lock refresh:** anyhow `1.0.102`→`1.0.104`, clap `4.6.1`→`4.6.6`, bytemuck `1.25.0`→`1.25.2`, crossbeam-queue `0.3.12`→`0.3.13`, libc `0.2.186`→`0.2.189`, plus transitive regex `1.12.4`→`1.13.1`.
5. Optional: refresh `tools_gen/Cargo.lock` (serde/serde_json/scip).

---

## TokenZero

**Workspace:** resolver 3; members `tokenzero-core`, `tokenzero-recovery`, `tokenzero-runtime`, `tokenzero-engine`, `tokenzero-filters`, `tokenzero-codemode`, `tokenzero-mcp-compat`, `tokenzero-test-support`, `tokenzero`, `tokenzero-install`, `tokenzero-pulse`, `tests`. Excludes `fuzz`.
**Package:** version **1.4.0**, edition 2024, `rust-version = "1.98"`.
**Style:** workspace.dependencies are exact pins (best comparison surface of the four repos).

`package/npm/package.json` (`@tokenzero/cli` 1.4.0): no npm dependencies -- already correct.

### Current vs latest (direct third-party)

| Crate | Declared | Locked (main) | Latest stable | Status |
|-------|----------|---------------|---------------|--------|
| anyhow | 1.0.104 | 1.0.104 | 1.0.104 | current |
| assert_cmd | 2.2.2 | 2.2.2 | 2.2.2 | current |
| clap | 4.6.6 | 4.6.6 | 4.6.6 | current |
| flate2 | 1.1.9 | 1.1.9 | 1.1.9 | current |
| fs4 | 1.1.0 | 1.1.0 | 1.1.0 | current |
| globset | 0.4.20 | 0.4.20 | 0.4.20 | current |
| memchr | 2.8.3 | 2.8.3 | 2.8.3 | current |
| predicates | 3.1.4 | 3.1.4 | 3.1.4 | current |
| proptest | 1.11.0 | 1.11.0 | 1.11.0 | current |
| regex | 1.13.1 | 1.13.1 | 1.13.1 | current |
| rusqlite | 0.40.1 | 0.40.1 | **0.40.2** | candidate (patch pin) |
| rustix | 1.1.4 | 1.1.4 | 1.1.4 | current |
| serde | 1.0.229 | 1.0.229 | 1.0.229 | current |
| serde_json | 1.0.150 | 1.0.151 | 1.0.151 | lock already latest; workspace pin 1 patch behind |
| sha2 | 0.11.0 | 0.11.0 (+ transitive 0.10.9) | 0.11.0 | current (direct) |
| similar | 3.1.2 | 3.1.2 | 3.1.2 | current |
| tempfile | 3.27.0 | 3.27.0 | 3.27.0 | current |
| thiserror | 2.0.19 | 2.0.19 | **2.0.20** | candidate (patch pin) |
| toml | 1.1.4 | 1.1.4+spec-1.1.0 | 1.1.4+spec-1.1.0 | current |
| wait-timeout | 0.2.1 | 0.2.1 | 0.2.1 | current |
| ah-ah-ah | 0.1 (dev) | 0.1.0 | 0.1.0 | current |
| arbitrary | 1 (fuzz) | 1.4.2 | 1.4.2 | current |
| criterion | 0.5 (dev) | 0.5.1 | **0.8.2** | candidate (0.x major) |
| getrandom | 0.2 (`tokenzero-recovery`) | 0.2.17 (+ transitive 0.3.4, 0.4.2) | **0.4.3** | candidate (major); lock not even on latest 0.4.3 |
| libfuzzer-sys | 0.4 (fuzz) | 0.4.13 | 0.4.13 | current |
| tiktoken-rs | 0.12 (dev) | 0.12.0 | 0.12.0 | current |

### PRESERVE

| Item | Why |
|------|-----|
| Hub `zero-abi`, `zero-gauge`, `zero-ledger`, `zero-ref`, `zero-process`, `zero-store`, `zero-testkit`, `zero-mcp` | git zerostack rev **`bd721f7fc4866b24dec0c552da3d96bd8d816fbc`** |
| Path members `tokenzero-*` | workspace |
| `nightly-2026-05-31` | nightly pin |
| npm `@tokenzero/cli` | loader/shim only; no registry deps |

### Candidates for later passes

1. **thiserror** workspace `2.0.19` → `2.0.20`.
2. **rusqlite** workspace `0.40.1` → `0.40.2`.
3. **serde_json** workspace pin `1.0.150` → `1.0.151` (lock already there).
4. **criterion** `0.5.1` → `0.8.2` (dev; `tokenzero-core`, `tokenzero-recovery`) -- align with GraphZero.
5. **getrandom** `0.2` → `0.4.3` -- major API; research. Direct pin is 0.2; 0.3/0.4 already appear transitively.

---

## Cross-repo notes

| Topic | Finding |
|-------|---------|
| Hub git pin | FSZero, GraphZero, TokenZero all pin zerostack **`bd721f7fc4866b24dec0c552da3d96bd8d816fbc`**. Not a crates.io upgrade; refresh only when operator advances the hub pin. |
| Toolchain | All four: `nightly-2026-05-31` + rustfmt/clippy. PRESERVE. |
| sha2 split | GraphZero workspace still `0.10`; hub/FS/Token declare `0.11`. All four locks contain both `0.10.9` (transitive) and `0.11.0`. |
| criterion split | GraphZero on `0.8.2`; ZeroStack + TokenZero still declare `0.5`. |
| clap / anyhow / libc | ZeroStack + TokenZero main locks are current; FSZero + GraphZero + ZeroStack conformance locks lag compatible patches. |
| `zsx-core` → `graphzero-query` | Path does not exist. Composition feature `graphzero` is stale vs `graphzero-engine`. |
| Transitive `getrandom` | 0.2 + 0.3 + 0.4 coexist in several locks. Only TokenZero declares it directly (0.2). |
| Transitive `windows-sys` | Multiple majors in locks (0.52/0.59/0.60/0.61). Direct pin is only ZeroStack `0.61`. Do not force-unify transitives in pass 2. |

---

## Successfully Updated

None this pass (discovery only).

---

## Failed Updates (Rolled Back)

None.

---

## Requires Attention (do not auto-bump)

| Package | Current | Latest | Why stop |
|---------|---------|--------|----------|
| jsonschema | 0.26.2 | 0.49.9 | Many 0.x releases; schema API likely churn |
| fsqlite / fsqlite-core | 0.1.19 | 0.3.0 | Store/pager; FSZero durability surface |
| ed25519-dalek | 2.2.0 | 3.0.0 | Major signing crate |
| scip | 0.8.1 | 0.9.0 | SCIP protobuf / index format |
| getrandom | 0.2.17 (direct) | 0.4.3 | Major; TokenZero recovery |
| criterion | 0.5.1 | 0.8.2 | Bench harness; GraphZero already migrated |
| sha2 (GraphZero) | 0.10.9 | 0.11.0 | Hash crate line change; digest compatibility |
| graphzero-query path | missing | n/a | Broken sibling path in `zsx-core` |

---

## Security Notes

No `cargo audit` / `pip-audit` / `npm audit` this pass (discovery only; no full-workspace cargo).

---

## Post-Upgrade Checklist

- [ ] All tests passing -- **not run** (out of scope)
- [ ] No deprecation warnings -- **not run**
- [ ] Manual smoke test -- **not run**
- [ ] Documentation updated -- this census log only
- [ ] Changes committed -- **not committed** (operator instruction)

---

## Commands Used

```bash
# Manifest discovery (zs CodeMode failed: invalid_frame)
find <repo> -name 'Cargo.toml' -o -name 'Cargo.lock' -o -name 'pyproject.toml' \
  -o -name 'package.json' -o -name 'uv.lock' ...

# Parse / join (local /tmp scripts; no cargo)
python3 /tmp/dep_census.py
python3 /tmp/dep_summarize.py
python3 /tmp/dep_lock_extract.py
python3 /tmp/dep_cratesio.py          # crates.io max_stable_version
python3 /tmp/dep_compare.py

# npm (video helper only)
curl -sS https://registry.npmjs.org/hyperframes/latest
```

---

## Notes

- Pass 2 should take **one crate at a time**, research-software on every C-class bump, and RCH targeted tests only (`CARGO_TARGET_DIR=/tmp/rch_target_<repo>`). Never full-workspace cargo on this Mac.
- Prefer lock refresh (`cargo update -p <crate>`) when the declared range already admits latest, before editing manifests.
- Do not advance hub git rev, fastmcp git rev, ast-sgrep git rev, or the nightly toolchain in a library-updater pass unless the operator asks.
- Engine AGENTS.md files forbid unsolicited docs in FSZero/GraphZero/TokenZero; this log lives only in ZeroStack as requested.

---

## Handoff for pass 2

Recommended order (smallest / safest first):

### Lock-only (A) -- no Cargo.toml edit if ranges allow

| Repo | name | current (lock) | latest |
|------|------|----------------|--------|
| ZeroStack/conformance | anyhow | 1.0.103 | 1.0.104 |
| ZeroStack/conformance | clap | 4.6.1 | 4.6.6 |
| ZeroStack/conformance | libc | 0.2.186 | 0.2.189 |
| ZeroStack/conformance | regex | 1.12.4 | 1.13.1 |
| FSZero | ignore | 0.4.30 | 0.4.33 |
| FSZero | memchr | 2.8.2 | 2.8.3 |
| FSZero | regex | 1.12.4 | 1.13.1 |
| FSZero | rusqlite | 0.40.1 | 0.40.2 |
| GraphZero | anyhow | 1.0.102 | 1.0.104 |
| GraphZero | clap | 4.6.1 | 4.6.6 |
| GraphZero | bytemuck | 1.25.0 | 1.25.2 |
| GraphZero | crossbeam-queue | 0.3.12 | 0.3.13 |
| GraphZero | libc | 0.2.186 | 0.2.189 |

### Compatible pin bumps (B)

| Repo | name | current (declared) | latest |
|------|------|--------------------|--------|
| ZeroStack | napi | 3.12.0 | 3.12.1 |
| ZeroStack | napi-derive | 3.6.2 | 3.6.3 |
| ZeroStack | napi-build | 2.4.0 | 2.4.1 |
| TokenZero | thiserror | 2.0.19 | 2.0.20 |
| TokenZero | rusqlite | 0.40.1 | 0.40.2 |
| TokenZero | serde_json | 1.0.150 | 1.0.151 |

### Research-required (C)

| Repo | name | current | latest |
|------|------|---------|--------|
| ZeroStack | jsonschema | 0.26.2 | 0.49.9 |
| ZeroStack + TokenZero | criterion | 0.5.1 | 0.8.2 |
| FSZero | fsqlite | 0.1.19 | 0.3.0 |
| FSZero | fsqlite-core | 0.1.19 | 0.3.0 |
| GraphZero | ed25519-dalek | 2.2.0 | 3.0.0 |
| GraphZero | scip | 0.8.1 | 0.9.0 |
| GraphZero | sha2 | 0.10.9 | 0.11.0 |
| TokenZero | getrandom | 0.2.17 | 0.4.3 |

---

# Pass 2 -- 2026-08-13 -- lockfile-compatible in-semver only

**Mission:** Refresh Cargo.lock / declared patch pins that already allow latest stable. No majors, no 0.x breaking lines, no lockfile added to ZeroStack, no hub pin change, no commit.

## Summary

- **Updated:** 15 named crates (+ expected companion lock packages)
- **Skipped / PRESERVE:** majors, 0.x line jumps, hub git `bd721f7`, path deps, ZeroStack `conformance/Cargo.lock`, ZeroStack workspace `Cargo.lock` (gitignored)
- **Failed:** 0
- **Needs attention:** FSZero `ignore` 0.4.33 only resolved after `regex-automata` 0.4.18 (see below)

## Updates

### ZeroStack -- declared pin bumps only (`crates/zsx-node/Cargo.toml`)

No lockfile written or staged (`**/*.lock` gitignored). Census already had these versions in the ignored lock.

| Crate | From | To | Breaking | Tests |
|-------|------|----|----------|-------|
| napi | 3.12.0 | 3.12.1 | None (fix: stop unloading addons with live native code; WASI registration randomness). Source: https://napi.rs/changelog/napi | check skipped -- lock already on 3.12.1 |
| napi-derive | 3.6.2 | 3.6.3 | None (local package bump of napi-derive-backend) | skipped |
| napi-build | 2.4.0 | 2.4.1 | None (same addon-unload fix as napi 3.12.1) | skipped |

### FSZero -- `cargo update -p` within existing ranges

One crate at a time. `CARGO_TARGET_DIR=/tmp/rch_target_fszero`.

| Crate | Lock from | Lock to | Notes |
|-------|-----------|---------|-------|
| ignore | 0.4.30 | 0.4.33 | First `-p ignore` stopped at 0.4.32 (`available: 0.4.33`). 0.4.33 requires `regex-automata ^0.4.18`. After `-p regex` landed 0.4.18, second `-p ignore` reached 0.4.33. |
| memchr | 2.8.2 | 2.8.3 | patch |
| regex | 1.12.4 | 1.13.1 | also `regex-automata` 0.4.14 → 0.4.18 |
| rusqlite | 0.40.1 | 0.40.2 | also `libsqlite3-sys` 0.38.1 → 0.38.2. Release: MSRV lowered to 1.88.0 only. |

**Check:** `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo check -p fszero-store` -- **PASS** (exit 0). Two pre-existing unused-fn warnings in `recovery/pack.rs`. `fsqlite`/`fsqlite-core` stayed **0.1.19**.

### GraphZero -- `cargo update -p` within ranges

One crate at a time. `CARGO_TARGET_DIR=/tmp/rch_target_graphzero`.

| Crate | Lock from | Lock to | Companion |
|-------|-----------|---------|-----------|
| anyhow | 1.0.102 | 1.0.104 | -- |
| clap | 4.6.1 | 4.6.6 | clap_builder 4.6.0→4.6.6, clap_derive 4.6.1→4.6.4 |
| bytemuck | 1.25.0 | 1.25.2 | -- |
| crossbeam-queue | 0.3.12 | 0.3.13 | -- |
| libc | 0.2.186 | 0.2.189 | -- |

**Not bumped (mission):** ed25519-dalek 2.2.0, scip 0.8.1, sha2 0.10.9 + 0.11.0. Check skipped -- patch/lock-only, no native sys crate.

### TokenZero -- declared pins + `cargo update -p`

`Cargo.toml` workspace pins then lock refresh. `CARGO_TARGET_DIR=/tmp/rch_target_tokenzero`.

| Crate | Declared from → to | Lock from → to | Breaking |
|-------|--------------------|----------------|----------|
| thiserror | 2.0.19 → 2.0.20 | 2.0.19 → 2.0.20 (+ thiserror-impl) | None. Clippy `redundant_field_names` suppression in generated code ([#454](https://github.com/dtolnay/thiserror/pull/454)). |
| rusqlite | 0.40.1 → 0.40.2 | 0.40.1 → 0.40.2 (+ libsqlite3-sys 0.38.1→0.38.2) | None. 0.40.2 only lowers MSRV to 1.88.0. |
| serde_json | 1.0.150 → 1.0.151 | already 1.0.151 | Additive only: `RawValue::from_string_unchecked` ([#1331](https://github.com/serde-rs/json/pull/1331)). `-p serde_json` locked 0 packages. |

`cargo update -p thiserror` also rewrote rusqlite in the same lock pass because the workspace pin was already 0.40.2. Subsequent `-p rusqlite` was a no-op.

**Check:** `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo check -p tokenzero-pulse` -- **PASS** (exit 0). criterion stayed **0.5.1**. getrandom stayed **0.2.17** (plus existing transitive 0.3.4 / 0.4.2).

## Failed Updates

None.

## Needs Attention

- FSZero `ignore` 0.4.33 is gated on `regex-automata >= 0.4.18`. Updating ignore before regex left the lock at 0.4.32. Order matters.
- ZeroStack `conformance/Cargo.lock` still stale (anyhow 1.0.103, clap 4.6.1, libc 0.2.186, regex 1.12.4). Explicitly skipped this pass.
- FSZero lock still has transitive anyhow 1.0.102 and libc 0.2.186 (not in this pass's crate list).
- GraphZero lock still has regex 1.12.4 (not in this pass's crate list).
- `zsx-core` path to missing `graphzero-query` is unchanged (census item; not an in-semver bump). First accidental `cargo update` from ZeroStack CWD failed on that path -- expected.

## PRESERVE (verified unchanged)

- Hub git pin `bd721f7fc4866b24dec0c552da3d96bd8d816fbc` in FSZero / GraphZero / TokenZero manifests and locks.
- Path deps; nightly-2026-05-31; no `cas.rs` edits.
- GraphZero `scripts/perf/` remains untracked rival/unrelated; not touched.

## Security Notes

No `cargo audit` this pass (in-semver lock/pin only; no full-workspace cargo).

## Commands Used

```bash
# ZeroStack
# edited crates/zsx-node/Cargo.toml only

# FSZero
cd /Users/aditya/AI/FSZero
env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo update -p ignore
env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo update -p memchr
env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo update -p regex
env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo update -p rusqlite
env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo update -p ignore   # 0.4.32 -> 0.4.33
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo check -p fszero-store

# GraphZero
cd /Users/aditya/AI/GraphZero
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p anyhow
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p clap
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p bytemuck
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p crossbeam-queue
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p libc

# TokenZero
# edited workspace pins in Cargo.toml
cd /Users/aditya/AI/TokenZero
env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo update -p thiserror
env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo update -p rusqlite
env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo update -p serde_json
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo check -p tokenzero-pulse
```

## Post-Upgrade Checklist

- [x] Targeted `cargo check` for rusqlite consumers (fszero-store, tokenzero-pulse)
- [ ] Full test suite -- not run (out of scope; no full-workspace cargo)
- [x] No lockfile added to ZeroStack
- [x] Hub pin preserved
- [ ] Changes committed -- **not committed** (orchestrator commits)

## Handoff for pass 3

Remaining C-class / skipped:

| Repo | name | current | latest | Why still waiting |
|------|------|---------|--------|-------------------|
| ZeroStack | jsonschema | 0.26.2 | 0.49.9 | 0.x major |
| ZeroStack + TokenZero | criterion | 0.5.1 | 0.8.2 | 0.x major |
| FSZero | fsqlite / fsqlite-core | 0.1.19 | 0.3.0 | 0.x major |
| GraphZero | ed25519-dalek | 2.2.0 | 3.0.0 | major |
| GraphZero | scip | 0.8.1 | 0.9.0 | 0.x |
| GraphZero | sha2 | 0.10.9 (ws) | 0.11.0 | line change |
| TokenZero | getrandom | 0.2.17 | 0.4.3 | major |
| ZeroStack/conformance | anyhow/clap/libc/regex | lock stale | latest | skipped this pass |
| FSZero | anyhow 1.0.102, libc 0.2.186 | lock stale | 1.0.104 / 0.2.189 | not in pass-2 list |
| GraphZero | regex | 1.12.4 | 1.13.1 | not in pass-2 list |

---

# Pass 3 -- 2026-08-13 -- workspace.dependencies serde / serde_json / sha2 (+ cousins)

**Mission:** Bump declared `[workspace.dependencies]` (or workspace-root `[dependencies]`) pins that are behind latest **stable** on the **same major** (or same `0.x` minor line). Target family: `serde`, `serde_json`, `sha2`, and cousins (`serde_derive`/`serde_core` via serde, `toml`, `hex`, `digest`). No majors. No `0.x` line jumps. `sha2 0.10 → 0.11` deferred to pass 5/6.

## Summary

- **Updated:** 0
- **Already at latest (left alone):** 10 declared workspace pins + GraphZero caret-`1` serde family
- **Skipped / PRESERVE:** GraphZero `sha2 = "0.10"` (line change), hub git pins, path deps, crate-level (non-workspace) pins
- **Failed:** 0
- **Checks:** none -- no bump that could break compile

**Method:** crates.io `GET /api/v1/crates/{name}` → `crate.max_stable_version` (User-Agent `ZeroStack-library-updater-pass3/1.0`). Compared to each repo's `[workspace.dependencies]` only.

## crates.io max_stable (this pass)

| Crate | max_stable | newest | Created (max_stable) |
|-------|------------|--------|----------------------|
| serde | **1.0.229** | 1.0.229 | 2026-07-18 |
| serde_json | **1.0.151** | 1.0.151 | 2026-07-20 |
| serde_derive / serde_core | 1.0.229 | 1.0.229 | (serde family; not independently pinned) |
| sha2 | **0.11.0** | 0.11.0 | 2026-03-25 |
| toml | **1.1.4+spec-1.1.0** | same | 2026-07-28 |
| hex | 0.4.3 | 0.4.3 | -- |
| digest | 0.11.3 | 0.11.3 | -- |

No newer stable than the pass-1 census for this family.

## Already at latest (not edited)

### ZeroStack `[workspace.dependencies]`

| Pin | Declared | Locked (local, gitignored) | crates.io | Action |
|-----|----------|----------------------------|-----------|--------|
| serde | 1.0.229 | 1.0.229 | 1.0.229 | leave |
| serde_json | 1.0.151 | 1.0.151 | 1.0.151 | leave |
| sha2 | 0.11 | 0.11.0 (+ transitive 0.10.9) | 0.11.0 | leave |

No other serde/sha2 cousins in this table.

### FSZero `[workspace.dependencies]`

No third-party crates. Table is hub git pins (`bd721f7…`) + path members only. Nothing in-family to bump.

Member crates declare their own `serde`/`serde_json`/`sha2` (not workspace pins). Out of this pass's exclusive scope. Note only:

| Location | Declared | Latest same-line | Note |
|----------|----------|------------------|------|
| `crates/fszero/Cargo.toml` | serde_json **1.0.150** | 1.0.151 | crate-level pin, 1 patch behind; **not** `[workspace.dependencies]` |
| `fszero-core` / `fszero-store` / `fs-zero` / `fszero-engine` | serde `1`, serde_json `1`, sha2 `0.11.0` | already latest | crate-level ranges |

### GraphZero `[workspace.dependencies]`

| Pin | Declared | Locked | crates.io | Action |
|-----|----------|--------|-----------|--------|
| serde | `1` (features derive) | 1.0.229 | 1.0.229 | leave -- caret already admits latest |
| serde_json | `1` | 1.0.151 | 1.0.151 | leave |
| sha2 | **0.10** | 0.10.9 (+ transitive 0.11.0) | 0.11.0 | **PRESERVE** -- 0.10→0.11 is pass 5/6 |
| tempfile | 3.27 | 3.27.0 | 3.27.0 | leave (not in-family; already current) |

`hex = "0.4"` lives in `crates/graphzero-pack/Cargo.toml` (crate-level). Lock already 0.4.3 == latest. Not a workspace pin.

Nested `crates/graphzero-scip/tools_gen/Cargo.lock` still has serde 1.0.228 / serde_json 1.0.150. tools_gen is its own tiny workspace (`serde_json = "1"`). Lock refresh is not a workspace-pin edit; left for a later lock pass.

### TokenZero `[workspace.dependencies]`

| Pin | Declared | Locked | crates.io | Action |
|-----|----------|--------|-----------|--------|
| serde | 1.0.229 | 1.0.229 | 1.0.229 | leave |
| serde_json | 1.0.151 | 1.0.151 | 1.0.151 | leave (already raised in pass 2) |
| sha2 | 0.11.0 | 0.11.0 (+ transitive 0.10.9) | 0.11.0 | leave |
| toml | 1.1.4 | 1.1.4+spec-1.1.0 | 1.1.4+spec-1.1.0 | leave (serde cousin) |

Spot-check of other TokenZero exact workspace pins (not this family's job, but none were stale vs crates.io tonight): anyhow 1.0.104, clap 4.6.6, thiserror 2.0.20, rusqlite 0.40.2, regex 1.13.1, memchr 2.8.3, tempfile 3.27.0.

## Updates

None. No `Cargo.toml` edit. No `cargo update`. No `cargo check`.

## Failed Updates

None.

## Needs Attention / out of exclusive scope

- GraphZero workspace `sha2 = "0.10"` still the only in-family declared pin not on latest stable. Intentional deferral (digest 0.10 vs 0.11).
- FSZero `crates/fszero/Cargo.toml` `serde_json = "1.0.150"` is the only **declared** serde-family pin still one patch behind. Not a workspace pin; did not edit.
- GraphZero `tools_gen` lock still on serde 1.0.228 / serde_json 1.0.150.
- Transitive `sha2 0.10.9` remains in all four main locks next to direct `0.11.0` (ZeroStack / FSZero / TokenZero) or next to workspace `0.10` (GraphZero). Do not force-unify transitives.

## PRESERVE (verified unchanged)

- Hub git pin `bd721f7fc4866b24dec0c552da3d96bd8d816fbc`
- Path deps; nightly-2026-05-31
- ZeroStack `Cargo.lock` still gitignored -- not added
- GraphZero `scripts/perf/` untracked; not touched
- No commit, no push, no `git add .`

## Security Notes

No `cargo audit` (no version change).

## Commands Used

```bash
# crates.io max_stable_version for serde / serde_json / sha2 / toml / cousins
python3  # urllib GET https://crates.io/api/v1/crates/{name}

# lock extract (read-only)
# ZeroStack/FSZero/GraphZero/TokenZero Cargo.lock + extras
```

## Post-Upgrade Checklist

- [x] crates.io researched before any edit
- [x] No major / 0.x line jump
- [x] GraphZero sha2 left on 0.10
- [x] No lockfile added to ZeroStack
- [x] Hub pin preserved
- [ ] Changes committed -- **not committed** (orchestrator commits; this pass has no Cargo.toml delta)

## Handoff for pass 4

Remaining C-class / skipped (unchanged from pass 2, plus the FSZero crate-level serde_json note):

| Repo | name | current | latest | Why still waiting |
|------|------|---------|--------|-------------------|
| ZeroStack | jsonschema | 0.26.2 | 0.49.9 | 0.x major |
| ZeroStack + TokenZero | criterion | 0.5.1 | 0.8.2 | 0.x major |
| FSZero | fsqlite / fsqlite-core | 0.1.19 | 0.3.0 | 0.x major |
| FSZero | fszero crate serde_json | 1.0.150 | 1.0.151 | crate-level, not workspace |
| GraphZero | ed25519-dalek | 2.2.0 | 3.0.0 | major |
| GraphZero | scip | 0.8.1 | 0.9.0 | 0.x |
| GraphZero | sha2 | 0.10 (ws) / 0.10.9 lock | 0.11.0 | line change -- pass 5/6 |
| GraphZero | tools_gen lock serde/json | 1.0.228 / 1.0.150 | 1.0.229 / 1.0.151 | nested lock, not ws pin |
| TokenZero | getrandom | 0.2.17 | 0.4.3 | major |
| ZeroStack/conformance | anyhow/clap/libc/regex | lock stale | latest | skipped |
| FSZero lock | anyhow 1.0.102, libc 0.2.186 | lock stale | 1.0.104 / 0.2.189 | not this family |
| GraphZero lock | regex | 1.12.4 | 1.13.1 | not this family |

---

# Pass 4 -- 2026-08-13 -- crate-local direct deps (same major, not workspace-inherited)

**Mission:** Bump versions written in **member** `Cargo.toml` files (not `[workspace.dependencies]`) that are behind latest **stable** on the **same major**. Also refresh GraphZero `tools_gen` nested lock serde / serde_json. No majors. No 0.x line jumps. No ZeroStack `Cargo.lock`. No commit.

## Summary

- **Updated:** 3 named crates (`serde_json`, `rayon`, `serde` family in nested lock)
- **Already at latest (left alone):** 15+ crate-local pins (see list)
- **Skipped / PRESERVE:** jsonschema 0.26, criterion 0.5, fsqlite 0.1.19, ed25519-dalek 2, scip 0.8.1, getrandom 0.2, GraphZero workspace sha2 0.10
- **Failed:** 0
- **Checks:** serde_json + rayon -- skipped (lock already on target; compile cannot break). `gen-fixture` -- rch preflight failed 3x (see Issues)

**Method:** crates.io `GET /api/v1/crates/{name}` → `crate.max_stable_version` (User-Agent `ZeroStack-library-updater-pass4/1.0`). One crate at a time.

## crates.io max_stable (this pass)

| Crate | max_stable | Notes |
|-------|------------|-------|
| serde | **1.0.229** | same as pass 3 |
| serde_json | **1.0.151** | same as pass 3 |
| rayon | **1.12.0** | 1.11+ deprecates `iter::repeatn` (old name kept) |
| napi / napi-derive / napi-build | 3.12.1 / 3.6.3 / 2.4.1 | already current |
| tree-sitter | 0.26.12 | already current |
| notify | 8.2.0 | 9.0.0-rc.4 is pre-release -- stay 8.2.0 |
| frankensearch / tempfile / sha2 | 0.3.2 / 3.27.0 / 0.11.0 | already current |
| jsonschema | 0.49.9 | SKIP -- 0.x break |
| criterion | 0.8.2 | SKIP -- 0.x break |
| fsqlite | 0.3.0 | SKIP -- 0.x break |
| ed25519-dalek | 3.0.0 | SKIP -- major |
| scip | 0.9.0 | SKIP -- 0.x |
| getrandom | 0.4.3 | SKIP -- major |

## Research

### serde_json: 1.0.150 → 1.0.151
- **Breaking:** None
- **Change:** additive `RawValue::from_string_unchecked` ([#1331](https://github.com/serde-rs/json/pull/1331)). Release: https://github.com/serde-rs/json/releases/tag/v1.0.151 (2026-07-20)
- **FSZero lock:** already 1.0.151 (pass 2/3). Manifest pin only.

### serde: 1.0.228 → 1.0.229 (tools_gen lock only)
- **Breaking:** None for 1.0 consumers
- **Change:** serde_derive updates to syn 3 (https://github.com/serde-rs/serde/releases/tag/v1.0.229, 2026-07-18). Companions: serde_core / serde_derive 1.0.229; lock now has syn 2.0.118 **and** syn 3.0.3
- **tools_gen `Cargo.toml`:** still `serde_json = "1"` (range already admits latest)

### rayon: 1.10 → 1.12 (declared floor)
- **Breaking:** None. 1.11 renamed `iter::repeatn` → `iter::repeat_n` (old name deprecated, still present). FSZero uses `std::iter::repeat_n`, not rayon `repeatn`.
- **FSZero lock:** already 1.12.0. Floor raise only; no lock rewrite.

## Updates

### FSZero -- crate-local pins

| Crate | File | From | To | Lock | Tests |
|-------|------|------|----|------|-------|
| serde_json | `crates/fszero/Cargo.toml` | 1.0.150 | 1.0.151 | already 1.0.151 | check skipped -- lock unchanged |
| rayon | `crates/fs-zero/Cargo.toml`, `crates/fszero-engine/Cargo.toml` | 1.10 | 1.12 | already 1.12.0 | check skipped -- lock unchanged |

`fsqlite` / `fsqlite-core` stayed **0.1.19**.

### GraphZero -- `crates/graphzero-scip/tools_gen` nested lock

One crate at a time. `CARGO_TARGET_DIR=/tmp/rch_target_graphzero`. Did **not** touch untracked `scripts/perf/`.

| Crate | Lock from | Lock to | Companion |
|-------|-----------|---------|-----------|
| serde_json | 1.0.150 | 1.0.151 | -- |
| serde | 1.0.228 | 1.0.229 | serde_core 1.0.228→1.0.229, serde_derive 1.0.228→1.0.229, **adds syn 3.0.3** (syn 2.0.118 remains) |

**Check:** `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo check -p gen-fixture` (cwd `tools_gen`) -- **not verified**. Three attempts: workers failed preflight. `rch diagnose`/`admit` said offload-eligible; verbose log showed project identity canonicalized to `/Users/aditya/AI` instead of GraphZero. Local fallback disabled. Did not invent green.

`scip` stayed **0.8.1**. `ed25519-dalek` stayed **2**. Workspace `sha2` stayed **0.10**.

### ZeroStack -- crate-local

No same-major bump. napi/tree-sitter already latest. jsonschema/criterion skipped.

### TokenZero -- crate-local

No same-major bump. `getrandom` 0.2 and `criterion` 0.5 skipped. Other crate-local third-party (`ah-ah-ah` 0.1, `tiktoken-rs` 0.12, fuzz `arbitrary`/`libfuzzer-sys`) already current.

## Already correct (crate-local, same major, left alone)

Named pins / ranges already on latest stable (or caret already admits latest; lock current):

1. **napi** 3.12.1 (`zsx-node`)
2. **napi-derive** 3.6.3 (`zsx-node`)
3. **napi-build** 2.4.1 (`zsx-node`)
4. **tree-sitter** 0.26.12 (`zero-codemode`, `graphzero-extract`)
5. **tree-sitter-javascript** 0.25.0 (`zero-codemode`)
6. **frankensearch** 0.3.2 (`fs-zero`, `fszero-engine`)
7. **notify** 8.2.0 (`fs-zero`, `graphzero-store`)
8. **tempfile** 3.27.0 / `3` (`fs-zero`, `fszero-engine`, `fszero-store`, tests, ZeroStack members)
9. **thiserror** `2` (`zero-store`) -- lock 2.0.20
10. **clap** `4` (`graphzero`, ZeroStack tests/conformance) -- lock 4.6.6
11. **libc** `0.2` (`zero-process`, `graphzero-store`, `graphzero-engine`) -- latest 0.2.189 (lock may still lag in some trees; range admits)
12. **windows-sys** `0.61` (`zero-process`, `zerostack-machine-permit`) -- latest 0.61.2
13. **proptest** `1` -- lock 1.11.0
14. **sha2** 0.11.0 (FSZero crate-local)
15. **hex** `0.4` (`graphzero-pack`) -- lock 0.4.3
16. **protobuf** `3.7` (`graphzero-scip`, `tools_gen`) -- lock 3.7.2
17. **tiktoken-rs** `0.12` (`tokenzero-core` dev)

## Failed Updates

None rolled back.

## Issues

- **rch + nested workspace:** `tools_gen` check could not run. `rch` resolved `canonical_root` to `/Users/aditya/AI` (parent of the four repos) when cwd was `GraphZero/crates/graphzero-scip/tools_gen`. Three `rch exec` attempts: `all workers failed preflight checks`; local fallback off. `papercuts` CLI/MCP not available in this harness -- logged here.
- GraphZero untracked `scripts/perf/` is rival/unrelated; not touched (one-writer).
- ZeroStack `conformance/Cargo.lock` still stale (anyhow/clap/libc/regex) -- lock-only, not crate-local pin; out of this pass.
- FSZero main lock still has transitive anyhow 1.0.102 / libc 0.2.186 -- not crate-local pins.
- GraphZero main lock still has regex 1.12.4 -- not crate-local pin.

## PRESERVE (verified unchanged)

- Hub git pin `bd721f7fc4866b24dec0c552da3d96bd8d816fbc`
- Path deps; nightly-2026-05-31
- ZeroStack `Cargo.lock` still gitignored -- **not added**
- GraphZero `scripts/perf/` untracked; not touched
- No commit, no push, no `git add .`

## Security Notes

No `cargo audit` (targeted pin/lock only; no full-workspace cargo).

## Commands Used

```bash
# crates.io max_stable for crate-local names
python3  # urllib GET https://crates.io/api/v1/crates/{name}

# FSZero -- Cargo.toml only (lock already current)
# edited crates/fszero/Cargo.toml          serde_json 1.0.150 -> 1.0.151
# edited crates/fs-zero/Cargo.toml         rayon 1.10 -> 1.12
# edited crates/fszero-engine/Cargo.toml   rayon 1.10 -> 1.12

# GraphZero tools_gen lock
cd /Users/aditya/AI/GraphZero/crates/graphzero-scip/tools_gen
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p serde_json
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p serde
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo check -p gen-fixture
# ^ preflight failed x3
```

## Post-Upgrade Checklist

- [x] crates.io researched before each bump
- [x] One crate at a time
- [x] No major / 0.x line jump
- [x] No lockfile added to ZeroStack
- [x] Hub pin preserved
- [ ] `gen-fixture` compile -- **not verified** (rch preflight)
- [ ] Changes committed -- **not committed** (operator instruction)

## Handoff for pass 5

Remaining C-class / lock-only leftovers:

| Repo | name | current | latest | Why still waiting |
|------|------|---------|--------|-------------------|
| ZeroStack | jsonschema | 0.26.2 | 0.49.9 | 0.x major |
| ZeroStack + TokenZero | criterion | 0.5.1 | 0.8.2 | 0.x major |
| FSZero | fsqlite / fsqlite-core | 0.1.19 | 0.3.0 | 0.x major |
| GraphZero | ed25519-dalek | 2.2.0 | 3.0.0 | major |
| GraphZero | scip | 0.8.1 | 0.9.0 | 0.x |
| GraphZero | sha2 | 0.10 (ws) / 0.10.9 lock | 0.11.0 | line change -- pass 5/6 |
| TokenZero | getrandom | 0.2.17 | 0.4.3 | major |
| ZeroStack/conformance | anyhow/clap/libc/regex | lock stale | latest | lock-only, not crate-local |
| FSZero lock | anyhow 1.0.102, libc 0.2.186 | lock stale | 1.0.104 / 0.2.189 | lock-only |
| GraphZero lock | regex | 1.12.4 | 1.13.1 | lock-only |

---

# Pass 5 -- 2026-08-13 -- shared pin alignment (same crate, same latest stable where majors already match)

**Mission:** Align crates that appear in more than one of the four repos with mismatched versions on a line they already share. Research first. `rch cargo check -p` one crate if compile could break. Never full-workspace cargo. ZeroStack no `Cargo.lock`. No commit.

## Summary

- **Updated:** 7 named crates (sha2 pin, criterion pin, leftover same-major locks)
- **Already aligned (3+ repos, left alone):** serde 1.0.229, serde_json 1.0.151, tempfile 3.27.0, proptest 1.11.0, notify 8.2.0, tree-sitter 0.26.12, rusqlite 0.40.2 (FS+TZ locks already current)
- **Skipped / PRESERVE:** fsqlite 0.1, jsonschema 0.26, ed25519-dalek 2, scip 0.8, getrandom 0.2 (not same-major); ZeroStack workspace/`conformance` locks (gitignored)
- **Failed:** 0 rollbacks
- **Needs attention:** `criterion::black_box` deprecated on 0.8; ZeroStack bench check blocked by existing `graphzero-query` path

**crates.io max_stable (this pass):** sha2 0.11.0, criterion 0.8.2, regex 1.13.1, anyhow 1.0.104, clap 4.6.6, libc 0.2.189 (1.0.0-alpha.4 is pre-release -- stay 0.2), rusqlite 0.40.2.

## Research

### sha2: GraphZero 0.10 → 0.11.0
- **Breaking (digest 0.11):** `generic-array` → `hybrid-array`; `core_api` → `block_api`; `CoreWrapper` / `VariableOutput` removed; `io::Write` moved to `digest_io`. MSRV 1.85. Source: https://github.com/RustCrypto/hashes/blob/master/sha2/CHANGELOG.md ; https://github.com/RustCrypto/traits/blob/master/digest/CHANGELOG.md
- **Call sites:** 27 GraphZero files, all `Sha256::new` / `update` / `finalize` / `digest` / `{:x}`. No `CoreWrapper`, no `generic-array`, no `io::Write`.
- **Migration size:** 1 manifest line + lock re-resolve. Not a design change. Under the 10-file edit threshold.
- **Unify-to-single-line:** `cargo update -p sha2@0.10.9 --precise 0.11.0` **refused** -- `ed25519-dalek 2.2.0` (and git `fastmcp-protocol`) still require `sha2 ^0.10`. Dual lock lines remain, matching hub/FS/Token.

### criterion: ZeroStack + TokenZero 0.5 → 0.8.2
- **0.6:** `html_reports` no longer default. **0.7:** `async_tokio` feature; plotting optional. **0.8:** `csv_output` no longer default; `SamplingMode::Auto`; `Throughput::BytesDecimal`; rustc-hash 2; ciborium 0.3; MSRV 1.88. Source: https://github.com/bheisler/criterion.rs/blob/master/CHANGELOG.md
- **ZS/TZ benches** use classic `Criterion` / `criterion_group!` / `criterion_main!` / `Throughput::Bytes` / `BenchmarkId` / `BatchSize` -- same surface GraphZero already runs on 0.8.
- **Files edited:** 6 `Cargo.toml` (4 ZS + 2 TZ). No bench source edits. Small.
- **Deprecation:** 0.8 deprecates `criterion::black_box` in favor of `std::hint::black_box`. GraphZero benches already migrated. TZ `tokenzero-core` benches: 17 warning sites. ZS benches still import `criterion::black_box`. >5 sites -- logged, not rewritten this pass.

### leftover same-major locks
- regex / anyhow / clap / libc / rusqlite already share major across repos. Stale locks only. rusqlite already 0.40.2 after pass 2.

## Updates

### GraphZero -- sha2 workspace pin + regex lock

| Item | From | To | Notes |
|------|------|----|-------|
| `Cargo.toml` `[workspace.dependencies] sha2` | 0.10 | 0.11 | members inherit |
| lock `sha2` (workspace crates) | 0.10.9 | 0.11.0 | re-resolved by `cargo check -p graphzero-types` |
| lock `sha2` (ed25519-dalek, fastmcp-protocol) | 0.10.9 | 0.10.9 | must stay |
| lock `regex` | 1.12.4 | 1.13.1 | also `regex-automata` 0.4.14 → 0.4.18 |

**Check:** `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo check -p graphzero-types` -- **PASS** (exit 0). Compiled `sha2 0.11.0` + `digest 0.11.3`.

### ZeroStack -- criterion pin only (no lock)

| File | From | To |
|------|------|----|
| `crates/zero-cert/Cargo.toml` | 0.5 | 0.8 |
| `crates/zero-gate/Cargo.toml` | 0.5 | 0.8 |
| `crates/zero-ledger/Cargo.toml` | 0.5 | 0.8 |
| `crates/zero-ref/Cargo.toml` | 0.5 | 0.8 |

**Check:** `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo check -p zero-cert --benches` -- **not verified**. Remote + local both fail on pre-existing `zsx-core` path `../../../GraphZero/crates/graphzero-query` (missing). Not introduced by this pass.

### TokenZero -- criterion pin + libc lock

| Item | From | To | Companion |
|------|------|----|-----------|
| `tokenzero-core` / `tokenzero-recovery` `criterion` | 0.5 | 0.8 | lock 0.5.1 → 0.8.2; criterion-plot 0.5.0 → 0.8.2; itertools 0.10.5 → 0.13.0; adds alloca 0.4.0, page_size 0.6.0; removes is-terminal 0.4.17 |
| lock `libc` | 0.2.186 | 0.2.189 | -- |

**Check:** `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo check -p tokenzero-core --benches` -- **PASS** (exit 0). 17 `criterion::black_box` deprecation warnings. No errors.

### FSZero -- leftover locks only

| Crate | Lock from | Lock to |
|-------|-----------|---------|
| anyhow | 1.0.102 | 1.0.104 |
| libc | 0.2.186 | 0.2.189 |

regex already 1.13.1, rusqlite already 0.40.2. Check skipped -- patch/lock-only, no native sys crate bump beyond libc patch.

## Already aligned (3+ repos, not edited this pass)

1. **serde** 1.0.229 -- all four
2. **serde_json** 1.0.151 -- all four (FS crate-local raised in pass 4)
3. **tempfile** 3.27.0 -- all four
4. **proptest** 1.11.0 -- all four
5. **notify** 8.2.0 -- FS + GZ
6. **tree-sitter** 0.26.12 -- ZS + GZ
7. **rusqlite** 0.40.2 -- FS + TZ locks already current
8. **sha2 0.11** -- now all four direct pins (this pass closed GZ)

## Failed Updates

None rolled back.

## Needs Attention

- **`criterion::black_box` deprecated in 0.8.** GraphZero already uses `std::hint::black_box`. TZ `hotpaths.rs` has 17 sites; ZS four bench files still import `criterion::black_box`. Mechanical, but >5 sites -- follow-up, not this pass.
- **ZeroStack bench compile** blocked by missing `graphzero-query` path in `zsx-core` (census item). Criterion 0.8 API compatibility evidenced by TokenZero `tokenzero-core --benches` PASS.
- **ZeroStack `conformance/Cargo.lock`** still stale (anyhow 1.0.103, clap 4.6.1, libc 0.2.186, regex 1.12.4) -- file is gitignored (`**/*.lock`). Not written.
- Transitive **sha2 0.10.9** remains next to 0.11.0 in GZ/FS/TZ (ed25519-dalek / fastmcp / other transitives). Do not force-unify.

## PRESERVE (verified unchanged)

- Hub git pin `bd721f7fc4866b24dec0c552da3d96bd8d816fbc`
- Path deps; nightly-2026-05-31
- ZeroStack `Cargo.lock` still gitignored -- **not added**
- GraphZero `scripts/perf/` untracked rival; not touched
- fsqlite 0.1.19, jsonschema 0.26, ed25519-dalek 2.2.0, scip 0.8.1, getrandom 0.2
- No commit, no push, no `git add .`

## Security Notes

No `cargo audit` (targeted pin/lock only; no full-workspace cargo).

## Commands Used

```bash
# crates.io max_stable
python3  # urllib GET https://crates.io/api/v1/crates/{name}

# GraphZero
# edited Cargo.toml  sha2 0.10 -> 0.11
cd /Users/aditya/AI/GraphZero
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p sha2
# ^ ambiguous (0.10.9 + 0.11.0 already in lock)
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p sha2@0.10.9 --precise 0.11.0
# ^ FAIL: ed25519-dalek 2.2.0 requires sha2 ^0.10
env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo update -p regex
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo check -p graphzero-types

# ZeroStack -- Cargo.toml only
# edited crates/zero-{cert,gate,ledger,ref}/Cargo.toml  criterion 0.5 -> 0.8
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo check -p zero-cert --benches
# ^ FAIL: missing graphzero-query (pre-existing)

# TokenZero
# edited crates/tokenzero-{core,recovery}/Cargo.toml  criterion 0.5 -> 0.8
cd /Users/aditya/AI/TokenZero
env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo update -p criterion
env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo update -p libc
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo check -p tokenzero-core --benches

# FSZero
cd /Users/aditya/AI/FSZero
env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo update -p anyhow
env CARGO_TARGET_DIR=/tmp/rch_target_fszero cargo update -p libc
```

## Post-Upgrade Checklist

- [x] Research before each bump
- [x] One crate at a time for lock updates
- [x] No fsqlite / jsonschema / ed25519-dalek / scip / getrandom majors
- [x] No lockfile added to ZeroStack
- [x] Hub pin preserved
- [x] `graphzero-types` check after sha2
- [x] `tokenzero-core --benches` check after criterion
- [ ] `zero-cert --benches` -- **blocked** (graphzero-query path)
- [ ] Changes committed -- **not committed** (operator instruction)

## Handoff for pass 6 (majors only)

| Repo | name | current | latest | Why still waiting |
|------|------|---------|--------|-------------------|
| ZeroStack | jsonschema | 0.26.2 | 0.49.9 | 0.x major |
| FSZero | fsqlite / fsqlite-core | 0.1.19 | 0.3.0 | 0.x major |
| GraphZero | ed25519-dalek | 2.2.0 | 3.0.0 | major (also pins transitive sha2 0.10) |
| GraphZero | scip | 0.8.1 | 0.9.0 | 0.x |
| TokenZero | getrandom | 0.2.17 | 0.4.3 | major |
| ZS + TZ benches | `criterion::black_box` | deprecated | `std::hint::black_box` | >5 sites; GraphZero already migrated |
