#!/usr/bin/env bash
# CLI cold-read profiling: cold removes ~/.tokenzero/recovery-cache.json; warm retains it.
# Hyperfine is preferred, with a checked Python perf_counter fallback. Output is the established markdown table.
set -euo pipefail
WARMUP="${WARMUP:-3}"; RUNS="${RUNS:-50}"; REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"; SMALL_FILE="${SMALL_FILE:-$REPO_ROOT/Cargo.toml}"
HARNESS=(python3 "$REPO_ROOT/benchmarks/harness.py"); log() { printf '[cli-cold-read] %s\n' "$*" >&2; }
fail() { printf '[cli-cold-read] ERROR: %s\n' "$*" >&2; exit 1; }; BIN="$("${HARNESS[@]}" resolve_bin)" || fail "tokenzero binary not found"
log "binary: $BIN"; log "small file: $SMALL_FILE"; log "hyperfine: $(command -v hyperfine || echo 'fallback: Python perf_counter')"
REF_JSON="$(mktemp)"; trap "rm -f $REF_JSON" EXIT
"$BIN" read --end-line 1 "$SMALL_FILE" --json >"$REF_JSON" || fail "initial read failed"
REF="$("${HARNESS[@]}" first_blob_ref "$REF_JSON")"
[[ -n "$REF" ]] || fail "initial read returned no blob ref"; log "expand ref: $REF"
run_cell() {
  local flags=(--runs "$RUNS" --warmup "$WARMUP"); [[ "$2" == cold ]] && flags+=(--cold)
  "${HARNESS[@]}" measure_cell "$1 ($2)" "$3" "${flags[@]}"
}
keys=(process_start store_open first_read first_expand); displays=('`process_start` (`--help`)' '`store_open` (`mem`)' '`first_read` (`read`)' '`first_expand` (`expand`)')
commands=("$BIN --help" "$BIN mem" "$BIN read --end-line 1 \"$SMALL_FILE\"" "$BIN expand \"$REF\""); declare -a cold warm
for i in "${!keys[@]}"; do
  cold[$i]="$(run_cell "${keys[$i]}" cold "${commands[$i]}")"; warm[$i]="$(run_cell "${keys[$i]}" warm "${commands[$i]}")"
done
read -r read_p50 _ <<<"${cold[2]}"; read -r help_p50 _ <<<"${cold[0]}"
cat <<'HDR'
| Component | cold p50 (ms) | cold p90 (ms) | cold p99 (ms) | warm p50 (ms) | warm p90 (ms) | warm p99 (ms) |
|---|---:|---:|---:|---:|---:|---:|
HDR
for i in "${!keys[@]}"; do
  read -r c50 c90 c99 <<<"${cold[$i]}"; read -r w50 w90 w99 <<<"${warm[$i]}"
  printf '| %s | %s | %s | %s | %s | %s | %s |\n' "${displays[$i]}" "$c50" "$c90" "$c99" "$w50" "$w90" "$w99"
done
printf '| **Startup tax** = cold first_read p50 − process_start p50 | **%s ms** | — | — | — | — | — |\n' "$((read_p50-help_p50))"; log "done. Pipe stdout into benchmarks/cli-cold-read-table.md to record results."
