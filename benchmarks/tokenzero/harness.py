#!/usr/bin/env python3
"""Shared, stdlib-only benchmark measurement helpers."""
from __future__ import annotations
import math

import argparse, hashlib, json; import os, platform, random, re, shutil, string; import subprocess, sys, tempfile, time; from contextlib import contextmanager
from datetime import datetime, timezone; from pathlib import Path; GUARD = Path('/tmp/zerostack-heavy-process.guard'); RECOVERY_CACHE = Path.home() / '.tokenzero' / 'recovery-cache.json'
REPO = Path(__file__).resolve().parents[1]
try:
    from benchmarks.bench_common import portable_argv, portable_command, portable_path, portable_text, portable_tree
except ModuleNotFoundError:
    from bench_common import portable_argv, portable_command, portable_path, portable_text, portable_tree
try:
    from benchmarks.keep_gate import CV_PCT_QUARANTINE, cv_pct as keep_cv_pct
except ModuleNotFoundError:
    from keep_gate import CV_PCT_QUARANTINE, cv_pct as keep_cv_pct

KEEP_GATE_PROFILE = 'release-perf'
FORBIDDEN_CLAIM_PROFILES = frozenset({'release', 'debug', 'dev', 'test'})
MIN_KEEP_SAMPLES = 3


def _target_root() -> Path:
    env = os.environ.get('CARGO_TARGET_DIR')
    return Path(env) if env else REPO / 'target'


def bin_path(profile=KEEP_GATE_PROFILE, env_var='TOKENZERO_BIN', required=True):
    """Resolve tokenzero for published measurement.

    Keep-gate claims use `release-perf`, never size-optimized `--release` or
    `debug`. TOKENZERO_BIN is an explicit operator override.
    """
    env = os.environ.get(env_var)
    if env:
        path = Path(env).expanduser()
        if path.is_file() and os.access(path, os.X_OK):
            return path
        if required:
            raise SystemExit(f'{env_var}={path} is not an executable tokenzero binary')
    if required and profile in FORBIDDEN_CLAIM_PROFILES:
        raise SystemExit(
            f'profile {profile!r} is not a keep-gate measurement profile; '
            'use release-perf (never --release for published latency claims)'
        )
    candidates = [
        _target_root() / profile / 'tokenzero',
        Path.home() / '.tokenzero/bin/tokenzero',
    ]
    which = shutil.which('tokenzero')
    if which:
        candidates.append(Path(which))
    for path in candidates:
        if path.is_file() and os.access(path, os.X_OK):
            return path
    if required:
        raise SystemExit(
            f'tokenzero binary not found for keep-gate profile {profile!r}. '
            'Build with: cargo build --profile release-perf -p tokenzero-cli '
            f'--bin tokenzero --no-default-features or set {env_var}=/path/to/tokenzero'
        )
    return _target_root() / profile / 'tokenzero'


def refuse_noisy_keep(label: str, times: list[float]) -> float:
    """Return cv_pct. cv>5 or fewer than 3 samples is not a latency keep."""
    if len(times) < MIN_KEEP_SAMPLES:
        raise RuntimeError(
            f"benchmark {label} has {len(times)} sample(s); "
            f"keep-gate needs >= {MIN_KEEP_SAMPLES}"
        )
    cv = keep_cv_pct([float(value) for value in times])
    if cv > CV_PCT_QUARANTINE:
        raise RuntimeError(
            f"benchmark {label} cv_pct={cv:.4f} > {CV_PCT_QUARANTINE}; "
            "noise, not eligible for keep"
        )
    return cv

def now_ms():
    return int(time.time() * 1000)

def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b''):
            digest.update(chunk)
    return digest.hexdigest()

def git_commit(cwd=None, short=False):
    command = ['git', 'rev-parse', *(['--short'] if short else []), 'HEAD']
    try:
        return subprocess.run(command, cwd=cwd or REPO, capture_output=True, text=True, check=True).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return 'unknown'

