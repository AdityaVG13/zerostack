#!/bin/bash
# Final fail-safe for genuinely runaway ZeroStack engine processes.
# Lifecycle deadlines and process-group ownership are the primary controls.
# This watchdog only acts on approved executable names after a sustained window.
# Compatible with macOS /bin/bash 3.2.
set -eu

THRESHOLD="${TOKENZERO_CPU_KILL_PCT:-85}"
WINDOW="${TOKENZERO_CPU_KILL_SAMPLES:-30}"
KILL_GRACE="${TOKENZERO_CPU_KILL_GRACE_SAMPLES:-10}"
INTERVAL="${TOKENZERO_CPU_WATCH_INTERVAL:-1}"
LOG="${TOKENZERO_CPU_WATCH_LOG:-$HOME/.tokenzero/cpu-watchdog.log}"
STATE_DIR="${TOKENZERO_CPU_WATCH_STATE:-/tmp/tokenzero-cpu-watchdog-state}"
DRY_RUN="${TOKENZERO_CPU_WATCH_DRY_RUN:-0}"

is_approved_executable() {
  case "$1" in
    tokenzero|*/tokenzero|    tokenzero-codemode|*/tokenzero-codemode|    tokenzero-xtask|*/tokenzero-xtask|    fszero-codemode|*/fszero-codemode|    graphzero-codemode|*/graphzero-codemode|    fszero-xtask|*/fszero-xtask|    graphzero-xtask|*/graphzero-xtask|    zerostack-xtask|*/zerostack-xtask) return 0 ;;
    *) return 1 ;;
  esac
}

identity_matches() {
  [ -n "$2" ] && [ "$1" = "$2" ]
}

# Emits CLEAR, COUNT:n, TERM:n, or KILL:n for one sample.
next_action() {
  strikes="$1"
  cpu="$2"
  executable="$3"
  if ! is_approved_executable "$executable"; then
    echo CLEAR
    return
  fi
  cpu_int=${cpu%.*}
  if [ "${cpu_int:-0}" -lt "$THRESHOLD" ]; then
    echo CLEAR
    return
  fi
  strikes=$((strikes + 1))
  if [ "$strikes" -eq "$WINDOW" ]; then
    echo "TERM:$strikes"
  elif [ "$strikes" -ge $((WINDOW + KILL_GRACE)) ]; then
    echo "KILL:$strikes"
  else
    echo "COUNT:$strikes"
  fi
}

self_test() {
  for executable in tokenzero tokenzero-codemode tokenzero-xtask       fszero-codemode graphzero-codemode zerostack-xtask       /opt/zs/bin/tokenzero-codemode /opt/zs/bin/tokenzero-xtask; do
    is_approved_executable "$executable" || {
      echo "self-test: rejected approved executable: $executable" >&2
      exit 1
    }
  done
  for executable in bash zsh cargo tokenzero-codemode-helper       /tmp/not-tokenzero /tmp/tokenzero-codemode-helper; do
    if is_approved_executable "$executable"; then
      echo "self-test: accepted unapproved executable: $executable" >&2
      exit 1
    fi
  done

  old_threshold="$THRESHOLD"
  old_window="$WINDOW"
  old_grace="$KILL_GRACE"
  THRESHOLD=85
  WINDOW=3
  KILL_GRACE=2
  [ "$(next_action 0 84.9 tokenzero-codemode)" = CLEAR ]
  [ "$(next_action 0 99.0 bash)" = CLEAR ]
  [ "$(next_action 0 99.0 tokenzero-codemode)" = COUNT:1 ]
  [ "$(next_action 1 99.0 tokenzero-codemode)" = COUNT:2 ]
  [ "$(next_action 2 99.0 tokenzero-codemode)" = TERM:3 ]
  [ "$(next_action 3 99.0 tokenzero-codemode)" = COUNT:4 ]
  [ "$(next_action 4 99.0 tokenzero-codemode)" = KILL:5 ]
  identity_matches "start:command" "start:command"
  if identity_matches "start:command" "different" || identity_matches "start:command" ""; then
    echo "self-test: PID identity fence accepted a stale process" >&2
    exit 1
  fi
  # Healthy short work resets before the sustained window and is never signalled.
  short_strikes=0
  for cpu in 99.0 99.0 0.0; do
    action=$(next_action "$short_strikes" "$cpu" tokenzero-xtask)
    case "$action" in
      COUNT:*) short_strikes=${action#COUNT:} ;;
      CLEAR) short_strikes=0 ;;
      *) echo "self-test: healthy short work was signalled: $action" >&2; exit 1 ;;
    esac
  done
  [ "$short_strikes" -eq 0 ]
  THRESHOLD="$old_threshold"
  WINDOW="$old_window"
  KILL_GRACE="$old_grace"
  echo "watchdog self-test: ok"
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit 0
fi

