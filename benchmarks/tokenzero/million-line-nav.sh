#!/usr/bin/env bash
# Million-line repo navigation benchmark (bead tokenzero-15w)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"; H=(python3 -m benchmarks.harness)
BIN="$("${H[@]}" resolve_bin)" || { echo "ERROR: tokenzero binary not found. Set TOKENZERO_BIN=/path/to/tokenzero" >&2; exit 1; }; NUM_DIRS=100; FILES_PER_DIR=10; LINES_PER_FILE=1000; NEEDLE=BENCH_NEEDLE_FN; BUDGET=32000
TOTAL_FILES=$((NUM_DIRS*FILES_PER_DIR)); TOTAL_LINES=$((TOTAL_FILES*LINES_PER_FILE)); WORK_DIR="$(mktemp -d /tmp/tz-million.XXXXXX)"; SYNTH="$WORK_DIR/repo"; TMP_JSON="$WORK_DIR/out.json"; TMP_RAW="$WORK_DIR/raw.out"; TMP_ERR="$WORK_DIR/command.err"; NEVER_WORSE_RECEIPT="$WORK_DIR/never-worse.tsv"
trap 'rm -rf "$WORK_DIR"' EXIT
log() { printf '[million-nav] %s\n' "$*" >&2; }; now_ms() { "${H[@]}" now_ms; }
tz_run() {
  local cmd="$1" start end bytes
  start=$(now_ms)
  if ! eval "$cmd" >"$TMP_JSON" 2>"$TMP_ERR"; then
    cat "$TMP_ERR" >&2
    return 1
  fi
  end=$(now_ms); bytes=$(wc -c <"$TMP_JSON" | tr -d ' ')
  printf '%s\t%s' "$((end-start))" "$bytes"
}
raw_run() {
  local cmd="$1" start end bytes
  start=$(now_ms)
  if ! eval "$cmd" >"$TMP_RAW" 2>"$TMP_ERR"; then
    cat "$TMP_ERR" >&2
    return 1
  fi
  end=$(now_ms); bytes=$(wc -c <"$TMP_RAW" | tr -d ' ')
  printf '%s\t%s' "$bytes" "$((end-start))"
}
capture_tz() { TZ_RESULT=$(tz_run "$1") || { log 'FAILED: TokenZero task command'; return 1; }; }
capture_raw() { RAW_RESULT=$(raw_run "$1") || { log 'FAILED: raw task command'; return 1; }; }
estimated_units() { printf '%s' $((($1 + 3) / 4)); }
emit_header() { printf '| # | Task | Tool | est_tokens | wall_ms | output_bytes | notes |\n|---|------|------|---:|---:|---:|------|\n'; }
emit_tz()  { printf '| %s | `%s` | `tokenzero` | %s | %s | %s | %s |\n' "$1" "$2" "$(estimated_units "$4")" "$3" "$4" "$5"; }
emit_raw() { printf '| %s | `%s` | `raw-cli` | %s | %s | %s | %s |\n' "$1" "$2" "$(estimated_units "$3")" "$4" "$3" "$5"; }
record_gate() {
  printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3"     "$(estimated_units "$2")" "$(estimated_units "$3")" >> "$NEVER_WORSE_RECEIPT"
}

log "binary: $BIN"; log "budget: $BUDGET estimated stdout tokens"; log "generating $TOTAL_LINES lines across $TOTAL_FILES files in $SYNTH ..."
"${H[@]}" generate_million "$SYNTH" --dirs "$NUM_DIRS" --files "$FILES_PER_DIR" --lines "$LINES_PER_FILE" --needle "$NEEDLE"; log "repo generated: $(find "$SYNTH" -type f | wc -l | tr -d ' ') files, $(find "$SYNTH" -type f -exec cat {} + | wc -l | tr -d ' ') lines"

