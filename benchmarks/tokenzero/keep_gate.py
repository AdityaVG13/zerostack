#!/usr/bin/env python3
# cargo bench invocation (keep-gate path):
#   rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero cargo bench -p tokenzero-core --bench hotpaths --profile release-perf
"""TokenZero performance keep-gate: quarantine, MT8 attribution, same-minute window."""

from __future__ import annotations

import argparse
import json
import math
import os
import shutil
import statistics
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

SCHEMA = "tokenzero.bench-history/v1"

# Single source for persist + keep compare bands (CC2-R5 / F-010).
KEEP_GATE_GEOMEAN_PCT = 3.0  # also persist-gate; stricter than skill geomean 5% (KNOWN, do not widen)
KEEP_GATE_PASS_PCT = 5.0
CV_PCT_QUARANTINE = 5.0
ALLOWED_LABELS = frozenset({"fixture-seed", "live"})
# Pattern 160: a keep names a frame ≥0.1% exclusive self-time. Below is the micro-lever trap.
MT8_MIN_SELF_PCT = 0.1
# KEEP-GATE-RULES rule 2: focused + broad share git SHA, machine, same minute.
SAME_RUN_WINDOW_SECONDS = 60

_NOT_SELF_TIME_KINDS = frozenset(
    {
        "enter_count",
        "enter-count",
        "invented",
        "placeholder",
        "synthetic",
        "inclusive",
    }
)
_INVENTED_SOURCE_MARKERS = (
    "invented",
    "placeholder",
    "synthetic-flamegraph",
    "synthetic_flamegraph",
    "fake-flamegraph",
)
_SHA_KEYS = ("git_sha", "commit", "git_commit", "sha")
_MACHINE_KEYS = ("machine", "hostname", "host", "host_id")
_TS_KEYS = ("timestamp", "recorded_at", "generated_at", "generated_at_utc", "ts")

ELF_MAGIC = b"\x7fELF"
MACHO_MAGICS = {
    b"\xfe\xed\xfa\xce",  # MH_MAGIC
    b"\xfe\xed\xfa\xcf",  # MH_MAGIC_64
    b"\xce\xfa\xed\xfe",  # MH_CIGAM
    b"\xcf\xfa\xed\xfe",  # MH_CIGAM_64
    b"\xca\xfe\xba\xbe",  # FAT_MAGIC / CAFEBABE
    b"\xbe\xba\xfe\xca",  # FAT_CIGAM
}


class KeepGateError(ValueError):
    """Fail-closed keep-gate / persist / resolve error."""


def cv_pct(samples: list[float]) -> float:
    """Population coefficient of variation in percent."""
    if not samples:
        raise KeepGateError("cv_pct requires a non-empty samples list")
    mean = statistics.fmean(samples)
    if mean == 0.0:
        return 0.0
    return (statistics.pstdev(samples) / abs(mean)) * 100.0


def _group_samples(group: dict[str, Any]) -> list[float] | None:
    raw = group.get("samples")
    if raw is None:
        return None
    if not isinstance(raw, list) or not raw:
        raise KeepGateError(f"group {group.get('name')!r}: samples must be a non-empty list")
    try:
        return [float(v) for v in raw]
    except (TypeError, ValueError) as error:
        raise KeepGateError(f"group {group.get('name')!r}: samples must be numeric") from error


def group_mean(group: dict[str, Any]) -> float:
    samples = _group_samples(group)
    if samples is not None:
        return statistics.fmean(samples)
    for key in ("mean", "mean_ns"):
        if key in group:
            try:
                return float(group[key])
            except (TypeError, ValueError) as error:
                raise KeepGateError(
                    f"group {group.get('name')!r}: {key} must be numeric"
                ) from error
    raise KeepGateError(
        f"group {group.get('name')!r}: need samples or mean/mean_ns"
    )


def group_cv_pct(group: dict[str, Any]) -> float:
    samples = _group_samples(group)
    if samples is not None:
        return cv_pct(samples)
    if "cv_pct" in group:
        try:
            return float(group["cv_pct"])
        except (TypeError, ValueError) as error:
            raise KeepGateError(
                f"group {group.get('name')!r}: cv_pct must be numeric"
            ) from error
    raise KeepGateError(
        f"group {group.get('name')!r}: need samples or cv_pct to quarantine"
    )


