#!/usr/bin/env python3
"""FSZero benchmark suite — regenerates benchmarks/demo-bench_results.json.

Measures on THIS repo as corpus: cold full index, warm read, search,
codemode plan, worlds cycle, history+undo, MCP round-trip.
All wall-time numbers are medians of >=5 runs.
"""
import json, os, shutil, statistics, subprocess, sys, tempfile, time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
DEMO = ROOT / "demo"
SCRIPTS = ROOT / "scripts"

# Integrity guard (docs/benchmark-integrity.md): timings of FAILED operations
# are never published. Any entry here aborts the run before the artifact is
# written.
FAILURES = []




def _bin():
    return os.environ.get("FSZERO_BIN", str(ROOT / "target" / "release-perf" / "fszero"))


def scenario_fingerprint():
    command = [
        sys.executable,
        str(SCRIPTS / "env_fingerprint.py"),
        "--root",
        str(ROOT),
        "--cache-state",
        "mixed",
        "--cargo-profile",
        "release-perf",
    ]
    run = subprocess.run(
        command, cwd=ROOT, capture_output=True, text=True, timeout=15, check=False
    )
    if run.returncode != 0:
        raise RuntimeError(
            f"environment fingerprint failed ({run.returncode}): {run.stderr.strip()}"
        )
    document = json.loads(run.stdout)
    required = {
        "schema_version",
        "run_id",
        "captured_at_utc",
        "cache_state",
        "repository",
        "cpu",
        "power",
        "kernel",
        "toolchain",
        "filesystem",
        "isolation",
    }
    missing = sorted(required - document.keys())
    if missing:
        raise RuntimeError(f"environment fingerprint missing keys: {missing}")
    if document["schema_version"] != "fszero.perf-fingerprint.v1":
        raise RuntimeError("environment fingerprint schema version mismatch")
    if document["cache_state"] != "mixed":
        raise RuntimeError("README benchmark fingerprint must declare cache_state=mixed")
    if document["toolchain"].get("cargo_profile") != "release-perf":
        raise RuntimeError("README benchmark fingerprint must bind cargo_profile=release-perf")
    if document["isolation"].get("status") != "provided":
        raise RuntimeError(
            "set FSZERO_PERF_ISOLATION_NOTE before publishing benchmark evidence"
        )
    return document


def p50(v):
    return statistics.median(v)


def p95(v):
    s = sorted(v)
    idx = max(0, int(len(s) * 0.95) - (1 if len(s) * 0.95 == int(len(s) * 0.95) else 0))
    return s[idx]


def run_codemode(plan, timeout=30, env_extra=None, root=None):
    """Execute a codemode plan in a fresh session. Returns (ack, ok, stderr)."""
    root = Path(root) if root else ROOT
    env = os.environ.copy()
    env["FSZERO_ROOT"] = str(root)
    if env_extra:
        env.update(env_extra)
    try:
        r = subprocess.run(
            [_bin(), "codemode", plan, "--root", str(root)],
            capture_output=True, text=True, timeout=timeout,
            cwd=root, env=env,
        )
    except subprocess.TimeoutExpired:
        return ("timeout", False, "")
    ack = r.stdout.strip()
    ok = r.returncode == 0 and not ack.startswith("X0")
    return (ack, ok, r.stderr)


class McpSession:
    """Persistent fszero --mode=mcp process for round-trip benchmarks."""

    def __init__(self):
        self.proc = subprocess.Popen(
            [_bin(), "--mode=mcp"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            cwd=ROOT,
        )
        self._handshake()

    def _handshake(self):
        self._send({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "bench", "version": "0"},
            },
        })
        self._recv()
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def _send(self, msg):
        line = json.dumps(msg, separators=(",", ":")) + "\n"
        self.proc.stdin.write(line.encode())
        self.proc.stdin.flush()

    def _recv(self):
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError("MCP server closed stdout")
        return json.loads(line)

    def call_tool(self, name, args):
        """Send tools/call, return elapsed milliseconds."""
        t0 = time.perf_counter()
        self._send({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": name, "arguments": args},
        })
        resp = self._recv()
        elapsed = (time.perf_counter() - t0) * 1000.0
        if "error" in resp:
            FAILURES.append(f"mcp {name}: {resp['error']}")
        elif isinstance(resp.get("result"), dict) and resp["result"].get("isError"):
            FAILURES.append(f"mcp {name}: isError result {str(resp['result'])[:120]}")
        return elapsed

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()


