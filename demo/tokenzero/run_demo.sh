#!/usr/bin/env bash
# Self-contained TokenZero demo for macOS and Linux.
# Bash port of run_demo.ps1: same scenarios, same JSON schema, same summary table.
#
# Walks an AI agent's "tool day in the life" through TokenZero and counts the
# tokens TokenZero hid vs the tokens the agent actually consumed, using
# TokenZero's own tokenizer on both sides (same-tokenizer compare).
#
# The demo:
#   1. Resolves a `tokenzero` binary
#        - if --binary-path (or $TOKENZERO_BINARY_PATH) is given, uses it
#        - else if `tokenzero` is on PATH, uses that
#        - else downloads the GitHub Release asset for the current OS/CPU
#          into demo/.tokenzero-bin/, verifies SHA256, and runs from there.
#   2. Uses an isolated cache file under demo/.cache/ so the demo never
#      touches the user's real TokenZero state.
#   3. Runs five real scenarios against THIS REPO:
#        - small read        (capsule-never-costs-more-than-raw guarantee)
#        - large read        (heavy savings + tz:// blob ref)
#        - grep `fn `        (recoverable hit set across crates/)
#        - expand            (round-trip the large-read ref, byte-exact check)
#        - recall            (re-find content already in the cache, no re-grep)
#        - run -- <cmd>      (cross-platform shell capture)
#   4. Counts raw tokens by piping the raw output through
#      `tokenzero ingest --stdin` and reading accounting.raw_tokens.
#   5. Prints a Markdown summary table and writes demo/demo_results.json.
#
# Requires: bash 3.2+, curl, tar, shasum (or sha256sum), and jq or python3.
#
# Usage:
#   ./demo/run_demo.sh
#   ./demo/run_demo.sh --binary-path /usr/local/bin/tokenzero
set -euo pipefail

# --- Args -------------------------------------------------------------------
BINARY_PATH="${TOKENZERO_BINARY_PATH:-}"
RELEASE_TAG="${TOKENZERO_RELEASE_TAG:-v1.0.1}"
SKIP_DOWNLOAD=0
NO_VIZ=0
OPEN_VIZ=0

usage() {
    cat <<'USAGE'
Usage: run_demo.sh [options]

  -b, --binary-path PATH   Explicit path to tokenzero (or $TOKENZERO_BINARY_PATH).
                           Falls back to PATH, then to a downloaded release binary.
  -t, --release-tag TAG    Release tag to download if a binary has to be fetched
                           (default: v1.0.1, or $TOKENZERO_RELEASE_TAG).
      --skip-download      Fail instead of downloading when no binary is found.
      --no-viz             Do not render demo_viz.html after the run.
      --open-viz           Open the rendered visualization when done.
  -h, --help               Show this help.
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -b|--binary-path)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            BINARY_PATH="$2"; shift 2 ;;
        -t|--release-tag)
            [[ $# -ge 2 ]] || { echo "Missing value for $1" >&2; exit 2; }
            RELEASE_TAG="$2"; shift 2 ;;
        --skip-download)
            SKIP_DOWNLOAD=1; shift ;;
        --no-viz)
            NO_VIZ=1; shift ;;
        --open-viz)
            OPEN_VIZ=1; shift ;;
        -h|--help)
            usage; exit 0 ;;
        *)
            echo "Unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

die()  { printf '%s\n' "$*" >&2; exit 1; }
warn() { printf 'WARNING: %s\n' "$*" >&2; }
have() { command -v "$1" >/dev/null 2>&1; }

# --- Locations ---------------------------------------------------------------
DEMO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$DEMO_DIR/.." && pwd)"
BIN_DIR="$DEMO_DIR/.tokenzero-bin"
CACHE_DIR="$DEMO_DIR/.cache"
mkdir -p "$BIN_DIR" "$CACHE_DIR"

CACHE_PATH="$CACHE_DIR/recovery-cache.json"
COUNT_CACHE="$CACHE_DIR/count-cache.json"
RESULTS_PATH="$DEMO_DIR/demo_results.json"
ROWS_JSONL="$CACHE_DIR/.rows.jsonl"

# Start every run from a clean cache so seen-set dedup numbers are reproducible.
rm -f "$CACHE_PATH" "$COUNT_CACHE"
: > "$ROWS_JSONL"

# --- JSON backend ------------------------------------------------------------
if have jq; then
    JSON_TOOL=jq
elif have python3; then
    JSON_TOOL=python3
