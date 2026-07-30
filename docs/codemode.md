# CodeMode and MCP mode

ZeroStack engines can run in one of two integration modes. These are alternatives, not layers.

## Standard MCP adapter

The harness registers engine tools and invokes each operation through MCP. This mode favors compatibility with clients that expect ordinary tool calls.

## CodeMode

The harness exposes a constrained JavaScript execution surface. An agent submits a plan that can call typed ZeroStack APIs sequentially or in parallel, reuse intermediate refs, and return a small final result.

~~~js
const [files, graph] = await Promise.all([
  zero.fs.compound("search", { query: "recoverable ref" }),
  zero.graph.orient("architecture", "ref flow"),
]);

return { files: files.ref, graph: graph.ref };
~~~

This Cloudflare-style execution model reduces protocol round trips and keeps intermediate values inside the sandbox rather than exposing each one to model context.

### Capability result shape

Every capability call resolves to the JSON object the connector returned, and nothing else. There are no POSIX-style aliases: `zero.token.shell` reports command output on `result` (with the rendered form on `visible` when present), not on `stdout`/`stderr`.

Reading a property the connector did not return throws a `TypeError` naming the property and listing the available ones, so a mistyped field fails loudly instead of yielding `undefined`:

~~~js
const run = await zero.token.shell("echo hi");
run.result;  // "hi\n"
run.stdout;  // TypeError: unknown property 'stdout' on token.shell result; available properties: result, visible
~~~

Use `Object.keys(value)` to inspect an unfamiliar result; enumeration is unaffected by the guard.

## Install the `zs` wrapper

The tracked Python wrapper supports Linux and macOS and has no third-party dependencies.

~~~sh
python3 scripts/install_zs.py --dry-run
python3 scripts/install_zs.py
python3 scripts/install_zs.py --verify
zs --version
~~~

The default destination is `~/.kimi-code/bin/zs`. Use `--prefix DIR` for another user bin directory. Installation uses a temporary sibling, fsync, executable-mode preservation, and atomic replacement.

Smoke from any directory while preserving engine root isolation:

~~~sh
zs -C . --verbose fs 'return await zero.fs.compound("list", { path: "." });'
printf '%s
' 'return await zero.graph.orient("delta");' | zs -C . graph -
zs -C . fs-search 'read a file'
~~~

`-C/--root` must precede the engine command. Plan paths stay relative to that validated root. `--verbose` reports wrapper and invoked engine version/revision. `--json` emits the complete engine result. Normal output preserves typed status and copyable scalar `fz://`, `gz://`, `tz://`, and `cm://` refs. Unpinned engines resolve through the same order as [binary discovery](#binary-discovery-for-embedded-harnesses), never from a live `target/release` tree: a Cargo build unlinks and replaces those files, so a build in one checkout would kill every concurrent session spawning from it. Binary overrides are `ZS_FSZERO_BIN`, `ZS_GRAPHZERO_BIN`, and `ZS_TOKENZERO_BIN`; `ZS_TIMEOUT_MS` changes the 120000 ms default.

## Binary discovery for embedded harnesses

A harness needs four executables: the aggregate host `zerostack-codemode-host` and
the `fszero-codemode` / `graphzero-codemode` / `tokenzero-codemode` delegates. None of
them belong in a config as an absolute path — a shipped config that names one
developer's worktree resolves nothing on any other machine and fails at spawn
time with a bare `ENOENT`.

Ask the host where things are instead:

~~~sh
zerostack-codemode-host --locate
~~~

It prints a `zerostack.binary_discovery.v1` JSON report naming, per binary, either
the resolved path and the rule that found it, or every candidate that was probed.

Resolution order, highest precedence first:

| Order | Source label | Location |
| --- | --- | --- |
| 1 | `zerostack_home` | `$ZEROSTACK_HOME/bin/<binary>` |
| 2 | `dev_checkout` | `$ZEROSTACK_DEV_ROOT/<Repo>/target/release/<binary>` |
| 3 | `xdg_data` | `$XDG_DATA_HOME/zerostack/bin/<binary>`, default `$HOME/.local/share/zerostack/bin` |
| 4 | `platform_install` | `/usr/local/lib/zerostack/bin`, `/opt/zerostack/bin`, `/usr/lib/zerostack/bin`; on Windows the `LOCALAPPDATA` and Program Files equivalents |
| 5 | `path` | each absolute `PATH` entry, in order |

`ZEROSTACK_DEV_ROOT` is the documented dev-checkout override: point it at the
parent directory holding the sibling `ZeroStack`, `FSZero`, `GraphZero`, and
`TokenZero` checkouts and their `target/release` builds are used directly.

Rules that keep resolution predictable:

- An empty or whitespace-only variable is treated as unset. Exporting a blank
  value is a shell artifact, not an instruction.
- A relative install root, or a blank `PATH` entry meaning "current directory",
  is skipped. Otherwise resolution would silently depend on the spawning cwd.
- An explicit `XDG_DATA_HOME` replaces the `~/.local/share` default rather than
  adding to it, so a redirect is honored instead of quietly bypassed.
- Only a regular file with an execute bit is accepted, so a same-named directory
  cannot shadow a real binary further down the order.
- Each directory is probed once even if several rules name it.

An engine that is not installed is reported as unresolved rather than aborting
discovery; the harness decides which delegates it actually requires.

## Exclusive deployment rule

Choose exactly one:

| Deployment | Register standard engine MCP tools | Register CodeMode |
| --- | ---: | ---: |
| Standard MCP mode | Yes | No |
| CodeMode | No | Yes |

Never register both for the same deployment. Duplicate surfaces waste context, create ambiguous routing, and can split state. The active ZeroStack deployment uses CodeMode only.

CodeMode is not a fourth engine. It is an execution mode over TokenZero, FSZero, and GraphZero.

See the [Release-N engine MCP compatibility policy](mcp-compatibility-policy.md) for defaults, maintenance scope, migration, and staged removal gates.
