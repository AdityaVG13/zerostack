#!/usr/bin/env python3
"""Adversarial e2e against `zsx mcp` over both NDJSON and LSP framing."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ZSX = Path(os.environ.get("ZSX_BIN", "/Users/aditya/.grok/plugins/zerostack/bin/zsx"))
FAILS: list[str] = []
PASSES: list[str] = []


def record(name: str, ok: bool, detail: str = "") -> None:
    bucket = PASSES if ok else FAILS
    bucket.append(f"{name}: {detail}" if detail else name)
    mark = "PASS" if ok else "FAIL"
    print(f"[{mark}] {name}" + (f" -- {detail}" if detail else ""), flush=True)


class Mcp:
    def __init__(self, framing: str, cwd: Path) -> None:
        self.framing = framing
        self.proc = subprocess.Popen(
            [str(ZSX), "mcp", "-C", str(cwd)],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            cwd=str(cwd),
        )
        assert self.proc.stdin and self.proc.stdout

    def send(self, msg: dict) -> None:
        raw = json.dumps(msg, separators=(",", ":")).encode()
        assert self.proc.stdin
        if self.framing == "ndjson":
            self.proc.stdin.write(raw + b"\n")
        else:
            self.proc.stdin.write(f"Content-Length: {len(raw)}\r\n\r\n".encode() + raw)
        self.proc.stdin.flush()

    def recv(self, timeout: float = 20.0) -> dict | None:
        assert self.proc.stdout
        # Never select()+read(1) on a buffered pipe: select sees an empty
        # fd after the first buffered read, and the rest of the frame sits
        # in Python's buffer forever.
        self.proc.stdout.flush()
        if self.framing == "ndjson":
            line = self.proc.stdout.readline()
            if not line:
                return None
            return json.loads(line)
        headers = b""
        while not headers.endswith(b"\r\n\r\n"):
            chunk = self.proc.stdout.readline()
            if not chunk:
                return None
            headers += chunk
        text = headers.decode()
        length = 0
        for row in text.split("\r\n"):
            if row.lower().startswith("content-length:"):
                length = int(row.split(":", 1)[1].strip())
        body = b""
        while len(body) < length:
            chunk = self.proc.stdout.read(length - len(body))
            if not chunk:
                break
            body += chunk
        return json.loads(body)

    def call(self, method: str, params: dict | None = None, msg_id: int = 1) -> dict | None:
        msg: dict = {"jsonrpc": "2.0", "id": msg_id, "method": method}
        if params is not None:
            msg["params"] = params
        self.send(msg)
        return self.recv()

    def notify(self, method: str) -> None:
        self.send({"jsonrpc": "2.0", "method": method})

    def close(self) -> tuple[int, str]:
        assert self.proc.stdin
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait(timeout=5)
        err = (self.proc.stderr.read() if self.proc.stderr else b"").decode(errors="replace")
        return self.proc.returncode or 0, err


def execute(mcp: Mcp, plan: str, root: str | None = None, timeout_ms: int = 15000, msg_id: int = 10) -> dict:
    args: dict = {"plan": plan, "timeout_ms": timeout_ms}
    if root:
        args["root"] = root
    reply = mcp.call("tools/call", {"name": "zero_execute", "arguments": args}, msg_id=msg_id)
    assert reply is not None, "no reply"
    return reply


def envelope(reply: dict) -> dict | None:
    text = (((reply.get("result") or {}).get("content") or [{}])[0]).get("text")
    if not text:
        return None
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        return None


def is_tool_error(reply: dict) -> bool:
    return bool((reply.get("result") or {}).get("isError"))


def main() -> int:
    if not ZSX.is_file():
        print(f"missing {ZSX}", file=sys.stderr)
        return 2

    scratch = Path(tempfile.mkdtemp(prefix="zsx-mcp-e2e-"))
    (scratch / "hello.txt").write_text("hello from stress\n")
    roots = []
    try:
        for i in range(6):
            r = scratch / f"root{i}"
            r.mkdir()
            (r / "f.txt").write_text(f"root-{i}\n")
            roots.append(r)

        # --- NDJSON handshake ---
        mcp = Mcp("ndjson", scratch)
        init = mcp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "stress", "version": "0"}})
        record("ndjson initialize", bool(init and init.get("result", {}).get("serverInfo", {}).get("name") == "zerostack-zsx"), str(init)[:200])
        mcp.notify("notifications/initialized")
        listed = mcp.call("tools/list", msg_id=2)
        names = [t.get("name") for t in ((listed or {}).get("result") or {}).get("tools") or []]
        record("ndjson tools/list", names == ["zero_execute", "zero_wait"], str(names))
        ping = mcp.call("ping", msg_id=3)
        record("ndjson ping", bool(ping and ping.get("result") == {}), str(ping))
        unknown = mcp.call("nope", msg_id=4)
        record("ndjson unknown method", bool(unknown and (unknown.get("error") or {}).get("code") == -32601), str(unknown)[:200])

        wait = mcp.call("tools/call", {"name": "zero_wait", "arguments": {}}, msg_id=5)
        wait_text = (((wait or {}).get("result") or {}).get("content") or [{}])[0].get("text", "")
        record("ndjson zero_wait mcp=true", '"mcp": true' in wait_text or '"mcp":true' in wait_text.replace(" ", ""), wait_text[:180])

        first = execute(mcp, 'return await zero.fs.compound("read", {path:"hello.txt"});', msg_id=10)
        env1 = envelope(first)
        record("ndjson execute read", bool(env1 and env1.get("ok") and env1.get("request_id") == 1), str(env1)[:240] if env1 else str(first)[:240])

        second = execute(mcp, "return 7;", msg_id=11)
        env2 = envelope(second)
        record("ndjson session reuse request_id=2", bool(env2 and env2.get("request_id") == 2), str(env2)[:200] if env2 else str(second)[:200])

        empty = execute(mcp, "   ", msg_id=12)
        record("ndjson empty plan isError", is_tool_error(empty), str(empty)[:200])

        badjs = execute(mcp, "???", msg_id=13)
        record("ndjson bad js isError", is_tool_error(badjs), str(badjs)[:240])

        missing = execute(mcp, "return 1;", root="/no/such/zsx-root", msg_id=14)
        record("ndjson missing root isError", is_tool_error(missing), str(missing)[:240])

        zero_to = execute(mcp, "return 1;", timeout_ms=0, msg_id=15)
        record("ndjson timeout_ms=0 isError", is_tool_error(zero_to), str(zero_to)[:200])

        # timeout: tight bound around a busy loop
        t0 = time.time()
        hung = execute(mcp, "while (true) {}", timeout_ms=800, msg_id=16)
        dt = time.time() - t0
        record(
            "ndjson while(true) times out",
            is_tool_error(hung) and 0.3 < dt < 8.0,
            f"dt={dt:.2f}s err={is_tool_error(hung)} body={str(hung)[:180]}",
        )

        # still alive after timeout
        after = execute(mcp, "return 99;", msg_id=17)
        env_after = envelope(after)
        record("ndjson session survives timeout", bool(env_after and env_after.get("ok") and env_after.get("result") == 99), str(env_after)[:200] if env_after else str(after)[:200])

        # 5 extra roots: cap is 4, plus scratch already live. Evict oldest.
        reuse_ids = []
        for i, r in enumerate(roots[:5]):
            reply = execute(mcp, "return await zero.fs.compound('read', {path:'f.txt'});", root=str(r), msg_id=30 + i)
            env = envelope(reply)
            reuse_ids.append(env.get("request_id") if env else None)
        record("ndjson 5 roots first-pass all ok", all(x == 1 for x in reuse_ids), str(reuse_ids))

        # re-hit first extra root: if evicted, request_id resets to 1; if kept, 2
        again = execute(mcp, "return 1;", root=str(roots[0]), msg_id=40)
        env_again = envelope(again)
        record("ndjson 5th root does not crash", bool(env_again and env_again.get("ok")), str(env_again)[:200] if env_again else str(again)[:200])

        # file used as root
        file_root = execute(mcp, "return 1;", root=str(scratch / "hello.txt"), msg_id=41)
        file_text = str(file_root)
        record(
            "ndjson file-as-root isError",
            is_tool_error(file_root) and "not a directory" in file_text,
            file_text[:240],
        )

        # unknown tool
        unk = mcp.call("tools/call", {"name": "zero_explode", "arguments": {}}, msg_id=42)
        record("ndjson unknown tool isError", bool(unk and (unk.get("result") or {}).get("isError")), str(unk)[:200])

        code, err = mcp.close()
        record("ndjson eof exit 0", code == 0, f"code={code} stderr={err[-300:]}")

        # --- LSP framing ---
        lsp = Mcp("lsp", scratch)
        init = lsp.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "stress", "version": "0"}})
        record("lsp initialize", bool(init and init.get("id") == 1), str(init)[:200])
        listed = lsp.call("tools/list", msg_id=2)
        names = [t.get("name") for t in ((listed or {}).get("result") or {}).get("tools") or []]
        record("lsp tools/list", names == ["zero_execute", "zero_wait"], str(names))
        first = execute(lsp, 'return await zero.fs.compound("read", {path:"hello.txt"});', msg_id=10)
        env1 = envelope(first)
        record("lsp execute read", bool(env1 and env1.get("ok")), str(env1)[:200] if env1 else str(first)[:200])
        code, err = lsp.close()
        record("lsp eof exit 0", code == 0, f"code={code} stderr={err[-200:]}")

        # --- mixed garbage then recover? first line not json ---
        garbage = Mcp("ndjson", scratch)
        assert garbage.proc.stdin
        garbage.proc.stdin.write(b"not-json-at-all\n")
        garbage.proc.stdin.flush()
        parse_err = garbage.recv()
        record(
            "garbage first line is parse error not hang",
            bool(parse_err and (parse_err.get("error") or {}).get("code") == -32700),
            str(parse_err)[:200],
        )
        alive = garbage.proc.poll() is None
        record("after garbage process still alive", alive, f"poll={garbage.proc.poll()}")
        if alive:
            init = garbage.call("initialize", {}, msg_id=1)
            record(
                "after garbage, initialize handled",
                bool(init and init.get("result", {}).get("serverInfo", {}).get("name") == "zerostack-zsx"),
                str(init)[:160] if init else "no reply",
            )
        garbage.close()

        # --- graph + token on ZeroStack root via mcp ---
        zs = Path("/Users/aditya/AI/ZeroStack")
        if zs.is_dir():
            zs_mcp = Mcp("ndjson", zs)
            zs_mcp.call("initialize", {}, msg_id=1)
            tok = execute(zs_mcp, "return await zero.token.shell({command: 'printf hi'});", root=str(zs), msg_id=50)
            tenv = envelope(tok)
            record("token.shell via mcp", bool(tenv and tenv.get("ok")), str(tenv)[:240] if tenv else str(tok)[:240])
            # graph may fail if no index; still must not kill the server
            graph = execute(zs_mcp, "return await zero.graph.orient('delta');", root=str(zs), timeout_ms=20000, msg_id=51)
            record("graph.orient does not kill mcp", graph.get("jsonrpc") == "2.0", str(graph)[:240])
            still = execute(zs_mcp, "return 'alive';", root=str(zs), msg_id=52)
            senv = envelope(still)
            record("alive after graph", bool(senv and senv.get("result") == "alive"), str(senv)[:200] if senv else str(still)[:200])
            zs_mcp.close()

    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    print()
    print(f"passed {len(PASSES)}  failed {len(FAILS)}")
    for row in FAILS:
        print(f"  FAIL {row}")
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
