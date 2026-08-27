# ADR 002 — ZeroRef v1: the portable cross-engine blob ref contract

- **Status:** Accepted
- **Version:** ZeroRef v1
- **Owner:** GraphZero (canonical annex; FSZero and TokenZero carry verbatim copies of the golden vectors)
- **Fixture:** `docs/contracts/zeroref-v1-fixtures.json`, `fixture_version: 2`, SHA-256 `9cd1b8ad5d3e478668e24b97a2b6e2c8e66258e8b40e02ba2d94f45a1eccd2e6`
- **Implementation:** `crates/graphzero-store/src/store/zeroref.rs` (`ZeroRef`), conformance test `crates/graphzero-store/tests/zeroref.rs`

## Decision

One versioned contract defines which refs interoperate across FSZero, GraphZero,
and TokenZero. **Interoperability scope is blob refs only:**

```text
(fz|gz|tz)://blob/<sha256>[#B<start>-<end> | #L<start>-<end>]
```

Everything else — `gz://node/…`, `gz://query/…`, `gz://snap/…`, `gz://mem/…`,
`gz://codemode/…`, `fz://codemode/…`, `tz://file/…`, compact `g:`/`q:` forms —
is **engine-owned**. A foreign engine receiving a non-blob ref must fail it as
`unsupported`; it must never canonicalize it into local storage or guess.

A matching scheme suffix is an **identity claim, not shared storage**. The
scheme denotes producer/provenance, not authorization or reachability. Two
processes interoperate on a blob ref only when the receiving process can reach
the same content-addressed object and verify its digest. Scheme acceptance
alone proves nothing.

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
- The canonical form emits plain decimal, lowercase hash, and the `-` fragment
  forms only. Parsing a canonical ref and re-emitting it is byte-identical.
- Extra path segments after the hash (`…/blob/<hash>/x`) are rejected.

## 2. Identity

- The hash is the SHA-256 of the **complete unfragmented bytes** of the
  object, encoded as full lowercase 64-hex.
- Emit full hashes, always. Reject uppercase, short prefixes, non-hex, and
  ambiguous ids at parse time (`malformed`). Legacy short-prefix resolution is
  an engine-internal concern (see §6).
- A fragment never changes identity: `#B`/`#L` select from the verified whole
  object; they are not addresses of separate objects.

## 3. Byte fragments `#B<start>-<end>`

- Zero-based, half-open: selects bytes `start..end` of the raw object bytes.
- `start == end` is an allowed empty selection, including at exactly the byte
  length. `start > end` is statically invalid and never parses (`malformed`).
- `end > byte length` is `range_out_of_bounds` at selection. Never clamp.
- All arithmetic is checked; overflow is `malformed`.
- `#B` operates on raw bytes; UTF-8 validity is not required.

### Deprecated alias

The pre-v1 GraphZero form `#B<start>+<len>` is accepted as a **documented
deprecated input alias**, normalized internally to `#B<start>-<start+len>`
with checked addition, and never emitted as ZeroRef v1.

## 4. Line fragments `#L<start>-<end>`

- One-based, inclusive: `start >= 1`, `start <= end` (both enforced at parse).
  Line **starts** past the real line count are `range_out_of_bounds` at
  selection and never clamp. A line **end** past EOF clamps to the final
  available line under the canonical v1 policy (`selection_policy.line_end =
  clamp`, fixture_version 2). Callers that need exact end validation use
  `LineEndPolicy::Strict` / `verify_and_select_with_policy`.
- **Line structure:** lines terminate at LF (`0x0A`). A selected line includes
  its terminating LF when present. CR (`0x0D`) is ordinary line content, so
  CRLF files keep the CR inside the line. The final line may be unterminated
  and is selected without appending a newline. The empty object has zero
  lines, so every `#L` on it is out of bounds.
- `#L` requires the complete object bytes to be valid UTF-8; otherwise the
  typed error `not_utf8` is returned. Golden fixtures pin the exact newline
  retention behavior.

## 5. Verification order

Integrity verification happens **before** fragment selection: resolve the
complete object bytes, verify SHA-256 against the ref hash
(`digest_mismatch` on failure), then select the fragment. A fragment is never
served from unverified or partially fetched bytes.

## 6. Errors

Stable, machine-readable error classes (shared verbatim in the fixture's
`error_classes` registry):

| Class | Stage | Meaning |
|-------|-------|---------|
| `malformed` | parse | Input does not match the v1 grammar (bad hash, bad fragment, reversed range, overflow) |
| `unsupported` | parse | Recognizable ref that is not a portable v1 blob ref: engine-owned kinds, unknown schemes, compact `g:`/`q:` forms |
| `range_out_of_bounds` | selection | Fragment exceeds real byte length or line count |
| `not_utf8` | selection | `#L` over non-UTF-8 bytes |
| `missing` | resolution | Object not present in any reachable store |
| `io` | resolution | Store I/O failure |
| `digest_mismatch` | resolution | Resolved bytes do not hash to the ref identity |
| `policy_denied` | resolution | Denied by storage policy (e.g. shared root not opted in) |
| `incompatible_version` | negotiation | Peer speaks an incompatible ZeroRef version |
| `legacy_ambiguity` | legacy resolution | Legacy short-prefix input matched zero-or-many objects |

The classes remain distinguishable end to end; surfaces must not collapse
them into a generic failure.

## 7. Storage

