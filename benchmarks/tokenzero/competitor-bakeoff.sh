#!/usr/bin/env bash
# Competitor head-to-head bake-off benchmark (bead tokenzero-8u9)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"; H=(python3 "$ROOT/benchmarks/harness.py")
BIN="$("${H[@]}" resolve_bin)" || { echo "ERROR: tokenzero binary not found. Set TOKENZERO_BIN=/path/to/tokenzero" >&2; exit 1; }; RUNS="${RUNS:-5}"; WARMUP="${WARMUP:-1}"; README="$ROOT/README.md"; CRATES="$ROOT/crates"; PATTERN="pub fn"
WORK_DIR="$(mktemp -d /tmp/tz-bakeoff.XXXXXX)"; SAMPLE="$WORK_DIR/sample_500.txt"; EDIT_FILE="$WORK_DIR/edit_sample.txt"; GREP_CACHE="$WORK_DIR/grep-cache.json"; TREE_CACHE_POINTER="$WORK_DIR/tree-cache-current"; NEVER_WORSE_RECEIPT="$WORK_DIR/never-worse.tsv"
trap 'rm -rf "$WORK_DIR"' EXIT
log() { printf '[bakeoff] %s\n' "$*" >&2; }; measure() { "${H[@]}" measure_median "$1" "$2" --runs "$RUNS" --warmup "$WARMUP" --prepare "$3" --teardown "$4"; }
emit() { printf '| `%s` | `%s` | %s | %s | %s | %s |\n' "$1" "$2" "$3" "$4" "$5" "$6"; }
declare -A TOKENZERO_BYTES TOKENZERO_UNITS RAW_BYTES RAW_UNITS
row() {
  local task="$1" tool="$2" cmd="$3" prepare="$4" teardown="$5" metrics m b e
  if [[ "$tool" != tokenzero && "$tool" != raw-cli ]] && ! command -v "$tool" >/dev/null 2>&1; then
    emit "$task" "$tool" - - - "not installed"; return
  fi
  if ! metrics=$(measure "$tool:$task" "$cmd" "$prepare" "$teardown"); then
    log "FAILED: $tool:$task"
    if [[ "$tool" != tokenzero && "$tool" != raw-cli ]]; then
      emit "$task" "$tool" - - - "execution failed; excluded from gate"
      return 0
    fi
    return 1
  fi
  read -r m b e <<<"$metrics"
  if [[ "$tool" == tokenzero ]]; then
    TOKENZERO_BYTES["$task"]="$b"; TOKENZERO_UNITS["$task"]="$e"
  elif [[ "$tool" == raw-cli ]]; then
    RAW_BYTES["$task"]="$b"; RAW_UNITS["$task"]="$e"
  fi
  if [[ "$b" == 0 ]]; then
    emit "$task" "$tool" "$m" "$b" "$e" "ran but produced no output (arg mismatch with installed version)"
  elif [[ "$task" == grep_read && "$tool" == tokenzero ]]; then
    emit "$task" "$tool" "$m" "$b" "$e" "warm/dedup; estimator:bytes-ceil-div4/v1 candidate"
  elif [[ "$task" == grep_read && "$tool" == raw-cli ]]; then
    emit "$task" "$tool" "$m" "$b" "$e" "estimator:bytes-ceil-div4/v1 raw baseline"
  elif [[ "$task" == tree_glob_read && "$tool" == tokenzero ]]; then
    emit "$task" "$tool" "$m" "$b" "$e" "fresh cache; lossless prefix trie; 40KiB Auto exact cutoff; estimator:bytes-ceil-div4/v1 candidate"
  elif [[ "$task" == tree_glob_read && "$tool" == raw-cli ]]; then
    emit "$task" "$tool" "$m" "$b" "$e" "same corpus; estimator:bytes-ceil-div4/v1 raw baseline"
  else
    emit "$task" "$tool" "$m" "$b" "$e" ""
  fi
}
prepare_for() {
  case "$1" in
    edit_verify) printf "%s > %q" "printf 'alpha\\nbeta\\ngamma'" "$EDIT_FILE" ;;
    tree_glob_read) printf 'cache_dir=$(mktemp -d "%s/tree-cache.XXXXXX"); printf "%%s" "$cache_dir/recovery-cache.json" > "%s"' "$WORK_DIR" "$TREE_CACHE_POINTER" ;;
    *) printf 'true' ;;
  esac
}
teardown_for() {
  # sed -i.bak leftover is teardown, not work. Must stay outside start.elapsed().
  case "$1:$2" in
    edit_verify:raw-cli) printf 'rm -f %q.bak' "$EDIT_FILE" ;;
    *) printf 'true' ;;
  esac
}
command_for() {
  local task="$1" tool="$2" exe="$tool"; [[ "$tool" == tokenzero ]] && exe="$BIN"
  case "$task:$tool" in
    read_500:tokenzero) echo "cd \"$WORK_DIR\"; $exe read --end-line 500 \"$SAMPLE\"";; read_500:raw-cli) echo "cat \"$SAMPLE\"";;
    read_500:rtk) echo "rtk read --limit 500 \"$SAMPLE\"";; read_500:lean-ctx) echo "lean-ctx read \"$SAMPLE\" --limit 500";;
    read_500:headroom) echo "headroom read --lines 500 \"$SAMPLE\"";; read_500:ztk) echo "ztk read --end-line 500 \"$SAMPLE\"";;
    read_500:context-mode) echo "context-mode read --limit 500 \"$SAMPLE\"";;
    grep_read:tokenzero) echo "TOKENZERO_CACHE_PATH=\"$GREP_CACHE\" $exe grep 'TokenZero' \"$README\"";;
    grep_read:raw-cli) echo "grep -n 'TokenZero' \"$README\"";; grep_read:*) echo "$exe grep 'TokenZero' \"$README\"";;
    tree_glob_read:raw-cli) echo "find \"$CRATES\" -maxdepth 2 -type f | sort; find \"$CRATES\" -name '*.rs' -type f | head -n 1; cat \"$first_rs\"";;
    tree_glob_read:tokenzero) echo "export TOKENZERO_CACHE_PATH=\$(cat \"$TREE_CACHE_POINTER\"); $exe tree \"$CRATES\" --depth 2; $exe glob '*.rs' \"$CRATES\"; $exe read \"$first_rs\"";;
    tree_glob_read:*) echo "$exe tree \"$CRATES\"; $exe glob '*.rs' \"$CRATES\"; $exe read \"$first_rs\"";;
    edit_verify:tokenzero) echo "cd \"$WORK_DIR\"; $exe edit --edits-json '[{\"find\":\"beta\",\"replace\":\"BETA\"}]' \"$EDIT_FILE\" && $exe read \"$EDIT_FILE\"";;
    edit_verify:raw-cli) echo "sed -i.bak 's/beta/BETA/g' \"$EDIT_FILE\"; cat \"$EDIT_FILE\"";;
    edit_verify:*) echo "$exe edit --find beta --replace BETA \"$EDIT_FILE\"; $exe read \"$EDIT_FILE\"";;
    multi_step:raw-cli) echo "grep -n '$PATTERN' $files_str; for f in $files_str; do cat \"\$f\"; done";;
    multi_step:*) echo "$exe grep '$PATTERN' $files_str; for f in $files_str; do $exe read \"\$f\"; done";;
  esac
}
log "binary: $BIN"; log "hyperfine: $(command -v hyperfine || echo 'fallback: Python perf_counter')"; head -n 500 "$README" > "$SAMPLE" 2>/dev/null || { : > "$SAMPLE"; while [[ $(wc -l < "$SAMPLE") -lt 500 ]]; do printf 'padding line %d\n' $(($(wc -l < "$SAMPLE")+1)) >> "$SAMPLE"; done; }
first_rs=$(find "$CRATES" -name "*.rs" -type f -print -quit)
hits=(); while IFS= read -r line; do hits+=("$line"); done < <(grep -rl "$PATTERN" "$CRATES" 2>/dev/null | sed -n '1,3p')
files_str=""; for file in "${hits[@]}"; do files_str="$files_str $(printf '%q' "$file")"; done
[[ -n "$files_str" ]] || files_str="$(printf '%q' "$first_rs")"
log 'priming isolated warm/dedup grep cache outside timing'
TOKENZERO_CACHE_PATH="$GREP_CACHE" "$BIN" grep 'TokenZero' "$README" >/dev/null
printf 'schema_version\tnever-worse/v1\nsuite\tcompetitor-bakeoff\nsurface_id\tcaptured-stdout-bytes/v1\nunit_id\testimator:bytes-ceil-div4/v1\ntask\tcandidate_bytes\traw_bytes\tcandidate_units\traw_units\n' > "$NEVER_WORSE_RECEIPT"
printf '| task | tool | wall_ms | output_bytes | est_tokens | note |\n|---|---|---:|---:|---:|---|\n'
tools=(tokenzero raw-cli rtk lean-ctx headroom ztk context-mode)
for task in read_500 grep_read tree_glob_read edit_verify multi_step; do
  for tool in "${tools[@]}"; do row "$task" "$tool" "$(command_for "$task" "$tool")" "$(prepare_for "$task")" "$(teardown_for "$task" "$tool")"; done
