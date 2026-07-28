#!/usr/bin/env python3
"""Versioned fail-closed wrapper for RCH remote execution."""
from __future__ import annotations
import os, re, shlex, shutil
from pathlib import Path
import subprocess as sp
import sys
from typing import Mapping, Sequence, TextIO

WRAPPER_VERSION="1"
REMOTE=re.compile(r"\[RCH\]\s+remote(?:\s|$)",re.I)
LOCAL=re.compile(r"\[RCH\]\s+local(?:\s|$)",re.I)
BUSY=re.compile(r"\[RCH\].*(?:queued|busy|no admissible workers|worker unavailable)",re.I)
ROOT=re.compile(r'^\s*canonical_root\s*=\s*"([^"]+)"\s*$')
LIMIT=1024*1024

def strict_env(source: Mapping[str,str]) -> dict[str,str]:
    env=dict(source)
    env.update(RCH_FORCE_REMOTE="true",RCH_REQUIRE_REMOTE="true",RCH_QUEUE_WHEN_BUSY="true")
    if env.get("RCH_VISIBILITY","").lower() != "verbose": env["RCH_VISIBILITY"]="summary"
    return env

def binary(env: Mapping[str,str]) -> str:
    value=env.get("RCH_REMOTE_BIN") or shutil.which("rch")
    if not value: raise RuntimeError("rch executable not found on PATH")
    return value

def canonical_root(rch: str, env: Mapping[str,str]) -> Path:
    result=sp.run([rch,"config","show"],env=dict(env),stdin=sp.DEVNULL,capture_output=True,text=True,check=False)
    if result.returncode:
        raise RuntimeError(f"rch config show failed ({result.returncode}): {result.stderr.strip() or result.stdout.strip()}")
    for line in result.stdout.splitlines():
        match=ROOT.match(line)
        if match: return Path(match.group(1)).expanduser().resolve()
    raise RuntimeError("rch config show did not report path_topology.canonical_root")

def stream(child: sp.Popen[bytes], output: TextIO) -> str:
    assert child.stdout is not None
    kept=bytearray(); shown=0; suppressed=False
    while chunk:=child.stdout.read(8192):
        kept.extend(chunk)
        if len(kept)>262144: del kept[:-262144]
        visible=chunk[:max(0,LIMIT-shown)]
        if visible: output.write(visible.decode(errors="replace")); output.flush(); shown+=len(visible)
        suppressed |= len(visible)<len(chunk)
    if suppressed: output.write("\nrch_remote: output limit reached; remaining output suppressed\n")
    return kept.decode(errors="replace")

def classify(text: str, code: int) -> tuple[str,int]:
    if LOCAL.search(text): return "forbidden_local_fallback",70
    if BUSY.search(text): return "queued_or_busy",code or 75
    if code: return "remote_failure",code
    if not REMOTE.search(text): return "forbidden_local_fallback",70
    return "remote_success",0

def run(argv: Sequence[str]) -> int:
    env=strict_env(os.environ)
    try: rch=binary(env); root=canonical_root(rch,env)
    except (OSError,RuntimeError) as error:
        print(f"rch_remote: classification=configuration_error: {error}",file=sys.stderr); return 78
    cwd=Path.cwd().resolve()
    try: cwd.relative_to(root)
    except ValueError:
        destination=root/"rch-worktrees"/cwd.name
        remedy=f"git -C {shlex.quote(str(cwd))} worktree add {shlex.quote(str(destination))} HEAD"
        print(f"rch_remote: configuration_error: cwd is outside RCH canonical root; resolved cwd={cwd}; resolved root={root}\nrch_remote: create a within-root worktree: {remedy}",file=sys.stderr)
        print("rch_remote: classification=configuration_error",file=sys.stderr); return 78
    try:
        child=sp.Popen([rch,"exec","--",*argv],env=env,stdout=sp.PIPE,stderr=sp.STDOUT,shell=False)
        text=stream(child,sys.stdout); code=child.wait()
    except OSError as error:
        print(f"rch_remote: classification=configuration_error: {error}",file=sys.stderr); return 78
    kind,code=classify(text,code); print(f"rch_remote: classification={kind}",file=sys.stderr); return code

def main(argv: Sequence[str]|None=None) -> int:
    args=list(sys.argv[1:] if argv is None else argv)
    if args==["--wrapper-version"]: print(WRAPPER_VERSION); return 0
    if args and args[0]=="--": args=args[1:]
    if not args: print("usage: rch_remote.py -- COMMAND [ARG ...]",file=sys.stderr); return 78
    return run(args)
if __name__=="__main__": raise SystemExit(main())
