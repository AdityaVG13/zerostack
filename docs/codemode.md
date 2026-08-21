# ZeroKernel cells

ZeroKernel executes JavaScript or TypeScript cells inside a restricted Rust interpreter. It is the composition runtime over FSZero, GraphZero, and TokenZero; it is not a fourth engine and it does not expose their legacy command catalogs.

## Direct surface

A fresh frame receives one `z` global. Every callable member maps directly to a typed engine or host method:

```js
const [files, callers] = await z.parallel([
  () => z.lookup("src", { pattern: "*.rs" }),
  () => z.asgrep("execute", { mode: "callers", path: "src" }),
]);
return { files, callers };
```

The direct surface includes:

| Area | Methods |
| --- | --- |
| Filesystem | `read`, `lookup`, `write`, `edit`, `remove`, `transact` |
| Structure | `asgrep` with typed structural modes |
| Token output | `measure`, `project`, `compress`, `expand` |
| Orchestration | `parallel`, `pipeline`, `shell` |
| Durable state | `state.get`, `state.set`, `state.delete`, `state.list` |

There is no `zero.fs.compound`, `token.shell`, `graph.orient`, method catalog lookup, operation string dispatch, or transport envelope on this path.

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

## Parallel and pipeline execution

`z.parallel` accepts thunks and starts independent calls concurrently:

```js
return await z.parallel([
  () => z.read("a.txt"),
  () => z.read("b.txt"),
]);
```

`z.pipeline` is stage-ordered. Thunks within one stage may run concurrently; the next stage starts only after the prior stage settles. Rejection cancels siblings and drains every task before the frame returns.

## Transactions

Filesystem effects are automatically staged under one host-owned transaction per cell. `z.transact` scopes a mutation callback, but its successful effects commit only with the cell terminal:

```js
return await z.transact(async () => {
  await z.write("generated.txt", "content");
  return "committed";
});
```

A failed or cancelled frame rolls back automatically. Effects use FSZero receipts and reverse-order restoration; no best-effort path guessing is accepted.

## Shell

`z.shell(command, options?)` delegates to the hub-owned `zero-process` implementation. It captures bounded stdout/stderr, owns the exact child tree, and terminates and reaps that tree on timeout, cancellation, frame failure, or host shutdown. TokenZero does not spawn processes.

## TypeScript

TypeScript syntax is erased before evaluation while preserving byte and line layout for diagnostics. Runtime constructs that require code generation, including enums and namespaces, fail closed instead of silently changing semantics.

## Node and ZMP

`@zerostack/zero-kernel` exposes the reusable host through an asynchronous N-API class. The package selects an exact platform prebuild or the explicit `ZERO_KERNEL_NATIVE_ADDON` development path. It never invokes Cargo, Git, a shell, a daemon, or a downloader at runtime.

ZMP's built-in `zero` tool calls this native package in-process. ZMP keeps `codemode` as a separate fallback tool; ZeroKernel does not automatically rerun failed source in CodeMode.

## Compatibility boundary

Raw-worker and MCP code may remain for explicit conformance or compatibility tests, but neither is part of canonical ZeroKernel execution. A supported harness must consume the Rust host or the Node package directly and must not register a second engine command catalog beside it.
