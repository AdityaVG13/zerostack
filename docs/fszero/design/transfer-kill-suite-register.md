# Transfer kill suite register (identity layer + WA misuse)

Bead: `fszero-ip16.6`
Packet: `.omega-math/runs/20260717T180104Z-fszero-omega-p07-transfer-stabilize/`
Date: 2026-08-07

Maps each isomorphic-transfer **kill experiment** and related claim falsifiers to either:

- a **checked regression** (named test / contract golden), or
- an **explicit no-claim** register (normative doc) so the kill cannot go green by silence.

Metric gate from bead: `kill_suite == all kills tested or documented no-claim`.

## Identity-layer transfer (Byte / Wire / Semantic)

| Kill # | Attack | Expected defense | Register | Evidence class |
| --- | --- | --- | --- | --- |
| K1 | Treat path **SemanticId** as **ByteId** (serve path string as content identity) | `fz://blob/<64-hex>` is content hash only; expand verifies digest or fails closed | **Tested** | [V] |
| K2 | Claim **WA&lt;1** by omitting expand / harness costs | TA/WA only with full visible+expand accounting; spent ≤ raw; expand is separate cost | **No-claim + partial test** | [E]/[T] |
| K3 | Claim multi-file **serializable** isolation from CodeMode compensating txn | Contract: compensating recovery ≠ multi-process SI / multi-file atomic visibility | **No-claim + tested single-process** | [V] |
| K4 | Claim **USL / linear scale-out** from single-N Point bench | Point benches only; no ScaleFamily CI without multi-N fit | **No-claim** | [E] |

### K1 -- cross-layer equality

| Item | Location |
| --- | --- |
| Normative | `docs/architecture.md` (byte authority); `docs/durability.md` (digest verify before serve); `docs/design/zeroref-v1-annex.md` (portable identity does not imply shared storage) |
| Tests | `tests/cas.rs` (sha256 empty blob, expand tiers, mismatch); `tests/capability.rs` / capability unit (scheme ads); `tests/cross_engine_claims.rs` where present |
| Contract | ZeroRef / filesystem contract content-address rows |
| Kill green incorrectly if | product docs call a workspace path a content hash, or expand returns unverified bytes |

### K2 -- WA&lt;1 by omitting expand

| Item | Location |
| --- | --- |
| Normative | `docs/telemetry.md` (`raw_token_estimate` vs `visible_token_estimate`; expand costs); `docs/memory.md` progressive disclosure; `src/core/usage_telemetry.rs` (`spent_tokens <= raw_tokens`) |
| Tests | usage telemetry unit tests (spent exceeds raw rejected); not a full multi-corpus TA ScaleFamily suite |
| Residual | Measurement bead `fszero-ip16.2` owns multi-corpus TA instrumentation; **do not** close WA→1 as verified without that evidence |
| Kill green incorrectly if | marketing claims "free" tokens when expand materializes payload |

### K3 -- SI from compensating CodeMode txn

| Item | Location |
| --- | --- |
| Normative no-claim | `docs/filesystem-contract-v1.md` (batch compensates; no multi-process serializability); `docs/design/preimage-isolation-honesty.md`; `docs/design/world-process-model.md` |
| Single-process tests | `tests/fidelity.rs` (`compound_multi_file_mutation_is_atomic_on_anchor_failure`, undo suite); `tests/filesystem_contract.rs` (`golden_invalid_paths_and_stale_edit_fail_without_mutation`); `tests/world_durability.rs` (stale preimage + kill partial commit oracles) |
| Tool-invisibility | `tests/tool_invisibility.rs` (edit/undo leave git-clean / mtime rules) |
| Kill green incorrectly if | README/MCP ads claim multi-process SI or "ACID multi-file" without external coordinator |

### K4 -- USL / scale-out from single-N

| Item | Location |
| --- | --- |
| Normative no-claim | Claim table C10 in packet `03-claim-table.md`; `docs/benchmark-integrity.md` Point vs ScaleFamily discipline |
| Point evidence only | `benchmarks/demo-bench_results.json` (when present); not CI ScaleFamily |
| Residual | `fszero-ip16.3` multi-N CodeMode vs MCP RTT bench |
| Kill green incorrectly if | release notes claim linear scale-out from one host Point run |

## Related claim falsifiers (C-table)

| Claim | Kill / attack | Disposition |
| --- | --- | --- |
| C1 ByteId | Corrupt pack bytes | Digests fail closed -- `docs/durability.md`; cas/crash tests |
| C3 preimage | Stale verified edit | `golden_invalid_paths_and_stale_edit_fail_without_mutation` |
| C3 TOCTOU multi-process | External rewrite race | **No-claim** SI -- `preimage-isolation-honesty.md` |
| C4 undo-of-undo | Double undo as erase | Fidelity undo suite; journal compensating |
| C5 TA | Expand every ref | Open measure (`fszero-ip16.2`); design only |
| C7 world overlap | Overlapping commits | World conflict / contract goldens |
| C8 mid-txn observe | Intermediate files visible | **Admitted** by contract (honesty, not a bug) |
| C11 catalog parity | Live catalog vs contract | Own regression (`filesystem_contract` live catalog); separate ergonomics epic if red |
| C13 path jail | `..` / absolute / symlink escape | Contract goldens + `path.rs` + connector symlink tests |
| C15 SSI marketing | Market as SSI | **Forbidden** -- contract no-claim register |

## Checker commands (targeted, RCH only -- never full workspace cargo test)

```bash
# contract + identity / preimage goldens
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero \
  cargo test --test filesystem_contract -- --test-threads=1

# compensating undo / multi-file anchor failure
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero \
  cargo test --test fidelity undo -- --test-threads=1

# tool-invisibility after edit/undo
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero \
  cargo test --test tool_invisibility -- --test-threads=1

# docs no-claim grep (must stay non-empty for SI / scale-out honesty)
rg -n 'does not claim multi-process|not claim multi-process|no multi-process SI|ScaleFamily|spent_tokens <= raw' \
  docs/filesystem-contract-v1.md docs/design/preimage-isolation-honesty.md docs/telemetry.md docs/benchmark-integrity.md
```

## Status

| Kill | Status 2026-08-07 |
| --- | --- |
| K1 cross-layer equality | Mapped -- tested + normative |
| K2 WA&lt;1 omit expand | Mapped -- no-claim + telemetry contract; measure residual → ip16.2 |
| K3 SI from compensating txn | Mapped -- no-claim + single-process tests |
| K4 USL from single-N | Mapped -- explicit no-claim; residual → ip16.3 |

**Bead acceptance:** complete for register mapping. Residual measurement work stays on child/sibling beads; this bead does not invent green ScaleFamily evidence.

## Non-goals

- Does not add new product APIs.
- Does not run full `cargo test` on this host.
- Does not authorize unlabeled 99%/100% claims (Q99 discipline).
