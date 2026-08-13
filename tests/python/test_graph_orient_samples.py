from __future__ import annotations

import re
import unittest
from pathlib import Path

REPO = Path(__file__).parents[2]  # ZeroStack/

# Active sample carriers that document the public zero.* CodeMode surface.
ACTIVE_SAMPLE_FILES = [
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

# Surfaces whose GraphZero implementation requires a query/name argument
# (QuerySurfaceError::MissingArgument when absent, per graphzero-query
# `surfaces.rs` context/symbol). orient() maps arg1=surface, arg2=query.
QUERY_REQUIRED_SURFACES = frozenset({"context", "symbol"})

# Captures `zero.graph.orient(<surface>[, <query>])`; query is optional.
_ORIENT_CALL = re.compile(
    r'zero\.graph\.orient\(\s*["\']([^"\']+)["\'](?:\s*,\s*["\']([^"\']*)["\'])?'
)


def orient_samples() -> list[tuple[str, int, str, str | None]]:
    hits: list[tuple[str, int, str, str | None]] = []
    for path in ACTIVE_SAMPLE_FILES:
        for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            match = _ORIENT_CALL.search(line)
            if match:
                hits.append((path.name, line_no, match.group(1), match.group(2)))
    return hits


def violations(
    samples: list[tuple[str, int, str, str | None]],
) -> list[tuple[str, int, str, str]]:
    """Return (path, line, surface, reason) for every drifting sample."""
    bad: list[tuple[str, int, str, str]] = []
    for path, line_no, surface, query in samples:
        if surface not in REGISTERED_ORIENT_SURFACES:
            bad.append((path, line_no, surface, "unregistered surface"))
        elif surface in QUERY_REQUIRED_SURFACES and not query:
            bad.append((path, line_no, surface, "missing required query"))
    return bad


class GraphOrientSampleTests(unittest.TestCase):
    def test_active_samples_use_registered_surface_and_required_query(self) -> None:
        drift = violations(orient_samples())
        self.assertEqual(
            drift,
            [],
            "active zero.graph.orient samples must use a surface registered "
            "in the GraphZero registry and supply the query that surface "
            "requires; unregistered surfaces (e.g. 'architecture') fail with "
            "QuerySurfaceError::UnknownSurface (zerostack-3xhk)",
        )

    def test_guard_fires_on_surface_and_query_drift(self) -> None:
        # Self-check that the gate catches the zerostack-3xhk drift class:
        # an unregistered surface must fail loudly...
        unregistered = [("INSTALL-FOR-AGENTS.md", 308, "architecture", "x")]
        self.assertEqual(
            [sample[3] for sample in violations(unregistered)],
            ["unregistered surface"],
        )
        # ...and a registered surface missing its required query must too.
        missing_query = [("INSTALL-FOR-AGENTS.md", 308, "context", None)]
        self.assertEqual(
            [sample[3] for sample in violations(missing_query)],
            ["missing required query"],
        )
        self.assertIn("context", REGISTERED_ORIENT_SURFACES)
        self.assertNotIn("architecture", REGISTERED_ORIENT_SURFACES)


if __name__ == "__main__":
    unittest.main()