done
for task in read_500 grep_read tree_glob_read edit_verify multi_step; do
  if [[ -z "${TOKENZERO_UNITS[$task]:-}" || -z "${RAW_UNITS[$task]:-}" ]]; then
    log "FAILED: missing never-worse row for $task"
    exit 1
  fi
  printf '%s\t%s\t%s\t%s\t%s\n' "$task" \
    "${TOKENZERO_BYTES[$task]}" "${RAW_BYTES[$task]}" \
    "${TOKENZERO_UNITS[$task]}" "${RAW_UNITS[$task]}" >> "$NEVER_WORSE_RECEIPT"
done
printf '\n'
python3 "$ROOT/benchmarks/never_worse_gate.py" "$NEVER_WORSE_RECEIPT"
estimated_saved_tokens=$((RAW_UNITS[grep_read] - TOKENZERO_UNITS[grep_read]))
estimated_savings_ppm=$((estimated_saved_tokens * 1000000 / RAW_UNITS[grep_read]))
printf '\nEstimated-token comparison (estimator:bytes-ceil-div4/v1; not Q99): candidate=%s bytes/%s estimated tokens; raw-cli baseline=%s bytes/%s estimated tokens; estimated numerator saved=%s tokens; heuristic savings=%s ppm; target >=850000 ppm: %s.\n' \
  "${TOKENZERO_BYTES[grep_read]}" "${TOKENZERO_UNITS[grep_read]}" \
  "${RAW_BYTES[grep_read]}" "${RAW_UNITS[grep_read]}" \
  "$estimated_saved_tokens" "$estimated_savings_ppm" \
  "$([[ "$estimated_savings_ppm" -ge 850000 ]] && printf PASS || printf FAIL)"
