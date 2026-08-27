#!/usr/bin/env python3
"""Audit README / docs shell claims without running documented commands."""
from __future__ import annotations

import argparse
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

AUDIT_SKIP = "<!-- audit:skip -->"
FENCE_RE = re.compile(r"^\s*```([A-Za-z0-9_-]+)?\s*$")
INLINE_RE = re.compile(r"`([^`\n]+)`")
KNOWN_BINARIES = {"tokenzero", "fszero", "graphzero", "zerostack", "node", "python3", "cargo", "git", "rustup"}
PROJECT_BINARIES = {"tokenzero", "fszero", "graphzero"}
IGNORED_COMMANDS = {"cd", "export", "printf", "echo", "cat", "mkdir", "cp", "mv", "rm", "true", "false"}
CONTROL_WORDS = {"if", "then", "else", "fi", "for", "do", "done", "while"}
SAFE_EXEC_ARGS = {"--help", "-h", "help", "--version", "-V", "version"}
CARGO_WRAPPER_HELP_SUBCOMMAND = "check"

@dataclass(frozen=True)
class Candidate:
    text: str
    line: int
    source: str

@dataclass(frozen=True)
class Finding:
    line: int
    message: str
    command: str

class Audit:
    def __init__(self, root: Path, readme: Path):
        self.root = root
        self.readme = readme
        self.help_cache: dict[tuple[str, ...], tuple[int, str]] = {}
        self.findings: list[Finding] = []
        self.skips: list[Candidate] = []

    def repo_binary(self, name: str) -> str | None:
        if name in PROJECT_BINARIES:
            release = self.root / "target" / "release" / name
            debug = self.root / "target" / "debug" / name
            if release.exists() and os.access(release, os.X_OK):
                return str(release)
            if debug.exists() and os.access(debug, os.X_OK):
                return str(debug)
        return shutil.which(name)

    def help_text(self, argv: list[str]) -> tuple[int, str]:
        key = tuple(argv)
        if key in self.help_cache:
            return self.help_cache[key]
        env = os.environ.copy()
        env.setdefault("NO_COLOR", "1")
        try:
            proc = subprocess.run(
                argv,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=15,
                env=env,
            )
            result = (proc.returncode, proc.stdout)
        except Exception as exc:
            result = (127, str(exc))
        self.help_cache[key] = result
        return result

    def extract(self) -> list[Candidate]:
        lines = self.readme.read_text(encoding="utf-8").splitlines()
        candidates: list[Candidate] = []
        active_lang: str | None = None
        for idx, line in enumerate(lines, start=1):
            fence = FENCE_RE.match(line)
            if fence:
                lang = (fence.group(1) or "").lower()
                if active_lang is not None:
                    active_lang = None
                elif lang in {"bash", "sh", "console"}:
                    active_lang = lang
                continue
            if active_lang in {"bash", "sh", "console"}:
                if idx > 1 and AUDIT_SKIP in lines[idx - 2]:
                    self.skips.append(Candidate(line.strip(), idx, "fence"))
                    continue
                for command in commands_from_shell_line(line):
                    candidates.append(Candidate(command, idx, "fence"))
            for match in INLINE_RE.finditer(line):
                text = match.group(1).strip()
                first = first_command_word(text)
                if first in KNOWN_BINARIES:
                    if idx > 1 and AUDIT_SKIP in lines[idx - 2]:
                        self.skips.append(Candidate(text, idx, "inline"))
                    else:
                        candidates.append(Candidate(text, idx, "inline"))
        return candidates

    def run(self) -> int:
        for candidate in self.extract():
            self.audit_candidate(candidate)
        for skip in self.skips:
            print(f"SKIP {self.readme}:{skip.line}: {skip.command if hasattr(skip, 'command') else skip.text}")
        for finding in self.findings:
            print(f"{self.readme}:{finding.line}: {finding.message}: {finding.command}")
        if self.findings:
            print(f"FAIL: {len(self.findings)} README command audit finding(s), {len(self.skips)} skip(s)")
            return 1
        print(f"OK: README command audit passed ({len(self.extract())} command(s), {len(self.skips)} skip(s))")
        return 0

    def audit_candidate(self, candidate: Candidate) -> None:
        try:
            argv = shlex.split(strip_prompt(candidate.text), posix=True)
        except ValueError as exc:
            self.findings.append(Finding(candidate.line, f"cannot parse shell command ({exc})", candidate.text))
            return
        argv = normalize_argv(argv)
        if not argv:
            return
        binary_name = Path(argv[0]).name
        if argv[0].startswith("./"):
            # Repo-relative script: verify it exists and is executable, do not run it.
            script = self.root / argv[0][2:]
            if script.is_file() and os.access(script, os.X_OK):
                return
            self.findings.append(Finding(candidate.line, f"script not found or not executable: {argv[0]}", candidate.text))
            return
        if binary_name not in KNOWN_BINARIES:
            if binary_name not in IGNORED_COMMANDS and binary_name not in CONTROL_WORDS:
                self.findings.append(Finding(candidate.line, f"unknown binary '{binary_name}'", candidate.text))
            return
        resolved = self.repo_binary(binary_name)
        if not resolved:
            self.findings.append(Finding(candidate.line, f"cannot resolve binary '{binary_name}'", candidate.text))
            return
        if binary_name == "zerostack":
            self.findings.append(Finding(candidate.line, "unknown binary 'zerostack'", candidate.text))
            return
        self.verify_args(candidate, resolved, binary_name, argv[1:])

    def verify_args(self, candidate: Candidate, resolved: str, binary_name: str, args: list[str]) -> None:
        top_code, top_help = self.help_text([resolved, "--help"])
        if top_code != 0 and not top_help.strip():
            self.findings.append(Finding(candidate.line, f"{binary_name} --help failed", candidate.text))
            return
        if binary_name == "cargo":
            self.verify_cargo(candidate, resolved, args, top_help)
            return
        subcommand = first_subcommand(binary_name, args)
        help_text = top_help
        if subcommand:
            code, text = self.help_text([resolved, subcommand, "--help"])
            if code != 0 and subcommand not in top_help:
                self.findings.append(Finding(candidate.line, f"unknown subcommand '{subcommand}'", candidate.text))
                return
            help_text = text + "\n" + top_help
        for flag in flags_in(args):
            probe = flag.split("=", 1)[0]
            if probe in {"--", "-"}:
                continue
            if probe not in help_text:
                self.findings.append(Finding(candidate.line, f"unknown flag '{probe}'", candidate.text))

    def verify_cargo(self, candidate: Candidate, resolved: str, args: list[str], top_help: str) -> None:
        delimiter = args.index("--") if "--" in args else len(args)
        cargo_args = args[:delimiter]
        passthrough = args[delimiter + 1:] if delimiter < len(args) else []
        subcommand = first_subcommand("cargo", cargo_args)
        help_text = top_help
        if subcommand:
            code, text = self.help_text([resolved, subcommand, "--help"])
            if code != 0 and subcommand not in top_help:
                self.findings.append(Finding(candidate.line, f"unknown subcommand '{subcommand}'", candidate.text))
                return
            help_text = text + "\n" + top_help
        wrapper_text = ""
        code, text = self.help_text([resolved, CARGO_WRAPPER_HELP_SUBCOMMAND, "--help"])
        if code == 0:
            wrapper_text = text
        for flag in flags_in(cargo_args):
            probe = flag.split("=", 1)[0]
            if probe not in help_text and probe not in wrapper_text:
                self.findings.append(Finding(candidate.line, f"unknown flag '{probe}'", candidate.text))
        if subcommand == "run" and passthrough:
            bin_name = cargo_run_bin(cargo_args) or project_binary_for_root(self.root)
            if bin_name:
                resolved_bin = self.repo_binary(bin_name)
                if not resolved_bin:
                    self.findings.append(Finding(candidate.line, f"cannot resolve binary '{bin_name}'", candidate.text))
                    return
                self.verify_args(candidate, resolved_bin, bin_name, passthrough)



