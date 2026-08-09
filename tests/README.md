# ZeroStack shared conformance suite

`tests/` is the canonical, harness-neutral shared suite. It contains active
contract schemas, fixtures, executable checks, adapter descriptors, and the
budget gate. `conformance/models/` remains evidence-only; old conformance and
`zero-testkit` paths remain as compatibility sources until explicit deletion
approval.

## Run the suite

Reference adapter, without Pi:

```sh
python3 tests/scripts/run_shared_suite.py --reference
```

One selected engine, with an explicit CodeMode binary:

```sh
python3 tests/scripts/run_shared_suite.py fszero --fszero-bin /path/to/fszero-codemode
```

All configured engines plus the reference adapter:

```sh
python3 tests/scripts/run_shared_suite.py --all \
  --fszero-bin /path/to/fszero-codemode \
  --graphzero-bin /path/to/graphzero-codemode \
  --tokenzero-bin /path/to/tokenzero-codemode
```

The engine commands delegate to `zerostack-shared-conformance` (override with
`ZEROSTACK_CONFORMANCE_BIN`). No engine crate or Pi package is imported.

## Transports

Conformance runs in two distinct, non-overlapping layers:

- **Plan-level G1-G10** (`--surface planner` with an explicit hub/reference
  planner adapter): drives `{ns}_execute_code` over JSON-RPC and checks planner
  semantics (ctx.step, coalescing, sandbox). Engines do not ship planner
  artifacts. See `tests/src/plan.rs`.
- **Raw-worker RW1-RW10** (`--surface codemode`, a `<engine>-codemode`
  artifact): drives the planner-free raw-worker v2 wire protocol and checks
  worker-boundary invariants. See `tests/src/raw_worker.rs`.

The two id vocabularies (G1-G10 vs RW1-RW10) never overlap; a report's
`contract_version` distinguishes `1.0` (plan) from `raw-worker-v2` (raw).
Revision- and digest-bound Linux evidence for the initial three-engine raw cut
is indexed in `tests/data/raw_worker_v2_native_linux_2026-08-09.json`; it does
not claim planner or cross-platform coverage.

- `*-codemode` artifacts are planner-free **raw-worker v2** binaries, not MCP
  servers and not planner hosts. The harness spawns them with the same
  serve/probe contract the aggregate host uses (FSZero: `--raw-worker --root
  <root>`; TokenZero: `raw-worker --root <root>`; GraphZero: default serve mode
  via the `GRAPHZERO_REPO` env) and probes every engine with
  `capabilities --json`. The hub `zero-codemode/session.rs` happens to use a
  different valid TokenZero probe, so this harness's TokenZero probe shape is a
  harness choice, not the engine's only valid one.
- `*-mcp` artifacts are MCP servers; conformance drives them over JSON-RPC and
  only G1 exposure applies.
- A raw worker is never treated as an MCP server: no initialize/tools/call
  framing, no planner, no JavaScript host, no capability catalog in workers.

The canonical TokenZero binary is the `<engine>-codemode` artifact; the
planner-free GraphZero binary is built with its default feature set (no
planner host).

The raw-worker gates test the **worker boundary only**: RW6 is
session-continuity (not a literal ctx.step primitive), RW8 is domain-authority
mutation that must succeed (the engine owns mutation; the hub owns
authorization), and RW9 is worker process reuse (not aggregate plan-level op
coalescing). Plan-level invariants are covered separately by the planner layer.

## Budget check

```sh
python3 tests/scripts/check_budget.py --self-test
```

This proves exactly 50 libtest registrations pass and an injected 51st fails.
