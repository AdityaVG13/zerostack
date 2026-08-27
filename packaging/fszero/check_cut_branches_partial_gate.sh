#!/usr/bin/env bash
# fszero-b4gk -- mode-aware partial RESULT gate (skill may live next to FSZero).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FIX="$ROOT/tests/fszero/fixtures/cut_branches_partial/RESULT.md"
CANDIDATES=(
  "$ROOT/../outfit-skills/cut-branches/scripts/validate_cut_branches.py"
  "$HOME/AI/outfit-skills/cut-branches/scripts/validate_cut_branches.py"
)
for s in "${CANDIDATES[@]}"; do
  if [[ -f "$s" ]]; then
    out="$(python3 "$s" "$FIX")"
    echo "$out"
    echo "$out" | grep -q 'partial_light\|mode=partial'
    echo "$out" | grep -Eq 'score: 1[0-9]{2}/100|score: [7-9][0-9]/100'
    echo "check_cut_branches_partial_gate: ok ($s)"
    exit 0
  fi
done
echo "check_cut_branches_partial_gate: skip (validator skill not found)" >&2
exit 0
