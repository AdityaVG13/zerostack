#!/usr/bin/env python3
"""Cargo test runner that caps aggregate rustc CPU usage on macOS.

Modes:
  1. User mode:  `scripts/cargo_test_guard.py [cargo-test-args...]` runs
     `cargo test` with `CARGO_BUILD_JOBS=1` and `RUSTC_WRAPPER` set to this
     script.
  2. Wrapper mode: Cargo invokes this script as `$RUSTC_WRAPPER <rustc-path>
     <rustc-args...>`. The wrapper spawns a gate subprocess that SIGSTOPs
     itself and then execs rustc, so rustc only executes during controlled
     windows.
  3. Gate mode: internal `--guard-exec <rustc-path> <rustc-args...>` used by
     the wrapper. The gate stops itself, waits for the parent to SIGCONT, and
     then replaces itself with rustc via `os.execv`.

Aggregate CPU semantics: macOS `ps -o %cpu=` reports percent of one logical
CPU (100 % == one fully busy core). The configured limit is an aggregate cap
across all rustc processes. With `CARGO_BUILD_JOBS=1` only one rustc process
is normally active, so its measured %cpu directly approximates the aggregate.
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from pathlib import Path


# Default aggregate CPU limit across all rustc processes.
DEFAULT_CPU_LIMIT_PERCENT: float = 80.0

# Fixed one-second control cycle. ps %cpu is a lagging average, so the
# controller's correctness does not rely on sampling every transient spike.
# The hard safety property comes from the absolute per-logical-core duty cap
# below; the one-second cycle just gives the process useful run/stop windows.
CONTROL_CYCLE: float = 1.0

# Shortest sleep interval to avoid busy-waiting.
TICK: float = 0.01

# Controller aims well below the hard limit so measurement lag cannot breach it.
# 0.60 * 80% = 48% control target for the default limit.
CONTROL_FRACTION: float = 0.60

# Maximum per-cycle increase of the duty ratio. The absolute cap is the
# primary safety bound; this growth limit only tempers convergence.
MAX_RATIO_GROWTH: float = 1.50

# Internal gate marker.
GUARD_EXEC_FLAG: str = "--guard-exec"


def _me() -> Path:
    return Path(__file__).resolve()


def _threshold() -> float:
    """Parse and validate RUSTC_CPU_LIMIT_PERCENT, defaulting to 80."""
    raw = os.environ.get("RUSTC_CPU_LIMIT_PERCENT")
    if raw is None:
        return DEFAULT_CPU_LIMIT_PERCENT
    try:
        value = float(raw)
    except ValueError as exc:
        raise RuntimeError(
            f"RUSTC_CPU_LIMIT_PERCENT must be a number (got {raw!r})"
        ) from exc
    if not (1.0 <= value <= 100.0):
        raise RuntimeError(
            f"RUSTC_CPU_LIMIT_PERCENT must be between 1 and 100 (got {value})"
        )
    return value


def _logical_cpu_count() -> int:
    count = os.cpu_count()
    if count is None or count <= 0:
        return 1
    return count


def _is_darwin() -> bool:
    return sys.platform == "darwin"


def _sample_cpu(pid: int) -> float | None:
    """Return %cpu for PID, or None if the process has exited."""
    try:
        proc = subprocess.run(
            ["/bin/ps", "-o", "%cpu=", "-p", str(pid)],
            capture_output=True,
            text=True,
            check=False,
            timeout=5.0,
        )
    except (subprocess.TimeoutExpired, FileNotFoundError, PermissionError):
        return None

    if proc.returncode != 0:
        return None

    text = proc.stdout.strip()
    if not text:
        return None
    try:
        return float(text.splitlines()[0].strip())
    except ValueError:
        return None


def _stop(pid: int) -> None:
    try:
        os.kill(pid, signal.SIGSTOP)
    except ProcessLookupError:
        pass


def _cont(pid: int) -> None:
    try:
        os.kill(pid, signal.SIGCONT)
    except ProcessLookupError:
        pass


def _kill_pgrp(pgid: int, sig: int) -> None:
    try:
        os.killpg(pgid, sig)
    except (ProcessLookupError, PermissionError):
        pass


def _wait_state(pid: int, timeout: float) -> tuple[int | None, int]:
    """Wait up to timeout for PID; return (status, pid) or (None, -1)."""
    deadline = time.monotonic() + timeout
    while True:
        try:
            waited, status = os.waitpid(pid, os.WNOHANG)
            if waited != 0:
                return status, waited
        except ChildProcessError:
            return None, -1
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None, -1
        time.sleep(min(TICK, remaining))


def _sleep_or_exit(pid: int, duration: float) -> tuple[int | None, bool]:
    """Sleep up to duration, returning child status if it exits early."""
    deadline = time.monotonic() + duration
    while True:
        try:
            waited, status = os.waitpid(pid, os.WNOHANG)
            if waited != 0:
                return status, True
        except ChildProcessError:
            return None, True
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return None, False
        time.sleep(min(TICK, remaining))


class _WrapperContext:
    """Holds wrapper-mode state and provides signal-safe cleanup."""

    def __init__(self, pgid: int, pid: int) -> None:
        self.pgid = pgid
        self.pid = pid
        self.stopped = False
        self.exit_status: int | None = None

    def stop(self) -> None:
        _stop(self.pid)
        self.stopped = True

    def cont(self) -> None:
        _cont(self.pid)
        self.stopped = False

    def shutdown(self, sig: int) -> None:
        self.cont()
        _kill_pgrp(self.pgid, sig)


def _run_gate(rustc_path: str, rustc_args: list[str]) -> int:
    """Stop self and exec rustc so the parent controls every execution window.

    On success `os.execv` does not return; on failure we return nonzero so the
    parent sees the gate failed before rustc started.
    """
    try:
        os.kill(os.getpid(), signal.SIGSTOP)
    except OSError as exc:
        print(f"cargo_test_guard: gate failed to stop itself: {exc}", file=sys.stderr)
        return 1

    # After the parent SIGCONTs us, replace this process with rustc.
    try:
        os.execv(rustc_path, [rustc_path, *rustc_args])
    except OSError as exc:
        print(f"cargo_test_guard: failed to exec rustc: {exc}", file=sys.stderr)
        return 1

    return 1


def _wait_for_gate_stop(pid: int) -> int | None:
    """Block until the gate child has stopped itself, returning its exit status if it exited early."""
    while True:
        try:
            waited, status = os.waitpid(pid, os.WUNTRACED)
        except ChildProcessError:
            print("cargo_test_guard: gate process vanished before stopping", file=sys.stderr)
            return 1
        if os.WIFSTOPPED(status):
            return None
        if os.WIFEXITED(status):
            return os.WEXITSTATUS(status)
        if os.WIFSIGNALED(status):
            return 1


def _run_wrapper(rustc_path: Path, rustc_args: list[str], threshold: float) -> int:
    """Throttle rustc on Darwin; fail closed on other platforms."""
    if not _is_darwin():
        print(
            "cargo_test_guard: rustc wrapper mode is only supported on macOS",
            file=sys.stderr,
        )
        return 1

    cpu_count = _logical_cpu_count()
    control_target = threshold * CONTROL_FRACTION

    # Absolute worst-case duty cap: if rustc could saturate every logical core,
    # this ratio keeps aggregate CPU at or below control_target. With 18 cores
    # and a 48% target, this is 2.67%. The hard 80% kill remains the final
    # fail-safe; this cap is the mathematical guarantee.
    absolute_ratio_cap = control_target / (cpu_count * 100.0)

    # Start at half the absolute cap, clamped to at least 1% so we still make
    # progress. The empirical controller below can only tighten this bound.
    run_ratio = max(0.01, min(absolute_ratio_cap, absolute_ratio_cap * 0.5))

    # Empirical estimate of full-duty CPU capacity (cpu / run_ratio). Tracked as
    # the maximum observed capacity so far; a higher observed capacity only
    # lowers the safe-ratio bound and makes the controller more conservative.
    max_cpu_per_ratio: float = 0.0

    # Do not pass inherited jobserver descriptors to the new gate/rustc session;
    # CARGO_BUILD_JOBS=1 already serializes the build.
    env = os.environ.copy()
    env.pop("CARGO_MAKEFLAGS", None)

    try:
        child = subprocess.Popen(
            [sys.executable, str(_me()), GUARD_EXEC_FLAG, str(rustc_path), *rustc_args],
            stdin=subprocess.DEVNULL,
            stdout=None,
            stderr=None,
            start_new_session=True,
            env=env,
        )
    except OSError as exc:
        print(f"cargo_test_guard: failed to start gate: {exc}", file=sys.stderr)
        return 1

    pid = child.pid
    pgid = os.getpgid(pid)
    ctx = _WrapperContext(pgid, pid)

    def _on_signal(signum: int, _frame: object) -> None:
        ctx.shutdown(signum)

    signal.signal(signal.SIGINT, _on_signal)
    signal.signal(signal.SIGTERM, _on_signal)

    # Wait until the gate has stopped itself; only then can we guarantee rustc
    # runs during controlled windows.
    early_exit = _wait_for_gate_stop(pid)
    if early_exit is not None:
        return early_exit
    ctx.stopped = True

    while True:
        # Run phase.
        ctx.cont()
        run_duration = CONTROL_CYCLE * run_ratio
        status, exited = _sleep_or_exit(pid, run_duration)
        if exited:
            ctx.exit_status = status
            break

        # Stop phase.
        ctx.stop()
        stop_duration = CONTROL_CYCLE - run_duration
        status, exited = _sleep_or_exit(pid, stop_duration)
        if exited:
            # Child exited while stopped: continue before reaping so the kernel
            # reports its final state correctly.
            ctx.cont()
            ctx.exit_status = status
            break

        # Sample while stopped; ps %cpu is a lagging average over a recent window.
        cpu = _sample_cpu(pid)
        if cpu is None:
            ctx.cont()
            status, waited = _wait_state(pid, 2.0)
            if waited != -1:
                ctx.exit_status = status
            break

        # Hard kill remains at the configured limit.
        if cpu > threshold:
            ctx.cont()
            print(
                f"cargo_test_guard: rustc CPU {cpu:.1f}% exceeded "
                f"{threshold:.1f}% aggregate limit; aborting build",
                file=sys.stderr,
            )
            ctx.shutdown(signal.SIGTERM)
            time.sleep(0.1)
            _kill_pgrp(pgid, signal.SIGKILL)
            try:
                os.waitpid(pid, 0)
            except ChildProcessError:
                pass
            return 1

        # The absolute per-core cap is the primary safety bound. The empirical
        # capacity estimate is an additional limiter that can only tighten the
        # bound if observed full-duty capacity is higher than expected.
        if cpu > 0:
            capacity = cpu / run_ratio
            if capacity > max_cpu_per_ratio:
                max_cpu_per_ratio = capacity
            safe_ratio = control_target / max_cpu_per_ratio
            run_ratio = max(0.01, min(absolute_ratio_cap, run_ratio * MAX_RATIO_GROWTH, safe_ratio))

    # Continue before reaping so a stopped child does not stay suspended.
    ctx.cont()

    if ctx.exit_status is None:
        print("cargo_test_guard: lost track of rustc process", file=sys.stderr)
        return 1

    status = ctx.exit_status
    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    if os.WIFSIGNALED(status):
        return 1
    return 1


def _run_user(cargo_args: list[str]) -> int:
    """Run `cargo test` with rustc wrapper and a single build job."""
    env = os.environ.copy()
    env["CARGO_BUILD_JOBS"] = "1"
    env["RUSTC_WRAPPER"] = str(_me())

    # Defensive recursion guard: refuse to re-enter if we are already acting
    # as the wrapper for a nested cargo invocation.
    if env.get("RUSTC_WRAPPER_IS_GUARD") == "1":
        print("cargo_test_guard: refusing wrapper recursion", file=sys.stderr)
        return 1
    env["RUSTC_WRAPPER_IS_GUARD"] = "1"

    try:
        child = subprocess.Popen(
            ["cargo", "test", *cargo_args],
            stdin=sys.stdin,
            stdout=sys.stdout,
            stderr=sys.stderr,
            env=env,
        )
    except FileNotFoundError:
        print("cargo_test_guard: cargo not found in PATH", file=sys.stderr)
        return 1

    def _forward_signal(signum: int, _frame: object) -> None:
        try:
            child.send_signal(signum)
        except ProcessLookupError:
            pass

    signal.signal(signal.SIGTERM, _forward_signal)
    # KeyboardInterrupt handles SIGINT once wait() returns.

    try:
        return child.wait()
    except KeyboardInterrupt:
        child.terminate()
        try:
            return child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            return child.wait()


def _detect_gate_mode(argv: list[str]) -> bool:
    """Return True when invoked as the internal pre-exec gate."""
    return len(argv) >= 3 and argv[1] == GUARD_EXEC_FLAG


def _detect_wrapper_mode(argv: list[str]) -> bool:
    """Return True when Cargo invoked us as RUSTC_WRAPPER."""
    wrapper = os.environ.get("RUSTC_WRAPPER")
    if wrapper is None:
        return False
    return Path(wrapper).resolve() == _me() and len(argv) >= 2


def main(argv: list[str] | None = None) -> int:
    if argv is None:
        argv = sys.argv

    if _detect_gate_mode(argv):
        return _run_gate(argv[2], argv[3:])

    try:
        threshold = _threshold()
    except RuntimeError as exc:
        print(f"cargo_test_guard: {exc}", file=sys.stderr)
        return 1

    if _detect_wrapper_mode(argv):
        rustc_path = Path(argv[1])
        rustc_args = argv[2:]
        return _run_wrapper(rustc_path, rustc_args, threshold)

    return _run_user(argv[1:])


if __name__ == "__main__":
    sys.exit(main())
