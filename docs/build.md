# Build and verify ZeroStack

ZeroStack is one product. The only program to build is `zero-kernel`. The files, structure, and tokens domains load behind that host; they are not separately installable products.

## Build

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build -p zero-kernel
```

ZeroKernel exposes:

| Domain | Adapter | Operations |
| --- | --- | --- |
| Files (`crates/fszero/`) | `zero-fs` | `z.read`, `z.edit`, `z.apply` |
| Structure (`crates/graphzero/`) | `zero-graph` | `z.find` |
| Tokens (`crates/tokenzero/`) | `zero-token` | automatic measurement and projection at operation and response boundaries |

Indexing for structure search happens inside `z.find`. Token measurement and exact recovery run through ZeroKernel handles, not a second product CLI.

## Store defaults

The structure domain defaults to a repository-local graph store. Shared family storage is explicit and project-scoped. A ref proves identity only when the process can reach and digest-verify the stored object.

## Verify a checkout

```bash
cargo run --manifest-path xtask/Cargo.toml -- doctor --json
cargo run --manifest-path xtask/Cargo.toml -- understand --check
python3 scripts/check_public_surface.py
cargo metadata --no-deps --format-version 1
cargo test -p <package> --test <target> <filter>
```

Use the narrowest package and an explicit `--lib`, `--bin`, or `--test` target. Full-workspace test runs are not the project gate.

Repository topology, contract rules, and contribution norms live in [CONTRIBUTING.md](../CONTRIBUTING.md).
