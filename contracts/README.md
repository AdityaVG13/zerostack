# Contracts

Machine-readable contracts used by the composed ZeroStack product and its conformance tests.

Only the files listed below are normative. Domain-specific operation catalogs are not part of the ZeroKernel surface.

## Files

| File | Purpose |
| --- | --- |
| `zeroref-fixtures.json` | ZeroRef golden vectors for blob and fragment parsing and expansion |
| `SurfaceMatrix.toml` | GraphZero domain coverage matrix with live tests and documentation anchors |

## Usage

GraphZero binds its domain coverage to `SurfaceMatrix.toml`. ZeroRef fixtures are shared across FSZero, GraphZero, and TokenZero.

Do not edit these files without updating the related product tests and the `crates/zerostack/zerostack-conformance` suite.

## Change process

See `CONTRIBUTING.md` and `crates/zerostack/zerostack-conformance/CONTRACT.md` for the change process and invariants.