# ---------------------------------------------------------------------------
# measurements
# ---------------------------------------------------------------------------

def measure_cold_index():
    """Cold full-index wall time. Clears .fszero, .zerostack, .asgrep each run.
    5 runs, median reported."""
    print("  cold index: clearing .fszero .zerostack .asgrep ...")
    times = []
    for i in range(5):
        for d in [ROOT / ".fszero", ROOT / ".zerostack", ROOT / ".asgrep"]:
            if d.exists():
                shutil.rmtree(d)
        subprocess.run(["sync"], capture_output=True)
        t0 = time.perf_counter()
        ack, ok, _ = run_codemode("explore", env_extra={"FSZERO_STARTUP_INDEX": "1"})
        elapsed = (time.perf_counter() - t0) * 1000.0
        times.append(elapsed)
        status = "ok" if ok else f"FAIL ack={ack}"
        if not ok:
            FAILURES.append(f"cold index run {i+1}: ack={ack}")
        print(f"    run {i+1}: {elapsed:.1f} ms  {status}")
    return times


def measure_warm_read():
    """Warm read p50/p95: 100 iterations of reading README.md via MCP.
    Index is pre-warmed once before measurement."""
    print("  warm read: ensuring index ...")
    run_codemode("explore", env_extra={"FSZERO_STARTUP_INDEX": "1"})
    sess = McpSession()
    try:
        return [sess.call_tool("fszero.read", {"path": "README.md"}) for _ in range(100)]
    finally:
        sess.close()


def measure_search():
    """Search p50: 30 iterations via MCP."""
    print("  search p50: 30 iterations ...")
    run_codemode("explore", env_extra={"FSZERO_STARTUP_INDEX": "1"})
    sess = McpSession()
    try:
        return [sess.call_tool("fszero.search", {"query": "fn "}) for _ in range(30)]
    finally:
        sess.close()


def measure_codemode_plan():
    """CodeMode 3-read plan p50: process-spawn per call, 20 iterations."""
    print("  codemode 3-read plan: 20 iterations ...")
    plan = (
        'const a=await zero.fs.read({path:"README.md"});'
        'const b=await zero.fs.read({path:"Cargo.toml"});'
        'const c=await zero.fs.read({path:"src/lib.rs"});'
        'return{a:a.ref,b:b.ref,c:c.ref};'
    )
    times = []
    for i in range(20):
        t0 = time.perf_counter()
        ack, ok, _ = run_codemode(plan)
        elapsed = (time.perf_counter() - t0) * 1000.0
        times.append(elapsed)
        if not ok:
            FAILURES.append(f"codemode plan run {i+1}: ack={ack}")
    return times


def measure_worlds_cycle():
    """Worlds cycle (new->commit) wall time in a scratch git repo.

    A world commit exports a real git commit (git_commit_world), so this
    never runs against the FSZero checkout. Scratch setup (git init, seed
    commit, index pre-warm) happens outside the timed section. The world id
    is parsed from the op detail ("world:1 W3" -> "W3"). 5 runs, median."""
    print("  worlds cycle: 5 runs (scratch git repo) ...")

    plan = (
        'const w=await zero.fs.world({arg:"new:w.txt:before|after"});'
        'if(!w.ok)return{error:w.detail};'
        r'const m=String(w.detail).match(/\bW\d+\b/);'
        'if(!m)return{error:"no world id in: "+w.detail};'
        'const c=await zero.fs.world({arg:"commit:"+m[0]});'
        'if(!c.ok)return{error:c.detail};'
        'return{committed:m[0]};'
    )
    times = []
    for i in range(5):
        with tempfile.TemporaryDirectory() as tmp:
            scratch = Path(tmp)
            git = ["git", "-C", str(scratch),
                   "-c", "user.email=bench@fszero", "-c", "user.name=bench"]
            subprocess.run(git + ["init", "-q"], check=True, capture_output=True)
            (scratch / "w.txt").write_text("before\n")
            subprocess.run(git + ["add", "w.txt"], check=True, capture_output=True)
            subprocess.run(git + ["commit", "-q", "-m", "seed"],
                           check=True, capture_output=True)
            # Pre-warm store/index so the timed run measures the worlds
            # cycle, not first-touch indexing of the scratch root.
            run_codemode("explore", root=scratch)

            t0 = time.perf_counter()
            ack, ok, _ = run_codemode(plan, root=scratch)
            elapsed = (time.perf_counter() - t0) * 1000.0
            times.append(elapsed)
            status = "ok" if ok else f"FAIL ack={ack}"
            if not ok:
                FAILURES.append(f"worlds cycle run {i+1}: ack={ack}")
            print(f"    run {i+1}: {elapsed:.1f} ms  {status}")

    return times


