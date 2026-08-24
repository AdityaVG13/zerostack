# ZeroKernel cells

ZeroKernel executes JavaScript or TypeScript cells inside a restricted Rust interpreter. It is the composition runtime over FSZero, GraphZero, and TokenZero; it is not a fourth engine and it does not expose their legacy command catalogs.

## Direct surface

A fresh frame receives one `z` global with exactly six operations:

```js
const [files, callers] = await Promise.all([
  z.read("src"),
  z.find("execute", { mode: "callers", path: "src" }),
]);
return { files, callers };
```

| Operation | Purpose |
| --- | --- |
| `read` | Files, directories, structured snapshots, and exact-handle recovery |
| `find` | Text, AST, symbol, relationship, and semantic search |
| `edit` | One-file create, substitute, replace, or remove |
| `apply` | Atomic multi-file effects and optional bounded verification |
| `run` | Supervised argv or script execution |
| `state` | Bounded JSON-serializable session facts |

Token accounting, output projection, compression, and exact recovery are host
behavior behind these operations. Engine selection, method catalogs, operation
string dispatch, and compatibility aliases are not model-facing.

## Frame and host lifecycle

The host is reusable; the interpreter is not. Each call:

1. validates source and finite budgets;
2. creates a fresh interpreter frame;
3. runs typed engine calls, shell children, and async tasks under shared cancellation;
4. appends one terminal event;
5. commits staged CAS state only after successful completion;
6. reaps tasks and child processes before returning;
7. destroys the frame.

A failed or cancelled call rolls back an open filesystem transaction and discards staged state. The canonical response reports the terminal outcome, visible value, event digest, state evidence, and live-resource ledger.

## Concurrency and staged work

Use normal JavaScript for orchestration:

```js
const values = await Promise.all([
  z.read("a.txt"),
  z.read("b.txt"),
]);
return values.map(value => value.toUpperCase());
```

Filesystem effects are automatically staged under one host-owned transaction
per cell. Use `z.edit` for one file and `z.apply` for an atomic multi-file
request:

```js
await z.edit("generated.txt", { create: "content" });
return await z.apply([
  { path: "src/a.rs", edit: { find: "old", replacement: "new" } },
  { path: "src/b.rs", create: "pub fn created() {}\n" },
]);
```

A failed or cancelled frame rolls back automatically. Effects use FSZero
receipts and reverse-order restoration; no best-effort path guessing is
accepted.

## Process execution

`z.run(command, options?)` delegates to the hub-owned `zero-process`
implementation. It captures bounded stdout and stderr, owns the exact child
tree, and terminates and reaps that tree on timeout, cancellation, frame
failure, or host shutdown. TokenZero does not spawn processes.

## TypeScript

TypeScript syntax is erased before evaluation while preserving byte and line layout for diagnostics. Runtime constructs that require code generation, including enums and namespaces, fail closed instead of silently changing semantics.

## Node and ZMP

`@oh-my-pi/zero-kernel` exposes the reusable host through an asynchronous N-API
class. Harness builds compile and atomically install the target-native addon.
Runtime loading pins the addon by digest and does not invoke Cargo, Git, a
daemon, or a downloader.

ZMP's built-in `zero` tool calls this native package in-process. The separate
`codemode` tool is not a fallback execution path for ZeroKernel failures.

## Compatibility boundary

Raw-worker and MCP code may remain for explicit conformance or compatibility tests, but neither is part of canonical ZeroKernel execution. A supported harness must consume the Rust host or the Node package directly and must not register a second engine command catalog beside it.
