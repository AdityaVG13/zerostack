#!/usr/bin/env bash
# Reclaim stale build artifacts across the ZeroStack family.
#
# Long agent sessions build all four repos repeatedly; cargo target/ dirs do not
# shrink on their own and reached 129G here (TokenZero 39G, FSZero 31G, GraphZero
# 25G, plus a 16G origin-main worktree). Running out of disk mid-session corrupts
# whatever was mid-write -- zerostack-e3g exists because three independent
# store.sqlite3 files went malformed on one machine.
#
# Default is a DRY RUN. It only ever reports; pass --apply to delete.
#
#   scripts/sweep_build_artifacts.sh                # report
#   scripts/sweep_build_artifacts.sh --apply        # reclaim
#   scripts/sweep_build_artifacts.sh --apply --days 3
#
# Only artifact directories are touched. Never source, never .beads, never a
# store: a store is state an engine may still own, and deleting one is the
# failure this script exists to prevent, not a way to reclaim space.
set -euo pipefail

APPLY=0
DAYS=7
while [ $# -gt 0 ]; do
  case "$1" in
    --apply) APPLY=1; shift ;;
    --days) DAYS="${2:?--days needs a value}"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

REPOS=(/Users/aditya/AI/ZeroStack /Users/aditya/AI/TokenZero /Users/aditya/AI/FSZero /Users/aditya/AI/GraphZero)

human() { du -sh "$1" 2>/dev/null | cut -f1; }

echo "disk before:"
df -h /System/Volumes/Data | tail -1

total_targets=()
for repo in "${REPOS[@]}"; do
  [ -d "$repo/target" ] || continue
  # `cargo clean -p` is not used: it needs a resolvable workspace, and several of
  # these repos currently fail to build standalone (fszero-ixka, graphzero-ml4r),
  # which would make the sweep fail exactly when it is most needed.
  for profile in debug release; do
    dir="$repo/target/$profile"
    [ -d "$dir" ] || continue
    if [ -z "$(find "$dir" -maxdepth 0 -mtime +"$DAYS" 2>/dev/null)" ]; then
      echo "keep  $(printf '%5s' "$(human "$dir")")  $dir (touched within ${DAYS}d)"
      continue
    fi
    echo "STALE $(printf '%5s' "$(human "$dir")")  $dir"
    total_targets+=("$dir")
  done
done

# rch stages remote builds into per-repo target dirs under TMPDIR. These are pure
# caches by construction -- rch re-syncs them -- so age is the only question.
while IFS= read -r dir; do
  [ -n "$dir" ] || continue
  echo "STALE $(printf '%5s' "$(human "$dir")")  $dir (rch scratch)"
  total_targets+=("$dir")
done < <(find "${TMPDIR:-/tmp}" -maxdepth 1 -type d -name 'rch_target_*' -mtime +"$DAYS" 2>/dev/null)

if [ "${#total_targets[@]}" -eq 0 ]; then
  echo "nothing stale (>${DAYS}d)"
  exit 0
fi

if [ "$APPLY" -eq 0 ]; then
  echo
  echo "DRY RUN: ${#total_targets[@]} dir(s) would be removed. Re-run with --apply."
  exit 0
fi

for dir in "${total_targets[@]}"; do
  echo "rm $dir"
  rm -rf "$dir"
done

echo "disk after:"
df -h /System/Volumes/Data | tail -1
