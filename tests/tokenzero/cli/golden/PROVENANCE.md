# TokenZero CLI Golden Artifacts

These goldens freeze canonical public JSON envelopes emitted by the `tokenzero`
CLI. They were generated from deterministic local fixtures through:

```bash
UPDATE_GOLDENS=1 cargo test -p tokenzero --test golden_outputs
```

Dynamic temp paths, TokenZero blob/file/search refs, capture byte counts, and
command latency are scrubbed before comparison. Transient mismatch files use the
`.actual` extension and are ignored by the repository root `.gitignore`.

Adapter approval goldens pin `TOKENZERO_RELEASE_CANDIDATE_ID=golden-test` and
compare both stdout and the `--output-json` artifact to freeze the public report
shape without approving runnable competitor execution.

Completion audit and artifact handoff goldens use a compact tempdir
`results/current` fixture with release-candidate-bound evidence artifacts,
macOS-only OS residuals, runnable-adapter residuals, and publication/release
gates still blocked. The artifact handoff golden runs with an empty `PATH` so
installed-wrapper discovery is deterministic; temp roots and the local workspace
root are scrubbed before comparison.

Review workflow:

```bash
cargo test -p tokenzero --test golden_outputs
UPDATE_GOLDENS=1 cargo test -p tokenzero --test golden_outputs
git diff crates/tokenzero-cli/tests/golden/
```
