# Install and build FSZero

FSZero currently builds from source. The repository pins its toolchain.

## Build the standalone CLI

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build --release -p fszero-cli
  # <!-- audit:skip --> binary is produced by the preceding cargo build
./target/release/fszero --help
```

The CLI is for FSZero-only diagnostics, batch operations, store inspection, worlds, history, and recovery maintenance. It is not a second model-facing planner.

## Use through ZeroKernel

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build -p zero-kernel
```

ZeroKernel loads `fszero-kernel` behind `z.read`, `z.edit`, and `z.apply`. Do not register retired FSZero MCP or engine-local CodeMode catalogs beside it.

## Store configuration

| Setting | Purpose |
| --- | --- |
| `FSZERO_ROOT` | Standalone workspace root |
| `FSZERO_ALLOW_EPHEMERAL=1` | Explicit disposable fallback when durable recovery cannot open |
| `FSZERO_SHARED_STORE=1` | Opt into a configured shared ZeroStack store |
| `ZEROSTACK_STORE_ROOT` | Shared root, honored only with explicit opt-in |

## Verify a checkout

```bash
cargo xtask doctor --json
cargo xtask understand --check
cargo test -p <package> <filter>
```

Repository-level quality and release gates run through DSR and RCH.
