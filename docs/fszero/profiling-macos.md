# macOS profiling runbook (FSZero) -- CPU + Allocations

Primary host class for published FSZero perf work is **Apple Silicon macOS**
(M-series). SIP limits dtrace attach; use this ladder when collecting CPU
stacks against `release-perf` + frame-pointer binaries.

Policy binding: [benchmark-integrity.md](benchmark-integrity.md) (Profilable
release builds). Generic samply/flamegraph/dhat how-to: bead `fszero-lghz`.

## Preconditions

1. Build via the frame-pointer wrapper (never bare `cargo build --release`):

   ```bash
   rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_fszero_profile \
     ./scripts/profile_build.sh -p fs-zero --bin fszero
   # or: ./scripts/profile_build.sh -p fs-zero --bench perf_harness
   ```

2. Binary under `target/release-perf/` (or `$CARGO_TARGET_DIR/release-perf/`).
3. Name the DEFINE / harness scenario you are profiling (see
   `benchmarks/perf_harness.md` and sibling scenario cards). Do not attach
   orphan flames to unlabeled ad-hoc shells.

## Primary: samply (spawn)

Prefer **spawn** over attach so the process starts with frame pointers and no
codesign attach fight:

```bash
OUT="${FSZERO_PERF_OUT:-tests/artifacts/perf/scratch}/cpu.json.gz"
mkdir -p "$(dirname "$OUT")"
BIN="${FSZERO_BIN:-target/release-perf/fszero}"
# Example: cold codemode over a corpus root (replace with a named scenario).
samply record --save-only -o "$OUT" -- \
  "$BIN" codemode 'return{ok:true}' --root "$FSZERO_ROOT"
# Interactive inspect later:
#   samply load "$OUT"
```

Artifact type for hotspot citations: **`samply` / Firefox Profiler JSON**
(`.json` / `.json.gz`). Cite path + tool name in any hotspot table.

### SIP / codesign notes

- **Spawn (`samply record -- cmd …`)** is the supported path. The child is
  launched under the sampler; no attach entitlement is required for the
  usual release-perf binary.
- **Attach to a running PID** can fail under SIP or when the binary is
  hardened without get-task-allow. Prefer spawn. If attach is required, use
  a debug/dev build only for local diagnosis -- do not publish stacks from a
  different profile than the latency claim.
- Do not disable SIP for product measurements. If samply cannot record, fall
  back below; document the tool switch in the artifact.

## Fallback 1: `/usr/bin/sample` (text stacks)

When samply hangs, returns empty, or is unavailable, use the system sampler.
This is the path that produced successful committed-era cold-index stack
dumps on M5 Max (`Analysis Tool: /usr/bin/sample`).

```bash
OUT_DIR="${FSZERO_PERF_OUT:-tests/artifacts/perf/scratch}"
mkdir -p "$OUT_DIR"
BIN="${FSZERO_BIN:-target/release-perf/fszero}"
# Start the workload in the background; sample during the hot phase.
"$BIN" codemode 'return{ok:true}' --root "$FSZERO_ROOT" &
PID=$!
# durationSeconds samplingIntervalMs -- 1ms matches historical cold_sample.txt
/usr/bin/sample "$PID" 30 1 -file "$OUT_DIR/sample.txt"
wait "$PID" || true
```

Artifact type for hotspot citations: **`/usr/bin/sample` text**
(`sample.txt`). Headers should retain `Analysis Tool: /usr/bin/sample`.
Parse heavy frames by hand or with a small script; do not relabel these rows
as samply/Firefox Profiler output.

Scratch under `tests/artifacts/perf/` is gitignored. Promote flames via
[`docs/evidence/perf/`](evidence/perf/README.md) (`fszero-seju`).

## Fallback 2: xctrace / Instruments Time Profiler

```bash
OUT_DIR="${FSZERO_PERF_OUT:-tests/artifacts/perf/scratch}"
mkdir -p "$OUT_DIR"
BIN="${FSZERO_BIN:-target/release-perf/fszero}"
# Record a Time Profiler trace while the command runs (spawn).
xctrace record --template 'Time Profiler' \
  --output "$OUT_DIR/time.trace" \
  --launch -- "$BIN" codemode 'return{ok:true}' --root "$FSZERO_ROOT"
# Open in Instruments GUI, or:
#   xctrace export --input "$OUT_DIR/time.trace" --toc
```

GUI path: Instruments → Time Profiler → choose the release-perf binary →
record the same named scenario. Artifact type: **Instruments `.trace`**
(and any exported CSV/JSON from `xctrace export`). Cite `.trace` / export
path, not samply.


## Allocations: Instruments / xctrace (local attribution)

