#!/usr/bin/env bash
# A/B harness: copilot CLI with and without TokenZero MCP, N replicates each.
# Bash port of run_agent_demo.ps1: same plan, same metrics, same results schema.
#
# Requires: bash 3.2+, python3 (stats, metrics parsing, default HTTP server),
#           a tokenzero binary, and the copilot CLI.
#
# Usage:
#   ./demo/run_agent_demo.sh
#   ./demo/run_agent_demo.sh --replicates 5 --model gpt-5-mini --no-serve
set -euo pipefail

# --- Args -------------------------------------------------------------------
REPLICATES=3
MODEL='gpt-5-mini'
CONDITIONS='baseline tokenzero'
PORT=8765
NO_SERVE=0
NO_OPEN=0
PER_RUN_TIMEOUT_SEC=300
BINARY_PATH="${TOKENZERO_BINARY_PATH:-}"
COPILOT_PATH="${COPILOT_PATH:-}"

usage() {
    cat <<'USAGE'
Usage: run_agent_demo.sh [options]

  -r, --replicates N        Runs per condition (default: 3).
  -m, --model NAME          Model passed to copilot --model (default: gpt-5-mini).
      --conditions "A B"    Conditions to run (default: "baseline tokenzero").
  -p, --port N              Port for the results HTTP server (default: 8765).
      --no-serve            Do not start the HTTP server.
      --no-open             Do not open the visualization in a browser.
      --per-run-timeout N   Per-run timeout in seconds (default: 300).
  -b, --binary-path PATH    Path to tokenzero (or $TOKENZERO_BINARY_PATH).
      --copilot-path PATH   Path to the copilot CLI (or $COPILOT_PATH).
  -h, --help                Show this help.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -r|--replicates)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            REPLICATES="$2"; shift 2 ;;
        -m|--model)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            MODEL="$2"; shift 2 ;;
        --conditions)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            CONDITIONS="$2"; shift 2 ;;
        -p|--port)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            PORT="$2"; shift 2 ;;
        --no-serve)
            NO_SERVE=1; shift ;;
        --no-open)
            NO_OPEN=1; shift ;;
        --per-run-timeout)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            PER_RUN_TIMEOUT_SEC="$2"; shift 2 ;;
        -b|--binary-path)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            BINARY_PATH="$2"; shift 2 ;;
        --copilot-path)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            COPILOT_PATH="$2"; shift 2 ;;
        -h|--help)
            usage; exit 0 ;;
        *)
            echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