TARGET_FILE="$SYNTH/mod_0050/file_0050_003.rs"; NEEDLE_FILE="$SYNTH/mod_0010/file_0010_000.rs"
EDIT_FILE="$WORK_DIR/edit_target.rs"; RAW_EDIT="$WORK_DIR/raw_edit_target.rs"
cp "$NEEDLE_FILE" "$EDIT_FILE"; cp "$EDIT_FILE" "$RAW_EDIT"; TOTAL_ESTIMATED=0
printf 'schema_version\tnever-worse/v1\nsuite\tmillion-line-nav\nsurface_id\tvisible-payload-bytes/v1\nunit_id\testimator:bytes-ceil-div4/v1\ntask\tcandidate_bytes\traw_bytes\tcandidate_units\traw_units\n' > "$NEVER_WORSE_RECEIPT"
emit_header

log "task A: bounded read"
capture_tz "$BIN read --json --start-line 1 --end-line 50 \"$TARGET_FILE\" --allowed-root \"$SYNTH\"" || exit 1; read -r ms_a _tz_env_a <<<"$TZ_RESULT"
tz_vis_a=$("${H[@]}" visible_payload_bytes "$TMP_JSON") || { log 'FAILED: read visible payload missing'; exit 1; }
TOTAL_ESTIMATED=$((TOTAL_ESTIMATED + $(estimated_units "$tz_vis_a"))); emit_tz A read_50_lines "$ms_a" "$tz_vis_a" "visible payload"
capture_raw "head -n 50 \"$TARGET_FILE\"" || exit 1; read -r bytes_a ms_raw_a <<<"$RAW_RESULT"; emit_raw A read_50_lines "$bytes_a" "$ms_raw_a" "head -n 50"; record_gate read_50_lines "$tz_vis_a" "$bytes_a"

log "task B: grep + expand-by-ref (budget is find visible; expand is integrity only)"
capture_tz "$BIN find --json \"$NEEDLE\" \"$SYNTH\" --max-files 10 --max-visible-tokens 2000 --allowed-root \"$SYNTH\"" || exit 1; read -r ms_b1 _ <<<"$TZ_RESULT"
BLOB_REF=$("${H[@]}" first_blob_ref "$TMP_JSON")
if [[ -z "$BLOB_REF" ]]; then log 'FAILED: grep task returned no expandable blob ref'; exit 1; fi
tz_vis_b=$("${H[@]}" visible_payload_bytes "$TMP_JSON") || { log 'FAILED: find visible payload missing'; exit 1; }
capture_tz "$BIN expand --json \"$BLOB_REF\"" || exit 1; read -r ms_b2 _tz_expand_envelope <<<"$TZ_RESULT"
RECOVERED=$("${H[@]}" expand_recovered_text "$TMP_JSON") || { log 'FAILED: expand integrity missing visible text'; exit 1; }
if [[ "$RECOVERED" != *"$NEEDLE"* ]]; then log 'FAILED: expand did not recover needle bytes'; exit 1; fi
ms_b=$((ms_b1+ms_b2)); TOTAL_ESTIMATED=$((TOTAL_ESTIMATED + $(estimated_units "$tz_vis_b"))); emit_tz B grep_expand "$ms_b" "$tz_vis_b" "find visible; expand-by-ref integrity"
capture_raw "grep -rn \"$NEEDLE\" \"$SYNTH\" | sed -n '1,20p'" || exit 1; read -r bytes_b ms_raw_b <<<"$RAW_RESULT"; emit_raw B grep_expand "$bytes_b" "$ms_raw_b" "grep -rn | first 20"; record_gate grep_expand "$tz_vis_b" "$bytes_b"

log "task C: tree + glob + read"
capture_tz "$BIN tree --json \"$SYNTH\" --depth 2 --max-files 50 --allowed-root \"$SYNTH\"" || exit 1; read -r ms_c1 _ <<<"$TZ_RESULT"
tz_vis_c1=$("${H[@]}" visible_payload_bytes "$TMP_JSON") || { log 'FAILED: tree visible payload missing'; exit 1; }
capture_tz "$BIN glob --json '*.rs' \"$SYNTH\" --max-files 10 --allowed-root \"$SYNTH\"" || exit 1; read -r ms_c2 _ <<<"$TZ_RESULT"
tz_vis_c2=$("${H[@]}" visible_payload_bytes "$TMP_JSON") || { log 'FAILED: glob visible payload missing'; exit 1; }
if ! GLOB_PICK=$("${H[@]}" glob_pick "$TMP_JSON"); then
  log 'FAILED: glob response is malformed; refusing fallback read'
  exit 1
