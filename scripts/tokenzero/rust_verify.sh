#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/rust_verify.sh [--dry-run] [--json|--robot] [--output-json PATH]

Runs the local Rust verification gate:
  cargo fmt --all -- --check
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  git diff --check

Options:
  --dry-run           Print the planned verification commands without running them
  --json, --robot     Emit machine-readable JSON to stdout
  --output-json PATH  Write the JSON report to PATH, atomically
EOF
}

dry_run=false
json=false
output_json=""

while (($#)); do
  case "$1" in
    --dry-run)
      dry_run=true
      ;;
    --json | --robot)
      json=true
      ;;
    --output-json)
      shift
      if (($# == 0)); then
        echo "error: --output-json requires a path" >&2
        usage >&2
        exit 2
      fi
      output_json="$1"
      ;;
    --output-json=*)
      output_json="${1#--output-json=}"
      if [[ -z "$output_json" ]]; then
        echo "error: --output-json requires a path" >&2
        usage >&2
        exit 2
      fi
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
  shift
done

json_escape() {
  local value="$1"
  value=${value//\\/\\\\}
  value=${value//\"/\\\"}
  value=${value//$'\n'/\\n}
  value=${value//$'\r'/\\r}
  value=${value//$'\t'/\\t}
  printf '%s' "$value"
}

step_names=(fmt test clippy diff_check)
step_commands=(
  "cargo fmt --all -- --check"
  "cargo test --workspace"
  "cargo clippy --workspace --all-targets -- -D warnings"
  "git diff --check"
)

build_dry_run_json() {
  printf '{"schema_version":"tokenzero.rust_verify.v1","dry_run":true,"success":true,"commands":['
  local i
  for i in "${!step_names[@]}"; do
    if ((i > 0)); then
      printf ','
    fi
    printf '{"name":"%s","command":"%s"}' \
      "$(json_escape "${step_names[$i]}")" \
      "$(json_escape "${step_commands[$i]}")"
  done
  printf ']}'
}

write_output_json() {
  local payload="$1"

  if [[ -z "$output_json" ]]; then
    return 0
  fi

  local output_dir tmp
  output_dir="$(dirname -- "$output_json")"
  mkdir -p -- "$output_dir"
  tmp="${output_json}.tmp.$$"
  printf '%s\n' "$payload" >"$tmp"
  mv -f -- "$tmp" "$output_json"
}

run_step() {
  local name="$1"
  shift

  if "$json"; then
    printf 'running %s: %s\n' "$name" "$*" >&2
  else
    printf '==> %s\n' "$*"
  fi

  local start end status duration
  start="$(date +%s)"
  set +e
  if "$json"; then
    "$@" >&2
  else
    "$@"
  fi
  status=$?
  set -e
  end="$(date +%s)"
  duration=$((end - start))

  STEP_RESULTS+=("${name}|${step_commands[$STEP_INDEX]}|${status}|${duration}")
  return "$status"
}

build_results_json() {
  local success="$1"
  printf '{"schema_version":"tokenzero.rust_verify.v1","dry_run":false,"success":%s,"steps":[' "$success"
  local i result name command status duration
  for i in "${!STEP_RESULTS[@]}"; do
    IFS='|' read -r name command status duration <<<"${STEP_RESULTS[$i]}"
    if ((i > 0)); then
      printf ','
    fi
    printf '{"name":"%s","command":"%s","exit_code":%s,"duration_seconds":%s}' \
      "$(json_escape "$name")" \
      "$(json_escape "$command")" \
      "$status" \
      "$duration"
  done
  printf ']}'
}

if "$dry_run"; then
  if "$json" || [[ -n "$output_json" ]]; then
    payload="$(build_dry_run_json)"
    write_output_json "$payload"
  fi

  if "$json"; then
    printf '%s\n' "$payload"
  else
    printf 'dry-run: verification commands\n'
    printf '  %s\n' "${step_commands[@]}"
  fi
  exit 0
fi

STEP_RESULTS=()
success=true

for STEP_INDEX in "${!step_names[@]}"; do
  case "${step_names[$STEP_INDEX]}" in
    fmt)
      run_step fmt cargo fmt --all -- --check || success=false
      ;;
    test)
      run_step test cargo test --workspace || success=false
      ;;
    clippy)
      run_step clippy cargo clippy --workspace --all-targets -- -D warnings || success=false
      ;;
    diff_check)
      run_step diff_check git diff --check || success=false
      ;;
  esac

  if ! "$success"; then
    break
  fi
done

if "$json" || [[ -n "$output_json" ]]; then
  payload="$(build_results_json "$success")"
  write_output_json "$payload"
fi

if "$json"; then
  printf '%s\n' "$payload"
fi

if "$success"; then
  exit 0
fi
exit 1
