# SPEC Tags Catalog

Auto-extracted at Phase 2 from sources in
`conformance/contracts/spec_version_contract.toml`.
Every Verifiable tag below MUST have a corresponding verifier function in
`crates/zerostack-harness/src/spec_oracle.rs` (or equivalent) by end of Phase 3.
This pass does **not** implement verifiers.

**Classification counts:** Verifiable=53, Charter-only=16, Ambiguous=4.
**Conflict status:** PASS -- see `ZeroStack__gauntlet_workspace/phase2_spec_conflict.md`.
**Verifier column:** `UNVERIFIED` until Phase 3 wires `spec_oracle.rs`.

Charter-only claims live in [`docs/CHARTER.md`](../CHARTER.md) and MUST NOT
receive `[SPEC-NNN]` tags. Ambiguous claims are listed in
`ZeroStack__gauntlet_workspace/phase2_unverifiable_assertions.md`.

| Tag | Statement | Source | Verifier | Classification |
|---|---|---|---|---|
| `[SPEC-COMP-001]` | Engines MUST NOT import each other. The hub composes them in one process. | `conformance/CONTRACT.md` §1 | `UNVERIFIED` | Verifiable |
| `[SPEC-COMP-002]` | FSZero file mutation is journaled; a later failed step MUST roll back earlier mutation in the same execute. | `conformance/CONTRACT.md` §1 | `UNVERIFIED` | Verifiable |
| `[SPEC-COMP-003]` | TokenZero mutation is `denied` -- no workspace file mutation. | `conformance/CONTRACT.md` §1 | `UNVERIFIED` | Verifiable |
| `[SPEC-COMP-004]` | GraphZero workspace file mutation MUST be rejected (`store_only`). | `conformance/CONTRACT.md` §1 | `UNVERIFIED` | Verifiable |
| `[SPEC-SURF-001]` | CodeMode and MCP are mutually exclusive catalogs. A process MUST serve one. | `conformance/CONTRACT.md` §2 | `UNVERIFIED` | Verifiable |
| `[SPEC-SURF-002]` | MCP tools are exactly `zero_execute` and `zero_wait`. | `conformance/CONTRACT.md` §2 | `UNVERIFIED` | Verifiable |
| `[SPEC-SURF-003]` | `zsx mcp` MUST be harness-owned stdio. It MUST die with the parent. It MUST NOT detach, wrap in Python, or register engine MCP servers. | `conformance/CONTRACT.md` §2 | `UNVERIFIED` | Verifiable |
| `[SPEC-SURF-004]` | `zero_wait` reports process identity and image freshness. It MUST NOT spawn a child. | `conformance/CONTRACT.md` §2 | `UNVERIFIED` | Verifiable |
| `[SPEC-SURF-005]` | Engine binaries MUST NOT be registered as MCP servers next to `zsx mcp`. | `conformance/CONTRACT.md` §2 | `UNVERIFIED` | Verifiable |
| `[SPEC-SURF-006]` | CodeMode entry is `zsx exec -C ROOT`. The plan calls `zero.fs.*`, `zero.graph.*`, `zero.token.*`. There is no MCP tool catalog on that process. | `conformance/CONTRACT.md` §2 | `UNVERIFIED` | Verifiable |
| `[SPEC-SURF-007]` | `zero_execute` takes a JavaScript plan and an optional `timeout_ms`. | `conformance/CONTRACT.md` §2 | `UNVERIFIED` | Verifiable |
| `[SPEC-RES-001]` | Every public `zero.*` call returns `zero-result/v1` with `ack` and `content` (inline value or typed ref). | `conformance/CONTRACT.md` §3 | `UNVERIFIED` | Verifiable |
| `[SPEC-RES-002]` | An oversize result MUST spill to a content-addressed ref. | `conformance/CONTRACT.md` §3 | `UNVERIFIED` | Verifiable |
| `[SPEC-RES-003]` | `savingsBytes` is not a token count. | `conformance/CONTRACT.md` §3 | `UNVERIFIED` | Verifiable |
| `[SPEC-REF-001]` | Consumers MUST preserve the ref scheme (`fz://`, `gz://`, `tz://`). | `conformance/CONTRACT.md` §4 | `UNVERIFIED` | Verifiable |
| `[SPEC-REF-002]` | A missing or stale ref MUST fail loudly. | `conformance/CONTRACT.md` §4 | `UNVERIFIED` | Verifiable |
| `[SPEC-REF-003]` | A blob ref is `{ns}://blob/<64 lowercase hex>` plus an optional `#Bstart-end` or `#Lstart-end` fragment. | `conformance/CONTRACT.md` §4 | `UNVERIFIED` | Verifiable |
| `[SPEC-HON-001]` | FSZero and GraphZero MUST NOT pretend to emit Exact `billed_tokens` / `raw_tokens` / `visible_tokens`. | `conformance/CONTRACT.md` §5 | `UNVERIFIED` | Verifiable |
| `[SPEC-HON-002]` | Spill receipts that cannot certify tokens MUST set `visibleTokenCount` to null and `visibleTokenCountStatus` to `requires_tokenzero_certification`. | `conformance/CONTRACT.md` §5 | `UNVERIFIED` | Verifiable |
| `[SPEC-HON-003]` | `recovery_tokens` is the cost of expanding a ref. It is not billed. | `conformance/CONTRACT.md` §5 | `UNVERIFIED` | Verifiable |
| `[SPEC-HON-004]` | Estimates MUST NOT be labeled Exact. | `conformance/CONTRACT.md` §5 | `UNVERIFIED` | Verifiable |
| `[SPEC-HON-005]` | A skipped measurement is not a pass. | `conformance/CONTRACT.md` §5 | `UNVERIFIED` | Verifiable |
| `[SPEC-HON-006]` | TokenZero MAY emit Exact `billed_tokens` / `raw_tokens` / `visible_tokens`. | `conformance/CONTRACT.md` §5 | `UNVERIFIED` | Verifiable |
| `[SPEC-SETL-001]` | If a call is already cancelled or timed out and a late Ok arrives, the reported kind MUST be `commit_race`, `retryable` false. The committed payload stays attached. | `conformance/CONTRACT.md` §6 | `UNVERIFIED` | Verifiable |
| `[SPEC-SETL-002]` | A late domain Err MUST stay that Err. | `conformance/CONTRACT.md` §6 | `UNVERIFIED` | Verifiable |
| `[SPEC-NEG-001]` | There is no in-repo conformance CLI. | `conformance/CONTRACT.md` §8 | `UNVERIFIED` | Verifiable |
| `[SPEC-NEG-002]` | There is no `{ns}_execute_code` catalog. Those were a previous surface. Do not resurrect them as synonyms for `zero_execute`. | `conformance/CONTRACT.md` §8 | `UNVERIFIED` | Verifiable |
| `[SPEC-NEG-003]` | Authority ledger entries start unproven. A hash is not a pass. | `conformance/CONTRACT.md` §7 | `UNVERIFIED` | Verifiable |
| `[SPEC-HUB-001]` | Daemonless: one session-owned sidecar, parent-death-bound -- never a machine-wide service. | `AGENTS.md` Law + composition header | `UNVERIFIED` | Verifiable |
| `[SPEC-HUB-002]` | Fail loud. No silent success, no heuristic labeled exact. | `AGENTS.md` Law item 7 | `UNVERIFIED` | Verifiable |
| `[SPEC-HUB-003]` | Process resource policy publishes idle/active RSS and CPU caps (`DEFAULT_IDLE_TREE_RSS_BYTES`, `DEFAULT_ACTIVE_TREE_RSS_BYTES`, `DEFAULT_ACTIVE_CPU_SECONDS`). | `AGENTS.md` Hard gates; `crates/zero-process/src/resource.rs` | `UNVERIFIED` | Verifiable |
| `[SPEC-HUB-004]` | Raw worker: no planner, no nested CodeMode, no MCP catalog. | `AGENTS.md` Hard gates | `UNVERIFIED` | Verifiable |
| `[SPEC-HUB-005]` | ABI digest bumps on Wire/version pins (C-23/24/26). | `AGENTS.md` Hard gates | `UNVERIFIED` | Verifiable |
| `[SPEC-CACHE-001]` | A cache hit can only be built from a key carrying a completeness witness. | `conformance/contracts/cache-entry-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-CACHE-002]` | The unsound direction (under-invalidation) fails closed. | `conformance/contracts/cache-entry-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-CACHE-003]` | The canonical key JSON is hashed with SHA-256 (`sha256_hex`) and used as the cache key. | `conformance/contracts/cache-entry-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-CACHE-004]` | Roots are non-empty content-addressed strings. | `conformance/contracts/cache-entry-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-FWV-001]` | The four components of an action's declared input sum exactly to `total_tokens`. | `conformance/contracts/fresh-work-vector-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-FWV-002]` | `eta_action` = fresh work / total as integer parts-per-million in `[0, 1_000_000]`. | `conformance/contracts/fresh-work-vector-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-FWV-003]` | Aggregation is checked integer addition. | `conformance/contracts/fresh-work-vector-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-EDIT-001]` | One generic `EDIT` operation whose argument is a list of `EditOp` values. | `conformance/contracts/zero-edit-protocol-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-EDIT-002]` | Verbs live in the payload (`v` discriminant), not in the tool namespace. | `conformance/contracts/zero-edit-protocol-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-EDIT-003]` | Version string is `zep/1`. | `conformance/contracts/zero-edit-protocol-v1.md` | `UNVERIFIED` | Verifiable |
| `[SPEC-RACC-001]` | Consumers preserve ref type and recovery path. | `docs/racc/RACC.md` Discipline | `UNVERIFIED` | Verifiable |
| `[SPEC-RACC-002]` | Surface an explicit error when a ref is unavailable or expired. | `docs/racc/RACC.md` Discipline | `UNVERIFIED` | Verifiable |
| `[SPEC-RACC-003]` | `DominanceReceipt::meets_token_target` is integer: `racc_input_tokens * 1_000_000 <= raw_input_tokens * target_retained_ppm`. | `docs/racc/RACC_CONTRACT.rs` | `UNVERIFIED` | Verifiable |
| `[SPEC-RACC-004]` | `exact_phase_valid` requires `byte_exact` AND `policy_exact_or_fallback` AND `task_verified` AND `meets_token_target`. | `docs/racc/RACC_CONTRACT.rs` | `UNVERIFIED` | Verifiable |
| `[SPEC-FU-001]` | `allowed_statuses` are exactly `present`, `partial`, `missing`, `excluded`. | `conformance/contracts/supported_surface_matrix.toml` | `UNVERIFIED` | Verifiable |
| `[SPEC-FU-002]` | `declared_feature_ids` matches every `[[feature]]` id (no missing, no unexpected). | `conformance/contracts/supported_surface_matrix.toml` | `UNVERIFIED` | Verifiable |
| `[SPEC-FU-003]` | `sum(weights) == 1.0` globally (tolerance 1e-9). Family labels are not independent scoring categories. | `conformance/contracts/supported_surface_matrix.toml`; `scripts/check_feature_universe_weights.py` | `UNVERIFIED` | Verifiable |
| `[SPEC-FU-004]` | Excluded rows still count as coverage debt for a strict-100% claim (non-zero weight retained). | FeatureUniverse promise + matrix `weight_policy` | `UNVERIFIED` | Verifiable |
| `[SPEC-FU-005]` | `missing` / `excluded` / `partial` rows require a load-bearing `retry_condition` (never later/TBD). | FeatureUniverse promise; weight script | `UNVERIFIED` | Verifiable |
| `[SPEC-FU-006]` | Evidence items are existing repo-relative paths, never bead IDs. | FeatureUniverse promise; weight script | `UNVERIFIED` | Verifiable |

## Notes

- `[SPEC-HUB-005]` is verifiable as "digest changes when wire/version pins change" for C-23/24/26. Semantic-mutation bumps (C-25) are **not** a coded checker (`AGENTS.md`); they are Ambiguous, not this tag.
- `[SPEC-HON-006]` is a permission, not a requirement. Verifier: hub MUST NOT reject Exact tokens that carry a TokenZero certification receipt.
- Duplicate claims across CONTRACT §4 and RACC Discipline (`preserve scheme` / `fail loud`) are complementary; CONTRACT is canonical.
- `RACC_CONTRACT.rs` is an interface skeleton. Production arithmetic lives in `crates/zero-ledger`. Phase 3 verifiers should drive the production types, not the skeleton, unless they are checking the skeleton's integer identity.
