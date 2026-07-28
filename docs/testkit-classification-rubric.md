# Testkit classification rubric

## Governing question

**Does this test's failure correspond to a user-visible broken promise?**

Classify the promise, not the amount of code exercised. Durability, data-loss prevention, integrity, and documented CLI behavior are user-visible even when their tests sit near implementation details. A self-roundtrip, private representation check, or internal enum-to-enum agreement without an external promise is a CUT candidate.

## Categories

| Category | Use when | Required disposition |
| --- | --- | --- |
| **KEEP** | Failure directly breaks one engine's documented behavior, safety, durability, integrity, or compatibility promise and the test belongs at that engine boundary. | Keep in the owning engine; tighten the promise sentence or boundary if unclear. |
| **SHARE/MOVE** | The promise is user-visible but is a cross-engine ZeroStack contract, duplicated fixture, protocol invariant, or conformance behavior better owned by the shared testkit. | Name the destination owner and migrate the assertion/fixture. The shared test must be green in every applicable engine before the original duplicate is removed. |
| **CUT** | No external promise is evidenced, or the test only confirms a self-roundtrip, private encoding choice, internal enum agreement, helper wiring, or another implementation coincidence. | Record the missing promise and nominate removal; do not reinterpret a convenient invariant as a public contract. |

Classification itself never deletes or moves a test. Deletion is a separate reviewed change. For SHARE/MOVE, migration must land and run green against every declared consumer before duplicate removal.

## Decision procedure and evidence

1. Write one sentence beginning **“Users can rely on…”** without naming a private type, helper, or algorithm. If that cannot be done from evidence, start at CUT.
2. Cite the public source of that promise: CLI help/docs, wire/schema/ABI contract, compatibility policy, data-integrity requirement, or reproduced user-visible failure. A descriptive test name alone is weak evidence.
3. Choose the narrowest owner: engine-specific behavior stays KEEP; an identical cross-engine contract goes SHARE/MOVE; no evidenced external contract goes CUT.
4. Record evidence and uncertainty. Do not infer documentation, consumers, or compatibility guarantees that were not inspected.

Evidence sufficient for high confidence is a public contract plus a boundary-level assertion, or a reproduced user-visible regression tied to the test. Medium confidence has a plausible boundary and test evidence but incomplete public-contract confirmation. Low confidence is name/path-only triage or relies on an unverified ownership assumption.

- **High:** classifier may assign KEEP; SHARE/MOVE still requires the current engine owner and proposed shared-testkit owner to review migration scope; CUT requires the current owner to confirm no compatibility or safety promise exists.
- **Medium:** current engine owner must review every category before action.
- **Low:** leave unclassified for action; obtain docs, failure history, or owner input first.
- Security, durability, corruption, data-loss, and compatibility tests always require owner review before CUT, regardless of confidence.

## Compact inventory schema

One row per test:

| Field | Format |
| --- | --- |
| Engine | FSZero, TokenZero, or GraphZero |
| Test | Repository-relative path plus exact test name |
| Promise | One sentence: “Users can rely on…” |
| Category | KEEP, SHARE/MOVE, or CUT |
| Confidence | high, medium, or low |
| Evidence | Public contract/failure reference, or explicit evidence gap |
| Review | Current owner; for SHARE/MOVE also destination owner |

## Worked sample

These are classifications from static path/name inspection, not deletion decisions. “Tentative” marks incomplete contract evidence.

