#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/rustfmt_targeted.sh [--check] <path/to/file.rs> [...]

Formats ONLY the explicit Rust files listed. Uses the workspace Rust edition (2024).

Options:
  --check   Run rustfmt --check (do not write); exits 1 if formatting needed
  -h, --help Show this help

Warning:
  cargo fmt -- path/to/file.rs is NOT a file allowlist. cargo fmt ignores
  trailing paths as a scope filter and may format the entire workspace
  (~60 files). Use this script (rustfmt --edition 2024 -- <explicit files>)
  for targeted formatting.

Examples:
  scripts/rustfmt_targeted.sh crates/tokenzero-core/src/lib.rs
  scripts/rustfmt_targeted.sh --check crates/tokenzero-core/src/lib.rs
EOF
}

check=false
files=()

while (($#)); do
  case "$1" in
    --check) check=true; shift ;;
    -h|--help) usage; exit 0 ;;
    --) shift; while (($#)); do files+=("$1"); shift; done; break ;;
    --*) echo "error: unknown flag: $1" >&2; usage >&2; exit 2 ;;
    *) files+=("$1"); shift ;;
  esac
done

if ((${#files[@]} == 0)); then
  echo "error: no files given" >&2
  usage >&2
  exit 2
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../.." && pwd)"

# Canonical repo root (resolve symlink/case via python if available)
if command -v python3 >/dev/null 2>&1; then
  REPO_ROOT="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$REPO_ROOT")"
fi

EDITION="2024"
failed=0
resolved=()

for f in "${files[@]}"; do
  if [[ ! -e "$f" ]]; then
    echo "error: not found: $f" >&2; failed=1; continue
  fi
  if [[ -d "$f" ]]; then
    echo "error: is a directory (pass explicit .rs files): $f" >&2; failed=1; continue
  fi
  if [[ "$f" != *.rs ]]; then
    echo "error: not a .rs file: $f" >&2; failed=1; continue
  fi
  # Resolve to absolute canonical path
  if command -v python3 >/dev/null 2>&1; then
    abs="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$f" 2>/dev/null || echo "$f")"
  else
    # fallback: prepend repo root for relative paths
    case "$f" in
      /*) abs="$f" ;;
      *) abs="$REPO_ROOT/$f" ;;
    esac
  fi
  if command -v python3 >/dev/null 2>&1; then
    if ! python3 - "$abs" "$REPO_ROOT" <<'PY'
import os
import sys

path = os.path.dirname(sys.argv[1])
root = sys.argv[2]
while True:
    if os.path.samefile(path, root):
        raise SystemExit(0)
    parent = os.path.dirname(path)
    if parent == path:
        raise SystemExit(1)
    path = parent
PY
    then
      echo "error: outside repo: $f -> $abs" >&2; failed=1; continue
    fi
  else
    case "$abs" in
      "$REPO_ROOT"/*) ;;
      *)
        echo "error: outside repo: $f -> $abs" >&2; failed=1; continue
        ;;
    esac
  fi
  resolved+=("$f")
done

if ((failed)); then
  exit 2
fi

if ((${#resolved[@]} == 0)); then
  echo "error: no valid files" >&2; exit 2
fi

for f in "${resolved[@]}"; do
  if "$check"; then
    rustfmt --edition "$EDITION" --check -- "$f"
  else
    rustfmt --edition "$EDITION" -- "$f"
  fi
done
