from __future__ import annotations

import re
import unittest
from pathlib import Path

REPO = Path(__file__).parents[2]  # ZeroStack/

# Active sample carriers that document the public zero.* CodeMode surface.
ACTIVE_SAMPLE_FILES = [
    REPO / "INSTALL-FOR-AGENTS.md",
    REPO / "README.md",
    *sorted((REPO / "docs").glob("*.md")),
]

# Canonical GraphZero orient registry, mirrored from the private engine
# `graphzero-query/src/query_surface/types.rs` (`QuerySurface::parse_surface`).
# "architecture" is deliberately NOT registered; an orient sample using it
# fails with QuerySurfaceError::UnknownSurface (zerostack-3xhk).
REGISTERED_ORIENT_SURFACES = frozenset(
    {
        "orient",
        "symbol",
        "callers",
        "deps",
        "outline",
        "context",
        "hot",
        "changes",
        "word",
        "search",
        "locate",
        "delta",
        "recall",
        "callpath",
        "reading_set",
        "reading-set",
        "readingset",
        "rg_l1", "rg-l1", "view_l1",
        "rg_l2", "rg-l2", "view_l2",
        "rg_l3", "rg-l3", "view_l3",
        "rg_l4", "rg-l4", "view_l4",
    }
)

_ORIENT_CALL = re.compile(r'zero\.graph\.orient\(\s*["\']([^"\']+)["\']')


def orient_surface_samples() -> list[tuple[str, int, str]]:
    hits: list[tuple[str, int, str]] = []
    for path in ACTIVE_SAMPLE_FILES:
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            match = _ORIENT_CALL.search(line)
            if match:
                hits.append((path.name, line_no, match.group(1)))
    return hits


def unregistered_surfaces(
    samples: list[tuple[str, int, str]],
) -> list[tuple[str, int, str]]:
    return [sample for sample in samples if sample[2] not in REGISTERED_ORIENT_SURFACES]


class GraphOrientSampleTests(unittest.TestCase):
    def test_active_samples_use_registered_orient_surfaces(self) -> None:
        drift = unregistered_surfaces(orient_surface_samples())
        self.assertEqual(
            drift,
            [],
            "active zero.graph.orient samples must use a surface registered "
            "in the GraphZero registry; unregistered surfaces (e.g. "
            "'architecture') fail with QuerySurfaceError::UnknownSurface "
            "(zerostack-3xhk)",
        )

    def test_guard_fires_on_architecture_drift(self) -> None:
        # Self-check that the gate actually catches the zerostack-3xhk drift.
        drifted = [("INSTALL-FOR-AGENTS.md", 308, "architecture")]
        self.assertEqual(
            [sample[2] for sample in unregistered_surfaces(drifted)],
            ["architecture"],
        )
        self.assertIn("context", REGISTERED_ORIENT_SURFACES)
        self.assertNotIn("architecture", REGISTERED_ORIENT_SURFACES)


if __name__ == "__main__":
    unittest.main()