die()  { printf '%s\n' "$*" >&2; exit 1; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$DEMO_DIR/.." && pwd)"
RUNS_DIR="$DEMO_DIR/agent_runs"
CACHE_DIR="$DEMO_DIR/.cache"
RESULTS_PATH="$DEMO_DIR/agent_results.json"
MCP_CONFIG="$DEMO_DIR/tokenzero-mcp.json"
mkdir -p "$RUNS_DIR" "$CACHE_DIR"

have python3 || die 'python3 is required by run_agent_demo.sh (stats, metrics parsing, HTTP server).'

# --- Executables -------------------------------------------------------------
TOKENZERO_BIN="$BINARY_PATH"
if [[ -z "$TOKENZERO_BIN" ]]; then
    TOKENZERO_BIN="$(command -v tokenzero 2>/dev/null || true)"
fi
if [[ -z "$TOKENZERO_BIN" || ! -e "$TOKENZERO_BIN" ]]; then
    die 'tokenzero binary not found. Pass --binary-path.'
fi
COPILOT_BIN="$COPILOT_PATH"
if [[ -z "$COPILOT_BIN" ]]; then
    COPILOT_BIN="$(command -v copilot 2>/dev/null || true)"
fi
[[ -n "$COPILOT_BIN" ]] || die 'copilot CLI not found on PATH. Pass --copilot-path.'

echo "tokenzero: $TOKENZERO_BIN"
echo "copilot:   $COPILOT_BIN"
echo "repo:      $REPO_DIR"
echo "runs dir:  $RUNS_DIR"

# --- MCP config ----------------------------------------------------------------
# Template shape lives in docs/install.md ("Manual MCP config"); generated inline here.
TZ_BIN_ENV="$TOKENZERO_BIN" REPO_ENV="$REPO_DIR" CACHE_ENV="$CACHE_DIR/agent-tokenzero.json" \
python3 - "$MCP_CONFIG" <<'PY'
import os, sys
out_path = sys.argv[1]
def esc(s):
    return s.replace('\\', '\\\\').replace('"', '\\"')
config = '{"mcpServers":{"tokenzero":{"type":"local","command":"__TOKENZERO_BIN__","args":["mcp-server","--allowed-root","__REPO__","--cache-path","__CACHE__"],"tools":["*"]}}}'
config = config.replace('__TOKENZERO_BIN__', esc(os.environ['TZ_BIN_ENV']))
config = config.replace('__REPO__', esc(os.environ['REPO_ENV']))
config = config.replace('__CACHE__', esc(os.environ['CACHE_ENV']))
open(out_path, 'w', encoding='utf-8').write(config)
PY
echo "wrote MCP config: $MCP_CONFIG"

NATIVE_DENY='view,bash,powershell,read_powershell,str_replace_editor,create,edit,grep,glob,find,read,write,run'
PROMPT="$(cat <<'EOF'
TASK: Find every place a JSON-RPC error response is constructed in the
tokenzero-mcp crate (crates/tokenzero-mcp/src/). For each, report file:line
and a short note about when it fires.

RULES (follow exactly):
- Start with a tool call IMMEDIATELY. Do not write a plan first.
- Use at most 6 tool calls.
- Final reply must be ONLY a markdown table with columns:
  | File:Line | Code | When |
- No prose. No reasoning. No "intent". Table only.
EOF
)"

# --- Run plan ------------------------------------------------------------------
read -ra CONDITIONS_ARR <<< "${CONDITIONS//,/ }"

