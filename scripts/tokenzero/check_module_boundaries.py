#!/usr/bin/env python3
"""Boundary fitness for the 2026-06 debloat decomposition.

Three rules keep the reorganized shape from drifting back:
1. Extracted module files must keep existing (no silent re-inlining).
2. Facade files must stay under a line ceiling (no mass re-accretion).
3. No crate may deep-import another crate's internal modules — consumers
   go through the crate-root pub use facades only.
Exits non-zero on violations.
"""
from __future__ import annotations
import re; import sys; from pathlib import Path; ROOT = Path(__file__).resolve().parents[2]
EXTRACTED_MODULES = ['crates/tokenzero-core/src/render/domain.rs', 'crates/tokenzero-core/src/render/noise.rs', 'crates/tokenzero-core/src/shell_display.rs', 'crates/tokenzero-core/src/shell_parse.rs', 'crates/tokenzero-core/src/tokens.rs', 'crates/tokenzero-mcp/src/collect.rs', 'crates/tokenzero-mcp/src/paths.rs', 'crates/tokenzero-mcp/src/render.rs', 'crates/tokenzero-install/src/content.rs', 'crates/tokenzero-install/src/doctor.rs', 'crates/tokenzero-install/src/inspect.rs', 'crates/tokenzero-install/src/windows_path.rs', 'crates/tokenzero-install/src/package_audit/paths.rs', 'crates/tokenzero-install/src/package_audit/tar.rs', 'crates/tokenzero-install/src/package_audit/zip.rs', 'crates/tokenzero/src/audits/bench.rs', 'crates/tokenzero/src/audits/os_reach.rs', 'crates/tokenzero/src/audits/recovery.rs', 'crates/tokenzero/src/audits/release.rs']; FACADE_CEILINGS = {'crates/tokenzero-core/src/lib.rs': 2500, 'crates/tokenzero-mcp/src/lib.rs': 2500, 'crates/tokenzero-install/src/lib.rs': 2500, 'crates/tokenzero-install/src/package_audit.rs': 2500, 'crates/tokenzero/src/main.rs': 2500}; INTERNAL_MODULES = {'tokenzero_core': ['shell_parse', 'tokens', 'render', 'shell_display'], 'tokenzero_mcp': ['collect', 'paths', 'render'], 'tokenzero_install': ['doctor', 'inspect', 'content', 'windows_path']}

def main() -> int:
    violations = []
    for rel in EXTRACTED_MODULES:
        if not (ROOT / rel).is_file():
            violations.append(f'{rel}: extracted module file is missing (was it re-inlined?)')
    for rel, ceiling in FACADE_CEILINGS.items():
        path = ROOT / rel
        if not path.is_file():
            violations.append(f'{rel}: facade file is missing')
            continue
        count = path.read_text().count('\n')
        if count > ceiling:
            violations.append(f'{rel}: {count} lines exceeds the {ceiling}-line facade ceiling; extract a module instead')
    deep = re.compile('|'.join((f"{crate}::(?:{'|'.join(mods)})::" for crate, mods in INTERNAL_MODULES.items())))
    for path in sorted(ROOT.glob('crates/*/src/**/*.rs')) + sorted(ROOT.glob('crates/*/benches/**/*.rs')) + sorted(ROOT.glob('tests/**/*.rs')):
        text = path.read_text()
        for i, line in enumerate(text.splitlines(), 1):
            m = deep.search(line)
            if m:
                violations.append(f'{path.relative_to(ROOT)}:{i}: deep import `{m.group(0)}...` bypasses the crate facade')
    for v in violations:
        print(v)
    if violations:
        print(f'FAIL: {len(violations)} boundary violation(s)')
        return 1
    print('OK: extracted modules present, facades under ceiling, no deep imports')
    return 0
if __name__ == '__main__':
    sys.exit(main())
