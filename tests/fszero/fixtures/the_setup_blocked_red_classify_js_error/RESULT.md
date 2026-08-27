# the-setup RESULT -- classify_js_error red-on-OLD (fszero-ccn4)

THE_SETUP_RESULT: blocked_red
status: MIXED
mode: residual honesty packet (not green-by-editing-test theater)

## Expected

1. Freeze hash of OLD body of `classify_js_error` / error classification path.
2. Red test fails on OLD binary/body for a named contract reason.
3. Green from production change alone (current body) without editing the test.

## Actual

- Cut-branches craft reduced `classify_js_error` mid-eval (CC ~20 → thin wrap to
  `host::classify_error` at `src/codemode/js.rs`).
- Freeze artifact path from craft epoch is absent from this tree (`.the-setup/` not
  checked in); cyclomatic card remains at
  `.cyclomatic-reduction/runs/20260717T064358Z-fszero/04-analysis-cards/classify_js_error.md`.
- Re-running red-on-OLD against the **pre-transform** body was not re-executed in
  the same craft window. Closing as **blocked_red / MIXED** is honest; claiming
  full RED→green theater-free craft would be false.

## Current product pin (green path only)

- Classification lives in `src/codemode/host.rs` `classify_error`.
- Unit pins include `classify_error_maps_permit_io_to_non_retryable_substrate` and
  related host tests -- contract-bearing for current body, **not** a red-on-OLD proof.

## Re-prove recipe (future worker; optional close upgrade)

```bash
# 1) Capture OLD body from pre-reduction commit (example: cyclomatic craft epoch)
git show <pre-cut-sha>:src/codemode/js.rs > /tmp/js_old.rs
# 2) Author failing test against OLD classification table (named bug class)
# 3) Record freeze hash of OLD body
# 4) Restore current tree; prove green without editing the test
# 5) Store red_command/green_command under this fixture dir
```

Do **not** mark full the-setup green until steps 2–4 complete.
