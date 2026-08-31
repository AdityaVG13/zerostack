# Development workflows

Commands are repository-relative and implemented by the standalone `xtask` crate. There is no `cargo xtask` alias; invoke the crate through its manifest.

```console
cargo run --manifest-path xtask/Cargo.toml -- doctor --json
cargo run --manifest-path xtask/Cargo.toml -- understand --check
cargo run --manifest-path xtask/Cargo.toml -- test-targeted <package> [test-filter]
```

| Command | Purpose |
| --- | --- |
| `doctor [--json]` | Verify toolchain, ZeroStack identity, and required layout files |
| `understand [--check]` | Print the layout inventory or confirm required paths exist |
| `check` | Run formatting and type checks without tests |
| `test-targeted <package> [filter]` | Run one selected test |
| `bench <bench> [args]` | Run one selected benchmark |
| `docs` | Build API documentation |
| `ci` | Run doctor and layout check only |

There is no xtask release recipe and no public package. `xtask release` exits with that fact.

See profiling.md and [benchmark-integrity.md](benchmark-integrity.md) before publishing performance claims. Use focused test targets; ZeroKernel composition is verified from the hub.
