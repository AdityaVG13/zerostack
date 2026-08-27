#!/usr/bin/env python3
"""Shared primitives for repository benchmark scripts."""
from __future__ import annotations
import os
import re
import statistics
import time
from pathlib import Path
from typing import Iterable

TMP_PLACEHOLDER = '<tmp>'
HOME_PLACEHOLDER = '<home>'
_ABSOLUTE = re.compile(r'^(?:/|[A-Za-z]:[\\/])')

def _distinct(paths: Iterable[Path]) -> list[Path]:
    seen: set[str] = set()
    result: list[Path] = []
    for path in paths:
        key = str(path)
        if key not in seen:
            seen.add(key)
            result.append(path)
    return result

def _temp_roots() -> list[Path]:
    import tempfile
    roots = [Path(tempfile.gettempdir()), Path('/tmp'), Path('/private/tmp'), Path('/var/folders'), Path('/private/var/folders')]
    return _distinct(root for candidate in roots for root in (candidate, Path(os.path.realpath(candidate))))

def _placed(candidates: list[Path], bases: Iterable[Path], render) -> str | None:
    for base in bases:
        for candidate in candidates:
            try:
                relative = candidate.relative_to(base)
            except ValueError:
                continue
            return render(relative.as_posix())
    return None

def portable_path(value: object, repo: Path) -> str:
    """Render a filesystem path without host-identifying components."""
    text = str(value)
    if not _ABSOLUTE.match(text):
        return text
    candidates = _distinct([Path(text), Path(os.path.realpath(text))])
    repos = _distinct([Path(repo), Path(os.path.realpath(repo))])
    inside_repo = _placed(candidates, repos, lambda relative: relative or '.')
    if inside_repo is not None:
        return inside_repo
    inside_tmp = _placed(candidates, _temp_roots(), lambda relative: f'{TMP_PLACEHOLDER}/{relative}' if relative not in ('', '.') else TMP_PLACEHOLDER)
    if inside_tmp is not None:
        return inside_tmp
    inside_home = _placed(candidates, _distinct([Path.home(), Path(os.path.realpath(Path.home()))]), lambda relative: f'{HOME_PLACEHOLDER}/{relative}' if relative not in ('', '.') else HOME_PLACEHOLDER)
    return inside_home if inside_home is not None else text

def portable_argv(argv: Iterable[object], repo: Path) -> list[str]:
    return [portable_path(value, repo) for value in argv]

def portable_command(argv: Iterable[object], repo: Path) -> str:
    return ' '.join(portable_argv(argv, repo))

def _prefix_rules(repo: Path) -> list[tuple[str, str]]:
    rules = [(str(base), '.') for base in _distinct([Path(repo), Path(os.path.realpath(repo))])]
    rules += [(str(base), TMP_PLACEHOLDER) for base in _temp_roots()]
    rules += [(str(base), HOME_PLACEHOLDER) for base in _distinct([Path.home(), Path(os.path.realpath(Path.home()))])]
    return sorted(rules, key=lambda rule: -len(rule[0]))

def portable_text(value: str, repo: Path) -> str:
    if _ABSOLUTE.match(value):
        rendered = portable_path(value, repo)
        if rendered != value:
            return rendered
    result = value
    for prefix, placeholder in _prefix_rules(repo):
        result = re.sub(re.escape(prefix) + '(?=[/\\s\'\"]|$)', placeholder, result)
    return result

def portable_tree(value: object, repo: Path) -> object:
    if isinstance(value, str):
        return portable_text(value, repo)
    if isinstance(value, dict):
        return {key: portable_tree(item, repo) for key, item in value.items()}
    if isinstance(value, list):
        return [portable_tree(item, repo) for item in value]
    return value

def process_is_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    return True

def _clean_guard(guard: Path) -> None:
    for child in guard.iterdir():
        if child.is_file():
            child.unlink()
    guard.rmdir()