- **Default storage is project-local** (`.graphzero/`, `.fszero/`,
  `.tokenzero/` equivalents). Nothing about a v1 ref implies a shared
  filesystem location.
- **Canonical CAS layout:** immutable objects live at
  `blobs/sha256/<first-two-hex>/<64-lowercase-hex>` under the selected store
  root (`SharedCas` in `crates/graphzero-store/src/store/shared_cas.rs`).
  Writes hash the complete bytes first, publish via a synced sibling temp
  file and atomic rename, verify-never-overwrite preexisting objects, and
  refuse symlinked object directories. Reads enforce a regular-file/size
  policy and re-hash the complete bytes before returning them.
- A cross-project shared blob root is **explicit opt-in**. See the store
  isolation rules in [`../architecture.md`](../architecture.md);
  `GRAPHZERO_SHARED_STORE`/`ZEROSTACK_SHARED_STORE` must be truthy and
  `ZEROSTACK_STORE_ROOT` must resolve to a usable root. Graph facts, indexes,
  mutable engine metadata remain namespaced per engine and per project even
  under a shared root; only immutable content-addressed blobs are shareable.
- **Resolution precedence** is deterministic: legacy local blob store → git
  OID → project-local CAS → shared CAS (opt-in) → other adapters → ref-index.
  Corruption at any tier is terminal and never falls through.
- Pinning, GC, and migration mechanics are out of scope for this annex
  (child beads of the ZeroRef v1 epic).

## 8. Migration

- The engine-internal `gz://` grammar (`GzRef` in
  `crates/graphzero-store/src/store/refs.rs`) is wider than ZeroRef v1: it
  still accepts short hash prefixes and emits engine-owned kinds. That grammar
  stays engine-internal; new cross-engine surfaces speak ZeroRef v1.
- `#B<start>+<len>` input remains accepted per §3 but is never emitted.
- Legacy short-prefix refs resolved against a store map ambiguity to
  `legacy_ambiguity`; v1 parsing rejects prefixes outright as `malformed`.
- Any contract change is a reviewed cross-repo decision: bump
  `fixture_version`, update the annexes in all three repos, and land the
  fixture copies atomically in coordinated PRs.

## 9. Threat model

- **Spoofed refs:** a ref names content but proves nothing. Digest
  verification of the complete bytes (§5) is mandatory before any bytes are
  served; a store that skips it can serve substituted content.
- **Prefix ambiguity:** short prefixes allow an attacker to pre-seed a store
  with a colliding-prefix object. v1 requires full 64-hex hashes.
- **Fragment smuggling:** byte bounds and line starts never clamp; violations
  are errors. Canonical line-end clamp only shortens an overstated end to the
  last real line and never invents content past EOF.
- **Cross-engine confusion:** treating scheme acceptance as authorization
  would let any process mint refs into another engine's store. Non-blob
  foreign refs are `unsupported`; blob refs only yield bytes the receiving
  store can itself verify.
- **Path traversal:** the hash is a single path-safe component; extra path
  segments are rejected at parse time.

## 10. Capability negotiation

Peers discover what a binary supports from a machine-readable descriptor —
never by guessing from scheme strings or version numbers. GraphZero publishes
one descriptor (`ZeroRefDescriptor` in
`crates/graphzero-store/src/store/zeroref_capability.rs`, schema
`zeroref-capability/v1`) built from the same constants the parser and store
use, and surfaces it verbatim on `graphzero capabilities`, `graphzero
doctor`, and the CodeMode capability manifest.

Fields: `schema`, `contract {major, minor}`, `hash {algorithm, hex_length,
accept_uppercase, accept_prefixes}`, `schemes {accepted, emitted}`,
`fragments {canonical, byte_span, line_span, clamps, legacy_input_aliases,
emitted_aliases}`, `shared_cas {layout, layout_version, read, write,
max_object_bytes}`, `error_classes`, and `effective {code_support,
shared_interop, shared_root_configured, detail?}`.

- `effective.shared_interop` distinguishes local-only scheme parsing from
  real shared-CAS foreign-read capability: `enabled`, `disabled` (no opt-in),
  `misconfigured` (opt-in without a usable root), `unhealthy` (root present
  but failing I/O). `code_support` stays true throughout.
- Validation is strict and happens before payload work: a missing or
  type-broken descriptor is `malformed`; an unknown `contract.major`,
  a different hash algorithm/length, or a different `shared_cas.layout_version`
  is `incompatible_version` with an actionable message. Additive fields and
  newer minors are ignored (forward-compatible).
- Shared interop is granted only when **both** peers report
  `effective.shared_interop == "enabled"`; every restriction is explained in
  the compatibility notes.
- Descriptors never contain secrets or absolute private paths; shared roots
  are reported as states and booleans.

Golden peer fixtures: `docs/contracts/zeroref-capability-fixtures.json`
(`fixture_version: 1`), consumed by
`crates/graphzero-store/tests/zeroref_capability_contract.rs` and shared with
FSZero and TokenZero.

## 11. Golden vectors

`docs/contracts/zeroref-v1-fixtures.json` is the machine-readable contract:
sample blobs (hex bytes + SHA-256), valid refs with canonical forms and
expected selected bytes, invalid refs with stable error classes, and the
error-class registry. GraphZero's conformance test consumes it as data;
FSZero and TokenZero commit the same file verbatim and do the same. The
fixture is the tie-breaker when prose and code disagree.
