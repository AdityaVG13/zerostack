#!/usr/bin/env bash
set -euo pipefail

target_dir="${CARGO_TARGET_DIR:-target/linux-docker}"; bin="${target_dir}/release-perf/tokenzero"
mkdir -p results/current

# Keep-gate: never size-optimized `--release` for latency claims.
cargo build --profile release-perf -p tokenzero-cli --bin tokenzero --no-default-features

measure() {
  local label="$1"; local threshold="$2"
  shift 2; local tmp
  tmp="$(mktemp)"
  for _ in $(seq 1 30); do
    local start end; start="$(date +%s%N)"
    "$@" >/dev/null; end="$(date +%s%N)"
    awk -v start="${start}" -v end="${end}" 'BEGIN { printf "%.6f\n", (end - start) / 1000000 }' >>"${tmp}"
  done
  sort -n "${tmp}" -o "${tmp}"; local count idx p95 min max ok
  count="$(wc -l <"${tmp}" | tr -d ' ')"; idx="$(awk -v count="${count}" 'BEGIN { idx = int(count * 0.95); if (idx < count * 0.95) idx += 1; if (idx < 1) idx = 1; print idx }')"
  p95="$(sed -n "${idx}p" "${tmp}")"; min="$(sed -n '1p' "${tmp}")"
  max="$(sed -n "${count}p" "${tmp}")"; ok="$(awk -v p95="${p95}" -v threshold="${threshold}" 'BEGIN { print (p95 <= threshold) ? "true" : "false" }')"
  rm -f "${tmp}"
  printf '"%s":{"p95_ms":%s,"min_ms":%s,"max_ms":%s,"count":%s,"threshold_ms":%s,"ok":%s}' \
    "${label}" "${p95}" "${min}" "${max}" "${count}" "${threshold}" "${ok}"
}

version_json="$(measure version 25 "${bin}" --version)"; run_json="$(measure run_echo 25 "${bin}" run -- echo ok)"
ok="true"
if [[ "${version_json}" != *'"ok":true'* || "${run_json}" != *'"ok":true'* ]]; then
  ok="false"
fi
status="ok"
if [[ "${ok}" != "true" ]]; then
  status="blocked"
fi

printf '{\n  "schema_version": "tokenzero.rust_perf_budget_linux_docker.v1",\n  "status": "%s",\n  "ok": %s,\n  "samples": {\n    %s,\n    %s\n  }\n}\n' \
  "${status}" "${ok}" "${version_json}" "${run_json}" \
  >results/current/rust_perf_budget_linux_docker.json

if [[ "${ok}" != "true" ]]; then
  exit 1
fi
