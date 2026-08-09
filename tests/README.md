# ZeroStack shared conformance suite

`tests/` is the canonical, harness-neutral shared suite. It contains active
contract schemas, fixtures, executable checks, adapter descriptors, and the
budget gate. `conformance/models/` remains evidence-only; old conformance and
`zero-testkit` paths remain as compatibility sources until explicit deletion
approval.

## Run the suite

Reference adapter, without Pi:

```sh
python3 tests/scripts/run_shared_suite.py --reference
```

One selected engine, with an explicit CodeMode binary:

```sh
python3 tests/scripts/run_shared_suite.py fszero --fszero-bin /path/to/fszero-codemode
```

All configured engines plus the reference adapter:

```sh
python3 tests/scripts/run_shared_suite.py --all \
  --fszero-bin /path/to/fszero-codemode \
  --graphzero-bin /path/to/graphzero-codemode \
  --tokenzero-bin /path/to/tokenzero-codemode
```

The engine commands delegate to `zerostack-shared-conformance` (override with
`ZEROSTACK_CONFORMANCE_BIN`). No engine crate or Pi package is imported.

## Budget check

```sh
python3 tests/scripts/check_budget.py --self-test
```

This proves exactly 50 libtest registrations pass and an injected 51st fails.
