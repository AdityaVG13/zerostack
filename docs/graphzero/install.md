# Structure domain (used by ZeroStack)

The structure library is a ZeroStack domain surface, not an installable product.

The public execution surface is ZeroKernel (`z.find`).
The only installable program is ZeroStack (`zero-kernel`). Indexing happens inside ZeroKernel.

## Use GraphZero through ZeroKernel

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build -p zero-kernel
```

ZeroKernel loads `zero-graph` as the structure authority behind `z.find`.

## Store isolation

GraphZero defaults to a repository-local graph store. Shared family storage is explicit and project-scoped. A ref proves identity only when the process can reach and digest-verify the stored object.

## Focused verification

```bash
cargo metadata --no-deps --format-version 1
python3 scripts/check_public_surface.py
cargo test -p <package> --test <target> <filter>
```

Repository-level quality and release gates run through DSR and RCH. GitHub workflows are retained only as manual cross-platform specifications.
