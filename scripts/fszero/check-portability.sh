#!/usr/bin/env bash
# Single CI-runnable portability adapter for FSZero.
#
# Fails if tracked files carry environment-specific state: absolute host paths
# (/Users/<name>, /home/<name>, C:\\Users\\<name>), literal "~" path
# components that no shell will expand, or an unscrubbed beads export (br
# stamps source_repo_path with the author's absolute workspace path).
#
# Each local entrypoint delegates shared policy to the immutable ZeroStack
# revision pinned in Cargo.lock; this script only sequences those adapters.
#
# Run: scripts/check-portability.sh
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

status=0
for gate in \
  "scripts/check_no_host_paths.py" \
  "scripts/check_no_literal_tilde_paths.py" \
  "scripts/scrub_beads_export.py --check"; do
  echo "== $gate"
  # shellcheck disable=SC2086
  if ! python3 $gate; then
    status=1
  fi
done

if [ "$status" -ne 0 ]; then
  echo "portability gate failed: tracked files carry host-specific state" >&2
fi
exit "$status"