else
    die 'error: need jq or python3 for JSON parsing'
fi

json_accounting_raw() {
    if [[ "$JSON_TOOL" == jq ]]; then
        jq -r '.accounting.raw_tokens // empty'
    else
        python3 -c 'import sys,json; print(json.load(sys.stdin)["accounting"]["raw_tokens"], end="")'
    fi
}

json_accounting_visible() {
    if [[ "$JSON_TOOL" == jq ]]; then
        jq -r '.accounting.visible_tokens // empty'
    else
        python3 -c 'import sys,json; print(json.load(sys.stdin)["accounting"]["visible_tokens"], end="")'
    fi
}

json_first_blob_ref() {
    if [[ "$JSON_TOOL" == jq ]]; then
        jq -r '[(.refs // [])[] | select(.kind=="blob")][0].ref // empty'
    else
        python3 -c '
import sys, json
d = json.load(sys.stdin)
blobs = [r for r in (d.get("refs") or []) if r.get("kind") == "blob"]
print(blobs[0]["ref"] if blobs else "", end="")'
    fi
}

json_mcp_second_call_text() {
    if [[ "$JSON_TOOL" == jq ]]; then
        jq -Rrn '[inputs | fromjson? | select(.id==3 and (.result.content != null))][0].result.content[0].text // empty'
    else
        python3 -c '
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        o = json.loads(line)
    except ValueError:
        continue
    r = o.get("result") if isinstance(o, dict) else None
    if o.get("id") == 3 and isinstance(r, dict) and r.get("content"):
        sys.stdout.write(str(r["content"][0].get("text", "")))
        break'
    fi
}

json_row() { # name raw vis pct note -> compact JSON object on stdout
    if [[ "$JSON_TOOL" == jq ]]; then
        jq -cn --arg w "$1" --argjson r "$2" --argjson v "$3" --argjson p "$4" --arg n "$5" \
            '{workload:$w, raw_tokens:$r, visible_tokens:$v, savings_pct:$p, note:$n}'
    else
        python3 -c '
import json, sys
_, w, r, v, p, n = sys.argv
print(json.dumps({"workload": w, "raw_tokens": int(r), "visible_tokens": int(v),
                  "savings_pct": float(p), "note": n}, ensure_ascii=False))' "$1" "$2" "$3" "$4" "$5"
    fi
}

json_rows_to_tsv() { # rows JSONL on stdin -> workload\traw\tvis\tnote per line
    if [[ "$JSON_TOOL" == jq ]]; then
        jq -r '[.workload, (.raw_tokens|tostring), (.visible_tokens|tostring), .note] | @tsv'
    else
        python3 -c '
import json, sys
for line in sys.stdin:
    if not line.strip():
        continue
    d = json.loads(line)
    print("\t".join([d["workload"], str(d["raw_tokens"]), str(d["visible_tokens"]), d["note"]]))'
    fi
}

json_totals() { # rows JSONL on stdin -> "<raw>\t<vis>" for rows with raw>0
    if [[ "$JSON_TOOL" == jq ]]; then
        jq -rs '[.[] | select(.raw_tokens > 0)] as $r
                | [(($r | map(.raw_tokens) | add) // 0), (($r | map(.visible_tokens) | add) // 0)] | @tsv'
    else
        python3 -c '
import json, sys
rows = [json.loads(l) for l in sys.stdin if l.strip()]
rows = [r for r in rows if r["raw_tokens"] > 0]
print(sum(r["raw_tokens"] for r in rows), sum(r["visible_tokens"] for r in rows), sep="\t")'
    fi
}

json_emit_results() { # total_raw total_vis total_pct -> writes $RESULTS_PATH
    local total_raw="$1" total_vis="$2" total_pct="$3"
    if [[ "$JSON_TOOL" == jq ]]; then
        jq -n --arg v "$TZ_VERSION" --arg b "$TZ_BIN" --arg repo "$REPO_DIR" --arg c "$CACHE_PATH" \
            --slurpfile w "$ROWS_JSONL" \
            --argjson tr "$total_raw" --argjson tv "$total_vis" --argjson tp "$total_pct" \
            '{tokenzero_version:$v, binary:$b, repo:$repo, cache:$c, workloads:$w,
              totals:{raw_tokens:$tr, visible_tokens:$tv, savings_pct:$tp}}' > "$RESULTS_PATH"
    else
        ROWS_JSONL="$ROWS_JSONL" python3 - "$RESULTS_PATH" "$TZ_VERSION" "$TZ_BIN" "$REPO_DIR" "$CACHE_PATH" \
            "$total_raw" "$total_vis" "$total_pct" <<'PY'
import json, os, sys
path, ver, binp, repo, cache, tr, tv, tp = sys.argv[1:9]
rows = [json.loads(l) for l in open(os.environ["ROWS_JSONL"], encoding="utf-8") if l.strip()]
payload = {"tokenzero_version": ver, "binary": binp, "repo": repo, "cache": cache,
           "workloads": rows,
           "totals": {"raw_tokens": int(tr), "visible_tokens": int(tv), "savings_pct": float(tp)}}
with open(path, "w", encoding="utf-8") as f:
    json.dump(payload, f, ensure_ascii=False, indent=2)
PY
    fi
}

# --- Platform helpers ---------------------------------------------------------
is_windows() { case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) return 0 ;; *) return 1 ;; esac }