def measure_history_undo():
    """History+undo round-trip wall time. Fresh file per iteration.

    Setup writes two versions (mutations land in the durable journal), then
    the timed plan queries history and undoes the latest mutation for the
    path (do_undo spec: `path` or `path|seq`). Write results are captured so
    op failures are detected explicitly (fszero-j7r regression guard).
    5 runs, median reported."""
    print("  history+undo: 5 runs ...")

    times = []
    for i in range(5):
        fname = f"scripts/bench_hist_{i}.txt"
        fpath = ROOT / fname
        # Two versions -> journal has a create + an overwrite for the path.
        for content in ("v0", "v1"):
            ack, ok, _ = run_codemode(
                f'const w=await zero.fs.write({{path:"{fname}",content:"{content}"}});'
                'if(!w.ok)return{error:w.detail};'
                'return{done:true};'
            )
            if not ok:
                FAILURES.append(f"history+undo setup write {content} run {i+1}: ack={ack}")
        t0 = time.perf_counter()
        ack, ok, _ = run_codemode(
            f'const h=await zero.fs.history({{arg:"{fname}|5"}});'
            'if(!h.ok)return{error:h.detail};'
            f'const u=await zero.fs.undo({{arg:"{fname}"}});'
            'if(!u.ok)return{error:u.detail};'
            'return{history:h.ack,undo:u.ack};'
        )
        elapsed = (time.perf_counter() - t0) * 1000.0
        times.append(elapsed)
        status = "ok" if ok else f"FAIL ack={ack}"
        if not ok:
            FAILURES.append(f"history+undo run {i+1}: ack={ack}")
        print(f"    run {i+1}: {elapsed:.1f} ms  {status}")

        # Cleanup
        if fpath.exists():
            fpath.unlink()

    return times


def measure_mcp_roundtrip():
    """MCP-mode tools/call round-trip p50: 30 iterations of read call."""
    print("  MCP tools/call round-trip: 30 iterations ...")
    sess = McpSession()
    try:
        return [sess.call_tool("fszero.read", {"path": "README.md"}) for _ in range(30)]
    finally:
        sess.close()


# ---------------------------------------------------------------------------
# reporting
# ---------------------------------------------------------------------------

def corpus_stats():
    """Return {files: N, bytes: N} for repo source corpus."""
    files = 0
    total_bytes = 0
    excludes = {".git", "target", ".asgrep", ".zerostack", ".fszero"}
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in excludes]
        for f in filenames:
            try:
                total_bytes += (Path(dirpath) / f).stat().st_size
                files += 1
            except OSError:
                pass
    return {"files": files, "bytes": total_bytes}


def hardware():
    """Return hardware identifier string."""
    try:
        cpu = subprocess.check_output(
            ["sysctl", "-n", "machdep.cpu.brand_string"], text=True
        ).strip()
    except Exception:
        cpu = "unknown"
    try:
        ram_bytes = int(
            subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True).strip()
        )
        ram = f"{ram_bytes // (1024**3)} GB"
    except Exception:
        ram = "unknown"
    return f"{cpu} / {ram}"


def git_provenance():
    """Return {commit, dirty} for the code state that produced this run.

    dirty covers tracked files only (-uno): the cited commit must describe
    the measured code; corpus stats already describe what was on disk. The
    output artifact itself is excluded -- it is this run's product and can
    never be committed at the moment it is produced.
    """
    try:
        commit = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True
        ).strip()
    except Exception:
        return {"commit": None, "dirty": None}
    try:
        status = subprocess.check_output(
            ["git", "status", "--porcelain", "-uno", "--",
             ".", ":(exclude)benchmarks/demo-bench_results.json"],
            cwd=ROOT, text=True,
        )
        dirty = bool(status.strip())
    except Exception:
        dirty = None
    return {"commit": commit, "dirty": dirty}


