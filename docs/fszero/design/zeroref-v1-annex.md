# ZeroRef v1 annex (FSZero)

- **Status:** Adopted (mirrors the canonical annex)
- **Canonical owner:** GraphZero `docs/adr/002-zeroref-v1.md`
  (bead graphzero-zeroref-v1-shared-cas-1ghi.1). FSZero carries a verbatim
  copy of the golden vectors and this engine-local annex.
- **Fixture:** `tests/fixtures/zeroref_v1_vectors.json`, `fixture_version: 1`
- **fixture_hash:** SHA-256 of the verbatim fixture file bytes:
  `0c81a0c850895734ac0b3a0f242cf544a252309d0324b0dc82fe4b89881459fc`
  (identical to GraphZero `docs/contracts/zeroref-v1-fixtures.json`; pinned
  by `tests/zeroref_v1_contract.rs::fixture_is_verbatim_canonical_copy`)
- **Implementation:** `src/core/zeroref.rs` (`ZeroRef`), conformance test
  `tests/zeroref_v1_contract.rs`
- Any disagreement between this annex, the canonical ADR, or the fixture is
  resolved as a reviewed cross-repo decision: bump `fixture_version`, update
  all three repos, land the fixture copies atomically. The fixture is the
  tie-breaker when prose and code disagree.

## Scope

One versioned contract defines which refs interoperate across FSZero,
GraphZero, and TokenZero. **Interoperability scope is blob refs only:**

```text
(fz|gz|tz)://blob/<sha256>[#B<start>-<end> | #L<start>-<end>]
```

Everything else -- `fz://seq/…`, `fz://file/…`, `fz://codemode/…`,
`view_N/...` aliases, `gz://node/…`, `tz://file/…`, compact forms -- is
**engine-owned**. A foreign engine receiving a non-blob ref must fail it as
`unsupported`; it must never canonicalize it into local storage or guess.

**Same-store limitation (current reality).** Cross-engine interop is a
contract only: there is no shared-CAS I/O in FSZero yet. A `gz://blob/…` or
`tz://blob/…` ref parses as a valid v1 identity claim, but FSZero can serve
its bytes only if the same content already exists in a store FSZero can
reach (recovery store or per-user ref index). Scheme normalization alone
does NOT provide interoperability: rewriting `tz://X` to `fz://X` changes
provenance labels, not reachability, and proves nothing about the bytes.
Shared-CAS read/write is future work under the ZeroRef v1 epic.

## 1. Grammar

```ebnf
zeroref   = scheme "://blob/" hash [ "#" fragment ] ;
scheme    = "fz" | "gz" | "tz" ;
hash      = 64 * lowhex ;                (* exactly 64 chars *)
lowhex    = "0"…"9" | "a"…"f" ;
fragment  = bytefrag | linefrag ;
bytefrag  = "B" number "-" number ;      (* zero-based half-open *)
linefrag  = "L" number "-" number ;      (* one-based inclusive *)
number    = 1 * digit ;                  (* ASCII digits only, u64 range *)
```

- Numbers are unsigned decimal digits only; explicit signs are rejected.
  Leading zeros are accepted on input and normalized away on emission.
- The canonical form emits plain decimal, lowercase hash, and the `-`
  fragment forms only. Parsing a canonical ref and re-emitting it is
  byte-identical.
- Extra path segments after the hash (`…/blob/<hash>/x`) are rejected.
- The pre-v1 GraphZero alias `#B<start>+<len>` is accepted as a documented
  deprecated input alias, normalized internally to `#B<start>-<start+len>`
  with checked addition, and never emitted.

## 2. Identity

- The hash is the SHA-256 of the **complete unfragmented bytes** of the
  object, encoded as full lowercase 64-hex.
- Emit full hashes, always. Reject uppercase, short prefixes, non-hex, and
  ambiguous ids at parse time (`malformed`).
- A fragment never changes identity: `#B`/`#L` select from the verified
  whole object; they are not addresses of separate objects.
- FSZero's minting path already conforms: `try_put_content_ref` in
  `src/core/recovery.rs` emits `fz://blob/<full lowercase 64-hex sha256>`.

## 3. Bounds semantics

**Byte fragments `#B<start>-<end>`** -- zero-based, half-open, over raw
bytes (UTF-8 not required):

- `start == end` is an allowed empty selection, including at exactly the
  byte length. `start > end` never parses (`malformed`).
- `end > byte length` is `range_out_of_bounds` at selection. Never clamp.
- All arithmetic is checked; overflow is `malformed`.

**Line fragments `#L<start>-<end>`** -- one-based, inclusive:

- `start >= 1` and `start <= end` are enforced at parse; `end <= real line
  count` is enforced at selection (`range_out_of_bounds`). Never clamp.
- Lines terminate at LF (`0x0A`); a selected line keeps its terminating LF
  when present. CR is ordinary line content, so CRLF files keep the CR
  inside the line. The final line may be unterminated and is selected
  without appending a newline. The empty object has zero lines, so every
  `#L` on it is out of bounds.
- `#L` requires the complete object bytes to be valid UTF-8; otherwise the
  typed error is `not_utf8`. Selection preserves the exact selected bytes
  and newlines; the golden fixtures pin the retention behavior.

## 4. Verification order and trust model

Integrity verification happens **before** fragment selection: resolve the
complete object bytes, verify SHA-256 against the ref hash
(`digest_mismatch` on failure), then select the fragment. A fragment is
never served from unverified or partially fetched bytes
(`ZeroRef::verify_and_select`).

A matching scheme suffix is an **identity claim, not shared storage**. The
scheme denotes producer/provenance, not authorization or reachability. Two
processes interoperate on a blob ref only when the receiving process can
reach the same content-addressed object and verify its digest.

Store writes are atomic at the frankensqlite transaction layer; the
per-user ref index appends whole NDJSON records and compacts via
temp-file-plus-rename (`write_compacted_ref_index_shard`). A ref is only
observable after its payload write committed.

## 5. Error taxonomy

Stable, machine-readable classes, shared verbatim with the fixture's
`error_classes` registry and `ZeroRefErrorClass` in `src/core/zeroref.rs`:

| Class | Stage | Meaning |
|-------|-------|---------|
| `malformed` | parse | Input outside the v1 grammar (bad hash, bad fragment, reversed range, overflow) |
| `unsupported` | parse | Recognizable ref that is not a portable v1 blob ref: engine-owned kinds, unknown schemes, compact forms |
| `range_out_of_bounds` | selection | Fragment exceeds real byte length or line count |
| `not_utf8` | selection | `#L` over non-UTF-8 bytes |
| `missing` | resolution | Object not present in any reachable store |
| `io` | resolution | Store I/O failure |
| `digest_mismatch` | resolution | Resolved bytes do not hash to the ref identity |
| `policy_denied` | resolution | Denied by storage policy (e.g. shared root not opted in) |
| `incompatible_version` | negotiation | Peer speaks an incompatible ZeroRef version |
| `legacy_ambiguity` | legacy resolution | Legacy short-prefix input matched zero-or-many objects |

The classes stay distinguishable end to end; surfaces must not collapse
them into a generic failure.

## 6. Storage and configuration precedence

- **Default storage is project-local**: the recovery store lives under
  `.fszero/` or the unified `.zerostack/fszero/` root of the workspace.
  Nothing about a v1 ref implies a shared filesystem location.
- FSZero's resolution precedence today is deterministic and documented in
  its error strings: explicit/env-cache -> current-root recovery store ->
  per-user ref index (`~/.fszero/ref-index`, override
  `FSZERO_REF_INDEX_PATH`, disable `FSZERO_REF_INDEX=0`). The ref index is
  a pointer layer: it maps a blob ref to the project store that minted it;
  payload bytes never leave project stores.
- A cross-project shared blob root is **explicit opt-in** and not yet
  implemented in FSZero; when it lands it follows the canonical shared-CAS
  layout (`blobs/sha256/<first-two-hex>/<64-hex>`) and the
  `ZEROSTACK_SHARED_STORE`/`ZEROSTACK_STORE_ROOT` opt-in defined by the
  canonical ADR. Until then, refusal to resolve foreign blobs from a shared
  root is the correct behavior, reported as `missing` (no configured tier)
  rather than `policy_denied` (explicitly denied tier).
- Mutable state -- indexes, facts/provenance, session views, access logs --
  remains namespaced per engine and per project even under any future
  shared root; only immutable content-addressed blobs are shareable.

## 7. Security

- **Spoofed refs:** a ref names content but proves nothing. Digest
  verification of the complete bytes precedes serving any bytes.
- **Prefix ambiguity:** short prefixes let an attacker pre-seed a store
  with a colliding-prefix object; v1 requires full 64-hex hashes.
- **Fragment smuggling:** clamping out-of-range fragments silently changes
  what an agent reads. Bounds are exact; violations are errors.
- **Cross-engine confusion:** treating scheme acceptance as authorization
  would let any process mint refs into another engine's store. Non-blob
  foreign refs are `unsupported`; blob refs only yield bytes the receiving
  store can itself verify.
