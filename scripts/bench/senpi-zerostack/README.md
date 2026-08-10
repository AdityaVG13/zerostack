# Senpi versus ZeroStack paired runtime diagnostic

This harness measures one pinned Senpi JavaScript kernel against one exact
`zerostack-codemode-host` binary on the same native machine. It uses a matched
in-memory read delegate. It does not run a model, UI, or real engine. Its output
is always `diagnostic_incomplete` and `publishable:false`.

## Run

```sh
git clone https://github.com/code-yeongyu/senpi.git /tmp/senpi-ymp3
git -C /tmp/senpi-ymp3 checkout 6951fba7a7c86c29b0ad394c934a31680b8d3740
cd /tmp/senpi-ymp3
npm ci --ignore-scripts
env -u FORCE_COLOR -u NO_COLOR npm run build

git clone /path/to/ZeroStack /tmp/zerostack-ymp3-source
git -C /tmp/zerostack-ymp3-source checkout 8afee02429fbe97b267412fb50256744426bd224

cd /path/to/ZeroStack
python3 scripts/bench/senpi-zerostack/run.py \
  --senpi-root /tmp/senpi-ymp3 \
  --zerostack-root /tmp/zerostack-ymp3-source \
  --zerostack-host /path/to/zerostack-codemode-host \
  --zerostack-revision <full-immutable-revision> \
  --profile quick \
  --output /tmp/senpi-zerostack-quick.json
```

Use `--profile full` only on an idle native release host. It runs 1,000 measured
samples per arm and workload, 10,000 stress calls per arm, and a 30-minute idle
curve. The runner uses isolated HOME, XDG, store, cache, fixture, and temp roots.
It does not change user configuration.

## Frozen comparison

`identity.json` pins the default ZeroStack revision, Senpi revision, schedule,
output budget, fake delegate, timeouts, and workloads. Pass
`--zerostack-revision` to bind a run to another immutable hub revision.

The benchmark covers:

- no-op JavaScript
- one 1 KiB read delegate round trip
- sixteen 1 KiB read delegate round trips

The runner hashes every comparison-identity coordinate, both sources, the exact
ZeroStack binary, the runner, the Senpi driver, and the config. It retains every
integer nanosecond sample and normalized output digest. It samples process-tree
RSS, FDs, threads, CPU text, stress growth, idle state, and teardown.

## Claim boundary

Do not use this receipt to claim that either product is faster. The following
acceptance evidence is still missing:

- journaled snap-edit and verification
- cancellation
- 1 MiB output finalization under matched budgets
- independent nanosecond stages that close to outer time within 0.25 ms
- a signed ZeroStack binary-to-source-revision binding
- calibrated wakeup, idle CPU average, and idle CPU p99 evidence
- real raw-worker engine time
- model-visible and tool-card settlement

Senpi runs its persistent Node worker. ZeroStack runs its private aggregate host
with `__zero.host.call` and a matched fake delegate. This is a runtime transport
comparison, not product end-to-end evidence.
