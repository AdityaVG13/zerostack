# ZS-STORE-001..009 Crosswalk Audit (store subsystem)

Auditor: crosswalk subagent. Date: 2026-08 (session). Sources audited:
- Hub: `/Users/aditya/AI/ZeroStack/crates/zero-store/` (primary authority per hub AGENTS.md: "Store schema/version skew rules are hub-owned; do not invent local CAS duplicates")
- Hub adapter: `/Users/aditya/AI/ZeroStack/crates/zsx-core/` (mutation attempt journals, CAS commit wiring, GC reachability publish)
- Engine: `/Users/aditya/AI/FSZero/crates/fszero-{core,store,engine}/` (snapshots, spans, overlays, worlds, delta, GC)
- Hub tests: `/Users/aditya/AI/ZeroStack/tests/rust/zero-store/{unit,gc_publication.rs}`; FSZero tests: `tests/engine/racc_r_adoption.rs`, `tests/store/journal_delta.rs`

Repo law read: `/Users/aditya/AI/FSZero/AGENTS.md` + `CLAUDE.md` (no doc changes; read-only audit).

Verified by execution (RCH, targeted, `--test-threads=1`): all listed suites pass -- see acceptance report.

---

## ZS-STORE-001 — Immutable content-addressed object store (P0)

| | |
|---|---|
| **STATUS** | **implemented** |
| **EVIDENCE** | Hub `crates/zero-store/src/cas.rs` (whole file, 706 ln): `SharedCas::put/put_prehashed/put_in_lock/get_verified/get_verified_limited/touch/list_objects/remove_object/quarantine_object`; layout `CAS_LAYOUT = "blobs/sha256/<hh>/<hash>"` (ln 26), `CAS_LAYOUT_VERSION=1`; publish protocol = unique sibling temp + write + `sync_all` + atomic rename + dir sync (`publish_temp_object`, ln ~404); preexisting object is re-verified and **never overwritten** (`try_return_existing_object`, ln ~317: digest mismatch = loud `CasError::DigestMismatch`); every read re-hashes complete bytes before serving (`read_verified_at`); symlink substitution refused (`check_regular`, `is_regular_file`); size policy `CAS_MAX_OBJECT_BYTES = 256 MiB`; quarantine dir for swept objects. Protocol contract in `crates/zero-store/src/lib.rs` ln 3-19. `zbf.rs`: ZBF-1 containers (graph fragments) persisted via CAS (`put_zbf/get_zbf`). Engine side: `crates/fszero-store/src/cas.rs` -- `CasStore` thin handle over `zero_store::SharedCas` (ln 14-17 doc: "Put/get publish through the hub so FS-written objects are readable by other engines at the same store root"), typed `Corrupt` class, `put_prehashed` always re-derives digest (ln ~250). Adapter wiring: `zsx-core/src/fszero.rs` (~ln 397) publishes every `fz://blob/` ref into `<session root>/blobs` via `SharedCas::put`; `zsx-core/src/connector.rs` `retain_reachability` (ln ~1308) verifies refs via `cas.get_verified`. Tests: `tests/rust/zero-store/unit/cas.rs` -- `put_get_roundtrip_and_dedup`, `corrupted_object_is_loud_and_returns_no_bytes`, `put_prehashed_rejects_a_wrong_digest_without_writing`, `converging_on_an_existing_object_is_not_a_creation`, `a_symlinked_object_is_not_present`, `sweeping_under_the_exclusive_guard_removes_and_quarantines` (29 pass). |
| **GAP** | None material. Documented contract: batch put is not all-or-nothing (partial batch = unreferenced garbage for sweeper; set-level atomicity via manifest-last). No cross-object transaction barrier -- by design. |
| **CONFIDENCE** | High (source + green tests) |

Acceptance test mapping: put/get round trip preserves exact bytes -- `put_get_roundtrip_and_dedup` + `get_verified` re-hash. Overwrite under same root rejected -- `try_return_existing_object` verifies and refuses; different bytes -> `DigestMismatch`, object untouched (`corrupted_object_is_loud_and_returns_no_bytes`).

