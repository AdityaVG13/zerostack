# Zero Edit Protocol v1 (ZEP/1)

Status: normative for the compact (Level-1) edit path across FSZero, GraphZero and TokenZero.
Types: `crates/zero-codemode/src/edit_protocol.rs`.
Bead: zerostack-racc-caching-output-vz89.1 (epic vz89, output-token frontier).

This document **unifies existing verbs and existing ref grammars** into one contract. It does
not invent a new ref syntax and does not add new engine capabilities.

## 1. Shape: one operation, nine verbs

The protocol is exposed to a model as **one** generic operation, `EDIT`, whose argument is an
`EditPlan`:

~~~json
{"p":"zep/1","ops":[{"v":"REPLACE","r":"src/lib.rs#L4-L6","text":"let x = 1;\n"}]}
~~~

`p` is the protocol tag (`zep/1`, defaulted on input). `ops` is an ordered list applied in order.
The verb lives in the `v` discriminant of the payload, **not** in the tool namespace: nine tool
definitions would repay their schema cost nine times, while one definition amortizes once.

## 2. Verb table

| Verb | Payload | Ref slots | Level-0 fallback |
| --- | --- | --- | --- |
| `READ` | `{r}` | span or path | `read <ref>` |
| `REPLACE` | `{r,text}` | span | `replace <ref>` + fenced text |
| `INSERT` | `{at,text,side?}` | span anchor | `insert before\|after <ref>` + fenced text |
| `DELETE` | `{r}` | span | `delete <ref>` |
| `MOVE` | `{from,to}` | object -> path | `move <a> -> <b>` |
| `COPY` | `{from,to}` | object -> path | `copy <a> -> <b>` |
| `RENAME` | `{sym,to}` | `gz://node/...` | `rename <sym> -> <name>` |
| `APPLY_PATCH` | `{base,patch}` | path or blob | `apply_patch <base>` + unified diff |
| `RUN` | `{cmd}` | command ref or literal | `run <cmd>` |

`side` defaults to `after` and is omitted from the wire when default.

### Provenance: found vs invented

Every verb already exists behind some surface; ZEP/1 only names them uniformly.

| Verb | Existing implementation |
| --- | --- |
| READ | FSZero `fs.read` (`src/core/read_ops.rs`), which already accepts `<path>#L<a>-L<b>` verbatim |
| REPLACE | FSZero `fs.edit` / `edit_spec` unique-old-to-new replace, and `fs.compound("mutate")` `edits[]` |
| INSERT / DELETE | Degenerate spans of the same FSZero edit path (empty old / empty new) |
| MOVE / COPY | FSZero path ops; `memory_rename` in `src/core/dispatcher.rs` is the in-store analogue |
| RENAME | GraphZero symbol identity (`gz://node/<sym>`, `crates/graphzero-pack/src/query.rs`) plus FSZero rewrite |
| APPLY_PATCH | FSZero `fs.write` / verified-edit path (`src/core/verified_edit.rs`) |
| RUN | `fszero.exec` / TokenZero `zero.token.shell` |

Nothing in the verb table is new capability. The new artifact is the *contract*.

## 3. Ref grammar (referenced, not redefined)

ZEP/1 accepts exactly the refs the stack already produces:

| Kind | Syntax | Owner |
| --- | --- | --- |
| `FileSpan` | `<path>#L<start>-L<end>`, 1-based inclusive | FSZero snap-to-file target grammar (`src/core/target_ref.rs`, `docs/design/target-ref-grammar.md`); adopted by GraphZero query hits in commit `79dd558` |
| `BlobSpan` | `fz://blob/<sha256>[#L<a>-L<b>\|#B<a>-<b>]`, also `gz://blob/...` and `tz://blob/...` | ZeroRef v1 (`crates/zero-ref`, `docs/zeroref.md`) |
| `Symbol` | `gz://node/<symbol>` | GraphZero |
| `Path` | bare workspace-relative path | FSZero |
| command | opaque command ref or literal command line (`RUN` only) | TokenZero |

