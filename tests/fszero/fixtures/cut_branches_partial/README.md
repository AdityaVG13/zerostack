# Partial cut-branches RESULT fixture (fszero-b4gk)

Validates that outfit-skills `cut-branches/scripts/validate_cut_branches.py`
accepts `CUT_BRANCHES_RESULT: partial` / `go_ahead: partial` without requiring
the full campaign scorecard (Scope 1000-point / Parity Evidence sections).

```bash
python3 ../outfit-skills/cut-branches/scripts/validate_cut_branches.py \
  tests/fixtures/cut_branches_partial/RESULT.md
# expect: mode=partial_light Validation score: 100/100
```
