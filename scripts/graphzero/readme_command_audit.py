#!/usr/bin/env python3
"""Audit README shell claims without running documented commands."""
from __future__ import annotations

import argparse
import json
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
AUDIT_SMOKE = "<!-- audit:smoke -->"
DEFAULT_MANIFEST = Path("docs/contracts/readme-command-manifest.json")
FENCE_RE = re.compile(r"^\s*```([A-Za-z0-9_-]+)?\s*$")
INLINE_RE = re.compile(r"`([^`\n]+)`")
KNOWN_BINARIES = {"tokenzero", "fszero", "graphzero", "zerostack", "node", "python3", "cargo", "git", "rustup"}
PROJECT_BINARIES = {"tokenzero", "fszero", "graphzero"}
IGNORED_COMMANDS = {"cd", "export", "printf", "echo", "cat", "mkdir", "cp", "mv", "rm", "true", "false"}
CONTROL_WORDS = {"if", "then", "else", "fi", "for", "do", "done", "while"}
SAFE_EXEC_ARGS = {"--help", "-h", "help", "--version", "-V", "version"}
CARGO_WRAPPER_HELP_SUBCOMMAND = "check"
SHELL_METACHARACTERS = frozenset(";$`")

@dataclass(frozen=True)
class Candidate:
    text: str
    line: int
    source: str
    execute: bool = False

@dataclass(frozen=True)
class Finding:
    line: int
    message: str
    command: str