- **Path traversal:** the hash is a single path-safe component; extra path
  segments are rejected at parse time. The per-user ref index shards by
  hex prefix and never derives paths from unvalidated ref text beyond
  hex-filtered characters.

## 8. Compatibility and version marker

- Version marker: `ZEROREF_VERSION = "v1"`, `ZEROREF_MAJOR = 1`,
  `ZEROREF_MINOR = 0` in `src/core/zeroref.rs`, matching the fixture's
  `zeroref_version`/`fixture_version`.
- A different major is `incompatible_version` before any payload work.
  Minor bumps are additive and forward-compatible.
- Capability negotiation (the `zeroref-capability/v1` descriptor defined by
  the canonical ADR §10) is not yet surfaced by FSZero; until it is,
  FSZero's effective shared-interop state is `disabled`.

## 9. Legacy behavior and migration window

The strict v1 layer (`src/core/zeroref.rs`) is wired into the live
expansion path (bead fszero-c6q.2): `RecoveryStore::expand_with_tiers` — the
single funnel behind `session.expand`, the `X` op (CLI/MCP `fszero.expand`)
and the CodeMode `fs.expand` connector — routes every input claiming the v1
jurisdiction (`fz://blob/…` and all `gz://`/`tz://` refs) through
`expand_zeroref`: strict `ZeroRef` parse, whole-object digest verification
in the read tiers (`verified_blob`), then `#B`/`#L` fragment selection.
Conformance: `tests/zeroref_expand.rs`. Remaining legacy behavior, honestly:

1. **Opaque key lookup for engine-owned keys.** Named keys (`search`,
   `read`), view aliases (`view_N/bytes`), and engine-owned `fz://` kinds
   (`fz://codemode/…`, `fz://seq/…` guidance errors) stay on the
   string-keyed compatibility path. Inputs in the v1 jurisdiction are now
   rejected as typed `malformed`/`unsupported` instead of missing to
   `ref_not_found`.
2. **Scheme rewrite narrowed (closed).** Foreign-scheme inputs no longer
   pass through `normalize_ref_scheme`: `gz://blob/…`/`tz://blob/…` resolve
   as same-store lookups of the local `fz://blob/<hash>` key (a provenance
   convenience, not interop — see "Same-store limitation" above); foreign
   non-blob refs fail typed as `unsupported`. `normalize_ref_scheme`
   survives only for refs the engine itself stored.
3. **Sentinel keys inside the blob namespace.** Failure paths mint
   `fz://blob/error`, and legacy stores may contain keys like
   `fz://blob/legacy`. These are not v1 identities and never leave the
   engine; the expansion path now rejects them as `malformed`.
4. **Error rendering.** The v1 path produces typed classes (§5), rendered
   on string surfaces as `class: message` with the canonical full ref —
   never a truncated hash, never a private store path. Legacy-path errors
   keep their prose strings (`ref_not_found`, `seq_ref_scoped`) during the
   window.
5. **Ref fragments (closed).** `#B`/`#L` on blob refs are honored by every
   expansion surface via the shared `select_fragment` algebra, applied only
   after whole-object digest verification.
6. **Adjacent, out-of-scope surface:** path reads `path#Bstart-end`
   (`parse_read_arg`/`read_range_bytes` in `src/core/read_ops.rs`) clamp
   ranges to file length. That surface addresses live files, not immutable
   blobs, and is not a ZeroRef; its clamping semantics do not apply to
   `#B` on blob refs, which never clamp.

**Migration window:** the remaining legacy behaviors (1, 3-legacy-stores, 4)
stay accepted engine-internal behavior for one minor release cycle after
this annex lands. New cross-engine surfaces must speak strict v1 from day
one. Shared-CAS I/O remains a separate bead of the ZeroRef v1 epic. The
conformance test
`tests/zeroref_v1_contract.rs::v1_parser_is_stricter_than_legacy_key_lookup`
pins the strict/lenient split so the boundary stays executable, not
folklore.

## 10. Non-goals

- Making sequence (`fz://seq/…`), execution, error, file (`fz://file/…`),
  or any engine-specific refs portable. Changing their scheme does not make
  them portable.
- Shared-CAS I/O in this change: this annex adopts the contract; it does
  not implement cross-engine byte transport.
- Short-prefix resolution as a contract feature. Legacy prefix handling is
  engine-internal and maps ambiguity to `legacy_ambiguity`.
- Pinning, GC, and store migration mechanics (separate beads of the epic).
- Hash agility: v1 is SHA-256 only; a different algorithm is a new major.
