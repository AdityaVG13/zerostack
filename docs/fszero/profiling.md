# FSZero profiling

Profile one named FSZero operation or benchmark scenario at a time. Performance claims follow [benchmark-integrity.md](benchmark-integrity.md).

## Build profile

Use the repository `release-perf` profile with frame pointers. Maintainers offload compilation through RCH:

```bash
rch exec -- env CARGO_TARGET_DIR="${RCH_TARGET_BASE}/rch_target_fszero_profile" \
  ./scripts/profile_build.sh -p fszero-cli --bin fszero
```

Record the exact commit, binary digest, profile, toolchain, host class, corpus, command, and sample count beside every promoted result.

## CPU profiling

Use a tool appropriate for the operating system:

- `samply` for portable profile capture;
- `perf` on Linux;
- Instruments or `/usr/bin/sample` on macOS.

Capture a named standalone CLI or benchmark-harness scenario. Do not profile retired MCP or engine-local CodeMode entrypoints.

```bash
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)-index-build
OUT_DIR=tests/artifacts/perf/${RUN_ID}
mkdir -p "$OUT_DIR"
samply record --save-only -o "$OUT_DIR/cpu.json" -- \
  ./target/release-perf/fszero --help
```

Replace the help probe with the actual named workload before using the output as evidence.

## Artifact locations

| Path | Role |
| --- | --- |
| `tests/artifacts/perf/<run-id>/` | Local scratch output; ignored by Git |
| `docs/evidence/perf/<run-id>/` | Explicitly promoted, clone-visible evidence |

A promoted package contains a fingerprint, scenario, compact profile artifact, hotspot table, and interpretation. Raw multi-megabyte traces stay local unless the owner approves their publication.

See [evidence/perf/README.md](evidence/perf/README.md) for promotion rules and [profiling-macos.md](profiling-macos.md) for macOS-specific tooling.