runs=()
index=0
for (( rep=1; rep<=REPLICATES; rep++ )); do
    for cond in "${CONDITIONS_ARR[@]}"; do
        index=$((index + 1))
        runs+=("$(python3 -c '
import json, sys
print(json.dumps({"index": int(sys.argv[3]), "condition": sys.argv[1], "replicate": int(sys.argv[2]),
                  "status": "pending", "wall_ms": None, "api_ms": None, "input_tokens": None,
                  "output_tokens": None, "tool_calls": None, "tool_output_tokens": None,
                  "exit_code": None, "note": "", "jsonl_path": ""}))' "$cond" "$rep" "$index")")
    done
done

run_get() { # idx key -> value on stdout (empty for null)
    printf '%s' "${runs[$1]}" | python3 -c '
import json, sys
v = json.load(sys.stdin).get(sys.argv[1])
print("" if v is None else v, end="")' "$2"
}

run_set_val() { # idx key json-literal-value
    runs[$1]="$(printf '%s' "${runs[$1]}" | python3 -c '
import json, sys
d = json.load(sys.stdin)
d[sys.argv[1]] = json.loads(sys.argv[2])
print(json.dumps(d))' "$2" "$3")"
}

run_set_str() { # idx key string-value
    runs[$1]="$(printf '%s' "${runs[$1]}" | python3 -c '
import json, sys
d = json.load(sys.stdin)
d[sys.argv[1]] = sys.argv[2]
print(json.dumps(d))' "$2" "$3")"
}

# --- Results writer -------------------------------------------------------------
START_EPOCH="$(date +%s)"
STARTED_AT="$(date '+%Y-%m-%d %H:%M:%S')"
META_JSON="$(python3 -c '
import json, sys
print(json.dumps({"task": "jsonrpc_errors", "model": sys.argv[1], "replicates": int(sys.argv[2]),
                  "conditions": sys.argv[3].split(), "repo": sys.argv[4], "started_at": sys.argv[5]}))' \
    "$MODEL" "$REPLICATES" "${CONDITIONS_ARR[*]}" "$REPO_DIR" "$STARTED_AT")"

save_results() {
    local elapsed=$(( ($(date +%s) - START_EPOCH) * 1000 ))
    local runs_nd="$CACHE_DIR/.runs.ndjson"
    if ((${#runs[@]})); then
        printf '%s\n' "${runs[@]}" > "$runs_nd"
    else
        : > "$runs_nd"
    fi
    META_JSON="$META_JSON" ELAPSED="$elapsed" python3 - "$runs_nd" "$RESULTS_PATH" <<'PY'
import json, math, os, sys
runs = [json.loads(l) for l in open(sys.argv[1], encoding='utf-8') if l.strip()]
meta = json.loads(os.environ['META_JSON'])
elapsed = int(os.environ['ELAPSED'])
def count(status):
    return sum(1 for r in runs if r.get('status') == status)
def stats(cond):
    rows = [r for r in runs if r.get('condition') == cond and r.get('status') == 'done']
    if not rows:
        return {'n': 0}
    def vals(p):
        return [r[p] for r in rows if r.get(p) is not None]
    def mean(p):
        v = vals(p)
        return (sum(v) / len(v)) if v else None
    def std(p):
        v = vals(p)
        if len(v) < 2:
            return None
        m = sum(v) / len(v)
        return math.sqrt(sum((x - m) ** 2 for x in v) / (len(v) - 1))
    return {'n': len(rows),
            'mean_tool_output_tokens': mean('tool_output_tokens'),
            'stddev_tool_output_tokens': std('tool_output_tokens'),
            'mean_tool_calls': mean('tool_calls'),
            'mean_wall_ms': mean('wall_ms'),
            'mean_api_ms': mean('api_ms'),
            'mean_input_tokens': mean('input_tokens'),
            'mean_output_tokens': mean('output_tokens')}
payload = {'meta': meta,
           'totals': {'done': count('done'), 'failed': count('failed'), 'running': count('running'),
                      'total': len(runs), 'elapsed_ms': elapsed},
           'summary': {'baseline': stats('baseline'), 'tokenzero': stats('tokenzero')},
           'runs': runs}
tmp = sys.argv[2] + '.tmp'
with open(tmp, 'w', encoding='utf-8') as f:
    json.dump(payload, f, ensure_ascii=False, indent=2)
os.replace(tmp, sys.argv[2])
PY
}

save_results
echo "wrote initial: $RESULTS_PATH"

VIZ_PATH="$DEMO_DIR/agent_viz.html"
if [[ ! -f "$VIZ_PATH" ]]; then
    if [[ -f "$DEMO_DIR/build_agent_viz.sh" ]]; then
        bash "$DEMO_DIR/build_agent_viz.sh" >/dev/null
    elif [[ -f "$DEMO_DIR/build_agent_viz.ps1" ]]; then
        ps_host="$(command -v pwsh 2>/dev/null || command -v powershell 2>/dev/null || true)"
        [[ -n "$ps_host" ]] || die 'PowerShell host not found for rendering visualization.'
        "$ps_host" -NoProfile -ExecutionPolicy Bypass -File "$DEMO_DIR/build_agent_viz.ps1" >/dev/null
    fi
fi

# --- HTTP server ------------------------------------------------------------------
SERVER_PID=""
if (( ! NO_SERVE )); then
    echo "starting HTTP server on port $PORT (serving $DEMO_DIR)..."
    PY="$(command -v python 2>/dev/null || command -v python3 2>/dev/null || command -v py 2>/dev/null || true)"
    [[ -n "$PY" ]] || die "python not found; pass --no-serve and serve $DEMO_DIR yourself."
    (cd "$DEMO_DIR" && exec "$PY" -u -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1) &
    SERVER_PID=$!
    sleep 0.6
    kill -0 "$SERVER_PID" 2>/dev/null || die "HTTP server exited immediately (port $PORT in use?)."
    echo "server PID: $SERVER_PID"
    if (( ! NO_OPEN )); then
        url="http://127.0.0.1:$PORT/agent_viz.html"
        echo "opening $url"
        case "$(uname -s)" in
            Darwin) open "$url" >/dev/null 2>&1 & ;;
            Linux)  if have xdg-open; then xdg-open "$url" >/dev/null 2>&1 & fi ;;
        esac
    fi
fi

# --- Helpers ------------------------------------------------------------------------
now_ms() {
    if have perl; then
        perl -MTime::HiRes=time -e 'printf "%d", time() * 1000'
    else
        python3 -c 'import time; print(int(time.time() * 1000))'
    fi
}

new_guid() {
    if have uuidgen; then
        uuidgen | tr -d '-'
    elif [[ -r /proc/sys/kernel/random/uuid ]]; then
        tr -d '-' < /proc/sys/kernel/random/uuid
    else
        python3 -c 'import uuid; print(uuid.uuid4().hex)'
    fi
}

mid_run_progress() { # jsonl -> "lines tool_calls messages" (fails if unreadable/missing)
    [[ -f "$1" ]] || return 1
    awk '
        /"type"[[:space:]]*:[[:space:]]*"tool\.execution_start"/ { t++ }
        /"type"[[:space:]]*:[[:space:]]*"assistant\.message"/    { m++ }
        /./                                                       { l++ }
        END { print l + 0, t + 0, m + 0 }' "$1" 2>/dev/null || return 1
}

measure_tool_tokens() { # textfile -> token count on stdout
    local f="$1"
    [[ -s "$f" ]] || { echo 0; return; }
    local out tok rc guid
    guid="$(new_guid)"
    local cache="$CACHE_DIR/ingest-$guid.json"
    if out="$("$TOKENZERO_BIN" ingest --stdin --json --cache-path "$cache" < "$f" 2>/dev/null)"; then
        if [[ -z "$out" ]]; then
            echo 0; return
        fi
        rc=0
        tok="$(printf '%s' "$out" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except ValueError:
    sys.exit(2)
for key in ("accounting.raw_tokens", "tokens", "token_count"):
    cur = d
    for part in key.split("."):
        if not isinstance(cur, dict) or part not in cur:
            cur = None
            break
        cur = cur[part]
    if cur is not None:
        print(int(cur))
        sys.exit(0)
sys.exit(3)' 2>/dev/null)" || rc=$?
        case "$rc" in
            0) echo "$tok"; return ;;
            3) echo 0; return ;;
        esac
    fi
    # Fallback: rough chars/4 estimate (matches the ps1 fallback semantics).
    local n
    n="$(wc -c < "$f" | tr -d ' ')"
    echo $(( n / 4 ))
}

