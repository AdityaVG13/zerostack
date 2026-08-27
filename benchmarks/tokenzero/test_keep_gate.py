from __future__ import annotations

import inspect
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import keep_gate

SCRIPT = Path(__file__).with_name("keep_gate.py")
HISTORY = (
    Path(__file__).resolve().parents[1]
    / ".bench-history"
    / "tokenzero-core.hotpaths.latest.json"
)


def _doc(groups: list[dict], **extra: object) -> dict:
    body: dict = {
        "schema": keep_gate.SCHEMA,
        "benchmark_id": "tokenzero-core.hotpaths",
        "primary": "count_tokens",
        "label": "fixture-seed",
        "note": (
            "Synthetic fixture-seed baseline for the keep-gate ratchet. "
            "Not a live unlabeled measurement percentage."
        ),
        "groups": groups,
    }
    body.update(extra)
    return body


def _write(path: Path, document: dict) -> None:
    path.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")


class KeepGateUnitTests(unittest.TestCase):
    def test_named_constants(self) -> None:
        self.assertEqual(keep_gate.KEEP_GATE_GEOMEAN_PCT, 3.0)
        self.assertEqual(keep_gate.KEEP_GATE_PASS_PCT, 5.0)
        self.assertEqual(keep_gate.CV_PCT_QUARANTINE, 5.0)
        self.assertEqual(keep_gate.MT8_MIN_SELF_PCT, 0.1)
        self.assertEqual(keep_gate.SAME_RUN_WINDOW_SECONDS, 60)
        self.assertEqual(
            keep_gate.ALLOWED_LABELS, frozenset({"fixture-seed", "live"})
        )
        # persist + keep share KEEP_GATE_GEOMEAN_PCT (not a leftover 25%).
        params = inspect.signature(keep_gate.persist_gate).parameters
        self.assertEqual(
            params["geomean_band_pct"].default, keep_gate.KEEP_GATE_GEOMEAN_PCT
        )
        params_c = inspect.signature(keep_gate.compare_to_history).parameters
        self.assertEqual(
            params_c["geomean_band_pct"].default, keep_gate.KEEP_GATE_GEOMEAN_PCT
        )

    def test_cv_pct_and_quarantine(self) -> None:
        stable = {"name": "stable", "samples": [100.0, 101.0, 99.0]}
        noisy = {"name": "noisy", "samples": [100.0, 200.0, 50.0]}
        self.assertLessEqual(keep_gate.cv_pct(stable["samples"]), 5.0)
        self.assertGreater(keep_gate.cv_pct(noisy["samples"]), 5.0)
        kept, quarantined = keep_gate.quarantine_groups([stable, noisy])
        self.assertEqual([g["name"] for g in kept], ["stable"])
        self.assertEqual([g["name"] for g in quarantined], ["noisy"])

    def test_all_quarantined_fails_closed(self) -> None:
        noisy = {"name": "only_noisy", "cv_pct": 31.0, "mean": 100.0}
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.quarantine_groups([noisy])
        self.assertIn("all primary groups quarantined", str(ctx.exception))

    def test_seed_history_compare_passes(self) -> None:
        history = keep_gate.load_history(HISTORY)
        self.assertEqual(history.get("label"), "fixture-seed")
        passed, messages = keep_gate.compare_to_history(history, history)
        self.assertTrue(passed, messages)
        self.assertTrue(any(line.startswith("PASS geomean") for line in messages))

    def test_plus_ten_percent_fails_compare_and_persist(self) -> None:
        history = keep_gate.load_history(HISTORY)
        worse_groups = []
        for group in history["groups"]:
            samples = [float(v) * 1.10 for v in group["samples"]]
            worse_groups.append(
                {
                    "name": group["name"],
                    "samples": samples,
                    "mean_ns": sum(samples) / len(samples),
                    "cv_pct": keep_gate.cv_pct(samples),
                }
            )
        current = _doc(worse_groups)
        compare_ok, compare_msgs = keep_gate.compare_to_history(current, history)
        persist_ok, persist_msgs = keep_gate.persist_gate(current, history)
        self.assertFalse(compare_ok, compare_msgs)
        self.assertFalse(persist_ok, persist_msgs)
        self.assertTrue(any("FAIL geomean" in line for line in compare_msgs))
        self.assertTrue(any("FAIL geomean" in line for line in persist_msgs))

    def test_quarantined_group_is_ineligible_for_keep(self) -> None:
        history = _doc(
            [
                {"name": "stable", "samples": [100.0, 100.0, 100.0]},
                {"name": "noisy", "samples": [100.0, 100.0, 100.0]},
            ]
        )
        current = _doc(
            [
                # within pass band vs history stable
                {"name": "stable", "samples": [101.0, 101.0, 101.0]},
                # would be a huge regression if averaged in. cv>5 is noise:
                # ineligible for keep, not dropped-then-PASS.
                {"name": "noisy", "samples": [100.0, 400.0, 50.0]},
            ]
        )
        passed, messages = keep_gate.compare_to_history(current, history)
        self.assertFalse(passed, messages)
        self.assertTrue(any("FAIL keep ineligible" in line for line in messages))
        self.assertTrue(any("noisy" in line for line in messages))
        self.assertTrue(any("PASS pass stable" in line for line in messages))
        self.assertFalse(any("PASS pass noisy" in line for line in messages))
        persist_ok, persist_msgs = keep_gate.persist_gate(current, history)
        self.assertFalse(persist_ok, persist_msgs)
        self.assertTrue(any("FAIL keep ineligible" in line for line in persist_msgs))

    def test_omitted_history_group_fails_closed(self) -> None:
        history = _doc(
            [
                {"name": "stable", "samples": [100.0, 100.0, 100.0]},
                {"name": "render_shell", "samples": [200.0, 200.0, 200.0]},
            ]
        )
        current = _doc(
            [
                {"name": "stable", "samples": [101.0, 101.0, 101.0]},
            ]
        )
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.compare_to_history(current, history)
        self.assertIn("history groups missing from current", str(ctx.exception))
        self.assertIn("render_shell", str(ctx.exception))

    def test_unlabeled_history_refuses_as_live(self) -> None:
        groups = [{"name": "stable", "samples": [100.0, 100.0, 100.0]}]
        labeled = _doc(groups)
        unlabeled = _doc(groups)
        del unlabeled["label"]
        unlabeled.pop("note", None)
        with self.assertRaises(keep_gate.KeepGateError) as persist_ctx:
            keep_gate.persist_gate(unlabeled, labeled)
        persist_msg = str(persist_ctx.exception).lower()
        self.assertIn("unlabeled", persist_msg)
        self.assertIn("live", persist_msg)
        with self.assertRaises(keep_gate.KeepGateError) as compare_ctx:
            keep_gate.compare_to_history(unlabeled, labeled)
        self.assertIn("unlabeled", str(compare_ctx.exception).lower())

        missing_note = _doc(groups)
        missing_note["note"] = ""
        with self.assertRaises(keep_gate.KeepGateError) as note_ctx:
            keep_gate.persist_gate(missing_note, labeled)
        self.assertIn("missing note", str(note_ctx.exception).lower())

    def test_q99_identity_refuses(self) -> None:
        groups = [{"name": "stable", "samples": [100.0, 100.0, 100.0]}]
        q99 = _doc(groups, note="Q99-Input estimator disguised as latency")
        labeled = _doc(groups)
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.compare_to_history(q99, labeled)
        self.assertIn("Q99", str(ctx.exception))

    def test_persist_refuses_live_over_fixture_seed(self) -> None:
        groups = [{"name": "stable", "samples": [100.0, 100.0, 100.0]}]
        history = _doc(groups)
        current = _doc(
            groups,
            label="live",
            note="live Criterion release-perf sibling; not fixture-seed",
        )
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.persist_gate(current, history)
        message = str(ctx.exception)
        self.assertIn("fixture-seed", message)
        self.assertIn("sibling", message)

    def test_benchmark_id_mismatch_fails_closed(self) -> None:
        history = _doc([{"name": "stable", "samples": [100.0, 100.0, 100.0]}])
        current = _doc(
            [{"name": "stable", "samples": [100.0, 100.0, 100.0]}],
            benchmark_id="other.bench",
        )
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.compare_to_history(current, history)
        self.assertIn("benchmark_id mismatch", str(ctx.exception))

    def test_detect_binary_os_magic(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            elf = root / "elf.bin"
            macho = root / "macho.bin"
            elf.write_bytes(b"\x7fELF" + b"\x00" * 12)
            macho.write_bytes(b"\xcf\xfa\xed\xfe" + b"\x00" * 12)
            self.assertEqual(keep_gate.detect_binary_os(elf), "linux")
            self.assertEqual(keep_gate.detect_binary_os(macho), "darwin")

    def test_resolve_bin_refuses_os_mismatch(self) -> None:
        host = keep_gate.host_os()
        wrong_magic = (
            b"\x7fELF" + b"\x00" * 12
            if host == "darwin"
            else b"\xcf\xfa\xed\xfe" + b"\x00" * 12
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "tokenzero"
            path.write_bytes(wrong_magic)
            env = os.environ.copy()
            env["TOKENZERO_BIN"] = str(path)
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "resolve-bin"],
                text=True,
                capture_output=True,
                check=False,
                env=env,
            )
            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn("refuse", result.stderr.lower())
            self.assertIn("mixup", result.stderr.lower())


