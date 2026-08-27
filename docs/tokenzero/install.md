# Install TokenZero

TokenZero is a released product. Install a verified release archive or build the standalone CLI from source.

## Release archive

Download the archive for your operating system from GitHub Releases, verify its checksum, and place `tokenzero` on `PATH`.

```bash
tokenzero install --global --plan --mcp --shell --cli --json
tokenzero install --global --apply --mcp --shell --cli --json
tokenzero doctor --json
```

Every apply records rollback data.

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

FSZero, GraphZero, and TokenZero will eventually publish the same version together. Until then, each repository reports only its current version.