parse_run_metrics() { # jsonl -> one JSON line on stdout; writes tool text to <jsonl>.tooltext
    python3 - "$1" "$1.tooltext" <<'PY'
import json, sys
path, textout = sys.argv[1], sys.argv[2]
events = []
with open(path, encoding='utf-8', errors='replace') as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        try:
            events.append(json.loads(line))
        except ValueError:
            pass
output_tokens = 0
for e in events:
    if e.get('type') == 'assistant.message':
        d = e.get('data') or {}
        v = d.get('outputTokens')
        output_tokens += int(v) if v else 0
tool_events = [e for e in events if e.get('type') == 'tool.execution_complete']
parts = []
def add(v):
    if v is None:
        return
    parts.append(v if isinstance(v, str) else str(v))
for e in tool_events:
    d = e.get('data') or {}
    r = d.get('result')
    if r is None:
        continue
    if isinstance(r, str):
        add(r)
        continue
    if isinstance(r, dict):
        c = r.get('content')
        if isinstance(c, str):
            add(c)
        elif c:
            for item in c:
                if isinstance(item, dict) and item.get('text'):
                    add(item['text'])
                elif isinstance(item, str):
                    add(item)
        dc = r.get('detailedContent')
        if isinstance(dc, str):
            add(dc)
        elif dc:
            add(json.dumps(dc, separators=(',', ':')))
        if r.get('output'):
            add(r['output'])
with open(textout, 'w', encoding='utf-8') as f:
    f.write(''.join(p + '\n' for p in parts))
last = None
for e in events:
    if e.get('type') == 'result':
        last = e
usage = None
if last:
    usage = last.get('usage')
    if not usage:
        d = last.get('data')
        if isinstance(d, dict):
            usage = d.get('usage')
usage = usage or {}
def num(key):
    v = usage.get(key)
    return None if v is None else int(v)
print(json.dumps({'output_tokens': output_tokens, 'input_tokens': None,
                  'tool_calls': len(tool_events),
                  'api_ms': num('totalApiDurationMs'),
                  'session_ms': num('sessionDurationMs'),
                  'premium_requests': num('premiumRequests')}))
PY
}

