# ZeroStack documentation

- [Architecture](architecture.md) -- system boundaries and cross-engine flow
- [ADR 0001: CodeMode execution boundary](adr/0001-codemode-execution-boundary.md) -- accepted one-runtime and raw-worker ownership contract
- [Release-N engine MCP compatibility policy](mcp-compatibility-policy.md) -- CodeMode defaults, compatibility maintenance, migration, and removal gates
- [RACC and typed refs](racc.md) -- recovery-aware context compression
- [ZeroRef v1 fragment policy](zeroref.md) -- canonical byte and line boundary behavior
- [CodeMode and MCP mode](codemode.md) -- deployment modes plus `zs` installation and smoke tests
- [Components](components.md) -- TokenZero, FSZero, and GraphZero
- [zero-abi UB/Miri canary](ub-runbook.md) -- package-scoped release and unsafe-surface runbook