if [[ "$estimated_savings_ppm" -lt 850000 ]]; then
  exit 1
fi

tree_saved_units=$((RAW_UNITS[tree_glob_read] - TOKENZERO_UNITS[tree_glob_read]))
tree_savings_ppm=$((tree_saved_units * 1000000 / RAW_UNITS[tree_glob_read]))
tree_required_saved_units=$(((960000 * RAW_UNITS[tree_glob_read] + 999999) / 1000000))
tree_max_candidate_units=$((RAW_UNITS[tree_glob_read] - tree_required_saved_units))
tree_max_candidate_bytes=$((tree_max_candidate_units * 4))
tree_byte_margin=$((tree_max_candidate_bytes - TOKENZERO_BYTES[tree_glob_read]))
printf '\nTree/glob/read comparison (estimator:bytes-ceil-div4/v1; not Q99): candidate=%s bytes/%s estimated tokens; raw-cli baseline=%s bytes/%s estimated tokens; estimated numerator saved=%s tokens; heuristic savings=%s ppm; target >=960000 ppm: %s; byte ceiling=%s; byte margin=%s.\n' \
  "${TOKENZERO_BYTES[tree_glob_read]}" "${TOKENZERO_UNITS[tree_glob_read]}" \
  "${RAW_BYTES[tree_glob_read]}" "${RAW_UNITS[tree_glob_read]}" \
  "$tree_saved_units" "$tree_savings_ppm" \
  "$([[ "$tree_savings_ppm" -ge 960000 ]] && printf PASS || printf FAIL)" \
  "$tree_max_candidate_bytes" "$tree_byte_margin"
if [[ "$tree_savings_ppm" -lt 960000 ]]; then
  exit 1
fi
log done.
