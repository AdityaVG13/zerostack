# Files domain (used by ZeroStack)

The files/state library is a ZeroStack domain surface, not an installable product.

The public execution surface is ZeroKernel (`z.read`, `z.edit`, `z.apply`).
The only installable program is ZeroStack (`zero-kernel`).

## Use through ZeroKernel

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build -p zero-kernel
```

ZeroKernel loads `zero-fs` behind `z.read`, `z.edit`, and `z.apply`.

## Verify a checkout

```bash
cargo run --manifest-path xtask/Cargo.toml -- doctor --json
cargo run --manifest-path xtask/Cargo.toml -- understand --check
cargo test -p <package> --test <target> <filter>
```

Repository-level quality and release gates run through DSR and RCH.