---

## ZS-STORE-002 — Project snapshots (P0)

| | |
|---|---|
| **STATUS** | **partial** |
| **EVIDENCE** | `crates/fszero-engine/src/racc/exact_snapshot.rs` (166 ln): `ExactSnapshot` = BTreeMap path -> `FileRecord{path,digest_hex,len}`, identity = domain-tagged root digest `snapshot_root_digest` ("FSZERO-SNAPSHOT-V1\0", order-independent, sorted); `normalize_path` rejects empty/absolute/`..`/NUL, normalizes backslashes. `racc/bridge.rs`: `snapshot_from_files` production bridge. `racc/safepoint.rs`: `RawBaselineSafepoint` binds snapshot root + overlay root + journal head + evidence refs + path-policy digest; `external_state_scope = "filesystem-project-state-only"` (explicitly no external DB/process claim). `racc/durability.rs`: kill/torn/disk-full matrix over `AtomicPublication`. Production wiring: `crates/fszero-engine/src/world.rs` `bind_racc_world_commit` (ln 704-780) -- base snapshot + candidate snapshot + safepoint persisted on every world commit (`last_world_snapshot/digest`, `last_world_safepoint/id`). Tests: `tests/engine/racc_r_adoption.rs` (`snapshots_ranges_evidence_pages`, `production_bridge_snapshot_and_safepoint_identity`, `commit_world_persists_last_world_racc_keys`, `durability_matrix_kill_torn_diskfull`; 10 pass). Related: `crates/fszero-engine/src/op_memo.rs` + `candidate_store.rs` bind dependency roots + toolchain root + witness root in memo/candidate keys (toolchain contract as root, not as snapshot coverage). |
| **GAP** | Snapshot covers **files only** (path+sha256+len): no modes, no symlinks, no metadata, no dependency lockfiles, no toolchain-contract capture, no declared external inputs. Requirement's "excluded metadata is explicitly declared nonsemantic" is unmet (no exclusion declaration exists). Safepoint explicitly disclaims external state rather than tracking it. |
| **CONFIDENCE** | High |

Acceptance test mapping: changing any covered input changes snapshot root -- yes (`FileRecord.digest_hex`, root digest). Excluded metadata declared nonsemantic -- **no**; nothing is excluded because nothing beyond bytes is captured.

---

## ZS-STORE-003 — Exact reads and spans (P0)

| | |
|---|---|
| **STATUS** | **partial** (span-fail-closed implemented; line-ending canonicalization missing) |
| **EVIDENCE** | `crates/fszero-engine/src/racc/evidence_page.rs` (174 ln): `ExactRange{start,end}`, `EvidencePage{source_root_digest, path, range, range_digest_hex, bytes}`; `verify_against_source` fails with `StaleSource` (root changed) or `DigestMismatch` (bytes drifted) -- a span **fails rather than silently drifts** (tests `extract_and_verify_round_trip`, `out_of_bounds_refused`). `crates/fszero-core/src/target_ref.rs`: canonical span grammar `<path>#L<start>-L<end>` (1-based inclusive), `parse_target_ref`, `window_byte_range`, `render_hit`; role classification. `crates/fszero-core/src/canonicalize.rs`: per-artifact-class canonical forms (sorted keys, stable ordering). `crates/fszero-engine/src/asof_snapshot.rs`: time-travel reads over the mutation journal with content-digest refs (`fz://blob/<hex>`), `AsofError::UnknownPath/DeletedAtOrdinal` fail-closed. Preimage roots: `recovery/mutation_log.rs` `pre_ref/post_ref` columns; `fszero-store/src/recovery/edit_intent.rs` (pre/post + pre_ref/post_ref + mtime + mode + xattrs). AST spans: `durable_integrity.rs` `CURRENT_TABLES` `ast_nodes(span_start, span_end)`. Path canonicalization: `normalize_path` (exact_snapshot), `rel_path_for_log_with_canon` (`access_log.rs`). |
| **GAP** | No end-to-end op "resolve span against snapshot root" (pieces exist: EvidencePage binding + asof journal + AST spans, but no single resolver API). Line-ending (CRLF vs LF) variants are **not** canonicalized -- raw-bytes identity treats them as distinct content; requirement says line-ending variants must canonicalize correctly. |
| **CONFIDENCE** | High |

