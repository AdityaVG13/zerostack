from __future__ import annotations

import json
import os
import shlex
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from benchmarks.bench_common import portable_path
from benchmarks.harness import (
    REPO,
    VisiblePayloadError,
    _times,
    bin_path,
    capture_environment,
    expand_recovered_text,
    first_blob_ref,
    glob_root_and_first,
    measure_median,
    measure_with_teardown,
    refuse_noisy_keep,
    visible_payload_bytes,
)

BLOB = "tz://blob/" + "a" * 64
ORDINAL = "tz://o/2/1"


class FirstBlobRefTests(unittest.TestCase):
    def test_full_envelope_selects_blob_kind(self) -> None:
        self.assertEqual(
            first_blob_ref(
                {
                    "refs": [
                        {"kind": "file", "ref": "tz://file/f123"},
                        {"kind": "blob", "ref": BLOB},
                    ]
                }
            ),
            BLOB,
        )

    def test_slim_envelope_accepts_only_first_durable_primary_ref(self) -> None:
        self.assertEqual(
            first_blob_ref({"refs": [ORDINAL, "tz://file/f123", "tz://search/h123"]}),
            ORDINAL,
        )
        self.assertEqual(first_blob_ref({"refs": ["tz://file/f123", ORDINAL]}), "")
        self.assertEqual(first_blob_ref({"refs": ["https://invalid", ORDINAL]}), "")
        self.assertEqual(first_blob_ref({"refs": ["tz://o/0/1", ORDINAL]}), "")

    def test_invalid_or_mixed_shapes_fail_closed(self) -> None:
        self.assertEqual(
            first_blob_ref({"refs": [ORDINAL, {"kind": "blob", "ref": BLOB}]}),
            "",
        )
        self.assertEqual(first_blob_ref({"refs": "not-a-list"}), "")
        self.assertEqual(first_blob_ref({"refs": [17]}), "")

    def test_legacy_detail_ref_requires_a_durable_primary_shape(self) -> None:
        self.assertEqual(first_blob_ref({"detail_ref": BLOB}), BLOB)
        self.assertEqual(first_blob_ref({"detail_ref": "tz://file/f123"}), "")

    def test_glob_parser_accepts_slim_and_full_visible_shapes(self) -> None:
        text = "# root: /work\nsrc/lib.rs\nsrc/main.rs"
        self.assertEqual(
            glob_root_and_first({"visible": text}), ("/work", "src/lib.rs")
        )
        self.assertEqual(
            glob_root_and_first({"visible": {"text": text}}),
            ("/work", "src/lib.rs"),
        )
        with self.assertRaisesRegex(ValueError, "visible text is missing"):
            glob_root_and_first({"visible": 7})

    def test_glob_parser_reconstructs_first_escaped_trie_file(self) -> None:
        root = '/work space/µ\n"quoted"'
        directories = ["src space", 'β\n"branch"']
        file_name = "item [separator-like].rs"
        text = "\n".join(
            [
                f"# root: {json.dumps(root, ensure_ascii=False)}",
                f"{json.dumps(directories[0], ensure_ascii=False)}/",
                f"  {json.dumps(directories[1], ensure_ascii=False)}/",
                f"    {json.dumps(file_name, ensure_ascii=False)}",
                '"later.rs"',
            ]
        )
        self.assertEqual(
            glob_root_and_first({"visible": {"text": text}}),
            (root, "/".join([*directories, file_name])),
        )

    def test_glob_parser_rejects_malformed_or_truncated_tries(self) -> None:
        root = "/work"
        encoded_root = json.dumps(root)
        malformed = [
            "# root: \nlegacy.rs",
            "# root: /work\nlegacy-directory/",
            f'# root: {encoded_root}\n"src"/',
            f'# root: {encoded_root}\n "odd-indent.rs"',
            f'# root: {encoded_root}\n    "skip-depth.rs"',
            f'# root: {encoded_root}\n"src"/\n"missing-child.rs"',
            f"# root: {encoded_root}\n17",
            f'# root: {encoded_root}\n"bad/name.rs"',
            f'# root: {encoded_root}\n"spaced-directory" /',
            f'# root: {encoded_root}\n# outside-roots\n"orphan.rs"',
        ]
        for text in malformed:
            with self.subTest(text=text):
                with self.assertRaisesRegex(ValueError, "malformed"):
                    glob_root_and_first({"visible": text})
        with self.assertRaisesRegex(ValueError, "malformed"):
            glob_root_and_first({"visible": '# root: "unterminated\n"file.rs"'})

    def test_glob_parser_returns_empty_only_for_typed_valid_no_match(self) -> None:
        response = {
            "status": "ok",
            "tool": "glob",
            "visible": "# glob no-match-*.rs — 0 matches",
        }
        self.assertEqual(glob_root_and_first(response), ("", ""))
        for mutation in [
            {**response, "status": "error"},
            {**response, "tool": "read"},
            {**response, "visible": "# glob no-match-*.rs — unknown"},
        ]:
            with self.subTest(mutation=mutation):
                with self.assertRaisesRegex(ValueError, "root header is missing"):
                    glob_root_and_first(mutation)

    def test_glob_pick_cli_status_cannot_be_hidden_by_command_substitution(
        self,
    ) -> None:
        harness = Path(__file__).with_name("harness.py")
        malformed = json.dumps(
            {"status": "ok", "tool": "glob", "visible": '# root: "/work"\n"src"/'}
        )
        reject_script = r"""
set -euo pipefail
if ! GLOB_PICK=$(printf '%s' "$1" | python3 "$2" glob_pick /dev/stdin); then
  printf 'rejected\n'
  exit 0
fi
printf 'SURVIVED:%s\n' "$GLOB_PICK"
exit 99
"""
        rejected = subprocess.run(
            ["bash", "-c", reject_script, "glob-pick-test", malformed, str(harness)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(rejected.returncode, 0, rejected.stderr)
        self.assertEqual(rejected.stdout, "rejected\n")
        self.assertIn("glob_pick failed: malformed glob output", rejected.stderr)

        no_match = json.dumps(
            {
                "status": "ok",
                "tool": "glob",
                "visible": "# glob no-match-*.rs — 0 matches",
            }
        )
        fallback_script = r"""
set -euo pipefail
if ! GLOB_PICK=$(printf '%s' "$1" | python3 "$2" glob_pick /dev/stdin); then
  exit 90
fi
IFS=$'\t' read -r GLOB_ROOT GLOB_REL <<<"$GLOB_PICK"
[[ -z "$GLOB_ROOT" && -z "$GLOB_REL" ]] || exit 91
printf 'valid-no-match-fallback\n'
"""
        fallback = subprocess.run(
            ["bash", "-c", fallback_script, "glob-pick-test", no_match, str(harness)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(fallback.returncode, 0, fallback.stderr)
        self.assertEqual(fallback.stdout, "valid-no-match-fallback\n")


class VisiblePayloadAccountingTests(unittest.TestCase):
    def test_string_and_text_object_count_payload_not_envelope(self) -> None:
        envelope = json.dumps(
            {
                "status": "ok",
                "visible": "needle",
                "refs": [BLOB],
                "accounting": {"raw_tokens": 99},
            }
        )
        self.assertEqual(visible_payload_bytes(envelope), len(b"needle"))
        self.assertEqual(
            visible_payload_bytes({"visible": {"text": "µ"}}),
            len("µ".encode()),
        )

    def test_missing_visible_refuses_stdout_fallback(self) -> None:
        with self.assertRaisesRegex(VisiblePayloadError, "refusing"):
            visible_payload_bytes({"status": "ok", "refs": [BLOB]})
        with self.assertRaisesRegex(VisiblePayloadError, "JSON object"):
            visible_payload_bytes(["not", "an", "object"])
        with self.assertRaisesRegex(VisiblePayloadError, "empty visible"):
            visible_payload_bytes({"status": "ok", "visible": ""})
        with self.assertRaisesRegex(VisiblePayloadError, "not ok"):
            visible_payload_bytes({"status": "error", "visible": "x"})
        with self.assertRaisesRegex(VisiblePayloadError, "invalid JSON"):
            visible_payload_bytes("{")

    def test_expand_recovered_text_is_integrity_not_budget(self) -> None:
        self.assertEqual(
            expand_recovered_text({"visible": {"text": "BENCH_NEEDLE_FN"}}),
            "BENCH_NEEDLE_FN",
        )
        with self.assertRaisesRegex(VisiblePayloadError, "missing visible"):
            expand_recovered_text({"status": "ok"})


class KeepGateMeasurementHonestyTests(unittest.TestCase):
    def test_noisy_latency_is_not_keep_eligible(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "not eligible for keep"):
            refuse_noisy_keep("noisy", [0.1, 1.0, 0.05])
        with self.assertRaisesRegex(RuntimeError, "keep-gate needs"):
            refuse_noisy_keep("short", [0.1, 0.1])
        cv = refuse_noisy_keep("stable", [0.100, 0.101, 0.099])
        self.assertLessEqual(cv, 5.0)

    def test_bin_path_refuses_size_optimized_release(self) -> None:
        env = os.environ.copy()
        env.pop("TOKENZERO_BIN", None)
        harness = Path(__file__).with_name("harness.py")
        result = subprocess.run(
            [sys.executable, str(harness), "resolve_bin", "--profile", "release"],
            text=True,
            capture_output=True,
            check=False,
            env=env,
        )
        self.assertNotEqual(result.returncode, 0, result.stdout)
        combined = (result.stderr + result.stdout).lower()
        self.assertIn("release-perf", combined)
        self.assertIn("never --release", combined)
        isolated = os.environ.copy()
        isolated.pop("TOKENZERO_BIN", None)
        with mock.patch.dict(os.environ, isolated, clear=True):
            with self.assertRaises(SystemExit) as raised:
                bin_path(profile="debug")
        self.assertIn("release-perf", str(raised.exception))

    def test_teardown_is_outside_timed_window(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            marker = Path(tmp) / "teardown_ran"
            py = (
                "import time; time.sleep(0.15); "
                f"open({json.dumps(str(marker))}, 'w').write('1')"
            )
            teardown = "python3 -c " + json.dumps(py)
            with mock.patch("benchmarks.harness.shutil.which", return_value=None):
                times = _times("true", 3, 0, "true", "teardown-out", False, teardown)
            self.assertTrue(marker.exists(), "teardown must still run")
            self.assertEqual(len(times), 3)
            self.assertTrue(
                all(sample < 0.10 for sample in times),
                f"teardown sleep leaked into timed window: {times}",
            )

    def test_measure_with_teardown_refuses_noisy_keep(self) -> None:
        with mock.patch(
            "benchmarks.harness._times",
            return_value=[0.10, 1.00, 0.05],
        ):
            with self.assertRaisesRegex(RuntimeError, "not eligible for keep"):
                measure_with_teardown("noisy", "true", "true", runs=3, warmup=0)

    def test_hyperfine_passes_cleanup_outside_window(self) -> None:
        calls: list[list[str]] = []

        def fake_run(argv: list[str], **_: object) -> subprocess.CompletedProcess:
            calls.append(argv)
            if argv[0] == "bash":
                return subprocess.CompletedProcess(argv, 0, stdout=b"", stderr=b"")
            artifact = Path(argv[argv.index("--export-json") + 1])
            artifact.write_text('{"results":[{"times":[0.01, 0.01, 0.01]}]}')
            return subprocess.CompletedProcess(argv, 0, stdout="", stderr="")

        with (
            mock.patch(
                "benchmarks.harness.shutil.which", return_value="/fake/hyperfine"
            ),
            mock.patch("benchmarks.harness.subprocess.run", side_effect=fake_run),
        ):
            times = _times(
                "printf ok", 3, 0, "true", "hf-cleanup", False, "rm -f leftover"
            )
        self.assertEqual(times, [0.01, 0.01, 0.01])
        hyperfine = next(argv for argv in calls if argv and argv[0] == "/fake/hyperfine")
        self.assertIn("--prepare", hyperfine)
        self.assertIn("--cleanup", hyperfine)
        cleanup = hyperfine[hyperfine.index("--cleanup") + 1]
        self.assertIn("rm -f leftover", cleanup)


class MeasurementFailureTests(unittest.TestCase):
    def test_fallback_sample_failure_stdout_never_becomes_measurement(self) -> None:
        command = "printf 'BAD-MEASUREMENT'; printf 'SAMPLE-FAILURE' >&2; exit 7"
        with mock.patch("benchmarks.harness.shutil.which", return_value=None):
            with self.assertRaisesRegex(
                RuntimeError, r"fallback sample 1 failed with 7: SAMPLE-FAILURE"
            ):
                measure_median("failed-sample", command, runs=1, warmup=0)

    def test_fallback_warmup_failure_is_loud(self) -> None:
        command = "printf 'WARMUP-FAILURE' >&2; exit 8"
        with mock.patch("benchmarks.harness.shutil.which", return_value=None):
            with self.assertRaisesRegex(
                RuntimeError, r"fallback warmup 1 failed with 8: WARMUP-FAILURE"
            ):
                measure_median("failed-warmup", command, runs=1, warmup=1)

    def test_captured_byte_probe_failure_stdout_never_becomes_measurement(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            counter = Path(tmp) / "counter"
            counter_arg = shlex.quote(str(counter))
            command = (
                f"n=$(cat {counter_arg} 2>/dev/null || printf 0); n=$((n + 1)); "
                f"""printf '%s' "$n" > {counter_arg}; """
                """if [ "$n" -ge 2 ]; then """
                "printf 'BAD-CAPTURE'; printf 'CAPTURE-FAILURE' >&2; exit 9; fi; "
                "printf 'GOOD-SAMPLE'"
            )
            with mock.patch("benchmarks.harness.shutil.which", return_value=None):
                with self.assertRaisesRegex(
                    RuntimeError,
                    r"captured-byte probe failed with 9: CAPTURE-FAILURE",
                ):
                    measure_median("failed-capture", command, runs=1, warmup=0)
            self.assertEqual(counter.read_text(), "2")

    def test_present_hyperfine_failure_never_selects_fallback(self) -> None:
        calls: list[list[str]] = []

        def fake_run(argv: list[str], **_: object) -> subprocess.CompletedProcess:
            calls.append(argv)
            if argv[0] == "bash":
                return subprocess.CompletedProcess(argv, 0, stdout=b"", stderr=b"")
            self.assertEqual(argv[0], "/fake/hyperfine")
            return subprocess.CompletedProcess(
                argv,
                12,
                stdout="ignored",
                stderr="HYPERFINE-COMMAND-FAILURE",
            )

        with (
            mock.patch(
                "benchmarks.harness.shutil.which", return_value="/fake/hyperfine"
            ),
            mock.patch("benchmarks.harness.subprocess.run", side_effect=fake_run),
        ):
            with self.assertRaisesRegex(
                RuntimeError,
                r"hyperfine execution failed with 12: HYPERFINE-COMMAND-FAILURE",
            ):
                measure_median("failed-hyperfine", "printf bad", runs=1, warmup=0)
        self.assertEqual(len(calls), 2, "failed hyperfine must not enter fallback")

    def test_invalid_hyperfine_samples_never_select_fallback(self) -> None:
        calls: list[list[str]] = []

        def fake_run(argv: list[str], **_: object) -> subprocess.CompletedProcess:
            calls.append(argv)
            if argv[0] == "bash":
                return subprocess.CompletedProcess(argv, 0, stdout=b"", stderr=b"")
            artifact = Path(argv[argv.index("--export-json") + 1])
            artifact.write_text('{"results":[{"times":[]}]}')
            return subprocess.CompletedProcess(argv, 0, stdout="", stderr="")

        with (
            mock.patch(
                "benchmarks.harness.shutil.which", return_value="/fake/hyperfine"
            ),
            mock.patch("benchmarks.harness.subprocess.run", side_effect=fake_run),
        ):
            with self.assertRaisesRegex(
                RuntimeError, r"hyperfine timing artifact has invalid samples"
            ):
                measure_median("invalid-hyperfine", "printf bad", runs=1, warmup=0)
        self.assertEqual(len(calls), 2, "invalid artifact must not enter fallback")

    def test_missing_hyperfine_uses_checked_fallback(self) -> None:
        with mock.patch("benchmarks.harness.shutil.which", return_value=None):
            wall_ms, output_bytes, estimated_units = measure_median(
                "fallback-ok", "printf ok", runs=1, warmup=0
            )
        self.assertGreaterEqual(wall_ms, 0)
        self.assertEqual((output_bytes, estimated_units), (2, 1))

    def test_failure_stderr_is_real_and_bounded(self) -> None:
        command = """python3 -c 'import sys; sys.stderr.write("x" * 5000 + "-TAIL-SENTINEL"); sys.exit(6)'"""
        with mock.patch("benchmarks.harness.shutil.which", return_value=None):
            with self.assertRaises(RuntimeError) as raised:
                measure_median("bounded-stderr", command, runs=1, warmup=0)
        message = str(raised.exception)
        self.assertIn("[... stderr truncated ...]", message)
        self.assertIn("-TAIL-SENTINEL", message)
        self.assertLess(len(message), 4300)


class PortablePathTests(unittest.TestCase):
    def test_portable_paths_hide_host_components(self) -> None:
        crate = REPO / "crates" / "tokenzero-engine" / "src" / "lib.rs"
        self.assertEqual(
            portable_path(crate, REPO),
            "crates/tokenzero-engine/src/lib.rs",
        )
        home_secret = Path.home() / "secret-file"
        rendered_home = portable_path(home_secret, REPO)
        self.assertTrue(
            rendered_home.startswith("<home>"),
            rendered_home,
        )
        self.assertNotIn(str(Path.home()), rendered_home)
        tmp = Path("/tmp") / "tokenzero-portable-check" / "x"
        rendered_tmp = portable_path(tmp, REPO)
        self.assertTrue(rendered_tmp.startswith("<tmp>"), rendered_tmp)
        self.assertNotIn(str(Path.home()), rendered_tmp)

    def test_capture_environment_records_no_host_path(self) -> None:
        fake_bin = REPO / "target" / "release" / "tokenzero"
        captured = capture_environment(fake_bin, "python3 benchmarks/harness.py")
        home = str(Path.home())
        for key in ("cwd", "binary"):
            value = captured[key]
            self.assertNotIn(home, value, key)
            self.assertFalse(
                str(value).startswith("/Users/") or str(value).startswith("/home/"),
                f"{key} leaked a host path: {value}",
            )
        self.assertEqual(captured["cwd"], ".")
        self.assertEqual(captured["binary"], "target/release/tokenzero")


if __name__ == "__main__":
    unittest.main()