def cargo_run_bin(args: list[str]) -> str | None:
    for idx, arg in enumerate(args):
        if arg == "--bin" and idx + 1 < len(args):
            return args[idx + 1]
        if arg.startswith("--bin="):
            return arg.split("=", 1)[1]
    return None


def project_binary_for_root(root: Path) -> str | None:
    name = root.name.lower()
    if name == "tokenzero":
        return "tokenzero"
    if name == "fszero":
        return "fszero"
    if name == "graphzero":
        return "graphzero"
    return None


def strip_prompt(text: str) -> str:
    text = text.strip()
    for prompt in ("$ ", "> ", "% "):
        if text.startswith(prompt):
            return text[len(prompt):].strip()
    return text


def normalize_argv(argv: list[str]) -> list[str]:
    out = list(argv)
    while out and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=.*$", out[0]):
        out.pop(0)
    if out and out[0] in {"env", "command"}:
        out.pop(0)
        while out and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=.*$", out[0]):
            out.pop(0)
    return out


def first_command_word(text: str) -> str | None:
    commands = commands_from_shell_line(text)
    if not commands:
        return None
    try:
        argv = normalize_argv(shlex.split(strip_prompt(commands[0]), posix=True))
    except ValueError:
        return None
    return Path(argv[0]).name if argv else None


def commands_from_shell_line(line: str) -> list[str]:
    raw = strip_prompt(line)
    if not raw or raw.startswith("#"):
        return []
    raw = raw.replace("\\\n", " ")
    pieces = re.split(r"\s*(?:&&|\|\||\|)\s*", raw)
    commands: list[str] = []
    for piece in pieces:
        piece = piece.strip()
        if not piece or piece.startswith("#"):
            continue
        if piece.endswith("\\"):
            piece = piece[:-1].strip()
        try:
            argv = normalize_argv(shlex.split(piece, posix=True))
        except ValueError:
            commands.append(piece)
            continue
        if not argv:
            continue
        cmd = Path(argv[0]).name
        if cmd in IGNORED_COMMANDS or cmd in CONTROL_WORDS:
            continue
        commands.append(piece)
    return commands