tokenzero_exe_name() {
    if is_windows; then echo 'tokenzero.exe'; else echo 'tokenzero'; fi
}

compute_release_asset() { # sets ASSET_RID ASSET_EXT ASSET_NAME
    local os arch
    os="$(uname -s)"; arch="$(uname -m)"
    case "$os" in
        MINGW*|MSYS*|CYGWIN*)
            case "$arch" in
                x86_64|amd64) ASSET_RID='x86_64-pc-windows-msvc'; ASSET_EXT='.zip' ;;
                *) die "unsupported platform for release download: OS=$os ARCH=$arch" ;;
            esac ;;
        Linux)
            case "$arch" in
                x86_64|amd64) ASSET_RID='x86_64-unknown-linux-gnu'; ASSET_EXT='.tar.gz' ;;
                *) die "unsupported platform for release download: OS=$os ARCH=$arch" ;;
            esac ;;
        Darwin)
            case "$arch" in
                arm64|aarch64) ASSET_RID='aarch64-apple-darwin'; ASSET_EXT='.tar.gz' ;;
                x86_64)        ASSET_RID='x86_64-apple-darwin';  ASSET_EXT='.tar.gz' ;;
                *) die "unsupported platform for release download: OS=$os ARCH=$arch" ;;
            esac ;;
        *) die "unsupported platform for release download: OS=$os ARCH=$arch" ;;
    esac
    ASSET_NAME="tokenzero-$RELEASE_TAG-$ASSET_RID$ASSET_EXT"
}

# --- Binary resolution ---------------------------------------------------------
download_release_binary() { # $1 = executable name; extracts into $BIN_DIR
    local exe="$1" base zip sha expected actual extract found
    compute_release_asset
    echo "==> Downloading TokenZero $RELEASE_TAG ($ASSET_RID) into $BIN_DIR"
    base="https://github.com/AdityaVG13/tokenzero/releases/download/$RELEASE_TAG"
    zip="$BIN_DIR/$ASSET_NAME"
    sha="$zip.sha256"

    curl -fsSL "$base/$ASSET_NAME" -o "$zip"
    curl -fsSL "$base/$ASSET_NAME.sha256" -o "$sha"

    expected="$(awk 'NR==1 {print $1}' "$sha" | tr 'A-F' 'a-f')"
    if have shasum; then
        actual="$(shasum -a 256 "$zip" | awk '{print $1}' | tr 'A-F' 'a-f')"
    elif have sha256sum; then
        actual="$(sha256sum "$zip" | awk '{print $1}' | tr 'A-F' 'a-f')"
    else
        die 'need shasum or sha256sum to verify the release asset'
    fi
    if [[ "$expected" != "$actual" ]]; then
        die "SHA256 mismatch for $ASSET_NAME
  expected: $expected
  actual:   $actual"
    fi

    extract="$BIN_DIR/extract"
    rm -rf "$extract"
    mkdir -p "$extract"
    if [[ "$ASSET_EXT" == .zip ]]; then
        unzip -q -o "$zip" -d "$extract"
    else
        tar -xzf "$zip" -C "$extract" || die "tar failed to extract $ASSET_NAME"
    fi
    found="$(find "$extract" -type f -name "$exe" | head -n 1)"
    [[ -n "$found" ]] || die "$exe not found inside $ASSET_NAME"
    cp -f "$found" "$BIN_DIR/$exe"
    chmod +x "$BIN_DIR/$exe" 2>/dev/null || true
}