def geomean(values: list[float]) -> float:
    if not values:
        raise KeepGateError("geomean requires at least one positive value")
    if any(v <= 0 for v in values):
        raise KeepGateError("geomean requires strictly positive values")
    return math.exp(statistics.fmean(math.log(v) for v in values))


def quarantine_groups(
    groups: list[dict[str, Any]],
) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Split groups by CV_PCT_QUARANTINE. Noisy groups are never averaged in."""
    kept: list[dict[str, Any]] = []
    quarantined: list[dict[str, Any]] = []
    for group in groups:
        if not isinstance(group, dict) or "name" not in group:
            raise KeepGateError("each group must be an object with a name")
        cv = group_cv_pct(group)
        if cv > CV_PCT_QUARANTINE:
            quarantined.append(group)
        else:
            kept.append(group)
    if not kept:
        names = [str(g.get("name")) for g in quarantined]
        raise KeepGateError(
            "refuse: all primary groups quarantined "
            f"(cv_pct > {CV_PCT_QUARANTINE}): {names}"
        )
    return kept, quarantined


def _identity_haystack(document: dict[str, Any]) -> str:
    parts: list[str] = []
    for key in ("label", "note", "benchmark_id", "primary", "unit_id", "unit"):
        value = document.get(key)
        if value is not None:
            parts.append(str(value))
    groups = document.get("groups")
    if isinstance(groups, list):
        for group in groups:
            if isinstance(group, dict) and group.get("name") is not None:
                parts.append(str(group["name"]))
    return " ".join(parts)


def require_measurement_identity(document: dict[str, Any], *, role: str) -> str:
    """Refuse unlabeled / Q99 documents as live keep-gate measurements."""
    label = document.get("label")
    if not isinstance(label, str) or not label.strip():
        raise KeepGateError(
            f"refuse: {role} unlabeled bench-history cannot persist as live "
            "(need label=fixture-seed or label=live)"
        )
    label = label.strip()
    if label not in ALLOWED_LABELS:
        raise KeepGateError(
            f"refuse: {role} label {label!r} is not a keep-gate measurement label "
            f"(allowed: {sorted(ALLOWED_LABELS)})"
        )
    note = document.get("note")
    if not isinstance(note, str) or not note.strip():
        raise KeepGateError(
            f"refuse: {role} missing note; unlabeled bench-history cannot persist as live"
        )
    if "Q99" in _identity_haystack(document).upper():
        raise KeepGateError(
            f"refuse: {role} Q99 is not a keep-gate unit "
            "(keep-gate measures labeled latency ns, not Q99-Input)"
        )
    return label


def _first_str(mapping: dict[str, Any], keys: tuple[str, ...]) -> str | None:
    for key in keys:
        value = mapping.get(key)
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def _parse_iso_timestamp(value: str, *, role: str) -> datetime:
    text = value.strip()
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError as error:
        raise KeepGateError(
            f"refuse: {role} timestamp {value!r} is not ISO-8601"
        ) from error
    if parsed.tzinfo is None:
        raise KeepGateError(
            f"refuse: {role} timestamp {value!r} is timezone-naive"
        )
    return parsed.astimezone(timezone.utc)


def extract_self_time_frames(
    document: dict[str, Any], *, role: str = "current"
) -> list[dict[str, Any]]:
    """Parse named exclusive self-time frames. Fail-closed; never invent flamegraphs."""
    raw = document.get("attribution", document.get("mt8"))
    frames_raw = document.get("profile_frames")

    if raw is None and frames_raw is None:
        raise KeepGateError(
            f"refuse: {role} keep requires a named frame ≥{MT8_MIN_SELF_PCT}% "
            "self-time; attribution missing (micro-lever trap; do not invent flamegraphs)"
        )

    kind: str | None = None
    source: str | None = None
    if isinstance(raw, str):
        kind = raw
    elif isinstance(raw, dict):
        kind_value = raw.get("kind", raw.get("attribution"))
        if isinstance(kind_value, str):
            kind = kind_value
        source_value = raw.get("source")
        if isinstance(source_value, str):
            source = source_value
        if frames_raw is None:
            nested = raw.get("frames", raw.get("top_frames"))
            if nested is not None:
                frames_raw = nested
        if frames_raw is None and raw.get("flamegraph") is not None:
            raise KeepGateError(
                f"refuse: {role} flamegraph path is not named-frame attribution "
                "(do not invent flamegraphs)"
            )
    elif raw is not None:
        raise KeepGateError(
            f"refuse: {role} attribution must be an object with frames or a kind string"
        )

    if isinstance(kind, str):
        kind_norm = kind.strip().lower().replace(" ", "_")
        if kind_norm in _NOT_SELF_TIME_KINDS:
            raise KeepGateError(
                f"refuse: {role} attribution kind {kind!r} is not exclusive self-time "
                "(enter-count / invented / inclusive cannot keep; do not invent flamegraphs)"
            )

    if isinstance(source, str):
        source_l = source.strip().lower()
        if any(marker in source_l for marker in _INVENTED_SOURCE_MARKERS):
            raise KeepGateError(
                f"refuse: {role} attribution source {source!r} is invented "
                "(do not invent flamegraphs)"
            )

    if not isinstance(frames_raw, list) or not frames_raw:
        raise KeepGateError(
            f"refuse: {role} attribution missing named frames "
            "(do not invent flamegraphs)"
        )

    frames: list[dict[str, Any]] = []
    for index, item in enumerate(frames_raw):
        if not isinstance(item, dict):
            raise KeepGateError(f"refuse: {role} frame {index} must be an object")
        name = item.get("name", item.get("symbol", item.get("frame")))
        if not isinstance(name, str) or not name.strip():
            raise KeepGateError(
                f"refuse: {role} frame {index} missing name/symbol "
                "(do not invent flamegraphs)"
            )
        if "self_pct" in item:
            raw_pct = item["self_pct"]
        elif "self_time_pct" in item:
            raw_pct = item["self_time_pct"]
        else:
            continue
        try:
            pct = float(raw_pct)
        except (TypeError, ValueError) as error:
            raise KeepGateError(
                f"refuse: {role} frame {name!r}: self_pct must be numeric"
            ) from error
        if pct < 0:
            raise KeepGateError(
                f"refuse: {role} frame {name!r}: self_pct must be ≥ 0"
            )
        frames.append({"name": name.strip(), "self_pct": pct})

    if not frames:
        raise KeepGateError(
            f"refuse: {role} attribution has no self_pct; inclusive-only is not "
            "self-time keep evidence (do not invent flamegraphs)"
        )
    return frames


def qualifying_mt8_frame(frames: list[dict[str, Any]]) -> dict[str, Any] | None:
    """Highest named frame at or above the 0.1% self-time keep floor, or None."""
    eligible = [frame for frame in frames if frame["self_pct"] >= MT8_MIN_SELF_PCT]
    if not eligible:
        return None
    return max(eligible, key=lambda frame: (float(frame["self_pct"]), str(frame["name"])))


def require_mt8_keep_attribution(
    document: dict[str, Any], *, role: str = "current"
) -> tuple[bool, list[str]]:
    """Keep requires a named frame ≥0.1% self-time. Missing attribution refuses."""
    frames = extract_self_time_frames(document, role=role)
    frame = qualifying_mt8_frame(frames)
    if frame is None:
        max_pct = max(float(item["self_pct"]) for item in frames)
        listed = ", ".join(
            f"{item['name']}={item['self_pct']}%" for item in frames[:5]
        )
        return False, [
            f"FAIL keep ineligible: micro-lever trap; no named frame "
            f"≥{MT8_MIN_SELF_PCT}% self-time (max={max_pct}%); {listed}"
        ]
    return True, [
        f"PASS mt8 attribution: {frame['name']} {frame['self_pct']}% "
        f"self-time (≥{MT8_MIN_SELF_PCT}%)"
    ]


def run_window_identity(document: dict[str, Any], *, role: str) -> dict[str, Any]:
    """Git SHA + machine + timezone-aware timestamp. Missing fields refuse."""
    window = document.get("run_window")
    env = document.get("detected_environment")
    mappings: list[dict[str, Any]] = []
    if isinstance(window, dict):
        mappings.append(window)
    mappings.append(document)
    if isinstance(env, dict):
        mappings.append(env)

    git_sha: str | None = None
    machine: str | None = None
    ts_raw: str | None = None
    for mapping in mappings:
        if git_sha is None:
            git_sha = _first_str(mapping, _SHA_KEYS)
        if machine is None:
            machine = _first_str(mapping, _MACHINE_KEYS)
        if ts_raw is None:
            ts_raw = _first_str(mapping, _TS_KEYS)

    missing: list[str] = []
    if not git_sha or git_sha.lower() == "unknown":
        missing.append("git_sha")
    if not machine or machine.lower() == "unknown":
        missing.append("machine")
    if not ts_raw:
        missing.append("timestamp")
    if missing:
        raise KeepGateError(
            f"refuse: {role} run window missing {', '.join(missing)} "
            "(focused+broad keep requires git SHA, machine, same-minute timestamp)"
        )
    return {
        "git_sha": git_sha,
        "machine": machine,
        "timestamp": _parse_iso_timestamp(ts_raw, role=role),
    }


def require_same_run_window(
    focused: dict[str, Any],
    broad: dict[str, Any],
) -> tuple[bool, list[str]]:
    """Focused + broad must share git SHA, machine, and a ≤60s (same-minute) window."""
    focused_id = run_window_identity(focused, role="focused")
    broad_id = run_window_identity(broad, role="broad")
    messages: list[str] = []
    passed = True
    if focused_id["git_sha"] != broad_id["git_sha"]:
        passed = False
        messages.append(
            "FAIL run window: git_sha mismatch "
            f"focused={focused_id['git_sha']!r} broad={broad_id['git_sha']!r}"
        )
    if focused_id["machine"] != broad_id["machine"]:
        passed = False
        messages.append(
            "FAIL run window: machine mismatch "
            f"focused={focused_id['machine']!r} broad={broad_id['machine']!r}"
        )
    delta = abs(
        (focused_id["timestamp"] - broad_id["timestamp"]).total_seconds()
    )
    if delta > SAME_RUN_WINDOW_SECONDS:
        passed = False
        messages.append(
            f"FAIL run window: timestamps {delta:.1f}s apart "
            f"(same-minute window {SAME_RUN_WINDOW_SECONDS}s)"
        )
    if passed:
        messages.append(
            f"PASS run window: git_sha={focused_id['git_sha']} "
            f"machine={focused_id['machine']} "
            f"delta={delta:.1f}s (≤{SAME_RUN_WINDOW_SECONDS}s)"
        )
    return passed, messages


def load_history(path: Path) -> dict[str, Any]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except OSError as error:
        raise KeepGateError(f"cannot read {path}: {error}") from error
    except json.JSONDecodeError as error:
        raise KeepGateError(f"invalid JSON in {path}: {error}") from error
    if not isinstance(document, dict):
        raise KeepGateError(f"{path}: root must be a JSON object")
    if document.get("schema") != SCHEMA:
        raise KeepGateError(
            f"{path}: schema must be {SCHEMA!r}, got {document.get('schema')!r}"
        )
    groups = document.get("groups")
    if not isinstance(groups, list) or not groups:
        raise KeepGateError(f"{path}: groups must be a non-empty list")
    require_measurement_identity(document, role=str(path))
    return document


def _index_by_name(groups: list[dict[str, Any]]) -> dict[str, dict[str, Any]]:
    indexed: dict[str, dict[str, Any]] = {}
    for group in groups:
        name = str(group["name"])
        if name in indexed:
            raise KeepGateError(f"duplicate group name {name!r}")
        indexed[name] = group
    return indexed


def _regression_pct(current: float, baseline: float) -> float:
    """Positive when current is slower (worse) than baseline."""
    if baseline <= 0:
        raise KeepGateError("baseline mean must be positive")
    return ((current - baseline) / baseline) * 100.0


def compare_to_history(
    current: dict[str, Any],
    history: dict[str, Any],
    *,
    geomean_band_pct: float = KEEP_GATE_GEOMEAN_PCT,
    pass_band_pct: float = KEEP_GATE_PASS_PCT,
    peer: dict[str, Any] | None = None,
) -> tuple[bool, list[str]]:
    """Keep-gate: quarantine, then geomean / per-pass bands. Lower latency wins.

    label=live is a keep candidate: it needs a named ≥0.1% self-time frame and a
    focused+broad peer in the same run window. fixture-seed is a ratchet seed,
    not a keep, so MT8 / window are not invented for it.
    """
    messages: list[str] = []
    passed = True
    current_groups = current.get("groups")
    history_groups = history.get("groups")
    if not isinstance(current_groups, list) or not current_groups:
        raise KeepGateError("current: groups must be a non-empty list")
    if not isinstance(history_groups, list) or not history_groups:
        raise KeepGateError("history: groups must be a non-empty list")

    current_id = current.get("benchmark_id")
    history_id = history.get("benchmark_id")
    if not isinstance(current_id, str) or not current_id:
        raise KeepGateError("current: benchmark_id must be a non-empty string")
    if current_id != history_id:
        raise KeepGateError(
            f"refuse: benchmark_id mismatch current={current_id!r} history={history_id!r}"
        )

    current_label = require_measurement_identity(current, role="current")
    history_label = require_measurement_identity(history, role="history")
    if current_label != history_label:
        raise KeepGateError(
            f"refuse: cannot treat {history_label} history as {current_label} baseline "
            f"(current={current_label!r} history={history_label!r}; "
            "write a labeled sibling, do not overwrite fixture-seed latest.json)"
        )

    hist_names = _index_by_name(list(history_groups))
    cur_names = _index_by_name(list(current_groups))
    omitted = sorted(set(hist_names) - set(cur_names))
    if omitted:
        raise KeepGateError(
            "refuse: history groups missing from current: " + ", ".join(omitted)
        )

    if current_label == "live":
        att_ok, att_msgs = require_mt8_keep_attribution(current, role="current")
        messages.extend(att_msgs)
        if not att_ok:
            passed = False
        if peer is None:
            raise KeepGateError(
                "refuse: live keep requires focused+broad in the same run window "
                "(pass peer/--broad; both gates same git SHA, machine, minute)"
            )
        peer_label = require_measurement_identity(peer, role="peer")
        if peer_label != "live":
            raise KeepGateError(
                f"refuse: live keep peer must be label=live, got {peer_label!r}"
            )
        win_ok, win_msgs = require_same_run_window(current, peer)
        messages.extend(win_msgs)
        if not win_ok:
            passed = False

    kept_current, quarantined = quarantine_groups(current_groups)
    # cv>5 is noise and not eligible for keep. Do not drop the group and PASS.
    if quarantined:
        names = [str(g["name"]) for g in quarantined]
        passed = False
        messages.append(
            f"FAIL keep ineligible: cv_pct>{CV_PCT_QUARANTINE} is noise, "
            f"not a keep: {', '.join(names)}"
        )

    # History noisy groups are also excluded from the compare denominator.
    kept_history, _ = quarantine_groups(list(history_groups))
    hist_kept = _index_by_name(kept_history)
    cur_kept = _index_by_name(kept_current)

    shared = sorted(set(cur_kept) & set(hist_kept))
    if not shared:
        raise KeepGateError(
            "refuse: no shared non-quarantined groups between current and history"
        )

    cur_means = [group_mean(cur_kept[name]) for name in shared]
    hist_means = [group_mean(hist_kept[name]) for name in shared]

    for name, cur_m, hist_m in zip(shared, cur_means, hist_means, strict=True):
        reg = _regression_pct(cur_m, hist_m)
        if reg > pass_band_pct:
            passed = False
            messages.append(
                f"FAIL pass {name}: +{reg:.4f}% vs history "
                f"(band {pass_band_pct}%, current={cur_m}, history={hist_m})"
            )
        else:
            messages.append(
                f"PASS pass {name}: {reg:+.4f}% vs history "
                f"(band {pass_band_pct}%)"
            )

    cur_geo = geomean(cur_means)
    hist_geo = geomean(hist_means)
    geo_reg = _regression_pct(cur_geo, hist_geo)
    if geo_reg > geomean_band_pct:
        passed = False
        messages.append(
            f"FAIL geomean: +{geo_reg:.4f}% vs history "
            f"(band {geomean_band_pct}%, current={cur_geo}, history={hist_geo})"
        )
    else:
        messages.append(
            f"PASS geomean: {geo_reg:+.4f}% vs history "
            f"(band {geomean_band_pct}%)"
        )

    return passed, messages


def persist_gate(
    current: dict[str, Any],
    history: dict[str, Any],
    *,
    geomean_band_pct: float = KEEP_GATE_GEOMEAN_PCT,
    peer: dict[str, Any] | None = None,
) -> tuple[bool, list[str]]:
    """Persist uses the same 3% geomean constant as keep-gate (not 25%).

    cv_pct > 5 is ineligible for persist (fail closed). A noisy group is not
    dropped so the remaining geomean can green a keep. Live persist is a keep
    and needs MT8 attribution plus a same-minute peer gate.
    """
    return compare_to_history(
        current,
        history,
        geomean_band_pct=geomean_band_pct,
        pass_band_pct=geomean_band_pct,
        peer=peer,
    )


def evaluate_keep(
    focused: dict[str, Any],
    broad: dict[str, Any],
    *,
    history_focused: dict[str, Any] | None = None,
    history_broad: dict[str, Any] | None = None,
) -> tuple[bool, list[str]]:
    """Keep candidate: named ≥0.1% self-time frame + both gates same run window."""
    messages: list[str] = []
    att_ok, att_msgs = require_mt8_keep_attribution(focused, role="focused")
    messages.extend(att_msgs)
    win_ok, win_msgs = require_same_run_window(focused, broad)
    messages.extend(win_msgs)
    passed = att_ok and win_ok
    if history_focused is not None:
        hist_ok, hist_msgs = compare_to_history(
            focused, history_focused, peer=broad
        )
        messages.extend(hist_msgs)
        passed = passed and hist_ok
    if history_broad is not None:
        hist_ok, hist_msgs = compare_to_history(
            broad, history_broad, peer=focused
        )
        messages.extend(hist_msgs)
        passed = passed and hist_ok
    return passed, messages


def detect_binary_os(path: Path) -> str:
    """Return 'linux' or 'darwin' from magic bytes (not file(1) alone)."""
    try:
        with path.open("rb") as handle:
            magic = handle.read(4)
    except OSError as error:
        raise KeepGateError(f"cannot read binary {path}: {error}") from error
    if len(magic) < 4:
        raise KeepGateError(f"{path}: file too short to detect binary OS")
    if magic == ELF_MAGIC:
        return "linux"
    if magic in MACHO_MAGICS:
        return "darwin"
    raise KeepGateError(
        f"{path}: unrecognized binary magic {magic.hex()} "
        "(expected ELF or Mach-O)"
    )


def host_os() -> str:
    """Host OS from rustc -vV host when available, else sys.platform."""
    try:
        completed = subprocess.run(
            ["rustc", "-vV"],
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError:
        completed = None
    if completed is not None and completed.returncode == 0:
        for line in completed.stdout.splitlines():
            if line.startswith("host:"):
                triple = line.split(":", 1)[1].strip()
                if "apple-darwin" in triple or triple.endswith("-darwin"):
                    return "darwin"
                if "linux" in triple:
                    return "linux"
                break
    platform = sys.platform
    if platform == "darwin":
        return "darwin"
    if platform.startswith("linux"):
        return "linux"
    raise KeepGateError(f"unsupported host platform {platform!r}")


def resolve_tokenzero_bin() -> Path:
    """TOKENZERO_BIN wins; otherwise host install / PATH. Refuse OS mismatch."""
    env = os.environ.get("TOKENZERO_BIN")
    if env:
        path = Path(env).expanduser()
    else:
        candidates = [
            Path.home() / ".tokenzero" / "bin" / "tokenzero",
        ]
        which = shutil.which("tokenzero")
        if which:
            candidates.append(Path(which))
        path = next((c for c in candidates if c.is_file()), None)
        if path is None:
            raise KeepGateError(
                "TOKENZERO_BIN unset and no host tokenzero binary found "
                "(tried ~/.tokenzero/bin/tokenzero and PATH)"
            )

    if not path.is_file():
        raise KeepGateError(f"tokenzero binary not found: {path}")

    binary = detect_binary_os(path)
    host = host_os()
    if binary != host:
        raise KeepGateError(
            f"refuse: host OS is {host} but binary {path} is {binary} "
            "(ELF vs Mach-O mixup; set TOKENZERO_BIN to a host-native binary)"
        )
    return path


def _optional_history(path: Path | None) -> dict[str, Any] | None:
    if path is None:
        return None
    return load_history(path)


def _cmd_compare(args: argparse.Namespace) -> int:
    current = load_history(args.current)
    history = load_history(args.history)
    peer = _optional_history(args.broad)
    passed, messages = compare_to_history(current, history, peer=peer)
    for line in messages:
        print(line)
    print("Result: PASS" if passed else "Result: FAIL")
    return 0 if passed else 1


def _cmd_persist(args: argparse.Namespace) -> int:
    current = load_history(args.current)
    history = load_history(args.history)
    peer = _optional_history(args.broad)
    passed, messages = persist_gate(current, history, peer=peer)
    for line in messages:
        print(line)
    print(
        f"persist-gate band={KEEP_GATE_GEOMEAN_PCT}% "
        f"(KEEP_GATE_GEOMEAN_PCT)"
    )
    print("Result: PASS" if passed else "Result: FAIL")
    return 0 if passed else 1


def _cmd_keep(args: argparse.Namespace) -> int:
    focused = load_history(args.focused)
    broad = load_history(args.broad)
    passed, messages = evaluate_keep(
        focused,
        broad,
        history_focused=_optional_history(args.history_focused),
        history_broad=_optional_history(args.history_broad),
    )
    for line in messages:
        print(line)
    print("Result: PASS" if passed else "Result: FAIL")
    return 0 if passed else 1


def _cmd_resolve_bin(_args: argparse.Namespace) -> int:
    path = resolve_tokenzero_bin()
    print(path)
    return 0


def _cmd_dry_run(_args: argparse.Namespace) -> int:
    print("keep_gate dry-run")
    print(f"schema={SCHEMA}")
    print(f"KEEP_GATE_GEOMEAN_PCT={KEEP_GATE_GEOMEAN_PCT}")
    print(f"KEEP_GATE_PASS_PCT={KEEP_GATE_PASS_PCT}")
    print(f"CV_PCT_QUARANTINE={CV_PCT_QUARANTINE}")
    print(f"MT8_MIN_SELF_PCT={MT8_MIN_SELF_PCT}")
    print(f"SAME_RUN_WINDOW_SECONDS={SAME_RUN_WINDOW_SECONDS}")
    print(f"ALLOWED_LABELS={sorted(ALLOWED_LABELS)}")
    print(
        "cargo bench: rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_tokenzero "
        "cargo bench -p tokenzero-core --bench hotpaths --profile release-perf"
    )
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="keep_gate.py",
        description=(
            "TokenZero keep-gate: cv_pct quarantine, MT8 ≥0.1% self-time, "
            "focused+broad same-minute window, .bench-history ratchet, "
            "host-native binary resolve."
        ),
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print named constants and cargo bench invocation, then exit",
    )
    sub = parser.add_subparsers(dest="command")

    compare = sub.add_parser(
        "compare",
        help=(
            f"keep-gate vs history (geomean {KEEP_GATE_GEOMEAN_PCT}%% / "
            f"pass {KEEP_GATE_PASS_PCT}%%)"
        ),
    )
    compare.add_argument("--current", type=Path, required=True)
    compare.add_argument("--history", type=Path, required=True)
    compare.add_argument(
        "--broad",
        type=Path,
        default=None,
        help="peer gate JSON; required for label=live keep (same-minute window)",
    )
    compare.set_defaults(func=_cmd_compare)

    persist = sub.add_parser(
        "persist",
        help=f"persist-gate vs history (geomean band {KEEP_GATE_GEOMEAN_PCT}%%)",
    )
    persist.add_argument("--current", type=Path, required=True)
    persist.add_argument("--history", type=Path, required=True)
    persist.add_argument(
        "--broad",
        type=Path,
        default=None,
        help="peer gate JSON; required for label=live persist",
    )
    persist.set_defaults(func=_cmd_persist)

    keep = sub.add_parser(
        "keep",
        help=(
            "keep candidate: named frame ≥0.1% self-time and focused+broad "
            "same git SHA/machine/minute"
        ),
    )
    keep.add_argument("--focused", type=Path, required=True)
    keep.add_argument("--broad", type=Path, required=True)
    keep.add_argument("--history-focused", type=Path, default=None)
    keep.add_argument("--history-broad", type=Path, default=None)
    keep.set_defaults(func=_cmd_keep)

    resolve = sub.add_parser(
        "resolve-bin",
        help="print host-native TOKENZERO_BIN path or refuse OS mismatch",
    )
    resolve.set_defaults(func=_cmd_resolve_bin)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    if args.dry_run:
        return _cmd_dry_run(args)
    if not getattr(args, "command", None):
        parser.print_help()
        return 0
    try:
        return int(args.func(args))
    except KeepGateError as error:
        print(f"keep_gate: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
