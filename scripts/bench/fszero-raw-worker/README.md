# FSZero raw-worker v2 warm latency gate

`run.py` is the acceptance harness for `zerostack-kw2p`. It starts one
persistent `fszero-codemode` process with explicit `--raw-worker --root` args,
performs a cold handshake, warms the process, then sends exactly 1,000 measured
`fs.read` calls by default. It measures the NDJSON child round trip, not
CodeMode, a model, or a model-visible response.

## Run on the native Spark

Use the source-bound artifact and source pins supplied by the build step:

```sh
python3 scripts/bench/fszero-raw-worker/run.py \
  --binary /path/to/source-bound/fszero-codemode \
  --source-head <40-hex-source-head> \
  --abi-digest <FSZero-semantic-contract-digest> \
  --protocol-digest e2daca4d95cbd2780f2e10b30b823e9398747bfe15e38ca0810f634a387aeace \
  --output /tmp/fszero-raw-worker.json
```

The explicit `--source-head` is required when the binary is copied from a
remote Spark. Omit `--source-root` for RCH artifacts: the remote checkout can
have a stale HEAD even when its source bytes were synced from the pinned local
head. The receipt records `root:null`, `dirty:null`, and marks this binding as
observational. For a local checkout, pass `--source-root`; a dirty checkout
fails the gate. The harness records the source head, binary SHA-256, platform,
exact argv, protocol/ABI digests, fixture, every trial, byte counts, stage
telemetry, and residual assumptions.

The gate requires RTT p50 <= 1 ms, RTT p95 <= 2 ms, engine p95 <= 1 ms, and
stage evidence closing within 0.25 ms. Missing stage evidence fails closed.
Handshake and shutdown are reported separately from measured calls. Every
read checks `abcdef` and its exact `fz://blob/<sha256>` ref. A missing-file
probe checks the typed `not_found` error and `retryable:false`.

## Mutant gate

The transport boundary supports a real injected delay. This command must exit
nonzero and write a `mutant/non-promotable` receipt:

```sh
python3 scripts/bench/fszero-raw-worker/run.py \
  --binary /path/to/source-bound/fszero-codemode \
  --source-head <40-hex-source-head> \
  --abi-digest <FSZero-semantic-contract-digest> \
  --inject-transport-delay-us 1000 \
  --output /tmp/fszero-raw-worker-mutant.json
```

`--abi-digest` can be replaced by `FSZERO_ABI_DIGEST`; absent both, the
harness probes `fszero-codemode capabilities --json`. The probe is metadata
only. The persistent benchmark worker remains the single raw-worker process.

Current run identities and results belong in bead notes or external receipts,
never in this README. Do not bake host paths, hostnames, or transient artifact
identities into repository instructions.

## Targeted tests

```sh
python3 -m unittest discover -s scripts/bench/fszero-raw-worker -p 'test_*.py' -v
python3 -m py_compile scripts/bench/fszero-raw-worker/run.py scripts/bench/fszero-raw-worker/test_run.py
```

Do not use this harness as a product or model latency claim. A passing receipt
is promotable only when the source-bound native artifact and all gates pass.
