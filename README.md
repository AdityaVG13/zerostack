# ZeroStack

Context infrastructure for coding agents: three engines, one composition host, typed recoverable refs.

TokenZero compacts tool output. FSZero reads and changes the live tree. GraphZero answers structural questions. ZeroStack is the hub that defines their shared contracts and runs them in one process so agents keep evidence without stuffing every intermediate byte into the model context.

## How it fits together

| Piece | Job |
| --- | --- |
| [TokenZero](https://github.com/AdityaVG13/tokenzero) | Compact, deduplicate, and selectively expand tool output (`tz://`) |
| FSZero | Live filesystem read, search, and controlled mutation (`fz://`) |
| GraphZero | Orientation, callers, impact, recall (`gz://`) |
| This repo | Shared ABI, refs, store, process identity, CodeMode host, `zsx` |

The engines stay independent. They do not import each other. This hub composes them.

```
FSZero  (bytes)     ─┐
GraphZero (structure)─┼─> typed refs ─> agent context
TokenZero (tokens)  ─┘
```

## RACC

**Recovery-aware context compression.** A large tool result is stored and replaced with a short typed handle plus a bounded preview. The handle is not a summary. Later steps expand only the lines or symbols they need, or pass the ref onward.

| Ref | Producer |
| --- | --- |
| `tz://` | TokenZero |
| `fz://` | FSZero |
| `gz://` | GraphZero |

See [docs/racc/RACC.md](docs/racc/RACC.md).

## CodeMode

The canonical runtime is `zsx`. It embeds the three engines in-process and executes one JavaScript plan. There is no worker process and no session socket.

```js
const [files, graph] = await Promise.all([
  zero.fs.compound("search", { query: "recoverable ref" }),
  zero.graph.orient("context", "architecture"),
]);

return { files: files.content, graph: graph.content };
```

Every public `zero.*` call returns `zero-result/v1`: `ack` plus `content`.

- `content.kind === "inline"` -- `content.value` is the **domain payload** (search hits, file bytes, shell `visible`, …), not the transport `{metadata, value}` wrapper.
- `content.kind === "ref"` -- the payload spilled; expand `content.ref` (`tz://blob/…`). Oversized plan results use this same shape, never a separate spill schema.

```js
const run = await zero.token.shell("echo hi");
if (run.content.kind === "inline") {
  return run.content.value; // domain payload (e.g. { visible, status })
}
return await zero.token.expand(run.content.ref);
```

`zsx mcp` is the one-catalog carrier (`zero_execute` / `zero_wait`). Do not also register engine MCP servers (fszero / graphzero / tokenzero) in the same harness. `zero-mcp` is an optional FastMCP crate and is not a second catalog on that path.

More: [docs/codemode.md](docs/codemode.md), [docs/architecture.md](docs/architecture.md).

## `zsx`

```text
zsx exec -C ROOT [--file PLAN] [--timeout-ms N]
```

Plan from `--file` or stdin. JSON on stdout. Default timeout 30000 ms.

```bash
cargo build -p zsx --release
./target/release/zsx exec -C "$PWD" --file plan.js
```

```bash
printf '%s\n' 'return (await zero.fs.compound("read", { path: "README.md" })).content;' \
  | ./target/release/zsx exec -C "$PWD"
```

## Build

Rust nightly as pinned in [`rust-toolchain.toml`](rust-toolchain.toml). License: MIT OR Apache-2.0.

```bash
git clone https://github.com/AdityaVG13/zerostack.git
cd zerostack
cargo build -p zero-abi -p zero-ref -p zero-store
```

`zsx` links the three engines through `zsx-core`. That crate expects sibling checkouts:

```text
AI/
  ZeroStack/     # this repo
  FSZero/
  GraphZero/
  TokenZero/
```

```bash
cargo build -p zsx --release
./target/release/zsx --help
```

Foundation crates (no engine source):

| Crate | Role |
| --- | --- |
| `zero-abi` | JSON contract, schema normalize, operation digest, raw-worker v2 |
| `zero-ref` | ZeroRef v1 parse, format, fragment select |
| `zero-store` | Content-addressed blob layout and publish protocol |
| `zero-process` | Process identity, owner-death, child-tree lifecycle |
| `zero-codemode` | Restricted in-process CodeMode host |
| `zero-gate` | Proof-carrying decision gate |
| `zero-ledger` / `zero-gauge` / `zero-cert` | Accounting, ordinal refs, evidence certificates |
| `zero-mcp` | Optional FastMCP stdio carrier |
| `zsx` / `zsx-core` / `zsx-node` | Single-process executable, composition core, Node binding |

## Layout

| Path | What |
| --- | --- |
| [`crates/`](crates/) | Hub crates listed above |
| [`docs/`](docs/) | Architecture, RACC, papers, Lean |
| [`conformance/`](conformance/) | Product contract |
| [`benchmarks/`](benchmarks/) | Measured savings and catalog |

## Limitations

- Building `zsx` needs the three engine checkouts on the paths `zsx-core` declares. The foundation crates build on their own.
- CodeMode and MCP are alternative surfaces. Do not register both.
- Intermediate results stay out of the model only if the agent keeps refs and expands on demand. Dumping payloads back into the prompt undoes RACC.

## FAQ

**Is this a replacement for the three engines?**
No. The engines own their domains. This repo owns contracts, the store/ref/ABI, and the process that composes them.

**Can I call one engine without the others?**
Yes. Each engine is its own repository. TokenZero is at [AdityaVG13/tokenzero](https://github.com/AdityaVG13/tokenzero).

**Why JavaScript plans instead of one tool call per step?**
A plan can fan out, reuse refs, and return one small result. The bulky intermediates never have to re-enter the model.

**Does `zsx` start a daemon?**
No. `zsx exec` runs the session in that process and exits.

## License

MIT OR Apache-2.0.
