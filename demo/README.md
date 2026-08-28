# Demo

Minimal ZeroStack demo that runs one real ZeroKernel cell combining FSZero, GraphZero, and automatic TokenZero output handling.

## Run

From the repository root:

```bash
node demo/run.js
```

## What it does

`run.js` loads the real Node binding in `bindings/node` with `require('../bindings/node')`, creates a `ZeroKernel` rooted at the repository, and executes a single cell that:

- calls `z.read('README.md')` to exercise FSZero
- calls `z.find('launch_cell', { mode: 'word', path: 'crates/zerostack/zero-kernel/src/runtime.rs' })` to exercise GraphZero
- returns the combined result, whose output is automatically measured and projected by TokenZero inside ZeroKernel

The cell runs in a bounded frame with `initialize` and `shutdown` around a single `executeCell`. The script requires `outcome === "Completed"`, nonzero GraphZero hits, at least one exact `z://blob/` handle, and `status().liveTasks == 0` after the cell and after shutdown.

## Requirements

- Node.js 18 or newer
- A built native addon at `bindings/node/prebuilds/<platform>/zero_kernel_product.node`, or set `ZERO_KERNEL_NATIVE_ADDON` to an absolute path. Build it with `cargo build --profile release-node -p zero-kernel-node` and copy the resulting library into that prebuild path.

## Output

The script prints the cell result as JSON to stdout. TokenZero measurement and projection are internal to ZeroKernel; the printed JSON is the final visible output.