def fmt_ms(v):
    """Format a millisecond value. When < 1 ms, show as µs."""
    if v < 1.0:
        return f"{v * 1000:.0f} µs"
    return f"{v:.2f} ms"


def fmt_ms_raw(v):
    """Return float ms for JSON (high precision)."""
    return round(v, 6)


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main():
    print("=== FSZero Benchmark Suite ===")
    print(f"Binary: {_bin()}")
    print(f"Root:   {ROOT}")
    print()
    fingerprint = scenario_fingerprint()

    # Ensure binary exists
    if not Path(_bin()).exists():
        print("Building FSZero release-perf binary ...")
        subprocess.run(
            [str(ROOT / "scripts" / "profile_build.sh"), "-p", "fszero-cli", "--bin", "fszero"],
            cwd=ROOT,
            check=True,
        )

    print("--- 1. Cold full index ---")
    cold_times = measure_cold_index()
    c_med = p50(cold_times)
    print(f"  cold index median: {fmt_ms(c_med)}")

    print()
    print("--- 2. Warm read (100 iterations) ---")
    read_times = measure_warm_read()
    r50 = p50(read_times)
    r95 = p95(read_times)
    print(f"  warm read p50: {fmt_ms(r50)}  p95: {fmt_ms(r95)}")

    print()
    print("--- 3. Search ---")
    search_times = measure_search()
    s50 = p50(search_times)
    print(f"  search p50: {fmt_ms(s50)}")

    print()
    print("--- 4. CodeMode 3-read plan ---")
    codemode_times = measure_codemode_plan()
    cm50 = p50(codemode_times)
    print(f"  codemode plan p50: {fmt_ms(cm50)}")

    print()
    print("--- 5. Worlds cycle (new->commit) ---")
    world_times = measure_worlds_cycle()
    w50 = p50(world_times)
    print(f"  worlds cycle median: {fmt_ms(w50)}")

    print()
    print("--- 6. History+undo round-trip ---")
    hu_times = measure_history_undo()
    hu50 = p50(hu_times)
    print(f"  history+undo median: {fmt_ms(hu50)}")

    print()
    print("--- 7. MCP tools/call round-trip ---")
    mcp_times = measure_mcp_roundtrip()
    m50 = p50(mcp_times)
    print(f"  MCP round-trip p50: {fmt_ms(m50)}")

    # Integrity guard: never publish timings of failed operations
    # (docs/benchmark-integrity.md).
    if FAILURES:
        print()
        print("=== INTEGRITY FAILURE: measured operations failed; artifact NOT written ===")
        for f in FAILURES:
            print(f"  - {f}")
        sys.exit(1)

    # Assemble and write results
    prov = git_provenance()
    results = {
        "hardware": hardware(),
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "git_commit": prov["commit"],
        "git_dirty": prov["dirty"],
        "scenario_fingerprint": fingerprint,
        "corpus": corpus_stats(),
        "results": {
            "cold_full_index_ms": fmt_ms_raw(c_med),
            "cold_full_index_n_runs": len(cold_times),
            "warm_read_p50_ms": fmt_ms_raw(r50),
            "warm_read_p95_ms": fmt_ms_raw(r95),
            "warm_read_iterations": len(read_times),
            "search_p50_ms": fmt_ms_raw(s50),
            "search_iterations": len(search_times),
            "codemode_3read_plan_p50_ms": fmt_ms_raw(cm50),
            "codemode_plan_iterations": len(codemode_times),
            "worlds_new_commit_cycle_ms": fmt_ms_raw(w50),
            "worlds_n_runs": len(world_times),
            "history_undo_roundtrip_ms": fmt_ms_raw(hu50),
            "history_undo_n_runs": len(hu_times),
            "mcp_tools_call_roundtrip_p50_ms": fmt_ms_raw(m50),
            "mcp_iterations": len(mcp_times),
        },
    }

    DEMO.mkdir(parents=True, exist_ok=True)
    out = DEMO / "bench_results.json"
    with open(out, "w") as f:
        json.dump(results, f, indent=2)
    print()
    print(f"=== Results written to {out} ===")


if __name__ == "__main__":
    main()