fi
IFS=$'\t' read -r GLOB_ROOT GLOB_REL <<<"$GLOB_PICK"
if [[ -z "$GLOB_ROOT" && -z "$GLOB_REL" ]]; then
  GLOB_FILE="$TARGET_FILE"
elif [[ -z "$GLOB_ROOT" || -z "$GLOB_REL" ]]; then
  log 'FAILED: glob parser returned a partial path; refusing fallback read'
  exit 1
else
  GLOB_FILE="${GLOB_ROOT}/${GLOB_REL}"
fi
capture_tz "$BIN read --json --start-line 1 --end-line 50 \"$GLOB_FILE\" --allowed-root \"$SYNTH\"" || exit 1; read -r ms_c3 _ <<<"$TZ_RESULT"
tz_vis_c3=$("${H[@]}" visible_payload_bytes "$TMP_JSON") || { log 'FAILED: tree/glob read visible payload missing'; exit 1; }
tz_vis_c=$((tz_vis_c1+tz_vis_c2+tz_vis_c3))
ms_c=$((ms_c1+ms_c2+ms_c3)); TOTAL_ESTIMATED=$((TOTAL_ESTIMATED + $(estimated_units "$tz_vis_c"))); emit_tz C tree_glob_read "$ms_c" "$tz_vis_c" "tree+glob+read visible"
capture_raw "find \"$SYNTH\" -maxdepth 2 -type f | sort | sed -n '1,20p'; find \"$SYNTH\" -name '*.rs' -type f -print -quit; head -n 50 \"$GLOB_FILE\"" || exit 1; read -r bytes_c ms_raw_c <<<"$RAW_RESULT"
emit_raw C tree_glob_read "$bytes_c" "$ms_raw_c" "find+find+head"; record_gate tree_glob_read "$tz_vis_c" "$bytes_c"

log "task D: grep → expand-by-ref → edit → verify (envelope JSON excluded from budget)"
capture_tz "$BIN find --json \"$NEEDLE\" \"$EDIT_FILE\" --allowed-root \"$WORK_DIR\"" || exit 1; read -r ms_d1 _ <<<"$TZ_RESULT"
tz_vis_d1=$("${H[@]}" visible_payload_bytes "$TMP_JSON") || { log 'FAILED: edit-find visible payload missing'; exit 1; }
D_BLOB_REF=$("${H[@]}" first_blob_ref "$TMP_JSON")
if [[ -z "$D_BLOB_REF" ]]; then log 'FAILED: edit task returned no expandable blob ref'; exit 1; fi
capture_tz "$BIN expand --json \"$D_BLOB_REF\"" || exit 1; read -r ms_d2 _ <<<"$TZ_RESULT"
D_RECOVERED=$("${H[@]}" expand_recovered_text "$TMP_JSON") || { log 'FAILED: edit expand integrity missing visible text'; exit 1; }
if [[ "$D_RECOVERED" != *"$NEEDLE"* ]]; then log 'FAILED: edit expand did not recover needle'; exit 1; fi
capture_tz "$BIN edit --json --edits-json '[{\"find\":\"$NEEDLE\",\"replace\":\"RENAMED_FN\"}]' \"$EDIT_FILE\" --allowed-root \"$WORK_DIR\"" || exit 1; read -r ms_d3 _ <<<"$TZ_RESULT"
# Non-dry-run edit clears visible.text (engine_edit). Mutation is integrity-only; budget is find+read-back.
capture_tz "$BIN read --json --start-line 498 --end-line 502 \"$EDIT_FILE\" --allowed-root \"$WORK_DIR\"" || exit 1; read -r ms_d4 _ <<<"$TZ_RESULT"
tz_vis_d4=$("${H[@]}" visible_payload_bytes "$TMP_JSON") || { log 'FAILED: edit read-back visible payload missing'; exit 1; }
READBACK=$("${H[@]}" expand_recovered_text "$TMP_JSON") || { log 'FAILED: edit read-back missing visible text'; exit 1; }
if [[ "$READBACK" != *RENAMED_FN* ]]; then log 'FAILED: edit integrity: read-back missing RENAMED_FN'; exit 1; fi
if [[ "$READBACK" == *"$NEEDLE"* ]]; then log 'FAILED: edit integrity: read-back still contains original needle'; exit 1; fi
tz_vis_d=$((tz_vis_d1+tz_vis_d4))
ms_d=$((ms_d1+ms_d2+ms_d3+ms_d4)); TOTAL_ESTIMATED=$((TOTAL_ESTIMATED + $(estimated_units "$tz_vis_d"))); emit_tz D grep_expand_edit_verify "$ms_d" "$tz_vis_d" "find+read visible; expand+edit integrity"
capture_raw "grep -n \"$NEEDLE\" \"$RAW_EDIT\"; sed -i.bak 's/$NEEDLE/RENAMED_FN/g' \"$RAW_EDIT\"; rm -f \"$RAW_EDIT.bak\"; sed -n '498,502p' \"$RAW_EDIT\"" || exit 1; read -r bytes_d ms_raw_d <<<"$RAW_RESULT"
emit_raw D grep_expand_edit_verify "$bytes_d" "$ms_raw_d" "grep+sed+sed"; record_gate grep_expand_edit_verify "$tz_vis_d" "$bytes_d"

