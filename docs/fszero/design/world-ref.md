# World-ref v1: stable overlay-enumeration contract (fszero-cbt)

Status: shipped v1. Consumer: graphzero speculative blast (blast radius over
a PLANNED edit, computed from an fszero world without materialization).
Producer: `fs.world` op `view:<wid>` (`src/core/world.rs::do_world_view`).

## Ref format

A world ref is `fz://world/<wid>` where `<wid>` matches `W[0-9]+`. World
refs are SESSION-SCOPED: they name an open (uncommitted) world in a live
fszero session and are invalid after that world commits or drops. They are
not content-addressed and must never be persisted as durable pointers --
consumers resolve them immediately.

## Enumeration API

`zero.fs.world("view:<wid>")` returns ack
`world:1 view:<wid> v=1 files=<n> ref=<fz://blob/...>`; the blob (also
stored under key `world_<wid>/view`) is JSON:

```json
{
  "version": 1,
  "world_ref": "fz://world/W3",
  "world": "W3",
  "files": [
    {
      "file": "src/a.rs",
      "hunks": [[12, 14], [40, 40]],
      "status": "clean",
      "base_hash": "<sha256 hex of current base bytes>",
      "post_hash": "<sha256 hex of would-be bytes>",
      "base_ref": "fz://blob/<base_hash>",
      "post_ref": "fz://blob/<post_hash>"
    },
    { "file": "src/b.rs", "hunks": [[3, 3]], "status": "conflict", "detail": "no match" },
    { "file": "src/c.rs", "hunks": [[1, 1]], "status": "unreadable", "detail": "..." }
  ]
}
```

Semantics:

- `file` is root-relative (same keying as `fs.history` / the access ledger).
- `hunks` are 1-based inclusive line spans of each staged edit within its
  staging preimage -- the same intervals cross-world conflict detection
  (fszero-4wp) uses.
- `status: "clean"` -- the overlay (journal replayed over the CURRENT base,
  fszero-1wm) resolves; `post_ref` is the exact bytes `commit:<wid>` would
  write if the base does not move again. `base_*` describe the base as of
  THIS enumeration, not fork time.
- `status: "conflict"` -- the world's edit no longer applies to the moved
  base (`detail`: `no match` | `ambiguous match`); commit would report a
  structured conflict (fszero-glg).
- `status: "unreadable"` -- the base file vanished (delete/rename under the
  world).
- Nothing is materialized: disk is untouched; `base_ref`/`post_ref` blobs
  are persisted content-addressed in the store so consumers fetch would-be
  bytes by ref without a live view of the tree.

## Stability guarantees

- `version` is bumped on ANY breaking shape change; fields may be ADDED
  within v1 without a bump. Consumers must ignore unknown fields and reject
  unknown `version`s loudly.
- Ack prefix `world:1 view:<wid> v=1 files=` and the `world_<wid>/view` key
  are part of the contract.
- Hash algorithm is SHA-256 (lowercase 64-hex), matching `fz://blob`
  content addressing (see docs/design/zeroref annex when adopted).

## Non-goals (v1)

- Cross-process world handles: `fz://world/<wid>` does not resolve from a
  different session/process. graphzero consumes the enumeration blob (which
  IS durable), not the live world.
- Created/deleted files in worlds: worlds stage edits to existing files
  today; the `files` array gains `status: "created" | "deleted"` entries
  when that lands (will not break v1 consumers that ignore unknown states).

Verified by `world_ref_enumeration_v1_contract` in tests/smoke.rs.

## Conflict resolution contract (fszero-e8s)

A conflicted `commit:<wid>` leaves the world active and its report under
`world_<wid>/conflict`. Resolution ops (stable v1):

| op | semantics |
| :-- | :-- |
| `resolve:<wid>:abort` | drop the world (alias of `drop:<wid>`) |
| `resolve:<wid>:<path>:mine` | my staged content wins: the file's edits collapse to one whose preimage is the CURRENT base; next commit fast-paths my would-be bytes over it (recreates the file if it was deleted) |
| `resolve:<wid>:<path>:theirs` | the moved base wins: my edits for that file are withdrawn |
| `resolve:<wid>:<path>:merged:<text>` | supply merged content verbatim (everything after `merged:`, no grammar); preimage set to the current base |

Rules:
- Resolution is per file; re-commit after resolving. Unresolved files still
  conflict — nothing is ever silently clobbered (fszero-glg invariant).
- If the base moves AGAIN between resolve and commit, the resolved file
  conflicts again (its preimage no longer matches) — resolution never
  grants a standing license to overwrite.
- Taxonomy coverage: content overlap (mine/theirs/merged), delete-vs-edit
  (theirs keeps the deletion; mine/merged recreate), mode-change-vs-edit is
  NOT a content conflict (commit lands, changed mode survives),
  create-create cannot occur in v1 (worlds stage edits to existing files
  only; revisit when worlds gain file creation).

Verified by world_resolve_mine_theirs_merged_abort,
world_resolve_delete_vs_edit_recreates_on_mine, and
world_commit_mode_change_vs_edit_no_conflict in tests/smoke.rs.
