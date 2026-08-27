# Development workflows

Commands are repository-relative and implemented by the standalone `xtask` crate.

```console
cargo xtask doctor
cargo xtask understand --check
cargo xtask test-targeted <package> [test-filter]
```

| Command | Purpose |
| --- | --- |
| `cargo xtask doctor [--json]` | Verify prerequisites and repository identity |
| `cargo xtask understand [--write|--check]` | Regenerate or validate `docs/knowledge/workspace.json` |
| `cargo xtask check` | Run formatting and type checks without tests |
| `cargo xtask test-targeted <package> [filter]` | Run one selected test |
| `cargo xtask bench <bench> [args]` | Run one selected benchmark |
| `cargo xtask docs` | Build API documentation |
| `cargo xtask release` | Build the standalone FSZero release artifact |

See profiling.md and [benchmark-integrity.md](benchmark-integrity.md) before publishing performance claims. Use focused test targets; ZeroKernel composition is verified from the hub.