copy_metrics() { # run_idx metrics_json nonnull(0|1)
    local ri="$1" json="$2" nonnull="$3" key val
    for key in output_tokens input_tokens tool_calls tool_output_tokens api_ms session_ms premium_requests; do
        val="$(printf '%s' "$json" | python3 -c '
import json, sys
v = json.load(sys.stdin).get(sys.argv[1])
print("" if v is None else v, end="")' "$key")"
        if (( nonnull )) && [[ -z "$val" ]]; then
            continue
        fi
        if [[ -z "$val" ]]; then
            run_set_val "$ri" "$key" null
        else
            run_set_val "$ri" "$key" "$val"
        fi
    done
}

merge_tool_tokens() { # metrics_json tool_tok -> metrics_json with tool_output_tokens set
    printf '%s' "$1" | python3 -c '
import json, sys
d = json.load(sys.stdin)
d["tool_output_tokens"] = int(sys.argv[1])
print(json.dumps(d))' "$2"
}

update_live_progress() { # run_idx jsonl elapsed_ms
    local ri="$1" jsonl="$2" elapsed="$3" prog lines tc msg
    prog="$(mid_run_progress "$jsonl")" || return 0
    [[ -n "$prog" ]] || return 0
    read -r lines tc msg <<< "$prog"
    run_set_str "$ri" note "live: $lines events, $tc tool calls, $msg msgs ($(( elapsed / 1000 ))s)"
    run_set_val "$ri" wall_ms "$elapsed"
    run_set_val "$ri" tool_calls "$tc"
    save_results
}

RC_EXIT_CODE=0
RC_WALL_MS=0
RC_TIMED_OUT=0
invoke_copilot_run() { # condition jsonl run_idx -> RC_* globals
    local condition="$1" jsonl="$2" ri="$3"
    local stderr_path="$jsonl.err"
    local -a args=(-p "$PROMPT" --output-format json --model "$MODEL" --no-ask-user
                   --allow-all-paths -C "$REPO_DIR" --log-level error)
    if [[ "$condition" == baseline ]]; then
        args+=(--allow-all-tools)
    else
        args+=(--additional-mcp-config "@$MCP_CONFIG" --allow-all-tools
               --excluded-tools "$NATIVE_DENY")
    fi
    args+=(--disable-builtin-mcps)
    local server
    for server in Azure icm-mcp-prod github; do
        args+=(--disable-mcp-server "$server")
    done
    rm -f "$jsonl" "$stderr_path"

    local start_ms now elapsed pid rc
    start_ms="$(now_ms)"
    (cd "$REPO_DIR" && exec "$COPILOT_BIN" "${args[@]}" > "$jsonl" 2> "$stderr_path") &
    pid=$!
    RC_TIMED_OUT=0
    while kill -0 "$pid" 2>/dev/null; do
        sleep 2
        kill -0 "$pid" 2>/dev/null || break
        now="$(now_ms)"
        elapsed=$(( now - start_ms ))
        update_live_progress "$ri" "$jsonl" "$elapsed"
        if (( elapsed > PER_RUN_TIMEOUT_SEC * 1000 )); then
            RC_TIMED_OUT=1
            kill "$pid" 2>/dev/null || true
            for _ in {1..10}; do
                kill -0 "$pid" 2>/dev/null || break
                sleep 0.5
            done
            if kill -0 "$pid" 2>/dev/null; then
                kill -9 "$pid" 2>/dev/null || true
                sleep 0.2
            fi
            kill -0 "$pid" 2>/dev/null && die "timed out waiting for process $pid to exit after kill"
            break
        fi
    done
    rc=0
    wait "$pid" || rc=$?
    now="$(now_ms)"
    RC_WALL_MS=$(( now - start_ms ))
    if (( RC_TIMED_OUT )); then
        RC_EXIT_CODE=-1
    else
        RC_EXIT_CODE=$rc
    fi
}

