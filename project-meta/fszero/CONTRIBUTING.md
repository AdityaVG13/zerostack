# Contributing to FSZero

FSZero is the byte and filesystem authority in the Zero family. Keep changes aligned with the current architecture and public contracts in `docs/`.

## Getting started

- Toolchain is pinned in `rust-toolchain.toml`. `rustup` will select it automatically.
- Development commands live in `docs/development.md` (`cargo xtask …`).
- Maintainers run CPU-heavy Cargo work through the configured remote build runner; contributors may use their normal Cargo target directory.

## Branch and review

- Base branch is `main`. Keep changes small and focused; avoid silent deletes or history rewrites.
- One logical change per commit. Include evidence for behavioral changes (commands run, test output).
- Do not import GraphZero or TokenZero. Depend only on hub contract crates, pinned by a pushed `origin/main` rev.

## Security issues

Do not file public issues for vulnerabilities. See `SECURITY.md`.