log "task E: recall"
capture_tz "$BIN recall --json \"$NEEDLE\" --max-hits 10 --allowed-root \"$WORK_DIR\"" || exit 1; read -r ms_e _ <<<"$TZ_RESULT"
tz_vis_e=$("${H[@]}" visible_payload_bytes "$TMP_JSON") || { log 'FAILED: recall visible payload missing'; exit 1; }
TOTAL_ESTIMATED=$((TOTAL_ESTIMATED + $(estimated_units "$tz_vis_e"))); emit_tz E recall "$ms_e" "$tz_vis_e" "cache search visible"
capture_raw "grep -rn \"$NEEDLE\" \"$SYNTH\" | sed -n '1,10p'" || exit 1; read -r bytes_e ms_raw_e <<<"$RAW_RESULT"; emit_raw E recall "$bytes_e" "$ms_raw_e" "grep -rn | first 10"; record_gate recall "$tz_vis_e" "$bytes_e"

printf '\n'
python3 "$ROOT/benchmarks/never_worse_gate.py" "$NEVER_WORSE_RECEIPT"
printf '\n## Estimated-output budget assertion\n\n| Metric | Value |\n|--------|-------|\n'
printf '| Total TokenZero visible-payload est_tokens (all 5 tasks) | %d |\n| Estimator | `estimator:bytes-ceil-div4/v1` |\n| Context budget | %d |\n| Remaining headroom | %d |\n| Utilization | %.1f%% |\n\n' \
  "$TOTAL_ESTIMATED" "$BUDGET" "$((BUDGET-TOTAL_ESTIMATED))" "$(python3 -c "print(f'{$TOTAL_ESTIMATED/$BUDGET*100:.1f}')")"
printf '> This heuristic budget is not Q99. Denominator is `visible-payload-bytes/v1` (JSON envelope excluded). Expand is integrity-only and is not summed into the never-worse row.\n\n'
if [[ "$TOTAL_ESTIMATED" -lt "$BUDGET" ]]; then
  printf '> **Result: PASS** -- all 5 navigation tasks fit within the 32k estimated-output budget.\n'
else
  printf '> **Result: FAIL** -- estimated output tokens (%d) exceed the 32k budget.\n' "$TOTAL_ESTIMATED"; exit 1
fi
printf '\n## Quality criteria\n\n'
printf '%s\n' \
  '- **Byte-exact recovery**: every `expand` call must recover needle bytes in `visible` (integrity, not a stdout budget row).' \
  '- **All tasks succeed**: each navigation task completes without error (exit code 0).' \
  '- **No content loss**: compact capsules hide raw content behind refs, but nothing is discarded — every byte is recoverable.' \
  '- **Edit integrity**: the multi-step edit produces a valid file with the replacement applied (verified by read-back visible text).' \
  '- **Never-worse denominator**: `visible-payload-bytes/v1` + `estimator:bytes-ceil-div4/v1`; captured JSON envelope is not compared to raw CLI.'
log "done. total estimated output tokens=$TOTAL_ESTIMATED budget=$BUDGET"
