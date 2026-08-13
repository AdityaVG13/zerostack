# Tests

Every test lives here. Crates under `crates/` are production code only.

```
tests/
  rust/                 crate integration tests + shared rust suite
    shared/             workspace-level rust tests
    zero-cert/          ...
    zero-codemode/
    ...
  python/               python tests
  benches/              criterion benches (invoked via cargo bench -p <crate>)
  zero-testkit/         shared test library (not a product crate)
  fixtures/             shared fixtures
  contracts/            schemas the suite validates
  scripts/              suite runners
  src/                  shared conformance harness crate
```

## Run

Python:

```sh
python3 -m unittest discover -s tests/python -t .
python3 tests/scripts/run_shared_suite.py --reference
```

Rust (targeted, via rch):

```sh
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
  cargo test -p zero-process --test child -- --test-threads=1
```

Unit tests that must see private items stay as `#[cfg(test)]` modules inside
the crate source. They are not a `tests/` folder in the crate.
