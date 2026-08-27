#!/usr/bin/env python3
"""Blast-as-prefetch oracle vs temporal-only baseline on the committed corpus.

Replays the corpus in chronological order. At each event both policies emit a
top-k prefetch set of test files before the faults are revealed; fault-rate is
the share of events whose actual faults intersect that set.

  graph_blast_oracle : rank blast break-site candidates by
                       callers x proximity x recent_change_frequency
  temporal_only      : most-recently-faulted test files (LRU, no graph signal)

Competitive ratio is measured against the offline optimum (an omniscient policy
that always prefetches the actual faults), and every losing event is published.
"""
from __future__ import annotations

import argparse
import collections
import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
CORPUS = HERE / "corpus.jsonl"
REPORT = HERE / "report.json"
METHODOLOGY = HERE / "METHODOLOGY.md"
BUILDER = HERE / "build_corpus.py"
RUNNER = Path(__file__).resolve()

K = 5
MIN_LIFT_PCT = 20.0
MIN_EVENTS = 50


def load_corpus() -> list[dict]:
    return [json.loads(line) for line in CORPUS.read_text(encoding="utf8").splitlines() if line.strip()]


def digests() -> dict[str, str]:
    return {
        p.name: hashlib.sha256(p.read_bytes()).hexdigest()
        for p in (CORPUS, BUILDER, RUNNER, METHODOLOGY)
        if p.exists()
    }


def replay(events: list[dict], policy: str, k: int) -> dict:
    recency: collections.OrderedDict[str, None] = collections.OrderedDict()
    freq: collections.Counter[str] = collections.Counter()
    hits = 0
    losses: list[dict] = []
    for event in events:
        faults = set(event["faults"])
        if policy == "temporal_only":
            predicted = list(reversed(recency))[:k]
        else:
            scored = {
                c["path"]: c["callers"] * c["proximity"] * (1 + freq[c["path"]])
                for c in event["graph_candidates"]
            }
            predicted = sorted(scored, key=lambda p: (-scored[p], p))[:k]
        hit = bool(faults & set(predicted))
        hits += hit
        if not hit:
            losses.append({
                "event_id": event["event_id"],
                "faults": sorted(faults),
                "predicted": predicted,
            })
        for fault in event["faults"]:
            recency.pop(fault, None)
            recency[fault] = None
            freq[fault] += 1
    total = len(events)
    return {
        "policy": policy,
        "k": k,
        "events": total,
        "hits": hits,
        "fault_rate": round(hits / total, 6),
        "competitive_ratio": round(hits / total, 6),
        "misses": len(losses),
        "losses": losses,
    }


def build_report(events: list[dict], k: int) -> dict:
    graph = replay(events, "graph_blast_oracle", k)
    temporal = replay(events, "temporal_only", k)
    lift = ((graph["fault_rate"] - temporal["fault_rate"]) / temporal["fault_rate"] * 100.0
            if temporal["fault_rate"] else float("inf"))
    return {
        "schema_version": 1,
        "benchmark": "blast_prefetch_oracle",
        "k": k,
        "event_count": len(events),
        "arms": [graph, temporal],
        "fault_rate_lift_percent": round(lift, 4),
        "gate": {"min_events": MIN_EVENTS, "min_lift_percent": MIN_LIFT_PCT, "k": K},
        "freshness": digests(),
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="verify committed report is fresh and passing")
    ap.add_argument("-k", type=int, default=K)
    args = ap.parse_args()

    events = load_corpus()
    if len(events) < MIN_EVENTS:
        raise SystemExit(f"corpus has {len(events)} events, need >={MIN_EVENTS}")
    report = build_report(events, args.k)

    graph, temporal = report["arms"]
    lift = report["fault_rate_lift_percent"]

    if args.check:
        if not REPORT.exists():
            raise SystemExit("report.json missing; run without --check to regenerate")
        committed = json.loads(REPORT.read_text(encoding="utf8"))
        if committed != report:
            raise SystemExit("report.json is stale; regenerate with run.py")
        if lift < MIN_LIFT_PCT:
            raise SystemExit(f"fault-rate lift {lift:.2f}% below {MIN_LIFT_PCT}% gate")
        print(f"ok: lift {lift:.2f}% over temporal baseline at k={args.k}")
        return

    REPORT.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf8")
    print(f"graph fault_rate={graph['fault_rate']:.4f} "
          f"temporal fault_rate={temporal['fault_rate']:.4f} lift={lift:.2f}% "
          f"(k={args.k}, {report['event_count']} events)")


if __name__ == "__main__":
    main()