Acceptance test mapping: span fails rather than drifting after source change -- `verify_against_source` (StaleSource/DigestMismatch). Line-ending/path-alias canonicalization -- path alias: yes (normalize_path, store_root `absolutize`); line-ending: **no**.

---

## ZS-STORE-004 — Child sandbox snapshots (P0)

| | |
|---|---|
| **STATUS** | **partial** (writable child overlays + path guards exist; process/network/env effect tracking and sandbox escape enforcement missing) |
| **EVIDENCE** | `crates/fszero-engine/src/racc/overlay_publish.rs` (309 ln): `Overlay::on_base(base)` -- writable child derived from immutable base; `materialize` refuses `WrongBase`/`Stale`; deterministic effects -> exact candidate root (`realize_effects`, test `same_effects_same_candidate_root`). `crates/fszero-engine/src/world.rs`: `fork_world` (ln 436), speculative worlds (`new:`, `newbatch:`, `fork`), virtual overlay reads without write-through (`virtual_overlay.rs` -- list/read resolve journal+base in memory). Parent immutability: base is `ExactSnapshot` (immutable), overlay materializes into new root; commit path (`commit_world` ln 541) plans without writing, re-checks preimage, rolls back on any failure. Path guards: `ensure_path_under_root` in `verified_edit.rs` (ln 150) and `world.rs` (ln 588) refuse absolute/`..` escapes; `SubstrateChild` supervision + durable stderr capture (`substrate_child.rs`). Hub: CAS publish dirs refuse symlink substitution (`ensure_object_publish_dirs`). |
| **GAP** | No sandbox: no OS-level isolation (chroot/seatbelt), no undeclared-network/process/env access tracking (requirement's "track every file, process, network, environment, generated-output effect" is unmet beyond planned file edits); no symlink-traversal policy for workspace reads; generated outputs outside the planned edit set are not traced (untracked side effects are not detected at fork/commit time). |
| **CONFIDENCE** | High |

Acceptance test mapping: sandbox escape / symlink traversal / undeclared network / parent writes blocked or Unsafe -- path-escape: blocked by guards; symlink traversal: partial (CAS + store marker only); undeclared network/process: **not enforced**; parent writes: parent snapshot is immutable, child writes never mutate base -- ok.

---

## ZS-STORE-005 — Exact delta sealing (P0)

| | |
|---|---|
| **STATUS** | **partial** (minimal-span deltas + deterministic reseal + journal exist; full independent rescan and effect receipts missing) |
| **EVIDENCE** | `crates/fszero-store/src/journal_delta.rs` (fszero-sa2v): `JournalDelta` -- minimal changed byte range (`JournalByteRange{start,before_end,after_end}`) + exact `replacement` bytes + `before_hash`/`after_hash`; `changed_span` derives the minimal diff; ops `Upsert/Remove`. Tests `tests/store/journal_delta.rs`: `integrate_journal_deltas_equals_batch_render_byte_for_byte`, `unequal_replacement_create_remove_ranges_hashes_and_wire_are_stable`, `durable_feed_is_gapless_bounded_and_survives_reopen`. Reseal determinism: `racc/overlay_publish.rs` `realize_effects` (same effects -> same candidate root, test). Commit sealing: `world.rs` `commit_world` -- preimage re-check (`live != current` -> abort+rollback), journaled mutation with pre/post refs (`record_mutation`), `record_commit_intent` + `recover_committing_worlds` (crash-safe multi-file publish), `maybe_crash_after_world_writes` SIGKILL oracle. External-edit detection: `crates/fszero-engine/src/external_edit.rs` (mtime/len baseline). Verify-and-rollback: `verified_edit.rs` (`verify` command, 120 s timeout, rollback on failure). Deletions/renames: `EffectMutation::Delete`, `SuccessorMap::record(RefFate::Moved/Deleted)` (`racc/successor_map.rs`). |
| **GAP** | No mandatory **independent full-workspace rescan** vs traced delta after execution: `ExternalEditDetector` is mtime/len on touched paths, not a content rescan of the whole tree; hidden/untracked mutations elsewhere are not detected. Deletions/renames are recorded in successor maps but not sealed into a single canonical delta document; formatter changes and external-effect receipts (requirement) are absent. |
| **CONFIDENCE** | High |

Acceptance test mapping: independent rescan matches traced delta -- partial (byte-exact integration test exists for delta feed; no end-to-end rescan-vs-trace gate). Hidden/untracked mutation causes verification failure -- partial (external-edit detector + commit-time preimage checks cover edited files only).

---

## ZS-STORE-006 — Atomic compare-and-swap commit (P0)

| | |
|---|---|
| **STATUS** | **partial** (expected-parent CAS commit fully implemented in hub journal; nonce/lease/protected-scope not in one binding) |
| **EVIDENCE** | Hub `crates/zero-store/src/durable_journal.rs`: `JournalBindingV1{old_root,new_root,transaction_id,assembly_manifest_digest,durable_profile_id,owner_identity_digest}` (ln 251); `prepare_journal_v1` refuses unless current published root == `binding.old_root` (`RootMismatch`, ln ~740: "prepare requires the preregistered old root"); `commit_journal_v1` publishes the new root only when `root.root_digest == binding.old_root` else `verify_new` (ln ~844-863) -- exact expected-parent compare-and-swap; continuation cartridge binds prepared-record digest; owner-death receipt finishes an interrupted commit; recovery receipt is idempotent. Mutation attempt journal: `attempt_journal.rs` -- hash-chained immutable entries Prepared -> DispatchCrossed -> Succeeded/Failed/Indeterminate; `prepare_attempt_v1` rejects a second different binding (`ImmutableEntryConflict`) and any re-dispatch of terminal entries (`AlreadyTerminal`, `recover_attempt_v1` never redispatches). Wiring: `zsx-core/src/connector.rs` `prepare_mutation_journal` -> `cross_mutation_journal` (persisted before dispatch) -> `succeed_mutation_journal` / `indeterminate_mutation_journal` (post-cross failures resolve Indeterminate, never SafeToRetry -- `fail_indeterminate`). WAL: `session_wal.rs` (append/replay, sealed segments, `foreign_write_detects_replaced_snapshot`). Atomic file ops: `fs_replace.rs`. Locking: `gc_lock.rs` `StoreLock` (shared publish / exclusive sweep). FSZero per-file CAS: `world.rs` `commit_world` re-checks live bytes == planned preimage before each write. Tests: `unit/durable_journal.rs` (`journal_recovery_root_disagreement_is_never_guessed`, `journal_recovery_finishes_a_cartridge_only_prepare_as_abort`, `journal_recovery_rejects_a_foreign_cartridge`; 8 pass), `unit/attempt_journal.rs` (`attempt_prepare_conflicts_and_terminal_guards`, `attempt_recovery_never_redispatches`, crash-boundary tests; 12 pass). |
| **GAP** | No single commit API binding **all five** required terms (parent root/epoch + authorized delta root + protected scope + nonce + lease) in one atomic record: durable journal binds old/new root + transaction + owner; lease protection is a separate GC record (`put_leased` in `gc.rs`); nonce and protected-scope are not part of the journal binding. No explicit two-writer concurrency integration test at the commit surface (journal-level semantics cover it: second writer's `old_root` mismatches -> `RootMismatch`). |
| **CONFIDENCE** | High |

Acceptance test mapping: two concurrent commits -> one success, one stale-root failure -- journal semantics enforce it (`prepare`/`commit` root equality; second commit sees new root != its old_root). Replayed lease cannot mutate state -- terminal journal entries immutable + lease epoch monotonic (`validate_next_lease_epoch`, `gc_publication` `leased_publish_is_atomic_and_release_enables_collection`).

---

## ZS-STORE-007 — Integrity scrubbing and replication (P1)

| | |
|---|---|
| **STATUS** | **partial** (corruption detection + repair + quarantine implemented; replication and L2/L3 distinction absent) |
| **EVIDENCE** | Verified-on-access reads: hub `cas.rs` `get_verified` re-hashes before serving (digest mismatch loud, no bytes served); FSZero `CasStore::get` maps to `Corrupt` class. Repair: hub `gc.rs` `repair_object` / `repair_object_receipted` (ln 2351+) -- corrupt object quarantined before replacement, immutable `RepairReceipt` persisted; tests `unit/gc.rs` `repair_replaces_corrupt_object_and_rejects_wrong_bytes`, `gc_publication.rs` `repair_quarantines_corruption_and_persists_receipt`. Quarantine: `CAS_QUARANTINE_DIR` (`cas.rs` ln 37) -- swept/corrupt bodies moved, not deleted. SQLite integrity gate: `crates/fszero-store/src/recovery/durable_integrity.rs` (GATE_VERSION 4, fail-closed stock-SQLite validation before fsqlite opens, forensic/salvage siblings bounded by count + byte budget, `pack_validation_pending` revalidation). Logical-vs-physical: `ReachabilitySnapshot` (`gc.rs` ln 167) is a producer-declared logical availability record, separate from physical CAS file presence -- the nearest existing analog, but no explicit L2/L3 terminology or model. |
| **GAP** | No periodic scrubber of idle objects (verification is on-access only); no replication topology or repair-from-replica (repair requires caller-supplied authoritative bytes); no explicit logical-L2 vs physical-L3 residency model; no background verification pass with reporting. |
| **CONFIDENCE** | High |

Acceptance test mapping: injected corruption detected before reuse -- yes (every get re-verifies; repair tests). Replica recovery preserves same root -- **not implemented** (no replicas).

---

## ZS-STORE-008 — Retention and garbage collection (P1)

| | |
|---|---|
| **STATUS** | **implemented** (lease-aware, root-safe, fault-tested; reachability is producer-declared) |
| **EVIDENCE** | Hub `crates/zero-store/src/gc.rs` (2449 ln): `run_gc` (ln 1563) -- exclusive coordinator lock, sweep progress with plan digest (resume after fault, tampered plan fails closed), reachability snapshots / pins / leases (epoch-validated, expiry + `GC_MIN_GRACE_SECONDS`), `GcVerdict::Retain/Collect/RetainUncertain`, dry-run reports, `repair_object`, `put_leased` (object + lease visible as one coordinator boundary). Root-safety: `gc_lock.rs` `StoreLock` shared publish / exclusive sweep + liveness recheck under held lock; `cas.rs` `sweep_target` restates the object under the exclusive guard (symlink substitution refused); `remove_object`/`quarantine_object` require the exclusive guard; `check_guard_root` refuses cross-store lock use. Reachability closure: `refs_from_verified_bytes` traces ZBF container children transitively (`gc_publication.rs` `refs_closure_defines_reachability_and_apply_commits_only_unreachable`, `nested_refs_closure_is_transitive`). Engine: `crates/fszero-store/src/cas.rs` `CasStore::gc` -- mark-and-sweep honoring pins (`blobs/pins`, `gc/pins`), `gc/roots/**` reachability snapshots (all engines), non-expired `gc/leases/**`; mtime grace window; `publish_fszero_gc_roots` (monotonic epoch, validated/deduped hashes); publish guard locks out concurrent GC (`publish_guard_locks_out_a_concurrent_gc`). Adapter: `zsx-core/src/connector.rs` `retain_reachability` + `publish_reachability` (closure-verifies every ref via `get_verified` before publishing roots). Tests: `unit/gc.rs` 26 pass (`pins_and_leases_preserve_unrooted_objects`, `expired_pin_does_not_wedge_collection`, `faulted_sweep_resumes_from_progress_record`, `stale_epoch_and_bad_version_fail_closed`, `malformed_metadata_is_uncertain_not_collectable`, `publish_is_blocked_during_sweep_unlink_window`, `symlinked_gc_namespace_is_uncertain_and_not_followed`); `gc_publication.rs` 13 pass. |
| **GAP** | Reachability is **producer-declared**: GC retains what engines publish in `gc/roots`; there is no automatic tracing from "live snapshots, continuations, receipts, authorities" to blob closure (the ZBF refs closure works only for container objects the producer stored). FSZero store GC of legacy roots is still listed open in `FSZero/AGENTS.md`. Retention policy = pins/leases/grace, not audit-policy-driven. |
| **CONFIDENCE** | High |

Acceptance test mapping: GC never deletes a reachable object -- reachability + pin + lease closure with liveness recheck under exclusive lock (tests cover cross-engine roots, leases, pins, publisher-vs-sweep window). Tombstone/reclamation races fault-tested -- `faulted_sweep_resumes_from_progress_record`, `a_publisher_cannot_slip_between_a_sweep_decision_and_its_unlink`, `tampered_resume_plan_fails_closed`.

---

## ZS-STORE-009 — Tenant isolation and encryption (P1)

| | |
|---|---|
| **STATUS** | **partial** (project-key namespacing + per-repo metadata isolation implemented; no authorization, no encryption, dedup side channel open) |
| **EVIDENCE** | Namespacing: hub `crates/zero-store/src/store_root.rs` -- `project_key` = sha256(abs root path) truncated to 16 hex (ln ~396), `PROJECTS_DIR/<key>/<engine>` for shared-store modes; `StoreMode::{LocalUnified,PinnedInsideProject,SharedNamespaced,Legacy}`; symlinked `.zerostack` marker refused (`local_marker_is_symlink`, fail-closed). Engine: `crates/fszero-store/src/zerostack_store.rs` -- per-repo metadata DB at `projects/<project_key>/fszero/store.sqlite3` ("never a single shared ... for unrelated roots. Only immutable digest-addressed CAS blobs may be shared."); `EngineFileError` validates engine file names. GC namespace hardening: `gc.rs` `validate_namespace` + symlink refusal (`symlinked_gc_namespace_is_uncertain_and_not_followed`, `symlinked_gc_namespace_is_refused`). `zsx-core/src/connector.rs` `retain_reachability` enforces engine ownership of refs (scheme check) and authorized-CAS availability. |
| **GAP** | **No encryption at rest anywhere** (grep across `zero-store`, `fszero-store`: zero cipher/AEAD usage; only crypto is sha256). No per-tenant authorization on CAS reads -- any process with store-root access reads any object. Project key is a 16-hex prefix of a path hash with no keyed MAC (guessable with path knowledge; no HMAC). Content-addressed dedup is shared across projects by design ("deliberately not project-namespaced") -- a cross-tenant dedup side channel that is not gated by policy. No timing-oracle hardening on missing objects (uniform `NotFound`/`Malformed` returns exist, but no test targets cross-tenant guessing). |
| **CONFIDENCE** | High |

Acceptance test mapping: cross-tenant handle/root guessing yields no data or timing oracle -- partial: engine-scoped ref validation + 64-hex full hashes make blind guessing infeasible, but known-content hash guessing succeeds (dedup side channel, permitted implicitly), no authorization layer, no encryption.

---

## Summary table

| ID | Title | STATUS | Key evidence | Top gap |
|---|---|---|---|---|
| ZS-STORE-001 | Immutable CAS object store | **implemented** | `zero-store/src/cas.rs`; `fszero-store/src/cas.rs`; `zsx-core/src/fszero.rs:397`; `unit/cas.rs` (29 pass) | none material (batch not all-or-nothing, documented) |
| ZS-STORE-002 | Project snapshots | **partial** | `racc/exact_snapshot.rs`; `racc/safepoint.rs`; `world.rs:704`; `racc_r_adoption.rs` | modes/symlinks/lockfiles/toolchain/external inputs not covered; no nonsemantic-exclusion declaration |
| ZS-STORE-003 | Exact reads and spans | **partial** | `racc/evidence_page.rs`; `fszero-core/src/target_ref.rs`; `asof_snapshot.rs` | no end-to-end span-resolver op; line-ending variants not canonicalized |
| ZS-STORE-004 | Child sandbox snapshots | **partial** | `racc/overlay_publish.rs`; `world.rs` fork/commit; `virtual_overlay.rs`; path guards | no OS sandbox; no process/network/env effect tracking; no symlink policy for workspace reads |
| ZS-STORE-005 | Exact delta sealing | **partial** | `journal_delta.rs`; `overlay_publish.rs::realize_effects`; `commit_world`; `external_edit.rs` | no mandatory full-workspace independent rescan vs traced delta; no external-effect receipts |
| ZS-STORE-006 | Atomic compare-and-swap commit | **partial** | `durable_journal.rs` (old_root/new_root); `attempt_journal.rs`; `connector.rs` wiring | no single binding of parent-root+delta+scope+nonce+lease; no 2-writer integration test |
| ZS-STORE-007 | Integrity scrubbing and replication | **partial** | `gc.rs::repair_object*`; verified-on-access; `durable_integrity.rs` | no periodic scrubber; no replication/repair-from-replica; no L2/L3 model |
| ZS-STORE-008 | Retention and GC | **implemented** | `gc.rs` (run_gc, leases, pins, progress); `gc_lock.rs`; `fszero-store/src/cas.rs::gc`; `connector.rs` reachability | reachability producer-declared; no automatic live-object tracing; legacy-root GC open |
| ZS-STORE-009 | Tenant isolation and encryption | **partial** | `store_root.rs` project keys; `zerostack_store.rs` per-repo DB; GC namespace hardening | no encryption; no per-tenant authorization; dedup side channel un-policy-gated; weak (non-keyed) project key |

## Files an implementer should open first

1. `/Users/aditya/AI/ZeroStack/crates/zero-store/src/cas.rs` -- the canonical CAS contract everything else thins over.
2. `/Users/aditya/AI/ZeroStack/crates/zero-store/src/durable_journal.rs` -- expected-parent CAS commit (ZS-STORE-006 core).
3. `/Users/aditya/AI/ZeroStack/crates/zero-store/src/gc.rs` -- leases/pins/reachability/repair (ZS-STORE-007/008 core).
4. `/Users/aditya/AI/FSZero/crates/fszero-engine/src/racc/exact_snapshot.rs` + `overlay_publish.rs` -- snapshot/overlay/delta-seal base (002/004/005).
5. `/Users/aditya/AI/FSZero/crates/fszero-engine/src/world.rs` -- production wiring of snapshots/overlays/CAS commits.
6. `/Users/aditya/AI/ZeroStack/crates/zsx-core/src/connector.rs` -- attempt journal + reachability wiring in the adapter.

## Constraints / risks

- RACC V6 `status` column ("Not implemented / audit required") is a corpus default, not repo state -- this audit is the ground truth for the store rows.
- Hub `zero-store` is pinned in FSZero `Cargo.toml` at rev `bd721f7fc4866b24dec0c552da3d96bd8d816fbc`; ABI digest bumps required if semantics change (per hub AGENTS.md).
- Overlay/world machinery is largely in-memory or recovery-store-backed, not OS-level sandboxing; do not present world isolation as a security boundary.
- All durability claims here rest on the fault-injection matrix (`racc/durability.rs`) plus journal crash-boundary tests, not on real kill/disk-full hardware runs.
