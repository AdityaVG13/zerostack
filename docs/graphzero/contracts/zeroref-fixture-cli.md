# ZeroRef v1 fixture CLI (`graphzero zeroref-fixture`)

GraphZero's producer/consumer surface for the three-binary ZeroRef
conformance matrix (TokenZero's orchestrator; counterpart beads
`tokenzero-itl`, `fszero-c6q.6`). Deterministic, non-interactive, and safe to
drive from any harness: machine-readable JSON is never mixed with object
bytes. Schema id: `zeroref-fixture/v1`.

## Commands

```text
graphzero zeroref-fixture descriptor
graphzero zeroref-fixture put    --store-root <dir> [--shared-root <dir>] [--input <file>] [--max-object-bytes <n>]
graphzero zeroref-fixture expand --store-root <dir> [--shared-root <dir>] --ref <zeroref> [--out <file>]
```

- `--store-root` selects an isolated project root. `--shared-root` selects an
  explicit shared CAS; when present it is the read/write target, exactly like
  cross-engine interop. Without it, everything stays project-local.
- `put` reads bytes from `--input` or stdin, publishes them at the canonical
  CAS path (`blobs/sha256/<hh>/<hash>`), and prints one JSON document on
  stdout: `schema`, `ok`, `binary {engine, version, commit}`, `capability`
  (the full `zeroref-capability/v1` descriptor), `ref`, `hash`, `size`,
  `shared_root_identity`, `fragments` examples, `os`.
- `expand` parses a strict ZeroRef v1 ref (with optional `#B`/`#L` fragment),
  digest-verifies the complete object, writes the exact selected bytes to
  stdout (or `--out`), and prints diagnostics JSON on **stderr** only.
- `--max-object-bytes` applies a stricter CAS size policy for conformance
  runs; violations are real `policy_denied` failures from the production
  code path, not mocks.
- `shared_root_identity` is the SHA-256 (16-hex prefix) of the canonicalized
  root path: two engines prove they used the same root without leaking it.
  Diagnostics never contain blob contents or raw filesystem paths.

## Exit codes

| Code | Class |
|------|-------|
| 0 | success |
| 1 | other |
| 2 | `malformed` |
| 3 | `unsupported` |
| 4 | `range_out_of_bounds` |
| 5 | `not_utf8` |
| 6 | `missing` |
| 7 | `io` |
| 8 | `digest_mismatch` |
| 9 | `policy_denied` |
| 10 | `incompatible_version` |
| 11 | `legacy_ambiguity` |

Failure diagnostics (stderr JSON) carry `error_class`, `exit_code`,
`message`, `ref`, `binary`, and `os`.

## Reproducing one producer/foreign-consumer pair locally

```bash
SHARED=$(mktemp -d)/shared-cas
printf 'alpha\nbeta\n' | graphzero zeroref-fixture put \
  --store-root /tmp/gz-a --shared-root "$SHARED"        # -> {"ref": "gz://blob/<hash>", ...}
# Foreign consumer (FSZero/TokenZero) resolves the same ref against $SHARED
# and must produce byte-identical output; GraphZero's own consumer side:
graphzero zeroref-fixture expand \
  --store-root /tmp/gz-b --shared-root "$SHARED" --ref 'gz://blob/<hash>#L2-2'   # -> beta
```

Self-tests: `cargo test -p graphzero-cli --test zeroref_fixture_cli`
(content-class round-trips including a deterministic 10 MiB blob, `#B`/`#L`
parity, per-class exit codes including deliberate corruption, default-root
isolation, explicit shared-root interop with matching root identities,
concurrent identical writers, no-leak diagnostics).

Anti-cheating: the orchestrator must invoke the built sibling binaries at
pinned SHAs and compare bytes/digests; a missing peer is a skip locally and
a failure in the integration CI gate. GraphZero never retags schemes or
injects bytes into a peer's private store.
