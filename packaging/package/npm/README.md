# Npm operator for @zerostack/zero-kernel

Private package at `packaging/package/npm`. It does not publish `@zerostack/zero-kernel` and does not duplicate `loader.js` or `zero-kernel.d.ts`. The real binding lives only in `bindings/node`.

## Validate the real binding

```bash
npm --prefix packaging/package/npm run validate
node packaging/package/npm/validate.js
```

The validator checks that `bindings/node/package.json` is `@zerostack/zero-kernel`, that `bindings/node/loader.js` and `bindings/node/zero-kernel.d.ts` exist, and reports whether any `bindings/node/prebuilds` are present.

## Pack the real binding

```bash
npm --prefix packaging/package/npm run pack
# equivalent to
npm pack ./bindings/node
```

This creates `zerostack-zero-kernel-*.tgz` from `bindings/node` without copying files into `packaging/package/npm`.

## Build a platform prebuild

```bash
packaging/package/npm/build-prebuild.sh
```

Builds `crates/zerostack/zero-kernel-node` with `cargo build --profile release-node -p zero-kernel-node` and installs the artifact to `bindings/node/prebuilds/<platform>/zero_kernel_product.node` with a 50 MB size budget. Pass `--stage-only` to skip the cargo build and only stage an existing `target/release-node` artifact.

Repository root is resolved as three directories above this package, so the destination remains `bindings/node/prebuilds` at the repo root.
