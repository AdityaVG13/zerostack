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

`-C/--root` must precede the engine command. Plan paths stay relative to that validated root. `--verbose` reports wrapper and invoked engine version/revision. `--json` emits the complete engine result. Normal output preserves typed status and copyable scalar `fz://`, `gz://`, `tz://`, and `cm://` refs. Binary overrides are `ZS_FSZERO_BIN`, `ZS_GRAPHZERO_BIN`, and `ZS_TOKENZERO_BIN`; `ZS_TIMEOUT_MS` changes the 120000 ms default.

## Exclusive deployment rule

Choose exactly one:

| Deployment | Register standard engine MCP tools | Register CodeMode |
| --- | ---: | ---: |
| Standard MCP mode | Yes | No |
| CodeMode | No | Yes |

Never register both for the same deployment. Duplicate surfaces waste context, create ambiguous routing, and can split state. The active ZeroStack deployment uses CodeMode only.

CodeMode is not a fourth engine. It is an execution mode over TokenZero, FSZero, and GraphZero.

See the [Release-N engine MCP compatibility policy](mcp-compatibility-policy.md) for defaults, maintenance scope, migration, and staged removal gates.
