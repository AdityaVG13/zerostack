# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| main | yes |
| older tags | no |

ZeroStack is pre-1.0; only the current `main` branch receives security fixes.

## Reporting a vulnerability

Do not open a public issue for security reports. Use GitHub's private
vulnerability reporting on this repository, or contact the maintainers
directly (see repository owner profile).

Include: affected component (`zero-kernel`, `zero-codemode`, `zero-store`,
`zero-process`, `zero-abi`), a minimal reproduction, and the impact you
believe it enables.

## Scope notes

- The CodeMode guest interpreter is a restricted JavaScript subset. Escape
  sequences, sandbox escapes, or resource-limit bypasses in it are in scope.
- `VerifiedChild` process ownership (spawn identity, tree teardown, exit
  observation) is security-critical; races or orphaning are in scope.
- Path confinement (`OutsideWorkspace`) and CAS digest handling are in scope.

## Disclosure

We fix first, publish an advisory with the release, and credit reporters by
default.