def first_subcommand(binary_name: str, args: list[str]) -> str | None:
    if binary_name == "node":
        return None
    idx = 0
    while idx < len(args):
        arg = args[idx]
        if arg == "--":
            return None
        if arg.startswith("-"):
            if "=" not in arg and idx + 1 < len(args) and not args[idx + 1].startswith("-") and option_takes_value(arg):
                idx += 2
            else:
                idx += 1
            continue
        if re.match(r"^[A-Za-z0-9_-]+$", arg):
            return arg
        return None
    return None


def option_takes_value(flag: str) -> bool:
    return flag in {"--manifest-path", "--package", "-p", "--bin", "--target", "--features", "--repo", "--root", "--config", "--toolchain", "--output", "--output-json", "--output-md", "--dist", "--intent", "--target-dir", "--message", "-m", "--name", "--query", "--path", "--surface", "--claim", "--budget", "--mode", "--params", "--protocolVersion"}


def flags_in(args: list[str]) -> list[str]:
    flags: list[str] = []
    for arg in args:
        if arg.startswith("-") and arg not in {"-", "--"}:
            flags.append(arg)
    return flags


def banned_mcp_server_launches(doc: Path) -> list[Finding]:
    """Fail if docs still recommend `fszero mcp-server` as a launch verb.

    Negation prose ("there is no … mcp-server") is allowed so the ban can
    stay documented. Affirmative launch lines are findings (fszero-rotation-i1-gqgt.16).
    """
    findings: list[Finding] = []
    if not doc.is_file():
        return findings
    for idx, line in enumerate(doc.read_text(encoding="utf-8").splitlines(), start=1):
        if "mcp-server" not in line:
            continue
        # Strip markdown emphasis/code so **no** / `fszero` still match
        stripped = re.sub(r"[*`_\[\]]", " ", line.lower())
        stripped = re.sub(r"\s+", " ", stripped)
        # Negation / historical ban documentation
        if re.search(r"\b(no|never|not|without|unsupported|do not|don't)\b.{0,60}mcp-server", stripped):
            continue
        if re.search(r"\bfszero\s+mcp-server\b", stripped):
            findings.append(
                Finding(
                    idx,
                    "banned launch form `fszero mcp-server` (use --mode=mcp / serve / fszero-mcp)",
                    line.strip(),
                )
            )
    return findings

def self_check() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        readme = root / "README.md"
        readme.write_text("""# Scratch\n\n```bash\nzerostack serve\n```\n\n<!-- audit:skip -->\n`zerostack serve`\n""", encoding="utf-8")
        audit = Audit(root, readme)
        code = audit.run()
        if code == 0:
            print("FAIL: self-check did not reject fake zerostack serve")
            return 1
        if not audit.skips:
            print("FAIL: self-check did not record skip marker")
            return 1
        bad = root / "docs" / "mcp.md"
        bad.parent.mkdir(parents=True, exist_ok=True)
        bad.write_text("stdio — classic MCP: fszero mcp-server\n", encoding="utf-8")
        banned = banned_mcp_server_launches(bad)
        if not banned:
            print("FAIL: self-check did not reject affirmative fszero mcp-server")
            return 1
        good = root / "docs" / "mcp-ok.md"
        good.write_text("There is **no** `fszero mcp-server` verb.\n", encoding="utf-8")
        if banned_mcp_server_launches(good):
            print("FAIL: self-check rejected negation of mcp-server")
            return 1
        print("OK: self-check rejected fake zerostack serve, banned mcp-server launch, kept negation")
        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Audit README/docs command claims")
    parser.add_argument("--readme", type=Path, default=None, help="Single doc path (legacy)")
    parser.add_argument(
        "--doc",
        action="append",
        type=Path,
        default=None,
        help="Doc to audit (repeatable). Default: README.md + canonical install/development guides",
    )
    parser.add_argument("--self-check", action="store_true")
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    root = Path(__file__).resolve().parents[2]
    if args.readme is not None:
        docs = [args.readme if args.readme.is_absolute() else root / args.readme]
    elif args.doc:
        docs = [d if d.is_absolute() else root / d for d in args.doc]
    else:
        docs = [root / "README.md", root / "docs" / "install.md", root / "docs" / "development.md"]

    exit_code = 0
    for doc in docs:
        if not doc.is_file():
            print(f"FAIL: missing doc {doc}")
            exit_code = 1
            continue
        audit = Audit(root, doc)
        code = audit.run()
        if code != 0:
            exit_code = code
        for finding in banned_mcp_server_launches(doc):
            print(f"{doc}:{finding.line}: {finding.message}: {finding.command}")
            exit_code = 1
    return exit_code

if __name__ == "__main__":
    sys.exit(main())
