# Git Dependency Pins

Git dependencies require explicit ownership because their revisions are not covered by the normal crates.io advisory matching performed by `cargo audit`.

## `zero-codemode`

- **Owner:** GraphZero repository maintainers.
- **Repository:** <https://github.com/AdityaVG13/zerostack>
- **Pinned revision:** `fa253840910ab4051635e2de95f04ddf6043a000`.
- **Reason for the pin:** the local crates.io sparse-index cache lists `zero-codemode 0.3.2`, matching the version recorded for the pinned dependency in `Cargo.lock`. However, no `0.3.2` crate archive or extracted registry source was present locally, so the published package could not be verified as content-identical to the pinned revision without fetching it. The git pin is retained to preserve the source currently built and tested by GraphZero. This is not a claim that the crates.io release is incompatible.
- **Audit cadence:** once per GraphZero release, and immediately before any revision update. The owner manually reviews the upstream diff from `1c66097b15dea9550e833ee9e433beeb66eefece..HEAD`, including dependency, build-script, unsafe-code, protocol, and transport changes. Record the reviewed target revision in the release review or pull request.
- **Risk:** git dependencies bypass crates.io advisory matching in `cargo audit`; a vulnerable or compromised pinned revision may therefore receive no automated advisory finding. Manual source review and focused verification are required until this dependency is migrated to a verified crates.io release.

### Update procedure

1. Review the full upstream diff from the currently pinned revision to the proposed commit and confirm that the commit belongs to the expected upstream repository.
2. Update the `rev` in `crates/graphzero-cli/Cargo.toml` and refresh the corresponding `Cargo.lock` entry. Update the pinned revision and diff base in this document in the same change.
3. Run the guarded package check:

   `ZEROSTACK_GUARD_REPOSITORY=GraphZero CARGO_BUILD_JOBS=1 /bin/sh /tmp/zerostack-guarded-run.sh cargo check -p graphzero-cli`

4. Run the focused MCP integration tests:

   `ZEROSTACK_GUARD_REPOSITORY=GraphZero CARGO_BUILD_JOBS=1 /bin/sh /tmp/zerostack-guarded-run.sh cargo test -p graphzero-cli --test mcp_stdio --test mcp_parity -- --test-threads=1`

5. Review lockfile changes for unexpected transitive dependency or source changes before merging.

## ZeroStack hub crates (`zero-abi`, `zero-ref`, `zero-codemode`)

- **Owner:** GraphZero repository maintainers (consume published/pinned hub crates; do not fork ABI contracts).
- **Repository:** <https://github.com/AdityaVG13/zerostack>
- **Pinned revision:** `fa253840910ab4051635e2de95f04ddf6043a000` for `zero-abi`, `zero-ref`, and `zero-codemode`.
  - `zero-abi` and `zero-ref` are pinned in the workspace `Cargo.toml`.
  - `zero-codemode` is consumed by `graphzero-mcp-compat`.
  - `Cargo.lock` records the same revision for all hub crates.
- **Reason for the pin:** GraphZero consumes one coordinated ZeroStack hub revision rather than mixing contract generations or inventing a second ABI/store/codemode copy.
- **License honesty:** hub crates must stay within the permissive licenses listed in `deny.toml` `[licenses].allow`. If a hub revision introduces a non-allowlisted license, fail `cargo deny` and either upgrade the allowlist with an explicit review note or refuse the revision.
- **Audit cadence:** once per GraphZero release, and immediately before any revision bump. Review the hub diff for the touched crate path only; record the reviewed revision in the release review or PR.
- **Risk:** same as other git dependencies — `cargo audit` does not match advisories the same way as crates.io. Manual source review is required on bumps.

### Update procedure

1. Confirm the target commit is on `AdityaVG13/zerostack` and review the crate-path diff.
2. Update the matching `rev` in workspace/`crates/*/Cargo.toml` and refresh `Cargo.lock`.
3. Update the pinned revision list in this section in the same change.
4. Run `cargo deny check` (once `cargo-deny` is installed) and focused GraphZero tests that exercise the bumped crate.
