#!/usr/bin/env bash
# bench_baseline.sh — same-host baseline wrapper (hyperfine + fingerprint).
#
# Captures env_fingerprint.py output, runs the user command under hyperfine,
# writes hyperfine JSON + summary.json under tests/artifacts/perf/<name>/<run-id>/.
#
# Usage:
#   scripts/bench_baseline.sh --name <scenario> --cmd "<command>" \
#       [--runs 20] [--warmup 3] [--output-dir tests/artifacts/perf] \
#       [--taskset 2,3] [--cold] [--cache-state warm]
#
# Requires: hyperfine, jq. Fingerprint via scripts/env_fingerprint.py.
# Does not tune the kernel; pair with host prep docs when needed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

name=""
cmd=""
runs=20
warmup=3
output_dir="tests/artifacts/perf"
taskset_cpus=""
cold_cache=0
cache_state="warm"

usage() {
  sed -n '2,16p' "$0"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --name)        name="$2"; shift 2;;
    --cmd)         cmd="$2"; shift 2;;
    --runs)        runs="$2"; shift 2;;
    --warmup)      warmup="$2"; shift 2;;
    --output-dir)  output_dir="$2"; shift 2;;
    --taskset)     taskset_cpus="$2"; shift 2;;
    --cache-state) cache_state="$2"; shift 2;;
    --cold)        cold_cache=1; cache_state="cold"; shift;;
    -h|--help)     usage; exit 0;;
    *) echo "unknown arg: $1" >&2; exit 2;;
  esac
done

[[ -n "$name" && -n "$cmd" ]] || { echo "ERROR: --name and --cmd required" >&2; usage >&2; exit 2; }
command -v hyperfine >/dev/null || { echo "ERROR: install hyperfine" >&2; exit 2; }
command -v jq        >/dev/null || { echo "ERROR: install jq" >&2; exit 2; }

run_id="$(date -u +%Y%m%dT%H%M%SZ)-$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo nosha)"
# resolve relative output_dir from repo root when not absolute
if [[ "$output_dir" != /* ]]; then
  output_dir="${REPO_ROOT}/${output_dir}"
fi
out="${output_dir}/${name}/${run_id}"
mkdir -p "$out"
echo "→ artifacts: $out"

if [[ -f "$SCRIPT_DIR/env_fingerprint.py" ]]; then
  python3 "$SCRIPT_DIR/env_fingerprint.py" \
    --root "$REPO_ROOT" \
    --run-id "$run_id" \
    --cache-state "$cache_state" \
    > "$out/fingerprint.json" || echo "WARN: env_fingerprint.py failed; continuing" >&2
else
  echo "WARN: env_fingerprint.py missing; skipping fingerprint" >&2
fi

runner="$cmd"
if [[ -n "$taskset_cpus" ]]; then
  runner="taskset -c ${taskset_cpus} bash -c $(printf '%q' "$cmd")"
fi

prepare_cmd=""
if [[ "$cold_cache" -eq 1 ]]; then
  echo "→ cold-cache mode requires: sudo sync && sudo sh -c 'echo 3 > /proc/sys/vm/drop_caches'"
  prepare_cmd='sync && sudo sh -c "echo 3 > /proc/sys/vm/drop_caches"'
fi

set +e
if [[ -n "$prepare_cmd" ]]; then
  hyperfine --warmup "$warmup" --runs "$runs" \
            --prepare "$prepare_cmd" \
            --export-json "$out/hyperfine.json" \
            --export-markdown "$out/hyperfine.md" \
            "$runner"
else
  hyperfine --warmup "$warmup" --runs "$runs" \
            --export-json "$out/hyperfine.json" \
            --export-markdown "$out/hyperfine.md" \
            "$runner"
fi
hf_rc=$?
set -e

if [[ -f "$out/hyperfine.json" ]]; then
  jq --arg scenario "$name" '
    .results[0] as $r
    | ($r.times | sort) as $t
    | ($t | length) as $n
    | (if $n == 0 then 0 else ($n - 1) end) as $max_i
    | def ceil_num: if . == floor then . else floor + 1 end;
    def pct_index(p):
      if $n == 0 then null
      else
        (($n * p | ceil_num) - 1) as $idx
        | if $idx < 0 then 0 elif $idx > $max_i then $max_i else $idx end
      end;
    def pct_ms(p): if $n == 0 then null else ($t[pct_index(p)] * 1000) end;
    {
      scenario: $scenario,
      runs: $n,
      mean_ms: ($r.mean * 1000),
      stddev_ms: ($r.stddev * 1000),
      p50_ms: pct_ms(0.50),
      p95_ms: pct_ms(0.95),
      p99_ms: pct_ms(0.99),
      max_ms: (if $n == 0 then null else ($t[-1] * 1000) end)
    }
  ' "$out/hyperfine.json" > "$out/summary.json" || true
  echo "→ summary.json:"
  cat "$out/summary.json" 2>/dev/null || true
fi

ln -sfn "$run_id" "${output_dir}/${name}/latest" 2>/dev/null || true
exit $hf_rc
