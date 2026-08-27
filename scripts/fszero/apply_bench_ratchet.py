#!/usr/bin/env python3
"""apply_bench_ratchet.py -- validate fszero.surface_wire_ratchet.v1 evidence.

Enforces the exact persistent-wire release ratchet (fszero-tep8.2,
H-PERF-016 replacement for the debug surface_bench keep-gate):

- schema must be fszero.surface_wire_ratchet.v1
- scope must be persistent_stdio_json_rpc (shipped persistent stdio surfaces)
- provenance.profile and provenance.cargo_profile must both be release-perf
- binary_sha256.codemode / fastmcp must be real 64-lowercase-hex digests
- raw_ordered_ns.codemode / fastmcp must be equal-length ordered numeric
  vectors with at least WIRE_RATCHET_MIN_SAMPLES (12) samples
- validation.responses_validated must be true (responses and operation
  outcomes were validated against independent fresh roots)
- ratchet.threshold_multiplier must be exactly 2 (never weakened)
- p50/p95 recomputed from the raw vectors (same nearest-rank rule as
  src/surface_bench.rs) must satisfy codemode <= 2 * fastmcp on BOTH p50
  and p95, and must match the harness-reported percentiles when present
- coefficient of variation (cv_pct = population stddev / mean * 100) is
  computed for both vectors and reported. CV is informational by default;
  pass --max-cv-pct N to fail when either vector's CV exceeds N.

Exit codes: 0 pass, 1 validation/gate failure, 2 usage/IO error.

Self-check: python3 scripts/apply_bench_ratchet.py --self-check
"""

from __future__ import annotations

import argparse
import json
import math
import re
import statistics
import sys
from pathlib import Path

SCHEMA = "fszero.surface_wire_ratchet.v1"
SCOPE = "persistent_stdio_json_rpc"
PROFILE = "release-perf"
WIRE_RATCHET_MULTIPLIER = 2
WIRE_RATCHET_MIN_SAMPLES = 12
SHA256_HEX = re.compile(r"^[0-9a-f]{64}$")


def rust_percentile(sorted_values: list[float], p: float) -> float:
    """Nearest-rank percentile matching src/surface_bench.rs::percentile_ns.

    Rust uses f64::round (half away from zero): idx = round((len-1)*p),
    clamped to the last index. floor(x + 0.5) reproduces that for the
    non-negative (len-1)*p values used here.
    """
    if not sorted_values:
        return 0.0
    idx = int(math.floor((len(sorted_values) - 1) * p + 0.5))
    idx = max(0, min(idx, len(sorted_values) - 1))
    return sorted_values[idx]


def cv_pct(values: list[float]) -> float:
    """Population coefficient of variation in percent."""
    if not values:
        return 0.0
    mean = statistics.fmean(values)
    if mean == 0.0:
        return 0.0
    return (statistics.pstdev(values) / abs(mean)) * 100.0


def _require(condition: bool, errors: list[str], message: str) -> bool:
    if not condition:
        errors.append(message)
    return condition