RESOLVED_BINARY=""
resolve_tokenzero_binary() { # sets RESOLVED_BINARY
    local on_path exe cached
    if [[ -n "$BINARY_PATH" ]]; then
        [[ -e "$BINARY_PATH" ]] || die "Binary not found at --binary-path: $BINARY_PATH"
        RESOLVED_BINARY="$(cd "$(dirname "$BINARY_PATH")" && printf '%s/%s\n' "$PWD" "$(basename "$BINARY_PATH")")"
        return
    fi
    on_path="$(command -v tokenzero 2>/dev/null || true)"
    if [[ -n "$on_path" ]]; then RESOLVED_BINARY="$on_path"; return; fi

    exe="$(tokenzero_exe_name)"
    cached="$BIN_DIR/$exe"
    if [[ -f "$cached" ]]; then RESOLVED_BINARY="$cached"; return; fi

    (( SKIP_DOWNLOAD )) && die 'tokenzero binary not found and --skip-download was set.'

    download_release_binary "$exe"
    RESOLVED_BINARY="$cached"
}

resolve_tokenzero_binary
TZ_BIN="$RESOLVED_BINARY"
echo "==> Using binary: $TZ_BIN"
if ! TZ_VERSION="$("$TZ_BIN" --version 2>&1)"; then
    die "resolved tokenzero binary is not runnable on this host: $TZ_BIN ($TZ_VERSION)"
fi
TZ_VERSION="$(printf '%s' "$TZ_VERSION" | paste -sd' ' -)"
echo "    $TZ_VERSION"

# --- Helpers -----------------------------------------------------------------
invoke_tz() { # runs tokenzero with stdout captured; dies with stderr on failure
    local out err rc
    err="$(mktemp "${TMPDIR:-/tmp}/tz-demo-err.XXXXXX")"
    set +e
    out="$("$TZ_BIN" "$@" 2>"$err")"
    rc=$?
    set -e
    if [[ $rc -ne 0 ]]; then
        printf 'tokenzero %s failed (exit=%s)\nSTDERR:\n%s\n' "$*" "$rc" "$(cat "$err")" >&2
        rm -f "$err"
        return "$rc"
    fi
    rm -f "$err"
    printf '%s' "$out"
}

count_raw_tokens() { # text on stdin -> raw token count on stdout
    local out
    out="$(invoke_tz ingest --stdin --json --cache-path "$COUNT_CACHE")"
    printf '%s' "$out" | json_accounting_raw
}

slurp() { # slurp PATH -> RAW_TEXT = exact file bytes (incl. trailing newlines)
    RAW_TEXT="$(cat "$1"; printf 'x')"
    RAW_TEXT="${RAW_TEXT%x}"
}

trim_end_crlf() { # echoes $1 with trailing CR/LF removed (ps1 TrimEnd("\r","\n"))
    local s="$1"
    s="$(printf '%s' "$s")"
    while [[ "$s" == *$'\r' ]]; do s="${s%$'\r'}"; done
    printf '%s' "$s"
}

format_pct() { # raw vis -> " 97.5%" style field, or "   -" when raw<=0
    local raw="$1" vis="$2"
    if (( raw <= 0 )); then
        printf '   -'
        return
    fi
    awk -v r="$raw" -v v="$vis" 'BEGIN { printf "%5.1f%%", 100.0 * (r - v) / r }'
}

add_row() { # name raw vis note
    local name="$1" raw="$2" vis="$3" note="$4" pct
    if (( raw > 0 )); then
        pct="$(awk -v r="$raw" -v v="$vis" 'BEGIN { printf "%.1f", 100.0 * (r - v) / r }')"
    else
        pct=0
    fi
    json_row "$name" "$raw" "$vis" "$pct" "$note" >> "$ROWS_JSONL"
}

# --- Scenarios ---------------------------------------------------------------
cd "$REPO_DIR"

echo
echo '=== TokenZero demo ==='
echo "Repo: $REPO_DIR"
echo "Cache (isolated): $CACHE_PATH"
echo

LARGE_REF=""
GREP_RAW_TOK=0

# 1. Small file pass-through (capsule-never-costs-more guarantee)
SMALL_FILE="crates/tokenzero-cli/Cargo.toml"
if [[ -f "$SMALL_FILE" ]]; then
    echo "[1/7] small read  : $SMALL_FILE"
    slurp "$SMALL_FILE"
    raw_tok="$(printf '%s' "$RAW_TEXT" | count_raw_tokens)"
    res_json="$(invoke_tz read "$SMALL_FILE" --json --cache-path "$CACHE_PATH" </dev/null)"
    vis_tok="$(printf '%s' "$res_json" | json_accounting_visible)"
    add_row 'small read (Cargo.toml)' "$raw_tok" "$vis_tok" 'pass-through; capsule never costs more than raw'
