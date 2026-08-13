#!/usr/bin/env python3
import json, pathlib, sys

root = pathlib.Path(sys.argv[1])
vectors = json.loads((root / "vectors.json").read_text())
guards = vectors["expected_guard_order"]
assert vectors["schema_version"] == 1
assert vectors["vector_set"] == "zerostack.two-phase-gate-kat.v1"
assert guards == [
    "g0_canonical", "g1_coherence", "g2_finite_plan", "g3_attribution",
    "g4_resources", "g5_robust_snap", "g6_safety_shield", "g7_performance",
    "g8_transaction_closure", "g9_receipt_commitment",
]

def verify(trace):
    events = trace["events"]
    if len(events) > len(guards):
        return "incomplete_trace"
    for index, event in enumerate(events):
        if event["guard"] != guards[index] or event["status"] != "passed":
            return "incomplete_trace"
        predecessor = None if index == 0 else guards[index - 1]
        if event["predecessor"] != predecessor:
            return "forged_predecessor"
    if len(events) != len(guards):
        return "incomplete_trace"
    return None

for case in vectors["trace_cases"]:
    failure = verify(case["trace"])
    expected = case["expected_failure_code"]
    assert failure == expected, (case["case_id"], failure, expected)
    assert (failure is None) == (case["expected_status"] == "passed")

required = {
    "execute_without_permit", "early_visible_byte", "irreversible_pre_evidence_effect",
    "forged_permit", "unbounded_worker", "semantic_cut_crossing", "incomplete_trace",
    "unaccounted_fallback", "missing_approval_grant", "forged_receipt",
}
assert required <= set(vectors["typed_mutants"])
print("two_phase_gate_kat:python:v1:passed")
