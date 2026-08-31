# Tokens domain (used by ZeroStack)

The token library is a ZeroStack domain surface, not an installable product.

The public execution surface is ZeroKernel. Measurement and recovery run through ZeroKernel (`z.read` handles). The only installable program is ZeroStack (`zero-kernel`).

## Use through ZeroKernel

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build -p zero-kernel
```

## Coordinated releases

FSZero, GraphZero, and TokenZero will adopt coordinated version parity when joint tagged releases begin. Until then, this workspace is the only source of truth.
