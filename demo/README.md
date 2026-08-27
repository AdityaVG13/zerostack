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

No download of prebuilt binaries, no visualization scripts, and no TokenZero-only tooling is involved. The cell runs in a bounded frame with `initialize` and `shutdown` around a single `executeCell`.

## Requirements

- Node.js 18 or newer
- A built prebuild at `bindings/node/prebuilds/<platform>/zero_kernel_product.node`, or set `ZERO_KERNEL_NATIVE_ADDON` to an absolute path. Build it with `packaging/package/npm/build-prebuild.sh`.

## Output

The script prints the cell result as JSON to stdout. TokenZero measurement and projection are internal to ZeroKernel; the printed JSON is the final visible output.
