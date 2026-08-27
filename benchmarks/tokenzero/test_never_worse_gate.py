from __future__ import annotations

import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("never_worse_gate.py")
REPO = Path(__file__).resolve().parents[1]
SCRIPTS_GATE = REPO / "scripts" / "never_worse_gate.py"
MILLION_LINE = Path(__file__).with_name("million-line-nav.sh")
BAKEOFF = Path(__file__).with_name("competitor-bakeoff.sh")
LIVE_DRIVER_SH = (MILLION_LINE, BAKEOFF)
NOT_LIVE_COMMENT_RE = re.compile(
    r"not the live never-worse driver|stale duplicate|legacy duplicate",
    re.IGNORECASE,
)


def receipt(
    *rows: str,
    unit_id: str = "estimator:bytes-ceil-div4/v1",
    surface_id: str = "captured-stdout-bytes/v1",
    suite: str = "test-suite",
) -> str:
    return "\n".join(
        [
            "schema_version\tnever-worse/v1",
            f"suite\t{suite}",
            f"surface_id\t{surface_id}",
            f"unit_id\t{unit_id}",
            "task\tcandidate_bytes\traw_bytes\tcandidate_units\traw_units",
            *rows,
            "",
        ]
    )


class NeverWorseGateTests(unittest.TestCase):
    def run_gate(self, content: str) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "receipt.tsv"
            path.write_text(content, encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(SCRIPT), str(path)],
                text=True,
                capture_output=True,
                check=False,
            )

    def test_equal_and_better_rows_pass(self) -> None:
        result = self.run_gate(receipt("read\t4\t8\t1\t2", "edit\t5\t5\t2\t2"))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Result: PASS", result.stdout)

    def test_worse_row_fails(self) -> None:
        result = self.run_gate(receipt("read\t9\t8\t3\t2"))
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("**FAIL**", result.stdout)

    def test_count_or_unit_mismatch_fails_closed(self) -> None:
        wrong_count = self.run_gate(receipt("read\t8\t8\t3\t2"))
        self.assertEqual(wrong_count.returncode, 2)
        self.assertIn("count mismatch", wrong_count.stderr)
        wrong_unit = self.run_gate(
            receipt("read\t8\t8\t2\t2", unit_id="provider:unverified")
        )
        self.assertEqual(wrong_unit.returncode, 2)
        self.assertIn("unit_id mismatch", wrong_unit.stderr)
        q99 = self.run_gate(receipt("read\t8\t8\t2\t2", unit_id="Q99-Input"))
        self.assertEqual(q99.returncode, 2)
        self.assertIn("Q99-Input is not a TokenZero product unit", q99.stderr)
        q99_suite = self.run_gate(
            receipt("read\t8\t8\t2\t2", suite="Q99-Input-bakeoff")
        )
        self.assertEqual(q99_suite.returncode, 2)
        self.assertIn("Q99-Input is not a TokenZero product unit", q99_suite.stderr)
        empty = self.run_gate(receipt("read\t0\t8\t0\t2"))
        self.assertEqual(empty.returncode, 2)
        self.assertIn("empty candidate", empty.stderr)

    def test_visible_payload_surface_is_accepted(self) -> None:
        result = self.run_gate(
            receipt(
                "grep_expand_edit_verify\t40\t80\t10\t20",
                surface_id="visible-payload-bytes/v1",
            )
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("visible-payload-bytes/v1", result.stdout)

    def test_unknown_surface_fails_closed(self) -> None:
        result = self.run_gate(
            receipt("read\t4\t8\t1\t2", surface_id="envelope-inclusive-stdout/v0")
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("not a never-worse denominator", result.stderr)
        wrong_suite_surface = self.run_gate(
            receipt(
                "read_50_lines\t4\t8\t1\t2",
                suite="million-line-nav",
                surface_id="captured-stdout-bytes/v1",
            )
        )
        self.assertEqual(wrong_suite_surface.returncode, 2)
        self.assertIn("must use surface", wrong_suite_surface.stderr)

    def test_duplicate_or_missing_rows_fail_closed(self) -> None:
        duplicate = self.run_gate(receipt("read\t4\t8\t1\t2", "read\t4\t8\t1\t2"))
        self.assertEqual(duplicate.returncode, 2)
        self.assertIn("duplicate task", duplicate.stderr)
        missing = self.run_gate(receipt())
        self.assertEqual(missing.returncode, 2)
        self.assertIn("task rows", missing.stderr)

    def test_bakeoff_edit_verify_teardown_is_outside_timed_command(self) -> None:
        text = BAKEOFF.read_text(encoding="utf-8")
        self.assertIn("--teardown", text)
        self.assertIn("teardown_for", text)
        self.assertIn("rm -f %q.bak", text)
        in_command_for = False
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.startswith("command_for()"):
                in_command_for = True
            elif stripped.startswith("teardown_for()") or stripped.startswith("log "):
                in_command_for = False
            if not in_command_for or stripped.startswith("#"):
                continue
            if "edit_verify:raw-cli" in stripped:
                self.assertNotIn(
                    "rm -f",
                    stripped,
                    "bak cleanup must not live inside the timed edit_verify command",
                )

    def test_live_drivers_invoke_benchmarks_gate_not_scripts_copy(self) -> None:
        for path in LIVE_DRIVER_SH:
            text = path.read_text(encoding="utf-8")
            self.assertIn(
                "benchmarks/never_worse_gate.py",
                text,
                f"{path.name} must invoke the live gate",
            )
            self.assertNotIn(
                "scripts/never_worse_gate.py",
                text,
                f"{path.name} must not invoke the scripts/ copy",
            )

    def test_scripts_copy_matches_live_or_is_marked_not_live(self) -> None:
        # scripts/never_worse_gate.py is gitignored leftover, not the live driver.
        # Missing file is fine. A present copy must match benchmarks/ or say so.
        if not SCRIPTS_GATE.is_file():
            return
        live = SCRIPT.read_text(encoding="utf-8")
        scripts = SCRIPTS_GATE.read_text(encoding="utf-8")
        if live == scripts:
            return
        self.assertRegex(
            scripts,
            NOT_LIVE_COMMENT_RE,
            "scripts/never_worse_gate.py diverges from benchmarks/ without a "
            "comment that it is not the live driver",
        )

    def test_million_line_receipt_header_is_estimator_not_q99(self) -> None:
        text = MILLION_LINE.read_text(encoding="utf-8")
        match = re.search(
            r"printf '([^']*schema_version\\tnever-worse/v1[^']*)'",
            text,
        )
        self.assertIsNotNone(match, "million-line-nav.sh must printf a never-worse receipt header")
        header = match.group(1)
        self.assertIn("suite\\tmillion-line-nav", header)
        self.assertIn("surface_id\\tvisible-payload-bytes/v1", header)
        self.assertIn("unit_id\\testimator:bytes-ceil-div4/v1", header)
        self.assertNotIn("Q99", header)
        self.assertNotIn("captured-stdout-bytes/v1", header)

    def test_million_line_expand_is_integrity_only(self) -> None:
        text = MILLION_LINE.read_text(encoding="utf-8")
        self.assertRegex(text, r"expand is integrity[- ]only", re.IGNORECASE)
        self.assertIn("envelope JSON excluded", text)
        self.assertIn('record_gate grep_expand "$tz_vis_b"', text)
        self.assertIn('record_gate grep_expand_edit_verify "$tz_vis_d"', text)
        self.assertIn("tz_vis_d=$((tz_vis_d1+tz_vis_d4))", text)
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.startswith("#") or "record_gate" not in stripped:
                continue
            self.assertNotIn("_tz_expand_envelope", stripped)
            self.assertNotIn("expand_envelope", stripped)

    def test_million_line_requires_exact_task_set(self) -> None:
        result = self.run_gate(
            receipt(
                "read_50_lines\t4\t8\t1\t2",
                suite="million-line-nav",
                surface_id="visible-payload-bytes/v1",
            )
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("must contain exactly", result.stderr)

    def test_published_million_line_fail_shape_still_fails(self) -> None:
        # Published FAIL in docs/benchmarks.md. Do not treat as a keep.
        # Numbers are ceil(bytes/4). Corpus not re-run this round.
        result = self.run_gate(
            receipt(
                "read_50_lines\t326\t2500\t82\t625",
                "grep_expand\t5862\t2420\t1466\t605",
                "tree_glob_read\t1880\t3634\t470\t909",
                "grep_expand_edit_verify\t1378\t258\t345\t65",
                "recall\t1823\t1210\t456\t303",
                suite="million-line-nav",
                surface_id="visible-payload-bytes/v1",
            )
        )
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("Result: FAIL", result.stdout)
        self.assertIn("`grep_expand`", result.stdout)
        self.assertIn("`grep_expand_edit_verify`", result.stdout)
        self.assertIn("`recall`", result.stdout)
        self.assertIn("visible-payload-bytes/v1", result.stdout)
        self.assertIn("estimator:bytes-ceil-div4/v1", result.stdout)
        self.assertIn("not Q99", result.stdout)


if __name__ == "__main__":
    unittest.main()
