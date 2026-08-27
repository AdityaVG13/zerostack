# GraphZero verify GitHub Action

Use `.github/actions/graphzero-verify` to gate pull request claims written as:

```text
GraphZero-Claim: no_remaining_callers old_symbol
GraphZero-Claim: symbol_removed Widget
```

The action checks out the repository, installs Rust stable by default, writes a non-empty PR body to the workspace-local `claims-file` path, runs `graphzero index`, runs `graphzero verify --claims-file`, uploads `graphzero-claim-report.json`, and updates one pull request comment marked with `<!-- graphzero-claim-report -->`. If `pr-body` is empty, the action scans an existing regular claims file. Action pins are full commit SHAs; refresh via Dependabot (see `docs/reproducibility.md`). There is no local mocked-`cargo` Rust dogfood test for the action YAML.

## Local dogfood workflow

The repository workflow at `.github/workflows/graphzero-claims.yml` runs the action on opened, edited, synchronized, and reopened pull requests:

```yaml
- uses: actions/checkout@v4
- uses: ./.github/actions/graphzero-verify
  with:
    pr-body: ${{ github.event.pull_request.body }}
    repo: .
```

## Inputs

| Input | Default | Purpose |
| --- | --- | --- |
| `claims-file` | `pr-body.md` | Workspace-local regular markdown file scanned for `GraphZero-Claim` lines. Symlinks and special files fail closed. |
| `pr-body` | empty | Pull request body text. When non-empty, it replaces `claims-file`; otherwise the existing file is scanned. |
| `repo` | `.` | Repository path passed to `graphzero index` and `graphzero verify`. |
| `comment` | `true` | Update the PR evidence comment when running on `pull_request`. |

## Outputs and artifacts

| Output | Meaning |
| --- | --- |
| `report-file` | Path to `graphzero-claim-report.json`. |
| `verified` | `true` only when every parsed claim verifies. |

The uploaded artifact contains the full JSON report, including each claim target, claim kind, verification result, and nested GraphZero evidence returned by the CLI.

## Action SHA pins and update cadence

Composite and workflow steps that run this action use **full commit SHA**
pins for third-party GitHub Actions (for example `actions/checkout`,
`actions/upload-artifact`, `actions/github-script`, and
`dtolnay/rust-toolchain` in `.github/actions/graphzero-verify/action.yml`
and `.github/workflows/*`). Do not revert those pins to mutable tags.

Pin refresh is automated by Dependabot (`.github/dependabot.yml`,
`package-ecosystem: github-actions`, weekly). Dependabot PRs bump the
SHA digests in place; review CI on those PRs and merge to stay current.
See `docs/reproducibility.md` (artifact and contract policy).
