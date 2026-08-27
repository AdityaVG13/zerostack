#!/usr/bin/env python3
"""Reproducible same-metric LOC counter over git archives (goal/loc).

Counts ``src/**/*.rs`` extracted from ``git archive <rev>`` with one stable
line classifier:

* blank — whitespace-only
* comment — line is only ``//...`` or inside/opening a ``/* ... */`` block
  (includes doc comments)
* code — every other non-empty line (trailing comments on code still count
  as code)
* total — blank + comment + code

Also reports densification-invariant ``non_ws_chars`` (non-whitespace byte
count across the same files). Packing/brace densification can shrink code LOC
while barely moving ``non_ws_chars``; that gap must be labeled densification,
never product improvement.

Canonical anchors under this exact method:

* baseline ``36a23a8`` → code LOC **26920**
* candidate ``bd9b712`` → code LOC **11616**

Rejected under this method: any claim that ``bd9b712`` is ~10.3k / 10300 code
LOC (false absolute).
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from typing import Any


BASELINE_REV = "36a23a8"
CANDIDATE_REV = "bd9b712"
BASELINE_CODE_LOC = 26920
CANDIDATE_CODE_LOC = 11616
FALSE_ABS_CODE_LOC_CLAIM = 10300


def classify_line_stream(lines: list[str]) -> dict[str, int]:
    total = code = blank = comment = 0
    in_block = False
    for line in lines:
        total += 1
        s = line.strip()
        if in_block:
            comment += 1
            if "*/" in s:
                in_block = False
            continue
        if not s:
            blank += 1
            continue
        if s.startswith("/*"):
            comment += 1
            if "*/" not in s[2:]:
                in_block = True
            continue
        if s.startswith("//"):
            comment += 1
            continue
        code += 1
    return {
        "total": total,
        "code": code,
        "blank": blank,
        "comment": comment,
    }


def count_src_tree(src_root: Path) -> dict[str, Any]:
    files = sorted(src_root.rglob("*.rs"))
    totals = {"total": 0, "code": 0, "blank": 0, "comment": 0}
    non_ws_chars = 0
    per_file: list[dict[str, Any]] = []
    for path in files:
        text = path.read_text(encoding="utf-8", errors="replace")
        non_ws_chars += sum(1 for c in text if not c.isspace())
        counts = classify_line_stream(text.splitlines())
        for k in totals:
            totals[k] += counts[k]
        rel = str(path.relative_to(src_root)).replace("\\", "/")
        per_file.append({"path": f"src/{rel}", **counts})
    return {
        "files": len(files),
        **totals,
        "non_ws_chars": non_ws_chars,
        "per_file": per_file,
    }


class _BytesReader:
    def __init__(self, data: bytes) -> None:
        self._data = data
        self._i = 0

    def read(self, n: int = -1) -> bytes:
        if n is None or n < 0:
            out = self._data[self._i :]
            self._i = len(self._data)
            return out
        out = self._data[self._i : self._i + n]
        self._i += n
        return out


def git_archive_src(repo: Path, rev: str, dest: Path) -> Path:
    dest.mkdir(parents=True, exist_ok=True)
    proc = subprocess.run(
        ["git", "archive", "--format=tar", rev, "src"],
        cwd=repo,
        check=True,
        stdout=subprocess.PIPE,
    )
    with tarfile.open(fileobj=_BytesReader(proc.stdout), mode="r|") as tar:
        tar.extractall(dest)
    src = dest / "src"
    if not src.is_dir():
        raise SystemExit(f"git archive {rev}: missing src/")
    return src


def resolve_rev(repo: Path, rev: str) -> str:
    return subprocess.check_output(
        ["git", "rev-parse", rev], cwd=repo, text=True
    ).strip()


def count_rev(repo: Path, rev: str) -> dict[str, Any]:
    full = resolve_rev(repo, rev)
    with tempfile.TemporaryDirectory(prefix="fsz-loc-") as tmp:
        src = git_archive_src(repo, full, Path(tmp))
        counts = count_src_tree(src)
    return {
        "rev": rev,
        "commit": full,
        "scope": "src/**/*.rs from git archive",
        "method": "git-archive-line-classifier-v1",
        **{
            k: counts[k]
            for k in ("files", "total", "code", "blank", "comment", "non_ws_chars")
        },
        "per_file": counts["per_file"],
    }


def densification_report(
    baseline: dict[str, Any], candidate: dict[str, Any]
) -> dict[str, Any]:
    """Separate packing/densification from product-shaped shrinkage.

    When code LOC falls much faster than non-whitespace payload, the excess
    is densification (formatting/brace packing), not product improvement.
    """
    code_delta = baseline["code"] - candidate["code"]
    non_ws_delta = baseline["non_ws_chars"] - candidate["non_ws_chars"]
    chars_per_code = (
        baseline["non_ws_chars"] / baseline["code"] if baseline["code"] else 0.0
    )
    productish_loc = (
        int(round(non_ws_delta / chars_per_code)) if chars_per_code else 0
    )
    densification_loc = max(0, code_delta - max(0, productish_loc))
    return {
        "definition": (
            "densification_loc = max(0, code_loc_delta - round(non_ws_chars_delta / "
            "baseline_non_ws_per_code_line)); packing must not be labeled product "
            "improvement"
        ),
        "code_loc_delta": code_delta,
        "non_ws_chars_delta": non_ws_delta,
        "baseline_non_ws_per_code_line": round(chars_per_code, 4),
        "productish_loc_estimate": productish_loc,
        "densification_loc": densification_loc,
        "packing_is_not_product_improvement": True,
        "labeling_rule": (
            "Never attribute densification_loc (or any packing-only code LOC drop) "
            "to product improvement, consolidation quality, or feature deletion."
        ),
    }


def _public_counts(row: dict[str, Any]) -> dict[str, Any]:
    return {
        "rev": row["rev"],
        "commit": row["commit"],
        "files": row["files"],
        "total": row["total"],
        "code": row["code"],
        "blank": row["blank"],
        "comment": row["comment"],
        "non_ws_chars": row["non_ws_chars"],
    }


def amendment_document(
    baseline: dict[str, Any],
    candidate: dict[str, Any],
    consolidation: dict[str, Any] | None = None,
) -> dict[str, Any]:
    dens = densification_report(baseline, candidate)
    cand_is_bd9 = candidate["rev"].startswith(CANDIDATE_REV) or candidate[
        "commit"
    ].startswith(CANDIDATE_REV)
    return {
        "schema": "fszero.loc-metric-amendment.v1",
        "method": "git-archive-line-classifier-v1",
        "scope": "src/**/*.rs from git archive",
        "anchors": {
            "baseline": {
                "rev": BASELINE_REV,
                "code_loc": BASELINE_CODE_LOC,
                "reproduced_code_loc": baseline["code"],
                "match": baseline["code"] == BASELINE_CODE_LOC,
            },
            "candidate_bd9b712": {
                "rev": CANDIDATE_REV,
                "code_loc": CANDIDATE_CODE_LOC,
                "reproduced_code_loc": candidate["code"] if cand_is_bd9 else None,
                "match_when_candidate_is_bd9b712": (
                    candidate["code"] == CANDIDATE_CODE_LOC if cand_is_bd9 else None
                ),
            },
        },
        "baseline": _public_counts(baseline),
        "candidate": _public_counts(candidate),
        "densification": dens,
        "consolidation": consolidation
        or {
            "true_deleted_implementation_loc": 0,
            "note": (
                "Fill with implementation LOC removed by unifying duplicate "
                "helpers; exclude formatting-only edits."
            ),
        },
        "rejected_claims": [
            {
                "claim": (
                    f"{CANDIDATE_REV} has ~10.3k / {FALSE_ABS_CODE_LOC_CLAIM} code LOC"
                ),
                "status": "rejected",
                "reason": (
                    f"Same method yields {CANDIDATE_CODE_LOC} code LOC for "
                    f"{CANDIDATE_REV}; observed candidate code LOC="
                    f"{candidate['code']}; 10.3k is a false absolute under this metric."
                ),
                "false_absolute_matches_observed": candidate["code"]
                == FALSE_ABS_CODE_LOC_CLAIM,
            },
            {
                "claim": "densification / brace packing is product improvement",
                "status": "rejected",
                "reason": dens["labeling_rule"],
            },
        ],
        "commands": {
            "count_baseline": f"python3 scripts/loc_git_archive.py --rev {BASELINE_REV}",
            "count_candidate": (
                f"python3 scripts/loc_git_archive.py --rev {CANDIDATE_REV}"
            ),
            "compare": (
                f"python3 scripts/loc_git_archive.py --baseline {BASELINE_REV} "
                f"--candidate {CANDIDATE_REV} --json"
            ),
            "verify_anchors": "python3 scripts/loc_git_archive.py --verify-anchors",
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--repo",
        type=Path,
        default=Path(__file__).resolve().parents[2],
        help="git repository root (default: parent of scripts/)",
    )
    parser.add_argument("--rev", help="count a single revision")
    parser.add_argument("--baseline", default=None, help="baseline rev for compare")
    parser.add_argument("--candidate", default=None, help="candidate rev for compare")
    parser.add_argument(
        "--json",
        action="store_true",
        help="emit machine-readable JSON",
    )
    parser.add_argument(
        "--amendment",
        action="store_true",
        help="emit loc-metric amendment document",
    )
    parser.add_argument(
        "--verify-anchors",
        action="store_true",
        help=(
            f"require {BASELINE_REV}={BASELINE_CODE_LOC} and "
            f"{CANDIDATE_REV}={CANDIDATE_CODE_LOC}"
        ),
    )
    parser.add_argument(
        "--include-per-file",
        action="store_true",
        help="include per-file breakdown (large)",
    )
    args = parser.parse_args(argv)
    repo = args.repo.resolve()

    if args.verify_anchors:
        b = count_rev(repo, BASELINE_REV)
        c = count_rev(repo, CANDIDATE_REV)
        ok = b["code"] == BASELINE_CODE_LOC and c["code"] == CANDIDATE_CODE_LOC
        doc = {
            "ok": ok,
            "baseline": _public_counts(b),
            "candidate": _public_counts(c),
            "expected": {
                "baseline_code": BASELINE_CODE_LOC,
                "candidate_code": CANDIDATE_CODE_LOC,
            },
            "rejected_false_10300_claim": c["code"] != FALSE_ABS_CODE_LOC_CLAIM,
        }
        print(json.dumps(doc, indent=2, sort_keys=True))
        return 0 if ok else 1

    if args.amendment or (args.baseline and args.candidate):
        baseline_rev = args.baseline or BASELINE_REV
        candidate_rev = args.candidate or CANDIDATE_REV
        baseline = count_rev(repo, baseline_rev)
        candidate = count_rev(repo, candidate_rev)
        if args.amendment:
            print(
                json.dumps(
                    amendment_document(baseline, candidate), indent=2, sort_keys=True
                )
            )
            return 0
        dens = densification_report(baseline, candidate)
        out: dict[str, Any] = {
            "baseline": _public_counts(baseline),
            "candidate": _public_counts(candidate),
            "densification": dens,
        }
        if args.include_per_file:
            out["baseline"]["per_file"] = baseline["per_file"]
            out["candidate"]["per_file"] = candidate["per_file"]
        if args.json:
            print(json.dumps(out, indent=2, sort_keys=True))
        else:
            _print_human_compare(out)
        return 0

    rev = args.rev or "HEAD"
    row = count_rev(repo, rev)
    if not args.include_per_file:
        row = _public_counts(row) | {
            "method": "git-archive-line-classifier-v1",
            "scope": "src/**/*.rs from git archive",
        }
    if args.json:
        print(json.dumps(row, indent=2, sort_keys=True))
    else:
        print(
            f"{row.get('rev', rev)}  files={row['files']}  total={row['total']}  "
            f"code={row['code']}  blank={row['blank']}  comment={row['comment']}  "
            f"non_ws_chars={row['non_ws_chars']}"
        )
    return 0


def _print_human_compare(out: dict[str, Any]) -> None:
    b, c, d = out["baseline"], out["candidate"], out["densification"]
    print(
        f"baseline {b['rev']}  code={b['code']} blank={b['blank']} "
        f"comment={b['comment']} total={b['total']} non_ws={b['non_ws_chars']}"
    )
    print(
        f"candidate {c['rev']}  code={c['code']} blank={c['blank']} "
        f"comment={c['comment']} total={c['total']} non_ws={c['non_ws_chars']}"
    )
    print(
        f"densification_loc={d['densification_loc']}  "
        f"productish_loc_estimate={d['productish_loc_estimate']}  "
        f"(packing is not product improvement)"
    )


if __name__ == "__main__":
    sys.exit(main())
