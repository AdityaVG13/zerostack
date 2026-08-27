# Profiling GraphZero on macOS

macOS samples are local diagnostic evidence. The host-timed CI gates currently
run on Linux only; do not present a Darwin sample as CI or cross-platform proof
unless a real macOS gate is added.

## Build the exact sampler binary

Build inside RCH with line tables and frame pointers:

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero_profile ./scripts/build-profilable.sh -p graphzero-cli --bin graphzero
```

The binary is
`/tmp/rch_target_graphzero_profile/release-perf/graphzero`. Record its SHA-256,
full `RUSTFLAGS`, host, isolation, corpus digest, and command. This
`release-perf+frame-pointers` derivative is not ordinary `release-perf` latency
evidence.

Prime the same corpus/store outside the timed sample. Then launch a single warm
request under one sampler:

```bash
samply record /tmp/rch_target_graphzero_profile/release-perf/graphzero orient --surface symbol --name run_index --budget 1 --repo .
samply record /tmp/rch_target_graphzero_profile/release-perf/graphzero blast --intent 'change signature of run_index' --budget 1 --repo .

xcrun xctrace record --template 'Time Profiler' --output /tmp/graphzero-orient.trace --launch -- /tmp/rch_target_graphzero_profile/release-perf/graphzero orient --surface symbol --name run_index --budget 1 --repo .
```

Use one command and one named scenario from
[benchmark-scenarios.md](benchmark-scenarios.md) per capture. Sampling overhead
invalidates latency quantiles; use the profile to attribute stacks, not to mint
a benchmark number.

## MCP and CodeMode

MCP and CodeMode are stdio servers. Build their mutually exclusive artifacts
with the same wrapper:

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero_profile ./scripts/build-profilable.sh -p graphzero-cli --bin graphzero-mcp --no-default-features --features tokenzero,surface-mcp
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero_profile ./scripts/build-profilable.sh -p graphzero-cli --bin graphzero-codemode --no-default-features --features tokenzero,surface-codemode
```

Launch the exact server process, then attach by PID (`samply record --pid <pid>`
or `xcrun xctrace record --template 'Time Profiler' --attach <pid>`). Keep the client and server PIDs distinct in the receipt. Do not
attribute client JSON/pipe time to the server. For one-call attribution, start
the sampler before sending the request and stop it after the response.

## macOS caveats

- Prefer sampler launch mode. SIP and hardened-runtime permissions can block
  attach; do not disable SIP. If macOS prompts, grant the sampler's normal
  Developer Tools permission and rerun.
- Do not ad-hoc codesign, strip, or rewrite the measured binary after hashing it.
- `[unknown]`-heavy stacks usually mean the wrong binary/profile or missing line
  tables/frame pointers. Rebuild; do not guess attribution.
- Record Apple silicon versus Intel, macOS version, filesystem, thermal/power
  state, and other active benchmark processes.
