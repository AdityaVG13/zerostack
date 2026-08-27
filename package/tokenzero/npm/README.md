# @tokenzero/cli

Node launcher for the TokenZero Rust CLI.

This package does not embed the Rust binary. Install the Rust CLI first, then
use this wrapper when a Node/npm bin surface is convenient.

```bash
tokenzero --version
tokenzero doctor --json
```

Before crates.io publication, install the Rust CLI from the latest GitHub
Release:

```bash
# Download the archive for your OS from:
# https://github.com/AdityaVG13/tokenzero/releases
tokenzero --version
```

The canonical npm name is `@tokenzero/cli`. The unrelated unscoped
`tokenzero` package is not an installation source for this project. Registry
publication requires an authenticated scope check and explicit operator
approval.

On Windows, the wrapper can launch `tokenzero.exe`, `tokenzero.cmd`, or
`tokenzero.bat` from `PATH`. It avoids npm shim recursion and uses
`cmd.exe /D /C call` for batch launchers.

Set `TOKENZERO_BIN` to an explicit executable path when `tokenzero` is not on
`PATH`:

```bash
TOKENZERO_BIN=/path/to/tokenzero tokenzero --version
```