def validate(document: object, max_cv_pct: float | None = None) -> tuple[bool, list[str], dict]:
    """Return (ok, errors, info). info carries recomputed stats and cv_pct."""
    errors: list[str] = []
    info: dict = {}

    if not isinstance(document, dict):
        return False, ["evidence is not a JSON object"], info

    if not _require(
        document.get("schema") == SCHEMA, errors, f"schema must be {SCHEMA}"
    ):
        return False, errors, info

    if not _require(
        document.get("scope") == SCOPE,
        errors,
        f"scope must be {SCOPE} (shipped persistent stdio surfaces)",
    ):
        return False, errors, info

    provenance = document.get("provenance")
    if not isinstance(provenance, dict):
        return False, errors + ["missing provenance object"], info
    for key in ("profile", "cargo_profile"):
        _require(
            provenance.get(key) == PROFILE,
            errors,
            f"provenance.{key} must be {PROFILE}, got {provenance.get(key)!r}",
        )

    binary_sha = provenance.get("binary_sha256")
    if not isinstance(binary_sha, dict):
        _require(False, errors, "missing provenance.binary_sha256")
    else:
        for key in ("codemode", "fastmcp"):
            value = binary_sha.get(key)
            _require(
                isinstance(value, str) and SHA256_HEX.match(value),
                errors,
                f"provenance.binary_sha256.{key} must be 64 lowercase hex, got {value!r}",
            )

    raw = document.get("raw_ordered_ns")
    if not isinstance(raw, dict):
        _require(False, errors, "missing raw_ordered_ns")
        return False, errors, info
    vectors: dict[str, list[float]] = {}
    for key in ("codemode", "fastmcp"):
        samples = raw.get(key)
        if not isinstance(samples, list) or not samples:
            _require(False, errors, f"raw_ordered_ns.{key} must be a non-empty list")
            continue
        try:
            numeric = [float(v) for v in samples]
        except (TypeError, ValueError):
            _require(False, errors, f"raw_ordered_ns.{key} must contain numbers")
            continue
        if any(v < 0 for v in numeric):
            _require(False, errors, f"raw_ordered_ns.{key} must be non-negative")
        vectors[key] = numeric

    if "codemode" in vectors and "fastmcp" in vectors:
        _require(
            len(vectors["codemode"]) == len(vectors["fastmcp"]),
            errors,
            "raw_ordered_ns vectors must have equal sample counts",
        )
        _require(
            len(vectors["codemode"]) >= WIRE_RATCHET_MIN_SAMPLES,
            errors,
            f"raw_ordered_ns requires >= {WIRE_RATCHET_MIN_SAMPLES} samples per surface",
        )

    validation = document.get("validation")
    if not isinstance(validation, dict):
        _require(False, errors, "missing validation block")
    else:
        _require(
            validation.get("responses_validated") is True,
            errors,
            "validation.responses_validated must be true",
        )

    ratchet = document.get("ratchet")
    if not isinstance(ratchet, dict):
        _require(False, errors, "missing ratchet block")
    else:
        _require(
            ratchet.get("threshold_multiplier") == WIRE_RATCHET_MULTIPLIER,
            errors,
            f"ratchet.threshold_multiplier must be exactly {WIRE_RATCHET_MULTIPLIER}",
        )

    # Recompute the gate from the raw vectors so evidence cannot be gamed by
    # a self-claimed pass. Requires both vectors valid at this point.
    if "codemode" in vectors and "fastmcp" in vectors:
        cm_sorted = sorted(vectors["codemode"])
        mcp_sorted = sorted(vectors["fastmcp"])
        cm_p50 = rust_percentile(cm_sorted, 0.50)
        cm_p95 = rust_percentile(cm_sorted, 0.95)
        mcp_p50 = rust_percentile(mcp_sorted, 0.50)
        mcp_p95 = rust_percentile(mcp_sorted, 0.95)
        info["codemode_p50_ns"] = cm_p50
        info["codemode_p95_ns"] = cm_p95
        info["fastmcp_p50_ns"] = mcp_p50
        info["fastmcp_p95_ns"] = mcp_p95
        info["gate_pass"] = (
            cm_p50 <= WIRE_RATCHET_MULTIPLIER * mcp_p50
            and cm_p95 <= WIRE_RATCHET_MULTIPLIER * mcp_p95
        )
        _require(
            info["gate_pass"],
            errors,
            "2x gate failed: codemode p50/p95 must not exceed 2 * fastmcp p50/p95 "
            f"(cm {cm_p50:.0f}/{cm_p95:.0f} ns vs mcp {mcp_p50:.0f}/{mcp_p95:.0f} ns)",
        )
        # Harness-reported percentiles, when present, must agree with ours.
        for surface_key, cm_key, p_key in (
            ("codemode", "codemode_p50_ns", "codemode_p95_ns"),
            ("fastmcp", "fastmcp_p50_ns", "fastmcp_p95_ns"),
        ):
            surface = document.get(surface_key)
            if isinstance(surface, dict):
                for doc_key, calc in (
                    ("p50_ns", info[f"{cm_key}"]),
                    ("p95_ns", info[f"{p_key}"]),
                ):
                    reported = surface.get(doc_key)
                    if reported is not None:
                        _require(
                            float(reported) == calc,
                            errors,
                            f"{surface_key}.{doc_key} ({reported}) does not match "
                            f"raw-vector percentile ({calc:.0f})",
                        )

        info["cv_pct"] = {
            "codemode": cv_pct(vectors["codemode"]),
            "fastmcp": cv_pct(vectors["fastmcp"]),
        }
        if max_cv_pct is not None:
            for surface_key, cv in info["cv_pct"].items():
                _require(
                    cv <= max_cv_pct,
                    errors,
                    f"cv_pct for {surface_key} ({cv:.2f}%) exceeds --max-cv-pct {max_cv_pct}",
                )

    if "gate_pass" not in info:
        _require(False, errors, "could not recompute the 2x gate from raw vectors")

    return not errors, errors, info