class Audit:
    def __init__(self, root: Path, readme: Path, manifest: Path | None = None):
        self.root = root
        self.readme = readme
        self.manifest_path = manifest or (root / DEFAULT_MANIFEST)
        self.manifest = self.load_manifest()
        self.help_cache: dict[tuple[str, ...], tuple[int, str]] = {}
        self.findings: list[Finding] = []
        self.skips: list[Candidate] = []


    def load_manifest(self) -> dict[str, dict[str, str]]:
        if not self.manifest_path.exists():
            return {}
        try:
            data = json.loads(self.manifest_path.read_text(encoding="utf-8"))  # ubs:ignore — JSONDecodeError is converted below.
        except (OSError, json.JSONDecodeError) as error:
            raise ValueError(f"invalid README command manifest: {error}") from error
        commands = data.get("commands", [])
        manifest: dict[str, dict[str, str]] = {}
        for row in commands:
            command = row.get("command")
            if not isinstance(command, str) or not command.strip():
                raise ValueError(f"invalid README command manifest row: {row!r}")
            manifest[command] = {str(k): str(v) for k, v in row.items()}
        return manifest

    def verify_manifest(self, candidates: list[Candidate]) -> None:
        if not self.manifest:
            return
        seen = {candidate.text for candidate in candidates}
        smoke_seen = {candidate.text for candidate in candidates if candidate.execute}
        for candidate in candidates:
            row = self.manifest.get(candidate.text)
            if row is None:
                self.findings.append(Finding(candidate.line, "command missing from README command manifest", candidate.text))
                continue
            audit_mode = row.get("audit", "help")
            if candidate.execute and audit_mode != "smoke":
                self.findings.append(Finding(candidate.line, "smoke command manifest row must use audit=smoke", candidate.text))
            if not row.get("purpose"):
                self.findings.append(Finding(candidate.line, "manifest row missing purpose", candidate.text))
        for command, row in self.manifest.items():
            if row.get("audit", "help") == "smoke" and command not in smoke_seen:
                self.findings.append(Finding(0, "manifest marks command smoke but README lacks audit:smoke marker", command))
        for command in sorted(set(self.manifest) - seen):
            self.findings.append(Finding(0, "stale command manifest entry not present in README", command))

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
        """Run a bounded help probe without a shell.

        ``argv`` is passed as discrete arguments, the inherited environment is
        copied with ``NO_COLOR`` made explicit, and the 15-second timeout bounds
        blocking. The tuple always carries an exit status plus diagnostic text;
        callers include the command name and status in findings.
        """
        key = tuple(argv)
        if key in self.help_cache:
            return self.help_cache[key]
        env = os.environ.copy()
        env.setdefault("NO_COLOR", "1")
        try:
            proc = subprocess.run(  # ubs:ignore — return code is intentionally recorded below.
                argv,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=15,
                env=env,
            )
            result = (proc.returncode, proc.stdout)
        except Exception as exc:
            result = (127, f"{argv[0]} failed to execute: {exc}")
        self.help_cache[key] = result
        return result

    def extract(self) -> list[Candidate]:
        lines = self.readme.read_text(encoding="utf-8").splitlines()
        candidates: list[Candidate] = []
        active_lang: str | None = None
        active_execute = False
        for idx, line in enumerate(lines, start=1):
            fence = FENCE_RE.match(line)
            if fence:
                lang = (fence.group(1) or "").lower()
                if active_lang is not None:
                    active_lang = None
                    active_execute = False
                elif lang in {"bash", "sh", "console"}:
                    active_lang = lang
                    active_execute = idx > 1 and AUDIT_SMOKE in lines[idx - 2]
                continue
            if active_lang in {"bash", "sh", "console"}:
                if idx > 1 and AUDIT_SKIP in lines[idx - 2]:
                    if lines[idx - 2].strip() == AUDIT_SKIP:
                        self.findings.append(Finding(idx, "audit skip marker missing reason", line.strip()))
                    self.skips.append(Candidate(line.strip(), idx, "fence"))
                    continue
                for command in commands_from_shell_line(line):
                    candidates.append(Candidate(command, idx, "fence", active_execute))
            for match in INLINE_RE.finditer(line):
                text = match.group(1).strip()
                first = first_command_word(text)
                if first in KNOWN_BINARIES:
                    if idx > 1 and AUDIT_SKIP in lines[idx - 2]:
                        if lines[idx - 2].strip() == AUDIT_SKIP:
                            self.findings.append(Finding(idx, "audit skip marker missing reason", text))
                        self.skips.append(Candidate(text, idx, "inline"))
                    else:
                        candidates.append(Candidate(text, idx, "inline", False))
        return candidates

    def run(self) -> int:
        candidates = self.extract()
        for candidate in candidates:
            self.audit_candidate(candidate)
        self.verify_manifest(candidates)
        for skip in self.skips:
            print(f"SKIP {self.readme}:{skip.line}: {skip.command if hasattr(skip, 'command') else skip.text}")
        smoke_count = sum(1 for candidate in candidates if candidate.execute)
        for finding in self.findings:
            print(f"{self.readme}:{finding.line}: {finding.message}: {finding.command}")
        if self.findings:
            print(f"FAIL: {len(self.findings)} README command audit finding(s), {len(self.skips)} skip(s), {smoke_count} smoke(s)")
            return 1
        print(f"OK: README command audit passed ({len(candidates)} command(s), {len(self.skips)} skip(s), {smoke_count} smoke(s))")
        return 0

    def audit_candidate(self, candidate: Candidate) -> None:
        if has_shell_metacharacters(candidate.text):
            self.findings.append(
                Finding(candidate.line, "shell metacharacters are not allowed", candidate.text)
            )
            return
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
            row = self.manifest.get(candidate.text, {})
            if binary_name in PROJECT_BINARIES and row.get("audit") == "manifest":
                return
            self.findings.append(Finding(candidate.line, f"cannot resolve binary '{binary_name}'", candidate.text))
            return
        if binary_name == "zerostack":
            self.findings.append(Finding(candidate.line, "unknown binary 'zerostack'", candidate.text))
            return
        self.verify_args(candidate, resolved, binary_name, argv[1:])
        if candidate.execute:
            self.execute_smoke(candidate, argv)

    def execute_smoke(self, candidate: Candidate, argv: list[str]) -> None:
        """Run an explicitly marked smoke command under the subprocess contract.

        The command uses discrete argv (never a shell), an explicit inherited
        environment, captured combined output, and a 60-second timeout. Spawn
        and exit failures retain the command name and status in the finding.
        """
        if Path(argv[0]).name not in {"cargo", "graphzero"}:
            self.findings.append(Finding(candidate.line, "smoke command must use cargo or graphzero", candidate.text))
            return
        env = os.environ.copy()
        env.setdefault("NO_COLOR", "1")
        try:
            proc = subprocess.run(  # ubs:ignore — smoke failures are converted to findings below.
                argv,
                cwd=self.root,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=60,
                env=env,
            )
        except Exception as exc:
            self.findings.append(Finding(candidate.line, f"{argv[0]} failed to execute with status 127 ({exc})", candidate.text))
            return
        if proc.returncode != 0:
            output = proc.stdout.strip().splitlines()[-1:]
            tail = output[0] if output else "no output"
            self.findings.append(Finding(candidate.line, f"{argv[0]} exited with status {proc.returncode}: {tail}", candidate.text))

    def record_command_failure(
        self,
        candidate: Candidate,
        argv: list[str],
        status: int,
        output: str,
    ) -> None:
        tail_lines = output.strip().splitlines()[-1:]
        tail = tail_lines[0] if tail_lines else "no output"
        command = shlex.join(argv)
        self.findings.append(
            Finding(
                candidate.line,
                f"{command} exited with status {status}: {tail}",
                candidate.text,
            )
        )

    def verify_args(self, candidate: Candidate, resolved: str, binary_name: str, args: list[str]) -> None:
        top_argv = [resolved, "--help"]
        top_code, top_help = self.help_text(top_argv)
        if top_code != 0:
            self.record_command_failure(candidate, top_argv, top_code, top_help)
            return
        if binary_name == "cargo":
            self.verify_cargo(candidate, resolved, args, top_help)
            return
        subcommand = first_subcommand(binary_name, args)
        help_text = top_help
        if subcommand:
            subcommand_argv = [resolved, subcommand, "--help"]
            code, text = self.help_text(subcommand_argv)
            if code != 0:
                self.record_command_failure(candidate, subcommand_argv, code, text)
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
            subcommand_argv = [resolved, subcommand, "--help"]
            code, text = self.help_text(subcommand_argv)
            if code != 0:
                self.record_command_failure(candidate, subcommand_argv, code, text)
                return
            help_text = text + "\n" + top_help
        wrapper_argv = [resolved, CARGO_WRAPPER_HELP_SUBCOMMAND, "--help"]
        code, wrapper_text = self.help_text(wrapper_argv)
        if code != 0:
            self.record_command_failure(candidate, wrapper_argv, code, wrapper_text)
            return
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