# --- Main loop -----------------------------------------------------------------------
echo
echo "starting ${#runs[@]} runs..."
total_runs=${#runs[@]}
for ri in "${!runs[@]}"; do
    cond="$(run_get "$ri" condition)"
    rep="$(run_get "$ri" replicate)"
    idx="$(run_get "$ri" index)"
    tag="$cond-r$rep"
    jsonl="$RUNS_DIR/$tag.jsonl"
    run_set_str "$ri" jsonl_path "$jsonl"
    run_set_str "$ri" status running
    save_results
    printf '  [%s/%s] %s ... ' "$idx" "$total_runs" "$tag"

    invoke_copilot_run "$cond" "$jsonl" "$ri"
    run_set_val "$ri" wall_ms "$RC_WALL_MS"
    run_set_val "$ri" exit_code "$RC_EXIT_CODE"
    wall_sec="$(awk -v m="$RC_WALL_MS" 'BEGIN { printf "%.1f", m / 1000 }')"

    if (( RC_TIMED_OUT )); then
        run_set_str "$ri" status failed
        run_set_str "$ri" note "timeout @ $PER_RUN_TIMEOUT_SEC s"
        echo "TIMEOUT ($wall_sec s)"
        if pm_json="$(parse_run_metrics "$jsonl" 2>/dev/null)"; then
            tool_tok="$(measure_tool_tokens "$jsonl.tooltext")"
            pm_json="$(merge_tool_tokens "$pm_json" "$tool_tok")"
            copy_metrics "$ri" "$pm_json" 1
            cur_note="$(run_get "$ri" note)"
            run_set_str "$ri" note "$cur_note (partial: $(run_get "$ri" tool_calls) tool calls, $(run_get "$ri" output_tokens) out tok)"
        fi
    elif (( RC_EXIT_CODE != 0 )); then
        run_set_str "$ri" status failed
        err_line="$(grep -m1 . "$jsonl.err" 2>/dev/null || true)"
        [[ -z "$err_line" ]] && err_line='(no stderr)'
        run_set_str "$ri" note "exit=$RC_EXIT_CODE $err_line"
        echo "FAILED exit=$RC_EXIT_CODE ($wall_sec s)"
    else
        if pm_json="$(parse_run_metrics "$jsonl" 2>/dev/null)"; then
            tool_tok="$(measure_tool_tokens "$jsonl.tooltext")"
            pm_json="$(merge_tool_tokens "$pm_json" "$tool_tok")"
            copy_metrics "$ri" "$pm_json" 0
            run_set_str "$ri" status 'done'
            api_ms="$(run_get "$ri" api_ms)"
            if [[ -n "$api_ms" ]]; then
                api_sec="$(awk -v m="$api_ms" 'BEGIN { printf "%.1f", m / 1000 }')"
            else
                api_sec='n/a'
            fi
            echo "OK  wall=${wall_sec}s api=${api_sec}s tools=$(run_get "$ri" tool_calls) toolTok=$(run_get "$ri" tool_output_tokens) outTok=$(run_get "$ri" output_tokens)"
        else
            run_set_str "$ri" status failed
            run_set_str "$ri" note 'parse error: parse_run_metrics failed'
            echo 'PARSE-FAIL: parse_run_metrics failed'
        fi
    fi
    save_results
done

echo
echo "all runs complete. results: $RESULTS_PATH"
if (( ! NO_SERVE )) && [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "HTTP server still running on port $PORT (PID $SERVER_PID)."
    echo "Stop with: kill $SERVER_PID"
fi
