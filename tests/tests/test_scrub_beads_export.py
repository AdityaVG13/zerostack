import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "scrub_beads_export.py"
SPEC = importlib.util.spec_from_file_location("zerostack_scrub_beads_export", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
SCRUB = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(SCRUB)


class BeadsScrubTests(unittest.TestCase):
    def test_relativize_removes_complete_home_prefixes(self) -> None:
        cases = {
            "C:" + "/Us" + "ers/u/x": "~/x",
            "C:" + "\\Us" + "ers\\u\\x": "~\\x",
            "/ho" + "me/u/x": "~/x",
            "/Us" + "ers/u/x": "~/x",
        }
        for original, expected in cases.items():
            with self.subTest(original=original):
                self.assertEqual(SCRUB.relativize(original), expected)


if __name__ == "__main__":
    unittest.main()