def token_estimate(data):
    size = data if isinstance(data, int) else len(data if isinstance(data, bytes) else data.encode())
    return (size + 3) // 4

def percentiles_ms(times, qs=(0.5, 0.9, 0.99)):
    if not times:
        return [0] * len(qs)
    values = sorted(times)
    return [round(values[min(len(values) - 1, int(q * (len(values) - 1)))] * 1000) for q in qs]

def median_ms(times):
    values = sorted(times)
    return int(values[len(values) // 2] * 1000) if values else 0

def run_json(argv, cwd=None, check=True):
    started = time.perf_counter(); proc = subprocess.run(argv, cwd=cwd or REPO, capture_output=True, text=True, check=check)
    try:
        raw = json.loads(proc.stdout) if proc.stdout.strip() else {}
    except json.JSONDecodeError:
        raw = {}
    return {'argv': list(map(str, argv)), 'elapsed_ms': round((time.perf_counter() - started) * 1000, 3), 'stdout_bytes': len(proc.stdout.encode()), 'raw_json': raw, 'stderr': proc.stderr}

def capture_environment(binary, harness_command, extra=None):
    binary_name = portable_path(binary, REPO)
    exists = binary.is_file(); result = {'generated_at_utc': datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z'), 'harness_command': harness_command, 'cwd': portable_path(REPO, REPO), 'os': platform.platform(), 'machine': platform.machine(), 'python': platform.python_version(), 'commit': git_commit(), 'binary': binary_name, 'binary_sha256': sha256(binary) if exists else '', 'binary_mtime_ns': binary.stat().st_mtime_ns if exists else 0, 'cargo_build_jobs': os.environ.get('CARGO_BUILD_JOBS'), 'cargo_incremental': os.environ.get('CARGO_INCREMENTAL')}; result.update(extra or {})
    return result

def _clear_guard():
    for child in GUARD.iterdir():
        if child.is_file():
            child.unlink()
    try:
        GUARD.rmdir()
    except OSError:
        pass

@contextmanager
def heavy_guard(command, repo=None):
    deadline = time.monotonic() + 600
    while True:
        try:
            GUARD.mkdir()
            break
        except FileExistsError:
            try:
                pid = int((GUARD / 'pid').read_text().strip()); os.kill(pid, 0)
            except (FileNotFoundError, ValueError, ProcessLookupError):
                _clear_guard()
                continue
            except PermissionError as err:
                raise SystemExit(f'cannot inspect heavy-process guard owner: {err}') from err
            if time.monotonic() >= deadline:
                raise SystemExit(f'heavy-process guard still held by live pid {pid}')
            time.sleep(2)
    values = {'pid': os.getpid(), 'repository': repo or REPO, 'command': command, 'started_at': datetime.now(timezone.utc).isoformat().replace('+00:00', 'Z')}
    for name, value in values.items():
        (GUARD / name).write_text(f'{value}\n')
    try:
        yield
    finally:
        try:
            owned = (GUARD / 'pid').read_text().strip() == str(os.getpid())
        except FileNotFoundError:
            owned = False
        if owned:
            _clear_guard()

def synthetic_tree(root, count):
    for number in range(count):
        shard, index = divmod(number, 1000); directory = root / f'd{shard:03d}'; directory.mkdir(parents=True, exist_ok=True); (directory / f'f{index:04d}.txt').write_text(f'{shard:03d}:{index:04d}\n')

def million_line_repo(root, n_dirs, n_files, n_lines, needle, seed=42):
    random.seed(seed); chars = string.ascii_letters + string.digits
    for i in range(n_dirs):
        directory = root / f'mod_{i:04d}'; directory.mkdir(parents=True, exist_ok=True)
        for j in range(n_files):
            with (directory / f'file_{i:04d}_{j:03d}.rs').open('w') as stream:
                for k in range(n_lines):
                    body = f'pub fn {needle}(x: usize) -> bool {{ true }}' if k == 499 and (i * n_files + j) % 20 == 0 else ''.join(random.choices(chars, k=36)); stream.write(f'// line {k:04d} {body}\n')

def file_count(root, excludes=('.git', 'target', '.zerostack')):
    total = 0
    for _, dirs, files in os.walk(root):
        dirs[:] = [name for name in dirs if name not in excludes]; total += len(files)
    return total

_MAX_BENCHMARK_STDERR_CHARS = 4096


def _bounded_stderr(value: bytes | str | None) -> str:
    if isinstance(value, bytes):
        text = value.decode('utf-8', errors='replace')
    else:
        text = value or ''
    if len(text) <= _MAX_BENCHMARK_STDERR_CHARS:
        return text
    return '[... stderr truncated ...]\n' + text[-_MAX_BENCHMARK_STDERR_CHARS:]


def _bounded_stderr_file(path: Path) -> str:
    try:
        with path.open('rb') as stream:
            stream.seek(0, os.SEEK_END)
            size = stream.tell()
            stream.seek(max(0, size - _MAX_BENCHMARK_STDERR_CHARS * 4))
            return _bounded_stderr(stream.read())
    except OSError:
        return ''


def _run_prepare(command: str) -> None:
    prepared = subprocess.run(['bash', '-c', command], capture_output=True)
    if prepared.returncode != 0:
        raise RuntimeError(
            f"benchmark prepare failed with {prepared.returncode}: "
            f"{_bounded_stderr(prepared.stderr)}"
        )


def _run_benchmark_command(command: str, stage: str, capture_stdout: bool = False) -> bytes:
    completed = subprocess.run(
        ['bash', '-c', command],
        stdout=subprocess.PIPE if capture_stdout else subprocess.DEVNULL,
        stderr=subprocess.PIPE,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"benchmark {stage} failed with {completed.returncode}: "
            f"{_bounded_stderr(completed.stderr)}"
        )
    return completed.stdout if capture_stdout else b''


def _times(
    command: str,
    runs: int,
    warmup: int,
    prepare: str,
    name: str,
    cold_warmup: bool = False,
    teardown: str = 'true',
) -> list[float]:
    if runs < 1 or warmup < 0:
        raise ValueError('benchmark requires runs >= 1 and warmup >= 0')
    # Preflight exposes the preparation command's real stderr. Hyperfine then
    # repeats prepare outside every warmup/sample and cleanup after (untimed).
    _run_prepare(prepare)
    hyperfine = shutil.which('hyperfine')
    if hyperfine:
        with tempfile.TemporaryDirectory(prefix='tz-hf-') as tmp:
            artifact = Path(tmp) / 'hf.json'
            stderr_log = Path(tmp) / 'command.stderr'
            stderr_path = json.dumps(str(stderr_log))
            timed_prepare = f"{{ {prepare}; }} 2>>{stderr_path}"
            timed_teardown = f"{{ {teardown}; }} 2>>{stderr_path}"
            timed_command = f"{{ {command}; }} 2>>{stderr_path}"
            try:
                probe = subprocess.run(
                    [
                        hyperfine,
                        '--warmup',
                        str(warmup),
                        '--runs',
                        str(runs),
                        '--style',
                        'basic',
                        '--export-json',
                        str(artifact),
                        '--prepare',
                        timed_prepare,
                        '--cleanup',
                        timed_teardown,
                        '--command-name',
                        name,
                        timed_command,
                    ],
                    capture_output=True,
                    text=True,
                )
            except FileNotFoundError:
                # The executable disappeared after discovery. This is the one
                # present-hyperfine condition that may select the fallback.
                hyperfine = None
            except OSError as error:
                raise RuntimeError(f'could not execute hyperfine: {error}') from error
            else:
                if probe.returncode != 0:
                    diagnostic = _bounded_stderr_file(stderr_log) or _bounded_stderr(
                        probe.stderr
                    )
                    raise RuntimeError(
                        f"benchmark hyperfine execution failed with {probe.returncode}: "
                        f"{diagnostic}"
                    )
                try:
                    times = json.loads(artifact.read_text())['results'][0]['times']
                except (json.JSONDecodeError, OSError, IndexError, KeyError, TypeError) as error:
                    raise RuntimeError('hyperfine succeeded without a valid timing artifact') from error
                if (
                    not isinstance(times, list)
                    or len(times) != runs
                    or not all(
                        isinstance(value, (int, float))
                        and not isinstance(value, bool)
                        and math.isfinite(value)
                        and value >= 0
                        for value in times
                    )
                ):
                    raise RuntimeError('hyperfine timing artifact has invalid samples')
                return [float(value) for value in times]
    # Fallback is allowed only when hyperfine was absent or disappeared.
    # Teardown (and prepare) stay outside start.elapsed() — never inside the
    # timed window (keep-gate measure_with_teardown).
    fallback_warmups = 1 if cold_warmup else warmup
    for index in range(fallback_warmups):
        _run_prepare(prepare)
        _run_benchmark_command(command, f'fallback warmup {index + 1}')
        _run_benchmark_command(teardown, f'fallback warmup teardown {index + 1}')
    times = []
    for index in range(runs):
        _run_prepare(prepare)
        started = time.perf_counter()
        _run_benchmark_command(command, f'fallback sample {index + 1}')
        elapsed = time.perf_counter() - started
        times.append(elapsed)
        _run_benchmark_command(teardown, f'fallback teardown {index + 1}')
    return times


def measure_cell(label, command, cold=False, runs=50, warmup=3, teardown='true'):
    prepare = f'rm -f {RECOVERY_CACHE}' if cold else 'true'
    times = _times(command, runs, warmup, prepare, label, cold, teardown)
    refuse_noisy_keep(label, times)
    return tuple(percentiles_ms(times))

def measure_median(
    label: str,
    command: str,
    runs: int = 5,
    warmup: int = 1,
    prepare: str = 'true',
    teardown: str = 'true',
) -> tuple[int, int, int]:
    # Byte/token never-worse uses the untimed captured-byte probe. Wall cv is
    # not the never-worse denominator; latency keeps use measure_cell /
    # measure_with_teardown which fail closed on cv_pct > 5.
    times = _times(command, runs, warmup, prepare, label, False, teardown)
    wall = median_ms(times)
    _run_prepare(prepare)
    output = _run_benchmark_command(command, 'captured-byte probe', capture_stdout=True)
    _run_benchmark_command(teardown, 'captured-byte teardown')
    return (wall, len(output), token_estimate(output))


def measure_with_teardown(
    label: str,
    command: str,
    teardown: str,
    runs: int = 5,
    warmup: int = 1,
    prepare: str = 'true',
) -> tuple[int, float]:
    """Latency keep sample: teardown runs after start.elapsed() is captured."""
    times = _times(command, runs, warmup, prepare, label, False, teardown)
    cv = refuse_noisy_keep(label, times)
    return median_ms(times), cv

def _json(data):
    if not isinstance(data, str):
        return data
    try:
        return json.loads(data)
    except json.JSONDecodeError:
        return {}

def mcp_schema_tokens(cap_file, tools_csv):
    cap = json.loads(Path(cap_file).read_text()); by_name = cap.get('commands_by_name', {}); commands = cap.get('commands', []); selected = {}
    for name in filter(None, map(str.strip, tools_csv.split(','))):
        found = by_name.get(name) or next((item for item in commands if item.get('name') == name), None)
        if found:
            selected[name] = found
    return token_estimate(json.dumps(selected, separators=(',', ':'), ensure_ascii=False).encode())

def quality_check(task, payload):
    data = _json(payload)
    if isinstance(data, dict):
        value, visible = (data.get('value', {}), data.get('visible', {})); text = str(value['text'] if isinstance(value, dict) and 'text' in value else visible['text'] if isinstance(visible, dict) and 'text' in visible else json.dumps(data))
    else:
        text = str(data)
    low = text.lower()
    passed = {'read_file': '[workspace]' in text, 'search_filter': 'TokenZero' in text and text.count('\n') >= 1, 'edit_verify': 'BETA' in text and 'beta' not in text, 'multi_step_nav': 'workspace' in low, 'shell_expand': 'Cargo.toml' in text}.get(task, False)
    return 'PASS' if passed else 'FAIL'

def accounting_tokens(payload, key='raw_tokens'):
    data = _json(payload)
    if not isinstance(data, dict):
        return 0
    bags = (data.get('accounting', {}), data.get('telemetry', {}), data.get('value', {}))
    return next((int(bag[key]) for bag in bags if isinstance(bag, dict) and key in bag), 0)

_DURABLE_PRIMARY_REF = re.compile(
    r"^tz://(?:blob/[0-9a-f]{64}|o/[1-9][0-9]*/[1-9][0-9]*)$"
)


def _durable_primary_ref(value):
    return value if isinstance(value, str) and _DURABLE_PRIMARY_REF.fullmatch(value) else ''


class VisiblePayloadError(ValueError):
    """JSON envelope present but no visible payload -- refuse stdout fallback."""


def _parse_tool_json(data):
    if isinstance(data, str):
        try:
            data = json.loads(data)
        except json.JSONDecodeError as error:
            raise VisiblePayloadError(f"invalid JSON: {error}") from error
    if not isinstance(data, dict):
        raise VisiblePayloadError(
            "visible payload requires a JSON object; refusing envelope-inclusive fallback"
        )
    status = data.get("status")
    if status is not None and status != "ok":
        raise VisiblePayloadError(
            f"status {status!r} is not ok; refusing to count error payload as never-worse"
        )
    return data


def _visible_text(data) -> str:
    visible = data.get("visible")
    if isinstance(visible, str):
        return visible
    if isinstance(visible, dict) and isinstance(visible.get("text"), str):
        return visible["text"]
    raise VisiblePayloadError(
        "missing visible.text; refusing to count captured stdout as the never-worse denominator"
    )


def visible_payload_bytes(data) -> int:
    """Count visible payload bytes only. Never the JSON envelope.

    TokenZero CLI `--json` objects carry `visible` as a string or
    `{text: ...}`. Envelope keys (refs, accounting, status) are excluded so
    never-worse rows cannot be lost to required protocol JSON (tokenzero-4bhr).
    Empty visible is refused: non-dry-run edit clears text, and a 0-byte
    candidate would beat any raw baseline.
    """
    text = _visible_text(_parse_tool_json(data))
    if text == "":
        raise VisiblePayloadError(
            "empty visible payload; refusing zero-byte never-worse denominator"
        )
    return len(text.encode())


def expand_recovered_text(data) -> str:
    """Exact recovered text from an expand JSON object (integrity, not budget)."""
    text = _visible_text(_parse_tool_json(data))
    if text == "":
        raise VisiblePayloadError("expand response has empty visible text")
    return text


def first_blob_ref(data):
    data = _json(data)
    if not isinstance(data, dict):
        return ''
    refs = data.get('refs')
    if refs:
        if not isinstance(refs, list):
            return ''
        if all(isinstance(item, str) for item in refs):
            return _durable_primary_ref(refs[0])
        if all(isinstance(item, dict) for item in refs):
            found = next(
                (
                    _durable_primary_ref(item.get('ref'))
                    for item in refs
                    if item.get('kind') == 'blob'
                ),
                '',
            )
            return found
        return ''
    return _durable_primary_ref(data.get('detail_ref') or data.get('ref'))

def glob_root_and_first(data):
    data = _json(data)
    if not isinstance(data, dict):
        raise ValueError('malformed glob output: response is not an object')
    visible = data.get('visible')
    if isinstance(visible, str):
        text = visible
    elif isinstance(visible, dict) and isinstance(visible.get('text'), str):
        text = visible['text']
    else:
        raise ValueError('malformed glob output: visible text is missing')
    lines = text.splitlines()
    try:
        root_index = next(
            index for index, line in enumerate(lines) if line.startswith('# root: ')
        )
    except StopIteration:
        valid_no_match = (
            data.get('status') == 'ok'
            and data.get('tool') == 'glob'
            and len(lines) == 1
            and lines[0].startswith('# glob ')
            and lines[0].endswith(' — 0 matches')
        )
        if valid_no_match:
            return ('', '')
        raise ValueError('malformed glob output: root header is missing')
    encoded_root = lines[root_index].removeprefix('# root: ').strip()
    if not encoded_root:
        raise ValueError('malformed glob output: root label is empty')
    if not encoded_root.startswith('"'):
        for line in lines[root_index + 1 :]:
            if line.lstrip().startswith('#'):
                raise ValueError('malformed legacy glob output: file row is missing')
            if line.strip():
                relative = line.strip()
                if relative.endswith('/'):
                    raise ValueError('malformed legacy glob output: file row is incomplete')
                return (encoded_root, relative)
        raise ValueError('malformed legacy glob output: file row is missing')
    try:
        root = json.loads(encoded_root)
    except json.JSONDecodeError as error:
        raise ValueError('malformed glob output: root label is invalid JSON') from error
    if not isinstance(root, str) or not root:
        raise ValueError('malformed glob output: root label is not a string')
    directories = []
    for line in lines[root_index + 1 :]:
        if line.lstrip().startswith('#') or not line:
            raise ValueError('malformed glob output: trie ends before a file row')
        spaces = len(line) - len(line.lstrip(' '))
        if spaces % 2:
            raise ValueError('malformed glob output: trie indentation is odd')
        depth = spaces // 2
        encoded = line[spaces:]
        is_directory = encoded.endswith('/')
        if is_directory:
            encoded = encoded[:-1]
        if len(encoded) < 2 or not encoded.startswith('"') or not encoded.endswith('"'):
            raise ValueError('malformed glob output: component label is not a JSON string')
        try:
            component = json.loads(encoded)
        except json.JSONDecodeError as error:
            raise ValueError('malformed glob output: component label is invalid JSON') from error
        if not isinstance(component, str) or not component or '/' in component:
            raise ValueError('malformed glob output: component label is invalid')
        if depth != len(directories):
            raise ValueError('malformed glob output: trie depth skips a parent')
        if is_directory:
            directories.append(component)
            continue
        return (root, '/'.join([*directories, component]))
    raise ValueError('malformed glob output: trie ends before a file row')


def main(argv=None):
    parser = argparse.ArgumentParser(prog='harness.py'); sub = parser.add_subparsers(dest='action', required=True)

    def add(name, *args):
        command = sub.add_parser(name)
        for flags, options in args:
            command.add_argument(*flags, **options)
    add('resolve_bin', (('--profile',), {'default': KEEP_GATE_PROFILE})); add('now_ms'); add('tok', (('--bytes',), {'type': int})); percentile = sub.add_parser('percentiles')
    group = percentile.add_mutually_exclusive_group(required=True); group.add_argument('--json'); group.add_argument('--times'); add('measure_cell', (('label',), {}), (('cmd',), {}), (('--cold',), {'action': 'store_true'}), (('--runs',), {'type': int, 'default': 50}), (('--warmup',), {'type': int, 'default': 3}), (('--teardown',), {'default': 'true'}))
    add('measure_median', (('label',), {}), (('cmd',), {}), (('--runs',), {'type': int, 'default': 5}), (('--warmup',), {'type': int, 'default': 1}), (('--prepare',), {'default': 'true'}), (('--teardown',), {'default': 'true'})); add('measure_with_teardown', (('label',), {}), (('cmd',), {}), (('teardown',), {}), (('--runs',), {'type': int, 'default': 5}), (('--warmup',), {'type': int, 'default': 1}), (('--prepare',), {'default': 'true'})); add('mcp_schema_tokens', (('cap_file',), {}), (('tools',), {})); add('quality', (('task',), {})); add('clear_cache')
    add('git_commit', (('--short',), {'action': 'store_true'})); add('accounting', (('--file',), {}), (('--key',), {'default': 'raw_tokens'})); add('first_blob_ref', (('file',), {})); add('visible_payload_bytes', (('file',), {})); add('expand_recovered_text', (('file',), {})); add('glob_pick', (('file',), {}))
    add('generate_million', (('root',), {}), (('--dirs',), {'type': int, 'default': 100}), (('--files',), {'type': int, 'default': 10}), (('--lines',), {'type': int, 'default': 1000}), (('--needle',), {'default': 'BENCH_NEEDLE_FN'})); add('tz_metrics', (('file',), {}), (('wall',), {})); args = parser.parse_args(argv); action = args.action
    if action == 'resolve_bin':
        result = bin_path(args.profile)
    elif action == 'now_ms':
        result = now_ms()
    elif action == 'tok':
        result = token_estimate(args.bytes if args.bytes is not None else sys.stdin.buffer.read())
    elif action == 'percentiles':
        values = json.loads(Path(args.json).read_text()).get('results', [{}])[0].get('times', []) if args.json else [float(x) for x in Path(args.times).read_text().splitlines() if x.strip()]; result = '\t'.join(map(str, percentiles_ms(values)))
    elif action == 'measure_cell':
        result = '\t'.join(map(str, measure_cell(args.label, args.cmd, args.cold, args.runs, args.warmup, args.teardown)))
    elif action == 'measure_median':
        result = '\t'.join(map(str, measure_median(args.label, args.cmd, args.runs, args.warmup, args.prepare, args.teardown)))
    elif action == 'measure_with_teardown':
        wall, cv = measure_with_teardown(args.label, args.cmd, args.teardown, args.runs, args.warmup, args.prepare)
        result = f'{wall}\t{cv}'
    elif action == 'mcp_schema_tokens':
        result = mcp_schema_tokens(args.cap_file, args.tools)
    elif action == 'quality':
        result = quality_check(args.task, sys.stdin.read())
    elif action == 'clear_cache':
        RECOVERY_CACHE.unlink(missing_ok=True)
        return 0
    elif action == 'git_commit':
        result = git_commit(short=args.short)
    elif action == 'accounting':
        result = accounting_tokens(Path(args.file).read_text() if args.file else sys.stdin.read(), args.key)
    elif action == 'first_blob_ref':
        result = first_blob_ref(Path(args.file).read_text())
    elif action == 'visible_payload_bytes':
        try:
            result = visible_payload_bytes(Path(args.file).read_text())
        except VisiblePayloadError as error:
            print(f'visible_payload_bytes failed: {error}', file=sys.stderr)
            return 2
    elif action == 'expand_recovered_text':
        try:
            result = expand_recovered_text(Path(args.file).read_text())
        except VisiblePayloadError as error:
            print(f'expand_recovered_text failed: {error}', file=sys.stderr)
            return 2
    elif action == 'glob_pick':
        try:
            result = '\t'.join(glob_root_and_first(Path(args.file).read_text()))
        except ValueError as error:
            print(f'glob_pick failed: {error}', file=sys.stderr)
            return 2
    elif action == 'generate_million':
        million_line_repo(Path(args.root), args.dirs, args.files, args.lines, args.needle); result = 'done'
    else:
        try:
            accounting = json.loads(Path(args.file).read_text()).get('accounting', {}); result = f"{accounting.get('visible_tokens', 0)}\t{accounting.get('raw_tokens', 0)}\t{args.wall}"
        except (json.JSONDecodeError, OSError):
            result = f'0\t0\t{args.wall}'
    print(result)
    return 0
if __name__ == '__main__':
    raise SystemExit(main())