fi

# 2. Large file read (heavy savings + tz:// refs)
LARGE_FILE="crates/tokenzero-mcp/src/lib.rs"
if [[ -f "$LARGE_FILE" ]]; then
    echo "[2/7] large read  : $LARGE_FILE"
    slurp "$LARGE_FILE"
    raw_tok="$(printf '%s' "$RAW_TEXT" | count_raw_tokens)"
    res_json="$(invoke_tz read "$LARGE_FILE" --json --cache-path "$CACHE_PATH" </dev/null)"
    vis_tok="$(printf '%s' "$res_json" | json_accounting_visible)"
    LARGE_REF="$(printf '%s' "$res_json" | json_first_blob_ref)"
    add_row "large read ($LARGE_FILE)" "$raw_tok" "$vis_tok" "ref: $LARGE_REF"
fi

# 3. Re-read same file via the MCP server: session seen-set dedup
#    CLI invocations are stateless against the cache file, so dedup is an
#    MCP-server feature (it tracks a per-session seen-set in memory). We
#    issue two JSON-RPC reads within the same stdio session.
if [[ -f "$LARGE_FILE" ]]; then
    echo "[3/7] re-read     : $LARGE_FILE (MCP session dedup)"

    abs_large="$REPO_DIR/$LARGE_FILE"
    path_arg="$(printf '%s' "$abs_large" | sed 's/\\/\\\\/g; s/"/\\"/g')" # JSON-escape
    jsonrpc="$(cat <<EOF
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"demo","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read","arguments":{"path":"$path_arg"}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"read","arguments":{"path":"$path_arg"}}}
EOF
)"

    mcp_out="$(printf '%s\n' "$jsonrpc" | invoke_tz mcp-server \
        --allowed-root "$REPO_DIR" \
        --cache-path   "$CACHE_PATH")"

    second_call_text="$(printf '%s\n' "$mcp_out" | json_mcp_second_call_text)"
    if [[ -z "$second_call_text" ]]; then
        warn 'MCP dedup scenario skipped: no response to second tools/call'
    else
        slurp "$LARGE_FILE"
        raw_tok="$(printf '%s' "$RAW_TEXT" | count_raw_tokens)"
        vis_tok="$(printf '%s' "$second_call_text" | count_raw_tokens)"
        add_row "re-read same file (MCP dedup)" "$raw_tok" "$vis_tok" \
            'second call routed through seen-set in same MCP session'
    fi
fi

# 4. Repo-wide grep (recoverable hit set)
echo "[4/7] grep        : 'fn ' across crates/"
raw_grep="$({
    find "$REPO_DIR/crates" -type f -name '*.rs' 2>/dev/null || true
} | while IFS= read -r f; do
    { grep -nE '(^|[^[:alnum:]_])fn[[:space:]]' "$f" 2>/dev/null || true; } | sed "s|^|$f:|"
done)"
match_count="$(printf '%s' "$raw_grep" | awk 'END { print NR + 0 }')"
raw_tok="$(printf '%s' "$raw_grep" | count_raw_tokens)"
res_json="$(invoke_tz grep 'fn ' crates --json --max-files 200 --cache-path "$CACHE_PATH" </dev/null)"
vis_tok="$(printf '%s' "$res_json" | json_accounting_visible)"
add_row "grep 'fn ' across crates/" "$raw_tok" "$vis_tok" "$match_count matching lines"
GREP_RAW_TOK="$raw_tok"

# 5. Recovery round-trip: expand the large-read blob and byte-compare
if [[ -n "$LARGE_REF" ]]; then
    echo "[5/7] expand      : $LARGE_REF (byte-exact check)"
    recovered="$(invoke_tz expand "$LARGE_REF" --raw --cache-path "$CACHE_PATH" </dev/null)"
    slurp "$LARGE_FILE"
    # Tolerate a single trailing newline that stdout streams sometimes add.
    rec="$(trim_end_crlf "$recovered")"
    orig="$(trim_end_crlf "$RAW_TEXT")"
    if [[ "$rec" != "$orig" ]]; then
        die "byte-exact recovery FAILED: expand returned ${#rec} chars, original is ${#orig} chars"
    fi
    add_row "expand round-trip (large file)" 0 0 'byte-exact: recovered == original'
