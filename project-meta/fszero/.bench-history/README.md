# Benchmark history

This directory is the committed input to FSZero performance ratchets. `schema.json` defines the common record envelope. Records keep source artifacts, host class, build profile, thresholds, observed status, and statistical limits without rewriting measured evidence.

Current records:

- `surface-baseline.json` is measured release-profile evidence from the configured macOS/aarch64 RCH runner. It is `tracked_fail`: both current CodeMode-to-FastMCP ratios exceed the unchanged 2x limit. It is not Linux evidence.
- `durable-open-tracked-fail.json` indexes the five-run FrankenSQLite 0.1.15 artifact. The 3.0 size-ratio and 64 MiB incremental-RSS limits remain unchanged. Its operating system was not recorded, so this history makes no platform claim.
- `cold-index-100k-tracked-fail.json` retains the historical 30.579s median against the unchanged 5s target.
- `cold-index-100k-recovery.json` retains the later observed 4.944s median. It remains `blocked`, not `pass`, because its two trials do not satisfy the 20-trial publication floor.

Ratchets compare only compatible runner classes and profiles. A new passing run may replace a prior pass only if it does not regress it. A threshold change requires a committed decision. A failing run never becomes a pass by rebasing the limit. Historical small-sample records stay visible with conservative exceptions and explicit removal conditions.