def _sha(value: int) -> str:
    return f"{value:064x}"


def _make_fixture(
    cm_values: list[int],
    mcp_values: list[int],
    *,
    schema: str = SCHEMA,
    scope: str = SCOPE,
    profile: str = PROFILE,
    multiplier: int = WIRE_RATCHET_MULTIPLIER,
    responses_validated: bool = True,
    sha_codemode: str | None = None,
    sha_fastmcp: str | None = None,
    gate_pass: bool = True,
) -> dict:
    sha_codemode = sha_codemode or _sha(0xAB)
    sha_fastmcp = sha_fastmcp or _sha(0xCD)
    cm_sorted = sorted(cm_values)
    mcp_sorted = sorted(mcp_values)
    return {
        "schema": schema,
        "scope": scope,
        "provenance": {
            "git_sha": "0" * 40,
            "git_dirty": False,
            "os": "linux",
            "arch": "aarch64",
            "profile": profile,
            "cargo_profile": profile,
            "transport": "persistent NDJSON stdio JSON-RPC",
            "measurement_scope": "shipped persistent stdio JSON-RPC surfaces; spawn/init excluded",
            "codemode_surface": "fszero-codemode compatibility surface",
            "fastmcp_surface": "fszero-mcp shipped surface",
            "n": 3,
            "samples": len(cm_values),
            "warmup_policy": "one validated warmup per persistent process; excluded from timed samples",
            "binary_sha256": {
                "codemode": sha_codemode,
                "fastmcp": sha_fastmcp,
            },
        },
        "n": 3,
        "samples": len(cm_values),
        "raw_ordered_ns": {"codemode": cm_values, "fastmcp": mcp_values},
        "codemode": {
            "surface": "fszero-codemode",
            "p50_ns": rust_percentile(cm_sorted, 0.50),
            "p95_ns": rust_percentile(cm_sorted, 0.95),
        },
        "fastmcp": {
            "surface": "fszero-mcp",
            "p50_ns": rust_percentile(mcp_sorted, 0.50),
            "p95_ns": rust_percentile(mcp_sorted, 0.95),
        },
        "validation": {
            "responses_validated": responses_validated,
            "operation_outcomes_validated": responses_validated,
            "workspace_policy": "independent fresh temp roots with identical payload bytes",
        },
        "ratchet": {
            "scope": "persistent_stdio_json_rpc",
            "threshold_multiplier": multiplier,
            "pass_p50": gate_pass,
            "pass_p95": gate_pass,
            "gate_pass": gate_pass,
            "codemode_p50_ns": rust_percentile(cm_sorted, 0.50),
            "codemode_p95_ns": rust_percentile(cm_sorted, 0.95),
            "fastmcp_p50_ns": rust_percentile(mcp_sorted, 0.50),
            "fastmcp_p95_ns": rust_percentile(mcp_sorted, 0.95),
        },
    }