fi

# 6. Recall: re-find content already in the cache without re-scanning
echo "[6/7] recall      : 'fn main' (no re-grep)"
res_json="$(invoke_tz recall 'fn main' --max-hits 10 --json --cache-path "$CACHE_PATH" </dev/null)"
vis_tok="$(printf '%s' "$res_json" | json_accounting_visible)"
# Same baseline as scenario 4 — we are showing recall vs re-running that grep.
add_row "recall 'fn main' vs re-grep" "$GREP_RAW_TOK" "$vis_tok" 'reuses cached payloads; no filesystem rescan'

# 7. Shell capture (always works even if cargo missing)
echo "[7/7] run         : capture a small shell command"
if have git; then
    probe=(git --version)
elif is_windows; then
    probe=(cmd /c ver)
else
    probe=(uname -a)
fi
if raw_out="$("${probe[@]}" 2>&1)"; then
    raw_tok="$(printf '%s\n' "$raw_out" | count_raw_tokens)"
    if res_json="$(invoke_tz run --json --cache-path "$CACHE_PATH" -- "${probe[@]}" </dev/null)"; then
        vis_tok="$(printf '%s' "$res_json" | json_accounting_visible)"
        add_row "run -- ${probe[*]}" "$raw_tok" "$vis_tok" 'process capture + recoverable stream'
    else
        warn 'shell scenario skipped: tokenzero run failed'
    fi
else
    warn 'shell scenario skipped: probe command failed'
fi

# --- Summary -----------------------------------------------------------------
read -r total_raw total_visible <<< "$(json_totals < "$ROWS_JSONL")"
: "${total_raw:=0}" "${total_visible:=0}"
if (( total_raw > 0 )); then
    total_pct="$(awk -v r="$total_raw" -v v="$total_visible" 'BEGIN { printf "%.1f", 100.0 * (r - v) / r }')"
else
    total_pct=0
fi

echo
echo '=== Results ==='
name_w=40
head="$(printf "%-${name_w}s %12s %12s %10s  %s" 'Workload' 'Raw tokens' 'Visible' 'Savings' 'Note')"
echo "$head"
printf '%*s\n' "${#head}" '' | tr ' ' '-'
while IFS=$'\t' read -r w r v note; do
    printf "%-${name_w}s %12s %12s %10s  %s\n" "$w" "$r" "$v" "$(format_pct "$r" "$v")" "$note"
done < <(json_rows_to_tsv < "$ROWS_JSONL")
printf '%*s\n' "${#head}" '' | tr ' ' '-'
printf "%-${name_w}s %12s %12s %10s\n" 'TOTAL (rows with raw baseline)' "$total_raw" "$total_visible" \
    "$(format_pct "$total_raw" "$total_visible")"
echo
echo 'Honest accounting: every TokenZero row above keeps exact tz:// refs.'
# shellcheck disable=SC2016
echo 'Hidden bytes are one `tokenzero expand <ref>` call away — and scenario 5'
echo 'proves the round-trip really is byte-exact.'
echo

json_emit_results "$total_raw" "$total_visible" "$total_pct"
echo "Wrote: $RESULTS_PATH"

if (( ! NO_VIZ )); then
    viz_sh="$DEMO_DIR/build_viz.sh"
    viz_ps1="$DEMO_DIR/build_viz.ps1"
    if [[ -f "$viz_sh" ]]; then
        echo
        echo '==> Rendering demo_viz.html'
        viz_args=(--results "$RESULTS_PATH" --out "$DEMO_DIR/demo_viz.html")
        (( OPEN_VIZ )) && viz_args+=(--open)
        bash "$viz_sh" "${viz_args[@]}"
    elif [[ -f "$viz_ps1" ]]; then
        echo
        echo '==> Rendering demo_viz.html'
        ps_host="$(command -v pwsh 2>/dev/null || command -v powershell 2>/dev/null || true)"
        [[ -n "$ps_host" ]] || die 'PowerShell host not found for rendering visualization.'
        viz_args=(-NoProfile -ExecutionPolicy Bypass -File "$viz_ps1"
                  -ResultsPath "$RESULTS_PATH"
                  -OutPath     "$DEMO_DIR/demo_viz.html")
        (( OPEN_VIZ )) && viz_args+=(-Open)
        "$ps_host" "${viz_args[@]}"
    else
        warn "no build_viz script found in $DEMO_DIR; skipping visualization."
    fi
fi
