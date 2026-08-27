# MEASUREMENT_GAP packet -- CodeMode Amdahl agent RTT (fszero-neqy)

MEASUREMENT_GAP: agent_rtt_R
status: open
symbol: R
meaning: end-to-end agent round-trip time for one CodeMode plan (request→result)
units: ms
why_unmeasured: requires instrumented multi-N agent harness against live codemode host;
  local s << 1 so N× speedup claims without R are not bound.
assumption_grid_E: [50, 200, 500] ms (labeled [E], not measured)
demo_local_only:
  source: benchmarks/demo-bench_results.json
  codemode_3read_plan_p50_ms: 14.356187
  note: local plan latency is not agent RTT R; do not substitute.
kill_cases: N× claims without measured R correctly FAIL theater checks
next: measure R under controlled agent harness; replace [E] grid with measured p50/p95
HOUSE_ODDS_RESULT: partial
status: measurement_gap
target_root: FSZero
mode: lookout
isomorphic transfer: not_applicable (performance ceiling craft)
Amdahl: named
p: unmeasured
s: unmeasured (local serial fraction assumed <<1; not campaign-bound)
R: [E] assumption grid pending MEASUREMENT_GAP close
