# Install TokenZero

TokenZero currently builds from source in this workspace. There is no tagged TokenZero or ZeroStack release, and no GitHub Release archive or checksum to verify.

## Build from source

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build --release -p tokenzero-cli --bin tokenzero
./target/release/tokenzero --help
```

## Use through ZeroKernel

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build -p zero-kernel
```

Do not register classic TokenZero MCP beside ZeroKernel in the same agent session unless a client has a specific compatibility requirement.

## Coordinated releases

FSZero, GraphZero, and TokenZero will adopt coordinated version parity when joint tagged releases begin. Until then, this workspace is the only source of truth.