CPU ladders above do not show allocator ownership. On Darwin the practical
**local** alloc path is Instruments **Allocations** (or `xctrace` with the
Allocations template). Prefer this for multi-process / large-store idle
attribution on the primary M-series host. Deterministic CI JSON still belongs
to dhat/heaptrack when that feature lands (`fszero-lghz`); Instruments is
**not** a CI gate substitute.

### Preferred target scenarios

| Scenario | Why |
| :-- | :-- |
| Large-store open + idle (kflx) | Peak during open and residual after idle; shared store + mmap packs. |
| `perf_harness` named scenarios | Reproducible DEFINE cards (`benchmarks/perf_harness.md`). |
| `fszero` / `fszero-codemode` cold start | Product binary path agents actually run. |

Always build **`release-perf` + frame pointers** via `scripts/profile_build.sh`
before recording so stacks and alloc sites match published latency builds.

### CLI: xctrace Allocations

```bash
OUT_DIR="${FSZERO_PERF_OUT:-tests/artifacts/perf/scratch}"
mkdir -p "$OUT_DIR"
BIN="${FSZERO_BIN:-target/release-perf/fszero}"
# Named scenario example -- replace with harness or kflx large-store open.
xctrace record --template 'Allocations'   --output "$OUT_DIR/alloc.trace"   --launch --   env FSZERO_STARTUP_INDEX=1   "$BIN" codemode 'return{ok:true}' --root "${FSZERO_ROOT:-.}"
```

- Trace lands at `$OUT_DIR/alloc.trace` (Instruments package directory).
- Open in Instruments GUI → Allocations instrument → sort by **Persistent
  Bytes** / **Transient Bytes** as appropriate.
- Export top alloc stacks for hotspot rows: Instruments share/export, or
  `xctrace export --input "$OUT_DIR/alloc.trace" --toc` then pull the
  Allocations table. Cite artifact type **Instruments Allocations (`.trace`)**.

### GUI: Instruments Allocations

1. Open Instruments → choose **Allocations** template.
2. Choose target: the `release-perf` binary under `target/release-perf/`
   (or `$CARGO_TARGET_DIR/release-perf/`), not plain `release`.
3. Set arguments/env to the **same named scenario** as the latency claim
   (e.g. large-store open for kflx; a `perf_harness` scenario name).
4. Record through the hot phase (open + optional idle soak).
5. Capture top call trees (persistent) for the REPORT hotspot table; keep the
   `.trace` under `tests/artifacts/perf/<run-id>/` with fingerprint.

### How to read for FSZero

- **Persistent** growth across idle after open → retained maps/caches (store,
  packs, index sidecars).
- **Transient** spikes only during index/build → expected; do not gate on
  transient peak alone without a scenario card.
- Cross-check multi-process pressure with RSS vs PSS notes in
  [`profiling.md`](profiling.md#rss-vs-pss-honest-multi-process-memory)
  (`fszero-szws`): macOS has no smaps PSS; do not invent PSS from RSS.

### What not to claim

- Instruments Allocations output as a CI regression gate (use dhat JSON when
  available; until then local attribution only).
- Alloc stacks from a debug or plain `--release` binary as matching a
  `release-perf` latency claim.
- Heaptrack/dhat Linux recipes as if they were the Darwin default.

Bead: `fszero-wgdu`. Related: CPU fallbacks above (`fszero-act0`), generic
alloc feature (`fszero-lghz`), large-store RSS (`fszero-kflx.4`).

## Citing hotspots (required)

Every hotspot table or REPORT row must name the **tool artifact type**:

| Artifact | Cite as |
| :-- | :-- |
| `cpu.json` / `cpu.json.gz` from samply | `samply` / Firefox Profiler JSON |
| `/usr/bin/sample` dump | `/usr/bin/sample` text |
| Instruments / xctrace Time Profiler | Instruments Time Profiler (`.trace`) |
| Instruments / xctrace Allocations | Instruments Allocations (`.trace`) |

Never claim a samply attribution when the file is a `sample` text dump (and
vice versa). If a historical report mixed labels, correct the citation when
touching the artifact.


## Systems I/O (fs_usage)

For fsync / F_FULLFSYNC / rename **count + %time** on durability paths, see
[`profiling.md` Systems I/O / syscall sampler](profiling.md#systems-io--syscall-sampler-fs_usage-strace--c)
(`fszero-sys-io-syscall-runbook-4dc7`). Prefer `fs_usage` over `dtruss` under SIP.

## What not to do

- Publish stacks from `target/release` or plain `--release` builds.
- Use dtrace-based tools as the default on SIP-enabled macOS without a
  documented exception.
- Attach orphan profiles without a scenario card / harness scenario name.
