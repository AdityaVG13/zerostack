# ADR 011 — Unified multi-engine ref resolution in the pi-zerostack router

- **Status:** Proposed (spec only — graphzero-zeroref-v1-shared-cas-1ghi.10)
- **Owner:** GraphZero (router annex; FSZero / TokenZero review before implementation)
- **Depends on:** ADR 002 (ZeroRef v1 blob contract), SharedCas layout (`cas-local` / opt-in shared root)

## Decision

Each engine must keep working **alone** with its native refs (`gz://`, `fz://`, `tz://`).
When **two or more** engines are active behind the pi-zerostack router, the router
exposes **one expand mental model**: any expand-capable entry accepts any
**registered** scheme, dispatches to the **owner engine**, and uses shared-CAS
identity only where gauge/conformance proves byte equivalence.

This ADR specifies activation, negotiation, errors, lifetime, auth, and failure
modes. **No router implementation ships from this document** until an owner
approval bead closes the recommendation section.

## 1. Solo vs multi-engine

| Mode | Expand behavior |
|------|-----------------|
| Solo GraphZero | Native `gz://` only (plus ZeroRef blob forms the store already resolves). Foreign schemes → `unsupported`. |
| Solo FSZero / TokenZero | Same, for `fz://` / `tz://`. |
| Multi (router) | One expand surface; scheme → owner table; owner returns bytes or typed error. |

Solo installs must not require the router. Multi-engine installs must not force
sequential cross-engine round trips when independent expands are requested.

## 2. Activation discovery

1. Process / session starts with a **capability handshake** per engine (raw-worker
   v2 or FastMCP equivalent).
2. Router builds `registered_schemes: { "gz": handle, "fz": handle, "tz": handle }`
   from engines that completed handshake with matching contract digests.
3. An engine absent from the table is **not** probed on every expand; missing
   scheme → typed `unsupported` / `missing_engine` (see §5).
4. Hot-add: a late handshake may register a scheme mid-session; expands that
   already failed as missing_engine are not automatically retried.

## 3. Capability negotiation

Per engine, advertise at least:

- `ref_scheme` (e.g. `gz`)
- `expand: true` with bounded `max_bytes` / deadline semantics
- `semantic_contract_digest` / worker revision
- Ownership: which refs this engine minted and will expand (`RefOwnership`)

Router refuses to route a scheme to an engine whose handshake digest does not
match the session’s expected digest (`worker_skew`).

## 4. Dispatch rules

1. Parse scheme (and refuse malformed refs before dispatch).
2. Look up owner in `registered_schemes`.
3. Forward expand with budget, deadline, and cancel token.
4. On success: return exact requested window; preserve source label in trace.
5. Shared-CAS shortcut: **only** for ZeroRef v1 blob identities when both
   engines declare the same CAS root and gauge evidence shows digest-verified
   equivalence. Never use CAS to “translate” opaque `gz://query` / `fz://file`
   handles into another engine’s namespace.

## 5. Error classes (typed)

| Token | When |
|-------|------|
| `not_found` | Owner active; object absent after full owner chain. |
| `wrong_root` | Ref ownership metadata points at a store root this session is not authorized for. |
| `expired` | Owner TTL / session ref lease elapsed. |
| `worker_skew` | Digest / revision mismatch between router expectation and owner. |
| `missing_engine` | Scheme not in `registered_schemes`. |
| `unsupported` | Scheme known but op/fragment not served by owner. |
| `digest_mismatch` | INV-001; terminal, no fallthrough. |

Harnesses branch on these tokens, not on Display strings alone.

## 6. Same content, different view

Two engines may mint different native refs for the same blob bytes. The router
may return **multiple native refs in one envelope** when an orient/snap-class
op intentionally offers handoff choices. It must **not** invent a new aggregate
scheme in v1 of this ADR.

**Recommendation (pending owner approval):** prefer
`{ "refs": ["gz://blob/…", "fz://file/…#L…"], "dispatch_set": ["gz","fz"] }`
over a new `xz://…` aggregate ref. Aggregate refs defer to a later ADR if
product evidence shows agents cannot choose among native refs.

## 7. Lifetime and GC

- Engine-owned refs follow the minting engine’s GC / session lease.
- Shared-CAS objects are immutable content; GC is coordinator-locked
  (existing SharedCas / TokenZero sweeper protocol).
- Router must not keep a second copy of blob bytes outside CAS/owner stores.

## 8. Authorization boundaries

- Expand of a ref minted under root R requires the caller’s session to be
  bound to R (or an explicit shared-store opt-in that includes R).
- Cross-root expand via ref-index remains owner-mediated; router does not
  broaden ACL beyond what the owner already enforces.

## 9. Cold / warm behavior

- Cold owner (no index / empty CAS): expand returns typed `not_found` or
  `substrate` with a next_action to index/warm — never a silent empty payload.
- Warm path: p50 expand for a known blob fragment stays within the owner’s
  existing budget class; router overhead is routing only (no re-hash of whole
  objects when the owner already verified).

## 10. Partial failure and parallelism

- Batch expand of N refs: per-item ok/error; one failure does not cancel siblings
  unless the caller’s cancel token fires.
- Independent expands must be schedulable concurrently (no forced sequential
  fan-out in the router). Dependent plans remain the caller’s CodeMode/JSON DAG
  concern.

## 11. Non-goals

- No speculative pre-expand of foreign schemes.
- No silent rewrite of `gz://query` into `fz://file` without an explicit
  edit-anchor / handoff field the owner already emitted.
- No second public plan language in the router.

## 12. Approval gate

Implementation may begin only after:

1. FSZero and TokenZero owners ACK this ADR (or file a counter-proposal), and
2. A follow-on bead tracks router code + conformance fixtures separately from
   this spec.
