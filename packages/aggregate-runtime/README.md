# @zerostack/aggregate-runtime

Public artifact for the ZeroStack aggregate runtime and substrate helpers.

Before this package, harnesses had to point `runtime_module` at a pi-stack
checkout's `raw-runtime.js` and `substrate_module` at
`router/lib/substrates.js`, then hand-write a bridge that dynamically imported
them and probed for `createAggregateRuntimeBridge` / `createSubstrateHelpers`.
That made a private cross-repo checkout into a public API (zerostack-x99l), and
it is why a second harness saw `aggregate_runtime_required` for any mixed
fs+token plan (zerostack-0pgp).

## Install

```sh
npm install /path/to/zerostack-aggregate-runtime-0.1.0.tgz
```

Build the tarball with `node scripts/build-aggregate-runtime.mjs` from the
ZeroStack repo root; it writes to `dist/`.

## Use

```js
import {
  createAggregateRuntimeBridge,
  createSubstrateHelpers,
  AGGREGATE_RUNTIME_CONTRACT,
} from "@zerostack/aggregate-runtime";
```

Subpath imports are also stable:

```js
import { RawWorkerSupervisor } from "@zerostack/aggregate-runtime/raw-runtime";
import { createSubstrateHelpers } from "@zerostack/aggregate-runtime/substrates";
```

Harness config points at the installed package, not a source checkout:

```yaml
runtime_module: "@zerostack/aggregate-runtime/raw-runtime"
substrate_module: "@zerostack/aggregate-runtime/substrates"
```

## Export contract

`AGGREGATE_RUNTIME_CONTRACT` names every symbol this package guarantees, so a
harness can assert compatibility at load time rather than probing. Adding a
symbol is a minor bump; removing one or changing a signature is a major bump.

## Provenance

`src/` is vendored from the pi-stack `pi-zerostack` package. The only edit is
rewriting `substrates.js`'s `../../shared/paths.js` import to the co-located
`./paths.js`. `createAggregateRuntimeBridge(pi)` still accepts a host object;
it reads only `pi.events`, so any harness can supply one.
