#!/usr/bin/env python3
"""Advisory file locks for the four shared ZeroStack working trees.

Several agent sessions edit the SAME checkout of ZeroStack, TokenZero, FSZero
and GraphZero concurrently. zerostack-xyk recorded the damage: duplicated work,
a `git stash push -u` that swept up a peer's untracked files, and a `git rebase`
that silently autostashed a peer's dirty tree. Beads claim WORK ITEMS, but two
agents can hold different beads that touch the same file, so bead claiming alone
never prevented the collisions.

This locks FILES, which is the missing half.

The lock is ADVISORY on purpose. It cannot stop a write, and pretending
otherwise would be worse than useless: the trees hold tens of thousands of files
and no agent will route every edit through a wrapper. What it does is make an
intention VISIBLE before the edit, so a peer can see "AzureOrchid is in
tokens.rs" and pick something else. Nothing here modifies tracked files.

State lives in ONE json file at the hub, so all four repos share a namespace and
a lock can name a path in any repo:

    ~/AI/ZeroStack/.agent-locks.json    (gitignored, never committed)

Locks are keyed by repo-relative "repo/path". They carry an owner, a reason, and
a timestamp, and they EXPIRE. A crashed session must not wedge a file forever,
so a lock older than --ttl (default 2h) is reported stale and is breakable
without --force.

Usage:
    agent_lock.py claim  TokenZero/crates/.../tokens.rs --who AzureOrchid --why "spz part 1"
    agent_lock.py list
    agent_lock.py check  TokenZero/crates/.../tokens.rs --who AzureOrchid
    agent_lock.py release TokenZero/crates/.../tokens.rs --who AzureOrchid

Exit codes: 0 ok / lock is yours or free, 1 held by someone else, 2 usage error.
`check` is the one to call before editing; it is silent on success so it can sit
in a script.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import json
import os
import sys
import time
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Iterator

try:
    import fcntl
except ImportError:  # pragma: no cover - supported platforms provide it
    fcntl = None  # type: ignore[assignment]

LOCK_FILE = Path(__file__).resolve().parent.parent / ".agent-locks.json"
SIDECAR_LOCK_FILE = LOCK_FILE.with_suffix(LOCK_FILE.suffix + ".lock")
DEFAULT_TTL_SECONDS = 2 * 60 * 60


@contextmanager
def state_lock(*, exclusive: bool) -> Iterator[None]:
    """Hold the stable sidecar lock for one complete state operation."""
    if fcntl is None:
        raise RuntimeError(
            "agent_lock.py requires POSIX file locking (fcntl) on macOS or Linux"
        )
    try:
        with SIDECAR_LOCK_FILE.open("a+", encoding="utf-8") as lock_handle:
            operation = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
            fcntl.flock(lock_handle.fileno(), operation)
            try:
                yield
            finally:
                fcntl.flock(lock_handle.fileno(), fcntl.LOCK_UN)
    except OSError as exc:
        raise RuntimeError(f"cannot lock {SIDECAR_LOCK_FILE}: {exc}") from exc


def load() -> dict:
    if not LOCK_FILE.is_file():
        return {}
    try:
        data = json.loads(LOCK_FILE.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        # A corrupt lock file must not block work. Locks are advisory and
        # expiring, so discarding them is recoverable; refusing to run is not.
        print(f"warning: {LOCK_FILE} is corrupt, ignoring existing locks", file=sys.stderr)
        return {}
    return data if isinstance(data, dict) else {}


def save(locks: dict) -> None:
    # Write-then-rename so readers outside this tool also see atomic snapshots.
    # Callers hold the exclusive sidecar lock across load/check/save.
    tmp = LOCK_FILE.with_suffix(".json.tmp")
    tmp.write_text(json.dumps(locks, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp, LOCK_FILE)


def normalize(path: str) -> str:
    """Return a collision-free repository namespace key."""
    normalized_separators = path.replace("\\", "/")
    windows_path = PureWindowsPath(path)
    if PurePosixPath(normalized_separators).is_absolute() or windows_path.is_absolute() or windows_path.drive:
        raise ValueError(f"path must be repository-relative: {path!r}")

    components = normalized_separators.split("/")
    if any(component == ".." for component in components):
        raise ValueError(f"parent traversal is not allowed: {path!r}")
    components = [component for component in components if component not in ("", ".")]
    if not components:
        raise ValueError("path must name a repository-relative file")
    return "/".join(components)


def age_str(seconds: float) -> str:
    if seconds < 90:
        return f"{int(seconds)}s"
    if seconds < 5400:
        return f"{int(seconds // 60)}m"
    return f"{seconds / 3600:.1f}h"


def is_stale(entry: dict, ttl: int) -> bool:
    return (time.time() - entry.get("ts", 0)) > ttl


def cmd_claim(args) -> int:
    key = normalize(args.path)
    with state_lock(exclusive=True):
        locks = load()
        held = locks.get(key)

        if held and held.get("who") != args.who and not is_stale(held, args.ttl) and not args.force:
            age = age_str(time.time() - held.get("ts", 0))
            print(
                f"LOCKED by {held.get('who')} ({age} ago): {key}\n"
                f"  reason: {held.get('why') or '(none)'}\n"
                f"  Coordinate over Agent Mail, pick different work, or --force if you know it is abandoned.",
                file=sys.stderr,
            )
            return 1

        if held and held.get("who") != args.who:
            why = "stale" if is_stale(held, args.ttl) else "forced"
            print(f"note: taking over {why} lock from {held.get('who')}", file=sys.stderr)

        locks[key] = {"who": args.who, "why": args.why, "ts": time.time()}
        save(locks)
    print(f"claimed {key} for {args.who}")
    return 0


def cmd_release(args) -> int:
    target = None if args.all_mine else normalize(args.path)
    released = []
    with state_lock(exclusive=True):
        locks = load()
        for key in list(locks):
            if args.all_mine:
                if locks[key].get("who") == args.who:
                    del locks[key]
                    released.append(key)
            elif key == target:
                if locks[key].get("who") != args.who and not args.force:
                    print(f"refusing: {key} is held by {locks[key].get('who')}, not {args.who}", file=sys.stderr)
                    return 1
                del locks[key]
                released.append(key)
        save(locks)
    print(f"released {len(released)} lock(s)" + (": " + ", ".join(released) if released else ""))
    return 0


def cmd_check(args) -> int:
    key = normalize(args.path)
    with state_lock(exclusive=False):
        held = load().get(key)
    if not held or held.get("who") == args.who or is_stale(held, args.ttl):
        return 0
    age = age_str(time.time() - held.get("ts", 0))
    print(f"LOCKED by {held.get('who')} ({age} ago): {key} — {held.get('why') or '(no reason)'}", file=sys.stderr)
    return 1


def cmd_list(args) -> int:
    with state_lock(exclusive=False):
        locks = load()
    if not locks:
        print("no active locks")
        return 0
    now = time.time()
    for key, entry in sorted(locks.items(), key=lambda kv: kv[1].get("ts", 0)):
        stale = " [STALE, breakable]" if is_stale(entry, args.ttl) else ""
        print(f"{entry.get('who','?'):<14} {age_str(now - entry.get('ts', 0)):>6} ago  {key}{stale}")
        if entry.get("why"):
            print(f"{'':<14} {'':>6}       {entry['why']}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ttl", type=int, default=DEFAULT_TTL_SECONDS, help="seconds before a lock is stale")
    sub = parser.add_subparsers(dest="cmd", required=True)

    who_default = os.environ.get("AGENT_NAME")

    p = sub.add_parser("claim", help="announce you are editing a file")
    p.add_argument("path")
    p.add_argument("--who", default=who_default, required=who_default is None)
    p.add_argument("--why", default="")
    p.add_argument("--force", action="store_true")
    p.set_defaults(func=cmd_claim)

    p = sub.add_parser("release", help="give a file back")
    p.add_argument("path", nargs="?", default="")
    p.add_argument("--who", default=who_default, required=who_default is None)
    p.add_argument("--all-mine", action="store_true", help="release every lock you hold")
    p.add_argument("--force", action="store_true")
    p.set_defaults(func=cmd_release)

    p = sub.add_parser("check", help="silent if free or yours, exit 1 if held")
    p.add_argument("path")
    p.add_argument("--who", default=who_default, required=who_default is None)
    p.set_defaults(func=cmd_check)

    p = sub.add_parser("list", help="show all active locks")
    p.set_defaults(func=cmd_list)

    args = parser.parse_args()
    if getattr(args, "all_mine", False) is False and args.cmd == "release" and not args.path:
        parser.error("release needs a path or --all-mine")
    try:
        return args.func(args)
    except (RuntimeError, ValueError) as exc:
        parser.error(str(exc))


if __name__ == "__main__":
    raise SystemExit(main())
