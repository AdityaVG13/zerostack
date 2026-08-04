# ADR 0002: Session-scoped aggregate sidecar

Status: Partial implementation. The tracked Rust slice uses one private mode-0700 runtime directory, a mode-0600 Unix socket, capability authentication, canonical-root checks, bounded 1 MiB NDJSON, and PID/start-identity owner death through Linux pidfd or macOS kqueue. `zsx exec -C ROOT` connects to an inherited session or creates a one-shot private session.

The current connector is an explicit fixture used to prove lifecycle and transport. Raw-worker ownership, capability probing, production multi-surface lowering, Windows named pipes/Job Objects, process-tree CPU/RSS enforcement, 30-minute idle soak, 10,000-call growth, cross-client parity, durable restart expansion, and p50/p95 gates remain unmet. This slice must not be presented as full q6am.
