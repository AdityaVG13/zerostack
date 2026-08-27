#!/usr/bin/env bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$REPO_ROOT" || exit 2

findings=0
available=0
skipped=0

run_scanner() {
    local name="$1"
    shift
    if command -v "$name" >/dev/null 2>&1; then
        available=$((available + 1))
        printf '[RUN ] %s\n' "$name"
        "$@"
        local status=$?
        if (( status == 0 )); then
            printf '[PASS] %s\n' "$name"
        else
            findings=$((findings + 1))
            printf '[FAIL] %s (exit %d)\n' "$name" "$status"
        fi
    else
        skipped=$((skipped + 1))
        printf '[SKIP] %s (not installed)\n' "$name"
    fi
}

run_scanner cargo-audit cargo-audit audit
run_scanner cargo-deny cargo-deny check
run_scanner gitleaks gitleaks detect --source "$REPO_ROOT" --no-banner --redact

if command -v python3 >/dev/null 2>&1 && command -v git >/dev/null 2>&1; then
    available=$((available + 1))
    printf '[RUN ] secret-keyword-ledger\n'
    status=0
    python3 "$SCRIPT_DIR/check_secret_keyword_ledger.py" || status=$?
    if (( status == 0 )); then
        printf '[PASS] secret-keyword-ledger\n'
    else
        findings=$((findings + 1))
        printf '[FAIL] secret-keyword-ledger (exit %d)\n' "$status"
    fi
else
    skipped=$((skipped + 1))
    printf '[SKIP] secret-keyword-ledger (python3 or git not installed)\n'
fi

printf 'Security scan summary: %d scanner(s) available, %d skipped, %d failed\n' "$available" "$skipped" "$findings"
if (( available == 0 )); then
    printf '[FAIL] no scanner is available\n'
    exit 2
fi
(( findings == 0 )) || exit 1
exit 0