class KeepGateCliTests(unittest.TestCase):
    def test_help(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--help"],
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("compare", result.stdout)
        self.assertIn("persist", result.stdout)
        self.assertIn("keep", result.stdout)
        self.assertIn("resolve-bin", result.stdout)

    def test_dry_run(self) -> None:
        result = subprocess.run(
            [sys.executable, str(SCRIPT), "--dry-run"],
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("KEEP_GATE_GEOMEAN_PCT=3.0", result.stdout)
        self.assertIn("MT8_MIN_SELF_PCT=0.1", result.stdout)
        self.assertIn("SAME_RUN_WINDOW_SECONDS=60", result.stdout)
        self.assertIn("CARGO_TARGET_DIR=/tmp/rch_target_tokenzero", result.stdout)

    def test_cli_compare_seed_pass_and_worse_fail(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            history = keep_gate.load_history(HISTORY)
            same = root / "same.json"
            worse = root / "worse.json"
            _write(same, history)
            worse_doc = json.loads(json.dumps(history))
            for group in worse_doc["groups"]:
                group["samples"] = [float(v) * 1.10 for v in group["samples"]]
                group["mean_ns"] = sum(group["samples"]) / len(group["samples"])
                group["cv_pct"] = keep_gate.cv_pct(group["samples"])
            _write(worse, worse_doc)

            ok = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "compare",
                    "--current",
                    str(same),
                    "--history",
                    str(HISTORY),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(ok.returncode, 0, ok.stderr + ok.stdout)
            self.assertIn("Result: PASS", ok.stdout)

            bad = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "compare",
                    "--current",
                    str(worse),
                    "--history",
                    str(HISTORY),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(bad.returncode, 1, bad.stderr + bad.stdout)
            self.assertIn("Result: FAIL", bad.stdout)

            persist_bad = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "persist",
                    "--current",
                    str(worse),
                    "--history",
                    str(HISTORY),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(
                persist_bad.returncode, 1, persist_bad.stderr + persist_bad.stdout
            )
            self.assertIn("KEEP_GATE_GEOMEAN_PCT", persist_bad.stdout)

    def test_cli_persist_unlabeled_refuses(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            unlabeled = json.loads(json.dumps(keep_gate.load_history(HISTORY)))
            unlabeled.pop("label", None)
            unlabeled.pop("note", None)
            path = root / "unlabeled.json"
            _write(path, unlabeled)
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "persist",
                    "--current",
                    str(path),
                    "--history",
                    str(HISTORY),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 2, result.stderr + result.stdout)
            self.assertIn("unlabeled", result.stderr.lower())
            self.assertIn("live", result.stderr.lower())


def _attribution(
    name: str = "tokenzero_core::tokens::count_tokens",
    self_pct: float = 0.44,
    *,
    kind: str = "self-time",
    source: str = "samply",
    extra_frames: list[dict] | None = None,
) -> dict:
    frames = [{"name": name, "self_pct": self_pct}]
    if extra_frames:
        frames.extend(extra_frames)
    return {"kind": kind, "workload": "MT8", "source": source, "frames": frames}


def _window(
    sha: str = "4ad6f579e6d9dab99722f1ca538c8009a14199cc",
    machine: str = "gauntlet-host",
    ts: str = "2026-08-24T19:00:00Z",
) -> dict:
    return {"git_sha": sha, "machine": machine, "timestamp": ts}


def _live_doc(groups: list[dict], **extra: object) -> dict:
    body = _doc(
        groups,
        label="live",
        note="live Criterion release-perf sibling; not fixture-seed",
    )
    if "attribution" not in extra:
        body["attribution"] = _attribution()
    if "run_window" not in extra:
        body["run_window"] = _window()
    body.update(extra)
    return body


class KeepGateMt8Tests(unittest.TestCase):
    def setUp(self) -> None:
        self.groups = [{"name": "stable", "samples": [100.0, 100.0, 100.0]}]

    def test_missing_attribution_refuses_live_keep(self) -> None:
        current = _live_doc(self.groups)
        del current["attribution"]
        peer = _live_doc(self.groups, benchmark_id="tokenzero-core.hotpaths.broad")
        history = _live_doc(self.groups)
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.compare_to_history(current, history, peer=peer)
        message = str(ctx.exception).lower()
        self.assertIn("attribution missing", message)
        self.assertIn("do not invent flamegraphs", message)
        self.assertIn("micro-lever trap", message)

    def test_flamegraph_path_without_frames_refuses(self) -> None:
        current = _live_doc(
            self.groups,
            attribution={"kind": "self-time", "flamegraph": "artifacts/fake.svg"},
        )
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.require_mt8_keep_attribution(current)
        self.assertIn("do not invent flamegraphs", str(ctx.exception).lower())

    def test_enter_count_is_not_self_time(self) -> None:
        current = _live_doc(
            self.groups,
            attribution="enter_count",
        )
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.extract_self_time_frames(current)
        self.assertIn("not exclusive self-time", str(ctx.exception).lower())

    def test_invented_source_refuses(self) -> None:
        current = _live_doc(
            self.groups,
            attribution=_attribution(source="invented-flamegraph"),
        )
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.require_mt8_keep_attribution(current)
        self.assertIn("invented", str(ctx.exception).lower())

    def test_inclusive_only_is_not_self_time(self) -> None:
        current = _live_doc(
            self.groups,
            attribution={
                "kind": "self-time",
                "source": "samply",
                "frames": [
                    {"name": "foo", "inclusive_pct": 2.5},
                ],
            },
        )
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.extract_self_time_frames(current)
        self.assertIn("inclusive-only", str(ctx.exception).lower())

    def test_micro_lever_trap_below_floor_is_not_a_keep(self) -> None:
        current = _live_doc(
            self.groups,
            attribution=_attribution(self_pct=0.05),
        )
        peer = _live_doc(self.groups)
        history = _live_doc(self.groups)
        passed, messages = keep_gate.compare_to_history(
            current, history, peer=peer
        )
        self.assertFalse(passed, messages)
        self.assertTrue(any("micro-lever trap" in line for line in messages))
        self.assertTrue(any("FAIL keep ineligible" in line for line in messages))

    def test_named_frame_at_floor_qualifies(self) -> None:
        current = _live_doc(
            self.groups,
            attribution=_attribution(self_pct=0.1),
        )
        peer = _live_doc(self.groups)
        history = _live_doc(self.groups)
        passed, messages = keep_gate.compare_to_history(
            current, history, peer=peer
        )
        self.assertTrue(passed, messages)
        self.assertTrue(any(line.startswith("PASS mt8 attribution") for line in messages))

    def test_live_without_peer_refuses_same_window(self) -> None:
        current = _live_doc(self.groups)
        history = _live_doc(self.groups)
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.compare_to_history(current, history)
        message = str(ctx.exception).lower()
        self.assertIn("same run window", message)
        self.assertIn("focused+broad", message)

    def test_git_sha_mismatch_fails_window(self) -> None:
        focused = _live_doc(self.groups, run_window=_window(sha="aaa"))
        broad = _live_doc(self.groups, run_window=_window(sha="bbb"))
        passed, messages = keep_gate.require_same_run_window(focused, broad)
        self.assertFalse(passed, messages)
        self.assertTrue(any("git_sha mismatch" in line for line in messages))

    def test_machine_mismatch_fails_window(self) -> None:
        focused = _live_doc(self.groups, run_window=_window(machine="host-a"))
        broad = _live_doc(self.groups, run_window=_window(machine="host-b"))
        passed, messages = keep_gate.require_same_run_window(focused, broad)
        self.assertFalse(passed, messages)
        self.assertTrue(any("machine mismatch" in line for line in messages))

    def test_timestamps_outside_same_minute_fail(self) -> None:
        focused = _live_doc(
            self.groups, run_window=_window(ts="2026-08-24T19:00:00Z")
        )
        broad = _live_doc(
            self.groups, run_window=_window(ts="2026-08-24T19:01:01Z")
        )
        passed, messages = keep_gate.require_same_run_window(focused, broad)
        self.assertFalse(passed, messages)
        self.assertTrue(any("timestamps" in line and "apart" in line for line in messages))

    def test_same_minute_window_passes(self) -> None:
        focused = _live_doc(
            self.groups, run_window=_window(ts="2026-08-24T19:00:00Z")
        )
        broad = _live_doc(
            self.groups, run_window=_window(ts="2026-08-24T19:01:00Z")
        )
        passed, messages = keep_gate.require_same_run_window(focused, broad)
        self.assertTrue(passed, messages)
        self.assertTrue(any(line.startswith("PASS run window") for line in messages))

    def test_missing_run_window_fields_fail_closed(self) -> None:
        focused = _live_doc(self.groups, run_window={"git_sha": "abc"})
        broad = _live_doc(self.groups)
        with self.assertRaises(keep_gate.KeepGateError) as ctx:
            keep_gate.require_same_run_window(focused, broad)
        message = str(ctx.exception).lower()
        self.assertIn("missing", message)
        self.assertIn("machine", message)

    def test_evaluate_keep_requires_frame_and_window(self) -> None:
        focused = _live_doc(self.groups)
        broad = _live_doc(self.groups)
        passed, messages = keep_gate.evaluate_keep(focused, broad)
        self.assertTrue(passed, messages)
        self.assertTrue(any("PASS mt8 attribution" in line for line in messages))
        self.assertTrue(any("PASS run window" in line for line in messages))

    def test_evaluate_keep_micro_lever_and_sha_split_fails(self) -> None:
        focused = _live_doc(
            self.groups,
            attribution=_attribution(self_pct=0.09),
            run_window=_window(sha="sha-focused"),
        )
        broad = _live_doc(self.groups, run_window=_window(sha="sha-broad"))
        passed, messages = keep_gate.evaluate_keep(focused, broad)
        self.assertFalse(passed, messages)
        self.assertTrue(any("micro-lever trap" in line for line in messages))
        self.assertTrue(any("git_sha mismatch" in line for line in messages))

    def test_cli_keep_pass_and_missing_attribution(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            focused = _live_doc(self.groups)
            broad = _live_doc(self.groups)
            focused_path = root / "focused.json"
            broad_path = root / "broad.json"
            _write(focused_path, focused)
            _write(broad_path, broad)
            ok = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "keep",
                    "--focused",
                    str(focused_path),
                    "--broad",
                    str(broad_path),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(ok.returncode, 0, ok.stderr + ok.stdout)
            self.assertIn("Result: PASS", ok.stdout)
            self.assertIn("PASS mt8 attribution", ok.stdout)
            self.assertIn("PASS run window", ok.stdout)

            missing = json.loads(json.dumps(focused))
            del missing["attribution"]
            missing_path = root / "missing.json"
            _write(missing_path, missing)
            bad = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "keep",
                    "--focused",
                    str(missing_path),
                    "--broad",
                    str(broad_path),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(bad.returncode, 2, bad.stderr + bad.stdout)
            self.assertIn("attribution missing", bad.stderr.lower())
            self.assertIn("do not invent flamegraphs", bad.stderr.lower())

    def test_cli_live_compare_without_broad_refuses(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            current = _live_doc(self.groups)
            history = _live_doc(self.groups)
            current_path = root / "current.json"
            history_path = root / "history.json"
            _write(current_path, current)
            _write(history_path, history)
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "compare",
                    "--current",
                    str(current_path),
                    "--history",
                    str(history_path),
                ],
                text=True,
                capture_output=True,
                check=False,
            )
            self.assertEqual(result.returncode, 2, result.stderr + result.stdout)
            self.assertIn("same run window", result.stderr.lower())


if __name__ == "__main__":
    unittest.main()