mkdir -p "$STATE_DIR"
log() { printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*" >>"$LOG"; }
strike_file() { printf '%s/%s.strikes' "$STATE_DIR" "$1"; }
identity_file() { printf '%s/%s.identity' "$STATE_DIR" "$1"; }

clear_state() {
  rm -f "$(strike_file "$1")" "$(identity_file "$1")"
}

get_strikes() {
  file=$(strike_file "$1")
  if [ -f "$file" ]; then cat "$file"; else echo 0; fi
}

process_identity() {
  ps -p "$1" -o lstart= -o comm= 2>/dev/null | tr -s ' ' | sed 's/^ //'
}

refresh_identity() {
  pid="$1"
  executable="$2"
  current=$(process_identity "$pid")
  [ -n "$current" ] || current="unknown:$executable"
  file=$(identity_file "$pid")
  previous=""
  [ ! -f "$file" ] || previous=$(cat "$file")
  if [ "$current" != "$previous" ]; then
    clear_state "$pid"
    printf '%s\n' "$current" >"$file"
  fi
}

signal_process() {
  signal="$1"
  pid="$2"
  file=$(identity_file "$pid")
  expected=""
  [ ! -f "$file" ] || expected=$(cat "$file")
  current=$(process_identity "$pid")
  if ! identity_matches "$expected" "$current"; then
    log "STALE_$signal pid=$pid identity_changed=1"
    clear_state "$pid"
    return
  fi
  if [ "$DRY_RUN" = 1 ]; then
    log "DRY_$signal pid=$pid"
  else
    kill "-$signal" "$pid" 2>/dev/null || true
  fi
}

log "watchdog start threshold=${THRESHOLD}% window=${WINDOW} grace=${KILL_GRACE} interval=${INTERVAL}s pid=$$ bash=$BASH_VERSION"

while true; do
  live=""
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    pid=$(echo "$line" | awk '{print $1}')
    cpu=$(echo "$line" | awk '{print $2}')
    executable=$(echo "$line" | awk '{print $3}')
    is_approved_executable "$executable" || continue
    live="$live $pid"
    refresh_identity "$pid" "$executable"
    action=$(next_action "$(get_strikes "$pid")" "$cpu" "$executable")
    case "$action" in
      CLEAR) clear_state "$pid" ;;
      COUNT:*)
        strikes=${action#COUNT:}
        printf '%s\n' "$strikes" >"$(strike_file "$pid")"
        if [ $((strikes % 10)) -eq 0 ]; then
          log "HIGH pid=$pid cpu=$cpu strikes=$strikes/$WINDOW executable=$executable"
        fi
        ;;
      TERM:*)
        strikes=${action#TERM:}
        printf '%s\n' "$strikes" >"$(strike_file "$pid")"
        log "TERM pid=$pid cpu=$cpu strikes=$strikes executable=$executable"
        signal_process TERM "$pid"
        ;;
      KILL:*)
        strikes=${action#KILL:}
        log "KILL pid=$pid cpu=$cpu strikes=$strikes executable=$executable"
        signal_process KILL "$pid"
        clear_state "$pid"
        ;;
    esac
  done <<EOF
$(ps -axo pid=,pcpu=,command= | awk '$2+0>=1')
EOF

  for file in "$STATE_DIR"/*.strikes "$STATE_DIR"/*.identity; do
    [ -e "$file" ] || continue
    pid=$(basename "$file")
    pid=${pid%%.*}
    case " $live " in
      *" $pid "*) ;;
      *) clear_state "$pid" ;;
    esac
  done
  sleep "$INTERVAL"
done
