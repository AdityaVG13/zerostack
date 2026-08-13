#!/usr/bin/env python3
"""Atomically install the repository's tracked zs wrapper."""
from __future__ import annotations
import argparse
import os
from pathlib import Path
import shutil
import stat
import tempfile

def verify(source: Path, destination: Path) -> bool:
    return destination.is_file() and os.access(destination, os.X_OK) and source.read_bytes() == destination.read_bytes()

def install(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".zs.", dir=destination.parent)
    temp = Path(temporary)
    try:
        with os.fdopen(fd, "wb") as output, source.open("rb") as input_file:
            shutil.copyfileobj(input_file, output); output.flush(); os.fsync(output.fileno())
        temp.chmod(stat.S_IMODE(source.stat().st_mode) | stat.S_IXUSR)
        os.replace(temp, destination)
        directory_fd = os.open(destination.parent, os.O_RDONLY)
        try: os.fsync(directory_fd)
        finally: os.close(directory_fd)
    finally: temp.unlink(missing_ok=True)

def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", type=Path, default=Path.home() / ".local/bin", help="installation directory (default: ~/.local/bin)")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args(); source, destination = Path(__file__).with_name("zs"), args.prefix.expanduser() / "zs"
    if args.dry_run:
        print(f"would install zs to {destination}"); return 0
    if args.verify:
        if verify(source, destination): print(f"verified {destination}"); return 0
        print(f"verification failed: {destination}"); return 1
    install(source, destination)
    if not verify(source, destination): print(f"verification failed after install: {destination}"); return 1
    print(f"installed {destination}"); return 0
if __name__ == "__main__": raise SystemExit(main())
