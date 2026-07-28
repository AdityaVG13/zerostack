# zero-abi UB/Miri canary

This permanent canary checks the public `zero-abi` library for undefined behavior under Miri. It is package/lib scoped only, never a workspace run. It does not certify FFI or private engines and does not replace targeted tests for changed behavior.

## Prerequisites

From the repository root, with the committed lockfile present:

```bash
rustup toolchain install nightly --component miri
cargo +nightly miri setup
```

Record the nightly toolchain and Miri version used. Do not update dependencies as part of the run.

## Run

Default canary:

```bash
cargo +nightly miri test -p zero-abi --lib --locked
```

Optional second pass using Tree Borrows:

```bash
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test -p zero-abi --lib --locked
```

### RCH/Spark agents

Agents configured to use the Spark remote worker must invoke the same command through the RCH wrapper. Keep the command repository-relative and reject any run that does not identify a remote worker:

```bash
output=$(rch exec -- cargo +nightly miri test -p zero-abi --lib --locked 2>&1)
status=$?
printf '%s
' "$output"
[ "$status" -eq 0 ] || exit "$status"
printf '%s
' "$output" | grep -Eq '^\[RCH\] remote [^[:space:]]+'
```

The output must contain `[RCH] remote <worker>`. A missing marker is a failed run; never retry locally or accept local fallback. For Tree Borrows, prefix the wrapped command with `MIRIFLAGS="-Zmiri-tree-borrows"` while preserving the package/lib/locked arguments.

## Evidence and cadence

Retain with the release evidence:

- date, commit SHA, nightly/Miri versions, exact command, and `MIRIFLAGS` if set;
- complete stdout/stderr, exit status, and the `[RCH] remote <worker>` line for remote runs;
- on failure, the first Miri diagnostic, relevant backtrace, and a link to the tracked fix or release blocker.

Run once per release and after changes to the `zero-abi` digest, schema, or unsafe surface. CI remains documentation-only/manual: the repository workflow is dispatch-only, and the cost of nightly Miri is intentionally not imposed on every change. A passing result means both the command and Miri tests exited successfully; any diagnostic, nonzero exit, local fallback, or scope drift fails the canary.
