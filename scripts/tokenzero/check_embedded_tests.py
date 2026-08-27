#!/usr/bin/env python3
"""Boundary fitness: no inline #[cfg(test)] mod body over the line ceiling.

Unit tests live in sibling-file modules (`#[cfg(test)] mod tests;` next to
src/<file>/tests.rs or src/tests.rs). A small inline mod (<= 50 lines) is
fine for a handful of focused cases; anything larger must move to a sibling
file so source files stay navigable. Exits non-zero on violations.
"""
from __future__ import annotations
import re; import sys; from pathlib import Path; MAX_INLINE_TEST_LINES = 50
ROOT = Path(__file__).resolve().parents[2]

def inline_test_mod_spans(text: str) -> list[tuple[int, int]]:
    """(start_line, body_line_count) for each inline `#[cfg(test)] mod x {`."""
    lines = text.splitlines(); spans = []
    for i, line in enumerate(lines):
        if line.strip() != '#[cfg(test)]':
            continue
        j = i + 1
        while j < len(lines) and lines[j].lstrip().startswith('#['):
            j += 1
        if j >= len(lines):
            continue
        m = re.match('\\s*(?:pub(?:\\([a-z]+\\))? )?mod\\s+\\w+\\s*\\{', lines[j])
        if not m:
            continue
        depth = 0; end = len(lines)
        for k in range(j, len(lines)):
            depth += lines[k].count('{') - lines[k].count('}')
            if depth <= 0:
                end = k
                break
        spans.append((i + 1, end - j))
    return spans

def main() -> int:
    violations = []
    for path in sorted(ROOT.glob('crates/*/src/**/*.rs')):
        for start, body_lines in inline_test_mod_spans(path.read_text()):
            if body_lines > MAX_INLINE_TEST_LINES:
                rel = path.relative_to(ROOT)
                violations.append(f'{rel}:{start}: inline #[cfg(test)] mod body is {body_lines} lines (max {MAX_INLINE_TEST_LINES}); move it to a sibling tests.rs module')
    for v in violations:
        print(v)
    if violations:
        print(f'FAIL: {len(violations)} oversized inline test mod(s)')
        return 1
    print(f'OK: no inline #[cfg(test)] mod body exceeds {MAX_INLINE_TEST_LINES} lines')
    return 0
if __name__ == '__main__':
    sys.exit(main())
