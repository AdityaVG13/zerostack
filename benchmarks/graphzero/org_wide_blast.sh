#!/usr/bin/env bash
# Legacy compatibility alias. The canonical harness measures one repository
# only and lives at benchmarks/single_repo_blast.sh.
set -euo pipefail
exec "$(dirname "$0")/single_repo_blast.sh" "$@"
