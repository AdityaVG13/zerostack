#!/usr/bin/env python3
"""Validate the libtest registered-test budget from ``--list --format terse``."""
import argparse
import sys

CAP = 50

def parse(lines):
    names = []
    for raw in lines:
        line = raw.rstrip("\n")
        if not line:
            continue
        if not line.endswith(": test"):
            raise ValueError(f"malformed enumeration line: {line!r}")
        name = line[:-6]
        if not name or name in names:
            raise ValueError(f"duplicate or empty test name: {name!r}")
        names.append(name)
    return names

def check(lines):
    names = parse(lines)
    if len(names) > CAP:
        raise ValueError(f"registered test cap exceeded: {len(names)} > {CAP}")
    return len(names)


def check_file(path):
    with open(path, encoding="utf-8") as stream:
        return check(stream)

def self_test():
    passing = [f"pass_{i}: test\n" for i in range(50)]
    failing = passing + ["fail_50: test\n"]
    assert check(passing) == 50
    try:
        check(failing)
    except ValueError:
        return
    raise AssertionError("51 registered tests must fail")

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("path", nargs="?", help="libtest enumeration file; default stdin")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        print("budget self-test passed: 50 accepted, 51 rejected")
        return
    try:
        with open(args.path, encoding="utf-8") if args.path else sys.stdin as stream:
            count = check(stream)
    except (OSError, ValueError) as exc:
        parser.error(str(exc))
    print(f"registered test budget: {count}/{CAP}")

if __name__ == "__main__":
    main()
