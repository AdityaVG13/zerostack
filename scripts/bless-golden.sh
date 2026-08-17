#!/usr/bin/env bash
# Operator-only golden recapture. Never invoked from CI.
set -euo pipefail

if [[ "${1:-}" != "--i-am-the-operator" ]]; then
  cat >&2 <<'EOF'
Refusing to bless goldens.

This script rewrites conformance/golden/ from live sources.
CI and automated agents must not recapture to make a red test green.

Operator invocation:
  scripts/bless-golden.sh --i-am-the-operator

Then run:
  python3 scripts/check_golden_integrity.py
  python3 scripts/check_feature_universe_weights.py
EOF
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
python3 scripts/capture_golden.py --write
python3 scripts/check_golden_integrity.py
echo "bless-golden: wrote conformance/golden/ and verified checksums/manifest/Tier-1 bytes"
