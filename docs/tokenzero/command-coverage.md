# Command Coverage

# Command Coverage

TokenZero ships native parser capsules for these 45 measured command families:

- `git status`
- `git diff`
- `git log`
- `gh pr list`
- `gh pr view`
- `gh issue list`
- `gh issue view`
- `rg`
- `grep`
- `find`
- `ls`
- `tree`
- `cat`
- `head`
- `tail`
- `wc`
- `pytest`
- `unittest`
- `cargo test`
- `go test`
- `npm test`
- `pnpm test`
- `yarn test`
- `jest`
- `vitest`
- `playwright`
- `tsc`
- `eslint`
- `ruff`
- `mypy`
- `clippy`
- `golangci-lint`
- `docker ps`
- `docker logs`
- `docker compose`
- `kubectl get`
- `kubectl logs`
- `kubectl describe`
- `curl`
- `wget`
- generic logs
- JSON config output
- YAML config output
- TOML config output
- unknown shell commands

Capsules preserve exit code, stderr text, errors, assertions, tracebacks, failing tests, file paths, line numbers, warnings, changed files, diff hunks, and exact refs where present.

Shell renderer coverage is exact-first. Command-family auto policy chooses
diagnostic renderers for test, build, lint, Python, JS, Rust, and Go failures;
diff-aware renderers for `git diff`, `git show`, and patch-like output;
structured renderers for JSON, TAP/JUnit-like output, docker, and kubectl
status; dedupe renderers for repeated logs; and repo-inventory summaries for
marker-heavy `find`, `sort`, `wc -l`, `ls`, and `tree` inspection chains.

Command status truth is part of coverage: transport success is separate from
child command success, and missing paths or pipeline-masked failures expose
failed segments and exact refs.

Coverage is covered by:

```bash
cargo test --workspace
cargo run -- exact-recovery-shell --output-json results/current/exact_recovery_shell.json
cargo run -- harm-eval --output-json results/current/harm_eval.json
cargo run -- false-success-shell --output-json results/current/false_success_shell.json
cargo run -- repo-inventory --output-json results/current/repo_inventory.json
cargo run -- prompt-cache-pack --output-json results/current/prompt_cache_pack.json
```
