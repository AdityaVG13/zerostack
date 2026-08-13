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
