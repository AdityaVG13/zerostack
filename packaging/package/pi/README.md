# @zerostack/pi

Private pre-release Pi package for ZeroStack. Version `0.0.0` is a scaffold. No native tools are registered yet.

## Install

From the repository root, install locally:

```bash
pi install ./packaging/package/pi
```

This registers the extension at `extensions/zerostack.ts` via `pi.extensions`.

## Command

Inside Pi, run:

```
/zerostack
```

The command reports pre-release scaffold status and points to the real ZeroKernel Node binding in `bindings/node` and the Rust crates under `crates/zerostack`.

## What this package does

- Registers a single `/zerostack` command that returns an honest status message.
- Does not register any native tools, does not start daemons, and does not download binaries.
- Declares `@earendil-works/pi-coding-agent` as a `*` peer dependency so Pi supplies the runtime API.

## Development

Edit `extensions/zerostack.ts` and reload with `/reload` in Pi. The extension uses `pi.registerCommand` and does not call `pi.registerTool`.