`classify_ref` is a **syntactic classifier only**. Digest verification, clamp policy (`#L` end
clamps, `#L` start is strict -- see `docs/zeroref.md`) and resolution remain the owning engine's
job; ZEP/1 must never reimplement them.

## 4. Error and fallback semantics

Validation errors carry a stable class:

| Class | Meaning |
| --- | --- |
| `malformed_ref` | ref matched no accepted grammar (also used for structurally invalid JSON) |
| `ref_kind_mismatch` | ref parsed, but the slot does not accept that kind (e.g. `RENAME` on a file span) |
| `empty_field` | a required non-ref field was empty |
| `unsupported_version` | `p` is not `zep/1` |

**Level 0 is always available.** Every verb has a total `level0()` rendering (plain text /
unified diff). A producer that cannot form a valid compact payload -- stale ref, unknown span,
engine without the compact path -- falls back to Level 0 and loses no capability, only token
efficiency. Consumers MUST accept Level 0 unconditionally. Compact-path failure is therefore
never a hard failure; it is a cost regression.

Fallback triggers:

1. `malformed_ref` / `ref_kind_mismatch` on emit -> re-emit the same intent at Level 0.
2. `unsupported_version` from a peer -> Level 0 for the remainder of the session.
3. Ref resolution failure at the engine (stale digest, moved file) -> the engine reports the
   ZeroRef error class; the producer re-anchors or falls back.

## 5. Schema-budget analysis

The hard acceptance criterion is `dCost = p_out*dC_out - p_in*dC_schema > 0`.

**dC_schema (input side, paid per uncached request).** Measured against the alternative of nine
separate tool definitions:

| Option | Tool defs | Est. schema tokens |
| --- | --- | --- |
| Nine verb tools | 9 | ~9 x 110 = ~990 |
| One `EDIT` op + inline verb table | 1 | ~260 |

ZEP/1 chooses the second: **dC_schema is about +260 tokens**, one time, and cache-resident after
the first request in a session. The nine-tool design would have cost ~990.

**dC_out (output side, paid per edit).** A single localized edit rendered three ways:

| Mode | Typical output tokens for a 6-line change in a 400-line file |
| --- | --- |
| Level 2 -- full file rewrite | ~4,000 |
| Level 0 -- unified diff with context | ~120 |
| Level 1 -- ZEP/1 `REPLACE` payload | ~45 |

dC_out vs the diff baseline is about **-75 tokens per edit**; vs full-file rewrite about **-3,955**.

**dCost.** With current frontier pricing the output/input price ratio `p_out/p_in` is about 5.
Break-even against the diff baseline needs `5 * 75 * N > 260`, i.e. `N > 0.7` edits -- the protocol
pays for itself on the **first** edit of a session, and the schema cost is paid once while the
output saving recurs per edit. Against full-file rewrite the first edit returns roughly 19,000
input-token-equivalents.

These are estimates, deliberately conservative (they ignore prompt caching of the schema, which
drives the effective `p_in` down by about 10x). The measured numbers are owned by bead **vz89.12**
(three-mode output benchmark harness: full-file vs diff vs edit-protocol), which is blocked on
this bead.

## 6. Scope of this deliverable

Shipped here: the contract, the Rust types (`EditOp`, `EditPlan`, `classify_ref`), and conformance
tests (round-trip every verb, reject malformed payloads, Level-0 form for every verb).

**Not** shipped here: execution wiring. `zero-codemode` today is a generic host (`Connector` trait +
QuickJS sidecar) with no filesystem edit dispatch point, so there is no clean seam to wire into.
Execution lands with the engine-side beads:

* **vz89.4** -- FSZero candidate persistence + delta-only repair loop (FSZero-side application).
* **vz89.8** -- GraphZero output-side closure (multi-site op generation).
* **vz89.12** -- TokenZero three-mode benchmark (measures the dCost above).
