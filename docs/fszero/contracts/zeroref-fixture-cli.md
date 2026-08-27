# ZeroRef v1 fixture CLI (`fszero zeroref-fixture`)

FSZero's producer/consumer surface for the three-binary ZeroRef conformance
matrix (TokenZero's orchestrator; counterpart beads `tokenzero-itl`,
`graphzero-zeroref-v1-shared-cas-1ghi.7`). Deterministic, non-interactive, and
safe to drive from any harness: machine-readable JSON is never mixed with object
bytes. Schema id: `zeroref-fixture/v1`.

## Commands

```text
fszero zeroref-fixture descriptor [--store-root <dir>]
fszero zeroref-fixture put    --store-root <dir> [--shared-root <dir>] [--input <file>] [--max-object-bytes <n>]
fszero zeroref-fixture expand --store-root <dir> [--shared-root <dir>] --ref <zeroref> [--out <file>]
```

- `--store-root` selects an isolated project root. FSZero creates its durable
  store (`.fszero` or `.zerostack/fszero`) under this root as usual.
- `--shared-root` selects an explicit shared CAS. When present it is the
  read/write target, exactly like cross-engine interop. Without it, the fixture
  creates a local canonical CAS under the project store root so writes still
  exercise the real CAS path.
- `put` reads bytes from `--input` or stdin, publishes them through the
  production FSZero recovery store + CAS, and prints one JSON document on
  stdout: `schema`, `ok`, `binary {engine, version, commit}`, `capability`,
  `ref`, `hash`, `size`, `shared_root_identity`, `fragments`, `os`.
- `expand` parses a strict ZeroRef v1 ref (with optional `#B`/`#L` fragment),
  digest-verifies the complete object, writes the exact selected bytes to
  stdout (or `--out`), and prints diagnostics JSON on **stderr** only.
- `--max-object-bytes` applies a stricter size policy for conformance runs;
  violations are real `policy_denied` failures, not mocks.
- `shared_root_identity` is the SHA-256 (16-hex prefix) of the canonicalized
  CAS root path. Two engines prove they used the same root without leaking it.
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

Failure diagnostics (stderr JSON) carry `error_class`, `exit_code`, `message`,
`ref`, `binary`, and `os`.

## Reproducing one producer/foreign-consumer pair locally

```bash
SHARED=$(mktemp -d)/shared-cas
printf 'alpha\\nbeta\\n' | fszero zeroref-fixture put \
  --store-root /tmp/fz-a --shared-root "$SHARED"
# -> {"ref": "fz://blob/<hash>", ...}
# Foreign consumer (TokenZero/GraphZero) resolves the same ref against $SHARED
# and must produce byte-identical output. FSZero's own consumer side:
fszero zeroref-fixture expand \
  --store-root /tmp/fz-b --shared-root "$SHARED" --ref 'fz://blob/<hash>#L2-2'   # -> beta
```

Self-tests: `cargo test -p fs-zero --test zeroref_fixture -- --test-threads=1`
(content-class round-trips, golden-vector digest stability, explicit shared-root
interop with matching root identities, isolated-store isolation, deliberate
corruption yielding `digest_mismatch`, `policy_denied` enforcement, malformed
ref rejection, CLI argument parsing).

Anti-cheating: the orchestrator must invoke the built sibling binaries at pinned
SHAs and compare bytes/digests; a missing peer is a skip locally and a failure
in the integration CI gate. FSZero never retags schemes or injects bytes into a
peer's private store.