def _self_check() -> int:
    """Deterministic in-memory fixture matrix; no builds, no network."""
    flat_cm = [4_000_000] * 12
    flat_mcp = [2_500_000] * 12
    spread_cm = [4_000_000 + 100_000 * (i % 5) for i in range(12)]
    spread_mcp = [2_500_000 + 50_000 * (i % 5) for i in range(12)]
    slow_cm = [6_000_000] * 12  # 2.4x -> both gates fail

    cases: list[tuple[str, object, bool, float | None]] = [
        ("pass flat", _make_fixture(flat_cm, flat_mcp), True, None),
        ("pass spread", _make_fixture(spread_cm, spread_mcp), True, None),
        ("fail gate", _make_fixture(slow_cm, flat_mcp, gate_pass=False), False, None),
        ("debug profile", _make_fixture(flat_cm, flat_mcp, profile="debug"), False, None),
        ("in-process scope", _make_fixture(flat_cm, flat_mcp, scope="agent_local_in_process"), False, None),
        ("weak multiplier", _make_fixture(flat_cm, flat_mcp, multiplier=3), False, None),
        (
            "responses unvalidated",
            _make_fixture(flat_cm, flat_mcp, responses_validated=False, gate_pass=False),
            False,
            None,
        ),
        ("short vectors", _make_fixture(flat_cm[:11], flat_mcp[:11]), False, None),
        ("unequal vectors", _make_fixture(flat_cm, flat_mcp[:11]), False, None),
        ("bad codemode sha", _make_fixture(flat_cm, flat_mcp, sha_codemode="deadbeef"), False, None),
        ("bad fastmcp sha", _make_fixture(flat_cm, flat_mcp, sha_fastmcp="Z" * 64), False, None),
        ("wrong schema", _make_fixture(flat_cm, flat_mcp, schema="fszero.surface_bench.v1"), False, None),
        ("cv over limit", _make_fixture(spread_cm, spread_mcp), False, 1.0),
        ("non-object", [1, 2, 3], False, None),
    ]

    failures: list[str] = []
    for name, doc, expected_ok, max_cv in cases:
        ok, errors, info = validate(doc, max_cv_pct=max_cv)
        if ok != expected_ok:
            failures.append(
                f"{name}: expected ok={expected_ok} got ok={ok} errors={errors}"
            )
        if name == "pass spread":
            # CV must be nonzero and reported for a spread vector.
            if not info.get("cv_pct"):
                failures.append("pass spread: cv_pct missing from info")
            else:
                if not (0 < info["cv_pct"]["codemode"] and 0 < info["cv_pct"]["fastmcp"]):
                    failures.append("pass spread: cv_pct must be nonzero for spread vectors")

    if failures:
        print("apply_bench_ratchet self-check: FAIL", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print("apply_bench_ratchet self-check: OK")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Validate fszero.surface_wire_ratchet.v1 release-perf evidence.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument("evidence", nargs="?", type=Path, help="wire evidence JSON file")
    parser.add_argument(
        "--self-check",
        action="store_true",
        help="run the deterministic in-memory fixture matrix and exit",
    )
    parser.add_argument(
        "--max-cv-pct",
        type=float,
        default=None,
        help="optional CV ceiling; fail when either raw vector exceeds it (default: report only)",
    )
    args = parser.parse_args(argv)

    if args.self_check:
        return _self_check()

    if args.evidence is None:
        parser.error("evidence file required (or --self-check)")
        return 2

    try:
        document = json.loads(args.evidence.read_text(encoding="utf-8"))
    except OSError as exc:
        print(f"apply_bench_ratchet: cannot read {args.evidence}: {exc}", file=sys.stderr)
        return 2
    except json.JSONDecodeError as exc:
        print(f"apply_bench_ratchet: invalid JSON in {args.evidence}: {exc}", file=sys.stderr)
        return 1

    ok, errors, info = validate(document, max_cv_pct=args.max_cv_pct)
    if not ok:
        print("apply_bench_ratchet: FAIL", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    cm_p50 = info["codemode_p50_ns"]
    cm_p95 = info["codemode_p95_ns"]
    mcp_p50 = info["fastmcp_p50_ns"]
    mcp_p95 = info["fastmcp_p95_ns"]
    cv = info["cv_pct"]
    print("apply_bench_ratchet: PASS")
    print(
        f"schema={SCHEMA} scope={SCOPE} profile={PROFILE} "
        f"samples={document.get('samples')}"
    )
    print(
        f"codemode p50/p95 = {cm_p50:.0f}/{cm_p95:.0f} ns | "
        f"fastmcp p50/p95 = {mcp_p50:.0f}/{mcp_p95:.0f} ns"
    )
    print(f"2x gate: p50 PASS p95 PASS")
    print(f"cv_pct: codemode={cv['codemode']:.2f}% fastmcp={cv['fastmcp']:.2f}%")
    summary = {
        "schema": SCHEMA,
        "valid": True,
        "gate_pass": True,
        "samples": document.get("samples"),
        "cv_pct": cv,
    }
    print(f"RATCHET_SUMMARY {json.dumps(summary, sort_keys=True)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
