# Reproducibility & provenance — as-built design (EPIC graphzero-consolidated-reproducibility-provenance-lend)

Release credibility = versioned schemas + deterministic runs + benchmark
provenance + artifact policy, working together. This annex names the as-built
pieces and their gates.

## Versioned schemas

Every committed machine-readable artifact declares identity: `schema` /
`schema_version` / `$schema`. Enforced by
`crates/graphzero-cli/tests/artifact_schema_sweep.rs` over `benchmarks/` and
`benchmarks/latency/`. Current schema ids include `gold-edge-accuracy/v1`,
`bench-results/v1`, `prevented-read-bakeoff/v1`, `zeroref-golden-vectors`,
`zeroref-capability/v1`, `zeroref-fixture/v1`, `gz-snap/v1`, and
`schema_version` markers on CLI/agent JSON surfaces.

## Deterministic, regenerable artifacts

- Local rotation snapshots (`.rotation/snapshot.json`) use the contract in
  `scripts/rotation_snapshot_sha.py`: SHA-256 over compact UTF-8 JSON with
  recursively sorted object keys, every root field except `snapshot_sha256`,
  array order preserved, and no trailing newline. `created_at` is included.
  Duplicate keys, floats, non-i64 integers, and non-lowercase digests fail
  closed. Verify a retained snapshot with
  `uv run python scripts/rotation_snapshot_sha.py .rotation/snapshot.json`.
- `benchmarks/gold/edge_accuracy_report.json` — committed historical
  measurement. Live gold-row/schema gate is
  `tests/cli/gold_edge_validation.rs`. The old
  `gold_edge_accuracy_metrics` scorer was removed (Track B fluff / frozen
  scorer); do not cite it as a current command.
- `benchmarks/real_task_eval/report.json` — `run.py --write/--check`.
- `benchmarks/prevented_read_bakeoff/report.json` — deterministic byte
  metrics; `--check` requires exact reproduction.
- `benchmarks/rebaseline/` — northstar latency baseline with history
  (`history.jsonl`, retained-run gate) and a corpus digest binding every
  Rust file, so silent tree drift fails the gate.
- `benchmarks/impact_bakeoff/report.json` — freshness digests over gold
  inputs.

## Benchmark provenance

Reports pin corpus commit and hardware where wall-clock claims are made
(rebaseline, prevented-read bake-off), record the exact commands executed
(bake-off arms), and default to release binaries
(`benchmarks/single_repo_blast.sh`, one repository only). The legacy
`benchmarks/org_wide_blast.sh` name is a compatibility alias, not an org-scale
benchmark. README command examples are audited by
  `python3 scripts/readme_command_audit.py` (and `scripts/readme_benchmark_audit.py`).

## Artifact & contract policy

- Reference contracts live in `docs/contracts/` (ZeroRef fixtures,
  capability fixtures, fixture-CLI schema, CLI exit codes, README command
  manifest).
- CLI exit codes are centralized in `docs/contracts/cli-exit-codes.md`.
- GitHub Actions are pinned to full commit SHAs in `.github/workflows`
  and `.github/actions/graphzero-verify/action.yml`. Do not unpin to
  mutable tags. Pin refresh is Dependabot weekly (`github-actions`
  ecosystem), not a Rust source-scan test.
- **SHA pin refresh cadence:** `.github/dependabot.yml` schedules weekly
  `github-actions` updates so Dependabot opens PRs that advance existing
  full-SHA pins (checkout, upload-artifact, github-script, dtolnay
  toolchain, caches, etc.). Review and merge those PRs; keep pins as
  full commit SHAs. See also `docs/graphzero-verify-action.md`.
- Wall-clock perf gates re-measure boundedly under load; making them
  load-robust is tracked (graphzero-zush).

## Known residual work (tracked)

- graphzero-zush — load-robust wall-clock perf gates.
- Residual math-audit invariant verification (tier-B merge clones,
  branch-to-shard integrity, why-chain freshness ordering, why-edge confidence
  docs, single-pass extraction dispatch) — see the triage bead created at epic
  close. Rotation snapshot SHA now has a checked writer/verifier contract.