| Engine | Test | Promise | Category | Confidence | Evidence | Review |
| --- | --- | --- | --- | --- | --- | --- |
| FSZero | `tests/cas.rs::wrong_bytes_under_existing_digest_rejected_and_never_overwritten` | Users can rely on stored content not being silently overwritten when integrity verification fails. | KEEP | high | Data-loss and corruption behavior is user-visible; boundary assertion is explicit. | FSZero owner |
| FSZero | `tests/cas.rs::read_time_corruption_is_typed_corrupt_and_never_served` | Users can rely on corrupted stored bytes never being returned as valid content. | KEEP | high | Integrity refusal is a user-visible safety promise. | FSZero owner |
| FSZero | `tests/search_freshness.rs::warmed_search_observes_external_rewrite_without_a_stale_window` | Users can rely on search reflecting an external file rewrite rather than stale indexed content. | KEEP | medium | Name and boundary indicate visible freshness; public wording was not verified. | FSZero owner |
| FSZero | `tests/wire_contract.rs::per_op_read_9kb_exposes_blob_ref_in_wire_refs` | Users can rely on oversized operation results exposing recoverable references through the shared wire envelope. | SHARE/MOVE | medium | Cross-engine wire/conformance shape is a shared-contract candidate; destination coverage is unverified. | FSZero + shared testkit owners |
| FSZero | `tests/cas.rs::put_get_roundtrip` | Users can rely on content written through the public store being readable by its returned reference. | CUT (tentative) | low | Static inspection shows a self-roundtrip; no independent external promise was verified. | FSZero owner, mandatory durability review |
| TokenZero | `crates/tokenzero/tests/cli_help_contract.rs::cli_bare_invocation_prints_useful_help` | Users can rely on a bare CLI invocation presenting actionable help. | KEEP | high | Documented CLI behavior is user-visible by rule and exercised at the CLI boundary. | TokenZero owner |
| TokenZero | `crates/tokenzero-engine/src/session_persist_tests.rs::v1_state_does_not_replay_v2_journal_into_seen_set` | Users can rely on persisted sessions not mixing incompatible journal generations. | KEEP | medium | Durability/compatibility impact is visible; exact public version policy was not verified. | TokenZero owner |
| TokenZero | `crates/tokenzero-core/tests/protocol_atoms.rs::ack2_golden_atoms_are_portable_and_deterministic` | Users can rely on shared acknowledgement atoms remaining deterministic across supported protocol consumers. | SHARE/MOVE | medium | Portable protocol golden behavior appears cross-engine; consumer set needs confirmation. | TokenZero + shared testkit owners |
| TokenZero | `crates/tokenzero-core/tests/protocol_atoms.rs::portable_intersection_matches_public_runtime_table` | Users can rely on the public runtime table matching its internally computed portable intersection. | CUT (tentative) | medium | This is internal table-to-computation agreement unless a published table contract is identified. | TokenZero owner |
| GraphZero | `crates/graphzero-reserve/tests/crash_boundary_reserve.rs::malformed_reservation_entry_returns_structured_error` | Users can rely on malformed durable reservation data failing explicitly rather than being silently accepted. | KEEP | high | Durability and corruption handling are user-visible safety behavior. | GraphZero owner |
| GraphZero | `crates/graphzero-store/tests/warm_fingerprint_content.rs::same_mtime_size_byte_rewrite_must_not_warm_skip` | Users can rely on changed source bytes being reindexed even when file metadata appears unchanged. | KEEP | high | Prevents stale query results and silent missed updates. | GraphZero owner |
| GraphZero | `crates/graphzero-types/tests/wire_compatibility.rs::content_hash_json_wire_shape_is_byte_array_newtype` | Users can rely on content hashes retaining the shared JSON wire shape used by ZeroStack consumers. | SHARE/MOVE | medium | Explicit wire-compatibility boundary suggests shared conformance ownership; consumers need confirmation. | GraphZero + shared testkit owners |
| GraphZero | `crates/graphzero-types/tests/wire_compatibility.rs::content_hash_hex_roundtrip_rejects_non_wire_hex` | Users can rely on a private hex conversion roundtrip agreeing with its parser. | CUT (tentative) | medium | Self-roundtrip/internal representation candidate; no external hex-input promise was verified. | GraphZero owner |
| GraphZero | `crates/graphzero-pack/tests/canonical_json_drift.rs::the_two_encoders_disagree_on_bytes` | Users can rely on two internal encoders continuing to disagree byte-for-byte. | CUT (tentative) | medium | The assertion describes implementation disagreement, not a user promise; confirm it is not a migration alarm. | GraphZero owner |

Sample: **14 tests** total: FSZero 5, TokenZero 4, GraphZero 5; KEEP 7, SHARE/MOVE 3, CUT candidates 4.
