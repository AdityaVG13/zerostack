# Changelog

All notable changes to GraphZero are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project follows semantic versioning after the initial public release.

## Unreleased

### Added

- Publish-time name-bigram sidecar (`GZNB` v1, `shards/name_bigram_*.bin`) so cold
  `GRAPHZERO_SEARCH_BIGRAM=1` loads densified postings instead of rebuilding via
  `OnceLock` (`graphzero-lrin`). Flag default remains off. Older binaries ignore
  the sidecar; missing/legacy snapshots fall back to in-process build.
- Explicit opt-in shareable telemetry permission (`GRAPHZERO_TELEMETRY`, `.graphzero/config.json`, `--telemetry` / `--no-telemetry`) with dry-run inspect (`graphzero telemetry inspect`), closed `graphzero.telemetry.v1` payload allowlist (`schema`, `version`, `raw_tokens`, `saved_tokens`), and truthful `exporter=none` (no upload path). Local CodeMode/query counters remain separate.

### Changed

- Bump `GZSV` semantic shards from v1 to v2 and append a SHA-256 digest over wires, vectors, and names. Current readers preserve v1 read-back as explicitly `LegacyUnverified`; older readers reject v2 as unsupported, so retain v1 artifacts for binary rollback.

## [0.1.0] - 2026-07-05

### Added

- Initial public release of GraphZero as a standalone code graph, causality, and decision memory engine.
- Code graph query surfaces for orient, blast, callers, and defs, with ref-first returns for compact agent handoffs.
- Causality and decision memory surfaces that keep why evidence beside graph navigation instead of treating it as prose outside the index.
- FastMCP catalog with ten lean tools for MCP clients, including graph navigation, verification, reservation, memory, and CodeMode entry points.
- CodeMode support with envelope v2 for plan-shaped graph work and small, reference-oriented responses.
- Per-user ref index with cross-process q: expansion so refs produced by one process can be reopened by another process for the same user.
- records_latest extraction reuse and warm re-index paths to avoid repeating unchanged extraction work.
- README command audit tooling to keep documented commands executable before release.
