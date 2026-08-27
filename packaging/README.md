# Packaging

Unified distribution root for ZeroStack. All installable artifacts live under `packaging/package`.

## Layout

- `package/homebrew/zerostack.rb` - Homebrew formula for `zero-kernel`. Head only and pre-release. It builds `crates/zerostack/zero-kernel` from source with `cargo install --locked`. No release URL or checksum is claimed until a tagged release exists.
- `package/npm/` - Private operator scaffold for the real Node binding in `bindings/node` (`@zerostack/zero-kernel`). This directory does not duplicate `loader.js` or `zero-kernel.d.ts`. Use its scripts to validate and pack the real binding.
- `package/pi/` - Private `@zerostack/pi` package for the Pi coding agent. Install locally with `pi install ./packaging/package/pi`. Provides `/zerostack` command that reports pre-release scaffold status. No native tools are registered yet.

## Homebrew

```bash
brew install --HEAD --build-from-source packaging/package/homebrew/zerostack.rb
```

The formula depends on `rust` at build time and runs `cargo install --locked --path crates/zerostack/zero-kernel`.

## Npm operator

```bash
npm --prefix packaging/package/npm run validate
npm --prefix packaging/package/npm run pack
# or pack directly from the real source
npm pack ./bindings/node
```

`build-prebuild.sh` in `package/npm` builds the platform prebuild at `bindings/node/prebuilds/<platform>/zero_kernel_product.node`.

## Pi

```bash
pi install ./packaging/package/pi
```

Then inside Pi, run `/zerostack` to see scaffold status.

No distribution artifact downloads a prebuilt TokenZero or ZeroStack binary. All builds are from source in this repository.
