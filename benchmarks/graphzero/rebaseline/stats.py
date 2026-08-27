"""Shared benchmark summary statistics.

Latency reports use medians plus Hyndman-Fan type 7 quantiles, matching
NumPy and R defaults. Type 7 interpolates between observations instead of
turning p95 into the maximum for small sample sizes. Benchmark drivers must
retain every *measured* sample; protocol warmups are discarded before
percentile aggregation and recorded separately in sample_accounting (W and N).
Callers choose W/N by environment or driver constants; reports include the
measured run count beside each percentile.

Variance envelope (same-host sample series and prior-run drift): CV and
MAD/median are computed from retained samples. CV > 10% (or drift > 10% vs
prior same-host baseline) means the series is outside the envelope and must
not be trusted as a publishable baseline without investigation.
"""

from __future__ import annotations

import math
import statistics
from typing import Any


def median(values: list[float], digits: int = 6) -> float:
    if not values:
        raise ValueError("median requires at least one sample")
    return round(statistics.median(values), digits)


def percentile(values: list[float], q: float, digits: int = 6) -> float:
    if not values:
        raise ValueError("percentile requires at least one sample")
    if not 0 <= q <= 1:
        raise ValueError("percentile q must be between 0 and 1")
    ordered = sorted(values)
    if len(ordered) == 1:
        return round(ordered[0], digits)
    rank = 1 + (len(ordered) - 1) * q
    lower = int(math.floor(rank)) - 1
    upper = min(lower + 1, len(ordered) - 1)
    fraction = rank - math.floor(rank)
    estimate = ordered[lower] + (ordered[upper] - ordered[lower]) * fraction
    return round(estimate, digits)


def p50(values: list[float], digits: int = 6) -> float:
    return median(values, digits)


def p95(values: list[float], digits: int = 6) -> float:
    return percentile(values, 0.95, digits)


def p99(values: list[float], digits: int = 6) -> float:
    return percentile(values, 0.99, digits)


# Below this N, type-7 p99 is near the sample max and must not be claimed as a
# reliable population p99. Publish the type-7 value but label it honestly.
P99_RELIABLE_MIN_N = 200

# Sample-series CV and same-host drift envelope (fraction, not percent).
# ≤10% is the publishable noise budget; >10% investigate; >20% escalate.
VARIANCE_ENVELOPE_MAX = 0.10
VARIANCE_ESCALATE = 0.20
VARIANCE_STABLE = 0.05


def p99_label(n: int) -> str:
    """Honesty label for published p99 given measured sample count N.

    Always pair with type-7 ``p99()``. For N < 200 the estimate is effectively
    worst-observed of N, not a stable population tail claim.
    """
    if n < 0:
        raise ValueError("p99_label requires n >= 0")
    if n < P99_RELIABLE_MIN_N:
        return "worst_observed_of_n"
    return "hyndman_fan_type7"


def mean(values: list[float], digits: int = 6) -> float:
    if not values:
        raise ValueError("mean requires at least one sample")
    return round(statistics.mean(values), digits)


def stdev(values: list[float], digits: int = 6) -> float | None:
    """Sample standard deviation (N-1). None when N < 2."""
    if len(values) < 2:
        return None
    return round(statistics.stdev(values), digits)


def coefficient_of_variation(values: list[float], digits: int = 6) -> float | None:
    """CV = sample_stdev / mean. None when N < 2 or mean is 0."""
    if len(values) < 2:
        return None
    m = statistics.mean(values)
    if m == 0:
        if all(v == 0 for v in values):
            return 0.0
        return None
    return round(statistics.stdev(values) / abs(m), digits)


def mad(values: list[float], digits: int = 6) -> float:
    """Median absolute deviation from the sample median."""
    if not values:
        raise ValueError("mad requires at least one sample")
    med = statistics.median(values)
    return round(statistics.median([abs(v - med) for v in values]), digits)


def mad_over_median(values: list[float], digits: int = 6) -> float | None:
    """Robust scale: MAD / median. None when median is 0 and MAD > 0."""
    if not values:
        raise ValueError("mad_over_median requires at least one sample")
    med = statistics.median(values)
    m = statistics.median([abs(v - med) for v in values])
    if med == 0:
        return 0.0 if m == 0 else None
    return round(m / abs(med), digits)


def envelope_status(
    ratio: float | None,
    *,
    max_ratio: float = VARIANCE_ENVELOPE_MAX,
    escalate: float = VARIANCE_ESCALATE,
    stable: float = VARIANCE_STABLE,
) -> str:
    """Map a relative ratio to skill envelope status.

    stable | noise (within envelope) | investigate | escalate | undefined
    """
    if ratio is None:
        return "undefined"
    if ratio < 0:
        raise ValueError("envelope ratio must be >= 0")
    if ratio <= stable:
        return "stable"
    if ratio <= max_ratio:
        return "noise"
    if ratio <= escalate:
        return "investigate"
    return "escalate"


def within_envelope(
    ratio: float | None, *, max_ratio: float = VARIANCE_ENVELOPE_MAX
) -> bool:
    """True only when ratio is defined and ≤ max_ratio (default 10%)."""
    if ratio is None:
        return False
    return ratio <= max_ratio


def variance_summary(values: list[float], digits: int = 6) -> dict[str, Any]:
    """CV, MAD, and envelope verdict for a retained sample series.

    Keeps the raw series out of this helper -- callers retain samples_ms/raw_ms
    for audit. CV is primary; MAD/median is the robust companion.
    """
    if not values:
        raise ValueError("variance_summary requires at least one sample")
    cv = coefficient_of_variation(values, digits=digits)
    mom = mad_over_median(values, digits=digits)
    cv_status = envelope_status(cv)
    return {
        "n": len(values),
        "mean": mean(values, digits=digits),
        "stdev": stdev(values, digits=digits),
        "cv": cv,
        "cv_pct": None if cv is None else round(cv * 100.0, digits),
        "mad": mad(values, digits=digits),
        "mad_over_median": mom,
        "mad_over_median_pct": None if mom is None else round(mom * 100.0, digits),
        "envelope_max": VARIANCE_ENVELOPE_MAX,
        "envelope_max_pct": round(VARIANCE_ENVELOPE_MAX * 100.0, 1),
        "status": cv_status,
        "within_envelope": within_envelope(cv),
    }


def relative_drift(
    current: float, prior: float, digits: int = 6
) -> float | None:
    """|current - prior| / prior. None when prior is 0 and current != 0."""
    if prior == 0:
        return 0.0 if current == 0 else None
    return round(abs(current - prior) / abs(prior), digits)


def drift_summary(
    current: float,
    prior: float,
    *,
    digits: int = 6,
    max_ratio: float = VARIANCE_ENVELOPE_MAX,
) -> dict[str, Any]:
    """Same-host drift of one scalar (typically p50_ms or p95_ms) vs prior."""
    drift = relative_drift(current, prior, digits=digits)
    status = envelope_status(drift, max_ratio=max_ratio)
    return {
        "current": current,
        "prior": prior,
        "drift": drift,
        "drift_pct": None if drift is None else round(drift * 100.0, digits),
        "envelope_max": max_ratio,
        "envelope_max_pct": round(max_ratio * 100.0, 1),
        "status": status,
        "within_envelope": within_envelope(drift, max_ratio=max_ratio),
    }
