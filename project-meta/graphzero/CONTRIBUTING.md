# Contributing to GraphZero

Thanks for considering a contribution. GraphZero is the structure and truth authority in the ZeroStack system.

## Getting started

- Toolchain is pinned in `rust-toolchain.toml` (nightly-2026-05-31). `rustup` will select it automatically.
- Clone and build: `cargo build --release`
- Before opening a PR, run focused checks for the changed scope:
  - `cargo fmt --all -- --check`
  - The targeted crate or test that covers the change
  - Relevant gates in `docs/benchmarks.md` for query or index changes
- Use `CARGO_TARGET_DIR=/tmp/rch_target_graphzero` for Rust builds and tests. Do not create per-task target directories.

## Branch and review

- Base branch is `main`. Keep changes small and focused; avoid silent deletes or history rewrites.
- One logical change per PR. Include evidence for behavioral changes (commands run, test output).
- Maintainer-tracked work uses the repository issue tracker and its recorded acceptance evidence.

## Reporting bugs and requesting features

- Use GitHub Issues. Include reproduction steps, expected vs actual behavior, and relevant `graphzero` / `rustc` versions.

## Security issues

Do not file public issues for vulnerabilities. See `SECURITY.md`.

## Code of conduct

Be respectful and constructive. Maintainer is @AdityaVG13 (see `.github/CODEOWNERS`).
