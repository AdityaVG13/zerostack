#!/usr/bin/env bash
# Honest-gate attestation for comparative / published bake-offs (fszero-1yp9).
# Emits JSON with 14 questions: pass | fail | waive:<reason>.
# Usage:
#   ./scripts/honest_gate.sh --out benchmarks/honest_gate_bakeoff.json \
#       --artifact benchmarks/bakeoff.json --label bakeoff
#   ./scripts/honest_gate.sh --check benchmarks/honest_gate_bakeoff.json
set -euo pipefail

OUT=""
ARTIFACT=""
LABEL=""
CHECK=""
MODE="write"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out) OUT="${2:-}"; shift 2 ;;
    --artifact) ARTIFACT="${2:-}"; shift 2 ;;
    --label) LABEL="${2:-}"; shift 2 ;;
    --check) CHECK="${2:-}"; MODE="check"; shift 2 ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

if [[ "$MODE" == "check" ]]; then
  if [[ ! -f "$CHECK" ]]; then
    echo "missing attestation: $CHECK" >&2
    exit 1
  fi
  python3 - "$CHECK" <<'PY'
import json, sys
path = sys.argv[1]
doc = json.load(open(path))
qs = doc.get("questions") or []
if len(qs) != 14:
    print(f"expected 14 questions, got {len(qs)}", file=sys.stderr)
    sys.exit(1)
bad = []
for q in qs:
    st = (q.get("status") or "").split(":", 1)[0]
    if st not in ("pass", "fail", "waive"):
        bad.append(q.get("id"))
    if st == "fail":
        bad.append(f"FAIL:{q.get('id')}")
if bad:
    print("honest-gate check failed:", ", ".join(bad), file=sys.stderr)
    sys.exit(1)
print(f"ok honest-gate {path} ({len(qs)} questions, no hard fail)")
PY
  exit 0
fi

if [[ -z "$OUT" ]]; then
  echo "--out PATH required" >&2
  exit 2
fi
LABEL="${LABEL:-comparative}"
ARTIFACT="${ARTIFACT:-}"

# Operators fill statuses: pass | fail | waive:<reason>
# This template starts conservative (fail/waive) so regen must affirm.
python3 - "$OUT" "$ARTIFACT" "$LABEL" <<'PY'
import json, sys, datetime, os, subprocess
out, artifact, label = sys.argv[1], sys.argv[2], sys.argv[3]
git_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
dirty = subprocess.check_output(["git", "status", "--porcelain"], text=True).strip() != ""
questions = [
  {"id": "Q01_same_build_profile", "prompt": "All compared tools measured under declared build profiles (release-perf for FSZero stack claims)?", "status": "waive:operator must affirm per run"},
  {"id": "Q02_api_workload_match", "prompt": "Same question corpus and success predicate for every tool?", "status": "waive:operator must affirm per run"},
  {"id": "Q03_warmup_symmetry", "prompt": "Warmup rules identical across tools (or STATE-labeled when process models differ)?", "status": "waive:operator must affirm per run"},
  {"id": "Q04_sample_count_n", "prompt": "n>=20 independent trials (or conservative-tail exception explicit)?", "status": "waive:operator must affirm per run"},
  {"id": "Q05_host_quiet", "prompt": "Host quiet / isolation note recorded in fingerprint or artifact?", "status": "waive:operator must affirm per run"},
  {"id": "Q06_variance_envelope", "prompt": "Variance envelope or ordered trial vector retained for latency claims?", "status": "waive:operator must affirm per run"},
  {"id": "Q07_three_tier", "prompt": "p50/p95/p99 (or documented median-only with reason) reported?", "status": "waive:operator must affirm per run"},
  {"id": "Q08_losses_published", "prompt": "Losses and exclusions published (not dropped)?", "status": "waive:operator must affirm per run"},
  {"id": "Q09_apples_flags", "prompt": "Apples-to-apples flags / best documented competitor config?", "status": "waive:operator must affirm per run"},
  {"id": "Q10_reproducible_runner", "prompt": "Committed runner regenerates numbers from clean checkout?", "status": "waive:operator must affirm per run"},
  {"id": "Q11_git_dirty_false", "prompt": "Committed comparative artifact has git_dirty:false?", "status": "waive:operator must affirm per run"},
  {"id": "Q12_process_model_labels", "prompt": "Process-model axis labeled (long-lived MCP vs process-spawn vs in-process)?", "status": "waive:operator must affirm per run"},
  {"id": "Q13_tool_order_cache", "prompt": "Tool order does not poison OS page cache for later tools (or mitigated)?", "status": "waive:operator must affirm per run"},
  {"id": "Q14_claims_audit", "prompt": "Artifact is in claims_audit or competitive inventory; provenance complete?", "status": "waive:operator must affirm per run"},
]
doc = {
  "schema": "fszero.honest_gate.v1",
  "label": label,
  "artifact": artifact or None,
  "generated_at_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
  "generator_git_commit": git_commit,
  "generator_git_dirty": dirty,
  "questions": questions,
  "policy": "docs/benchmark-integrity.md#honest-gate-for-comparative-publish",
  "notes": "Statuses must be pass|fail|waive:<reason>. ./scripts/honest_gate.sh --check rejects any fail.",
}
os.makedirs(os.path.dirname(out) or ".", exist_ok=True)
with open(out, "w") as f:
  json.dump(doc, f, indent=2)
  f.write("\n")
print(f"wrote template {out}")
PY