def acquire_guard(guard: Path, repo: Path, command: str, *, wait_seconds: float | None=None, wait_step: float=0.25) -> None:
    started = time.monotonic()
    while True:
        try:
            guard.mkdir()
        except FileExistsError:
            try:
                pid = int((guard / 'pid').read_text().strip())
            except (FileNotFoundError, ValueError):
                _clean_guard(guard)
                continue
            if process_is_alive(pid):
                if wait_seconds is None or time.monotonic() - started >= wait_seconds:
                    raise SystemExit(f'heavy-process guard held by live pid {pid}')
                time.sleep(wait_step)
                continue
            _clean_guard(guard)
            continue
        break
    (guard / 'pid').write_text(f'{os.getpid()}\n')
    (guard / 'repository').write_text(f'{repo}\n')
    (guard / 'command').write_text(f'{command}\n')
    (guard / 'started_at').write_text(time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()) + '\n')

def release_guard(guard: Path) -> None:
    if not guard.exists() or not (guard / 'pid').exists():
        return
    if (guard / 'pid').read_text().strip() == str(os.getpid()):
        _clean_guard(guard)

def percentile(values: list[float], q: float, *, empty_zero: bool=False) -> float:
    if not values and empty_zero:
        return 0.0
    ordered = sorted(values)
    index = (len(ordered) - 1) * q
    lo = int(index)
    hi = min(lo + 1, len(ordered) - 1)
    return ordered[lo] + (ordered[hi] - ordered[lo]) * (index - lo)

def summary(values: list[float], *, include_p99: bool=False, empty_zero: bool=False) -> dict[str, float | int]:
    if not values and empty_zero:
        return {'n': 0, 'p50_ms': 0.0, 'p95_ms': 0.0, 'mean_ms': 0.0}
    result: dict[str, float | int] = {'n': len(values), 'p50_ms': round(percentile(values, 0.5), 6), 'p95_ms': round(percentile(values, 0.95), 6)}
    if include_p99:
        result['p99_ms'] = round(percentile(values, 0.99), 6)
    result['mean_ms'] = round(statistics.fmean(values), 6)
    return result

def find_ref(value: object, *, pattern: str='(?:tz|fz)://\\S+', strip_punctuation: bool=False) -> str | None:
    if isinstance(value, str):
        match = re.search(pattern, value)
        if match:
            return match.group(0).rstrip('.,;') if strip_punctuation else match.group(0)
    items: Iterable[object]
    if isinstance(value, dict):
        items = value.values()
    elif isinstance(value, list):
        items = value
    else:
        return None
    for item in items:
        found = find_ref(item, pattern=pattern, strip_punctuation=strip_punctuation)
        if found:
            return found
    return None

def deterministic_text(size: int, seed: str) -> str:
    line = f'{seed}:0123456789abcdef: TokenZero deterministic performance corpus\n'
    return (line * (size // len(line) + 1))[:size]

def utc_timestamp() -> str:
    return time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())

def sha256_bytes(data: bytes) -> str:
    import hashlib
    return hashlib.sha256(data).hexdigest()

def pct_saved(baseline: int, candidate: int) -> float:
    return round(100 * (baseline - candidate) / baseline, 3)

def environment(repo: Path, binary: Path | None=None) -> dict[str, object]:
    import platform
    import subprocess
    diff = subprocess.run(['git', 'diff', '--binary'], cwd=repo, capture_output=True, check=True).stdout
    commit = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=repo, capture_output=True, text=True, check=True).stdout.strip()
    result: dict[str, object] = {'commit': commit, 'machine': platform.machine(), 'os': platform.platform(), 'python': platform.python_version(), 'source_diff_sha256': sha256_bytes(diff)}
    if binary is not None:
        result.update({'binary': str(binary.relative_to(repo)), 'binary_sha256': sha256_bytes(binary.read_bytes())})
    return result

def write_json(path: Path, value: object, *, emit: bool=False) -> None:
    import json
    rendered = json.dumps(value, indent=2, sort_keys=True)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(rendered + '\n')
    if emit:
        print(rendered)
