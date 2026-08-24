# ZeroKernel cells

A ZeroKernel call evaluates one bounded JavaScript or TypeScript cell against a global `z` object. The host is reusable; the interpreter frame is fresh on every call.

## Direct surface

| Operation | Purpose |
| --- | --- |
| `z.read` | Read files, directories, structured snapshots, selections, and exact handles |
| `z.find` | Search text and syntax; query definitions, references, callers, imports, and call paths |
| `z.edit` | Create, replace, remove, or patch one file against exact authority |
| `z.apply` | Apply an atomic multi-file effect request |
| `z.run` | Run one bounded process with owned cancellation and recoverable output |
| `z.state` | Carry small JSON facts across otherwise fresh frames |

Engine names, transports, method catalogs, and transaction opcodes are not model-facing.

## Read and find together

Use ordinary JavaScript to compose independent work:

```javascript
const [source, callers] = await Promise.all([
  z.read("src/lib.rs"),
  z.find("execute_cell", { mode: "callers", path: "src" }),
]);

return { source, callers };
```

A normal UTF-8 file within the inline budget returns complete content. A larger result returns a bounded view plus an exact handle. Pass the handle back to `z.read`, or request a line or byte selection. Do not write an outline or preview back as if it were the source file.

## Guarded edits

A structured read can return a snapshot that binds the selected path and exact preimage:

```javascript
const snapshot = await z.read({
  path: "src/config.rs",
  select: { lines: [18, 42] },
  snapshot: true,
});

return await z.edit(snapshot, {
  find: "const RETRIES: usize = 2;",
  replacement: "const RETRIES: usize = 3;",
});
```

If authority changed after the snapshot, the edit conflicts instead of overwriting unseen bytes. Use `z.apply` when several files must publish together.

## State across frames

Interpreter variables never survive the terminal boundary. `z.state` is a bounded session-scoped JSON map for a path, cursor, checkpoint, or user decision needed by the next cell.

Good state is small and explicit:

```javascript
z.state.set("selected-path", "src/lib.rs");
return await z.read(z.state.get("selected-path"));
```

Repository data belongs in files. Large values belong behind exact handles. Secrets do not belong in session state.

## Failure and cancellation

One cancellation token reaches the interpreter, engine calls, concurrent promises, and owned process tree. On failure or cancellation the host:

1. requests sibling cancellation;
2. terminates exact child processes;
3. waits for bounded settlement;
4. restores staged file effects and state;
5. records one terminal event;
6. destroys the frame.

A retry receives a new frame and the last committed state. It never resumes partially evaluated JavaScript.

## TypeScript

TypeScript annotations are erased while preserving byte and line layout for diagnostics. Runtime constructs that require code generation fail closed rather than silently changing semantics.

## Embedding

Rust applications construct `ZeroKernel` directly. Node applications load `@zerostack/zero-kernel`, initialize one host, execute cells, then shut the host down. Production packages select a staged platform prebuild and do not compile, download, or launch a service at runtime.