def has_shell_metacharacters(text: str) -> bool:
    """Reject shell evaluation syntax while allowing angle-bracket placeholders."""
    command = strip_prompt(text)
    return any(char in command for char in SHELL_METACHARACTERS) or bool(
        re.search(r"(?:^|\s)(?:[<>]|&)(?:\s|$)", command)
    )


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


def self_check() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        readme = root / "README.md"
        readme.write_text(
            """# Scratch

```bash
zerostack serve
cargo --version; definitely-not-a-command
```

<!-- audit:skip -->
`zerostack serve`
""",
            encoding="utf-8",
        )
        audit = Audit(root, readme)
        code = audit.run()
        messages = {finding.message for finding in audit.findings}
        if code == 0 or not any("binary" in message for message in messages):
            print("FAIL: self-check did not reject fake zerostack serve")
            return 1
        if "shell metacharacters are not allowed" not in messages:
            print("FAIL: self-check did not reject shell metacharacters")
            return 1
        if not audit.skips:
            print("FAIL: self-check did not record skip marker")
            return 1
        print("OK: self-check rejected fake binaries and shell metacharacters and reported skip")
        return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Audit README command claims")
    parser.add_argument("--readme", type=Path, default=None)
    parser.add_argument("--self-check", action="store_true")
    parser.add_argument("--manifest", type=Path, default=None)
    args = parser.parse_args()
    if args.self_check:
        return self_check()
    root = Path(__file__).resolve().parents[2]
    readme = args.readme or (root / "README.md")
    return Audit(root, readme, args.manifest).run()

if __name__ == "__main__":
    sys.exit(main())
