#!/usr/bin/env python3
"""Adversarial bake-off: FSZero vs watchman vs git status (fszero-0l1).

Named competitors, identical conditions, published losses. On each corpus
size (deterministic synthetic tree, gen_corpus.py, seed 42) this measures:

1. Change-detection wall after touching K seeded files:
   - `git status --porcelain` cold-ish (first run after the touches) and
     warm (second run, opportunistically refreshed index),
   - watchman: event latency (poll since-queries until all K appear) and
     settled since-query wall, over a persistent unix-socket client,
   - fszero: one-shot trivial CodeMode op wall on a warm store — its
     startup incremental refresh (walk + manifest sig-diff) applies the
     deltas. NOTE: this wall includes a full index UPDATE (parse + ingest +
     store txn), not just detection; the no-change one-shot wall is
     published as the detection-only floor.

2. Watch event latency end-to-end: touch K files, poll the long-lived
   `fszero --mode=codemode` server (FSZERO_WATCH=1) with search plans until
   all K are visible — includes FSEvents delivery, drain, reindex, search,
   and poll quantization. Watchman is measured with the same poll loop.

3. Event fidelity: create / modify / delete / rename — observed
   classification per system vs expected.

INTEGRITY (docs/benchmark-integrity.md): every git/watchman observation is
verified to be exactly the touched set; every fszero warm refresh must
report incremental=true, files_walked=N and dirty=K, or the benchmark
ABORTS rather than publishing a lie. Losses are published as measured.

Usage: python3 benchmarks/watch_bakeoff.py [--sizes 23000,100000]
       [--trials 5] [--k 10]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import socket
import statistics
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WATCHMAN = os.environ.get("WATCHMAN_BIN", "/opt/homebrew/bin/watchman")
DEFAULT_SIZES = [23000, 100000]
POLL_SLEEP_S = 0.002
POLL_TIMEOUT_S = 120.0
STORE_DIRS = (".fszero", ".zerostack", ".asgrep")


def fszero_bin() -> str:
    return os.environ.get("FSZERO_BIN", str(ROOT / "target" / "release-perf" / "fszero"))


def fszero_env(corpus: Path, n_files: int, extra: dict[str, str] | None = None) -> dict[str, str]:
    env = os.environ.copy()
    for k in ("FSZERO_SKIP_STARTUP_INDEX", "ZEROSTACK_STORE_ROOT", "ZERO_STACK_STORE_ROOT"):
        env.pop(k, None)
    env.update({
        "FSZERO_ROOT": str(corpus),
        "FSZERO_STARTUP_INDEX": "1",
        "FSZERO_INDEX_PHASES": "1",
        "FSZERO_INDEX_MAX_FILES": str(n_files + 1000),
    })
    env.update(extra or {})
    return env


def git_provenance() -> dict[str, object]:
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    status = subprocess.check_output(
        ["git", "status", "--porcelain", "-uno", "--", ".",
         ":(exclude)benchmarks/watch-bakeoff.json", ":(exclude)benchmarks/watch-bakeoff.md"],
        cwd=ROOT, text=True,
    )
    return {"git_commit": commit, "git_dirty": bool(status.strip())}


def hardware() -> str:
    try:
        cpu = subprocess.check_output(["sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
        ram = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True).strip())
        return f"{cpu} / {ram // (1024 ** 3)} GB"
    except Exception:
        return "unknown"


def versions() -> dict[str, str]:
    git_v = subprocess.check_output(["git", "--version"], text=True).strip()
    wm_v = json.loads(subprocess.check_output([WATCHMAN, "version"], text=True))["version"]
    bin_sha = hashlib.sha256(Path(fszero_bin()).read_bytes()).hexdigest()[:16]
    return {"git": git_v, "watchman": wm_v, "fszero_bin_sha256_16": bin_sha}


# ---------------------------------------------------------------- clients


class WatchmanClient:
    """Persistent unix-socket JSON-protocol client (symmetric with the
    persistent fszero server pipe; avoids per-poll CLI spawn overhead)."""

    def __init__(self) -> None:
        sockname = json.loads(subprocess.check_output([WATCHMAN, "get-sockname"]))["sockname"]
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.connect(sockname)
        self.buf = b""

    def cmd(self, *args: object) -> dict:
        self.sock.sendall(json.dumps(list(args)).encode() + b"\n")
        while True:
            while b"\n" not in self.buf:
                chunk = self.sock.recv(1 << 20)
                if not chunk:
                    raise RuntimeError("watchman closed the socket")
                self.buf += chunk
            line, self.buf = self.buf.split(b"\n", 1)
            resp = json.loads(line)
            if "log" in resp or "subscription" in resp:
                continue  # unilateral PDU
            if "error" in resp:
                raise SystemExit(f"INTEGRITY: watchman error: {resp['error']}")
            return resp


class FszeroServer:
    """Long-lived `fszero --mode=codemode` stdio JSON-RPC client."""

    def __init__(self, corpus: Path, n_files: int, watch: bool) -> None:
        extra = {"FSZERO_WATCH": "1"} if watch else {}
        self.proc = subprocess.Popen(
            [fszero_bin(), "--mode=codemode"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, cwd=corpus,
            env=fszero_env(corpus, n_files, extra),
        )
        self.next_id = 1
        self.rpc("initialize", {
            "protocolVersion": "2024-11-05", "capabilities": {},
            "clientInfo": {"name": "watch-bakeoff", "version": "0"},
        })
        self.proc.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        self.proc.stdin.flush()

    def rpc(self, method: str, params: dict) -> dict:
        req = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
        self.next_id += 1
        self.proc.stdin.write(json.dumps(req) + "\n")
        self.proc.stdin.flush()
        line = self.proc.stdout.readline()
        if not line:
            raise SystemExit("INTEGRITY: fszero server died mid-benchmark")
        return json.loads(line)

    def plan(self, code: str) -> dict:
        resp = self.rpc("tools/call", {
            "name": "fz_execute_code",
            "arguments": {"plan": code, "envelope": "v2"},
        })
        return resp.get("result", {}).get("structuredContent", {}).get("value", {}).get("result", {})

    def search_hits(self, marker: str) -> int:
        r = self.plan(
            f"const r = await fs.search({{query: '{marker}'}}); return {{detail: r.detail}};")
        detail = r.get("detail", "")
        if detail.startswith("search:"):
            return int(detail.split(":")[1].split()[0])
        return -1

    def expand_json(self, key: str, _depth: int = 0) -> dict:
        r = self.plan(f"const e = zero.token.expand('{key}'); return {{p: e.payload}};")
        p = r.get("p", "")
        if isinstance(p, dict):
            # Large payloads spill to a blob: {ref: fz://blob/..., preview}.
            inner = p.get("ref", "")
            if isinstance(inner, str) and inner.startswith("fz://") and _depth < 2:
                return self.expand_json(inner, _depth + 1)
            return p
        try:
            return json.loads(p) if p else {}
        except (TypeError, ValueError):
            return {}

    def close(self) -> None:
        try:
            self.proc.stdin.close()
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()


def oracle_expand_json(corpus: Path, key: str) -> dict:
    """Read a store key with full fidelity through a fresh `--mode=mcp`
    process: the per-op wire carries exact payload bytes (wire_contract.rs),
    unlike CodeMode plan results, which spill payloads > 64 tokens to refs.
    Fresh process = fresh store open = current committed feed state. Only
    used on untimed paths (fidelity classification)."""
    env = fszero_env(corpus, 0, {"FSZERO_SKIP_STARTUP_INDEX": "1"})
    env.pop("FSZERO_STARTUP_INDEX", None)
    proc = subprocess.Popen(
        [fszero_bin(), "--mode=mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True, env=env, cwd=corpus)
    try:
        def rpc(i: int, method: str, params: dict) -> dict:
            proc.stdin.write(json.dumps(
                {"jsonrpc": "2.0", "id": i, "method": method, "params": params}) + "\n")
            proc.stdin.flush()
            return json.loads(proc.stdout.readline())

        rpc(1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                              "clientInfo": {"name": "bakeoff-oracle", "version": "0"}})
        proc.stdin.write(json.dumps(
            {"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        proc.stdin.flush()
        r = rpc(2, "tools/call", {"name": "fszero.expand", "arguments": {"arg": key}})
        content = r.get("result", {}).get("content", [])
        if len(content) >= 2:
            sc = json.loads(content[1]["text"])
            payload = sc.get("payload")
            if isinstance(payload, str) and payload:
                # Reading while the watch server writes can surface a
                # concatenated old+new value; take the LAST complete JSON
                # document (the newest write). Torn tails are skipped —
                # callers poll, so the next read sees a settled value.
                decoder = json.JSONDecoder()
                docs, idx = [], 0
                while idx < len(payload):
                    try:
                        doc, end = decoder.raw_decode(payload, idx)
                    except ValueError:
                        break
                    docs.append(doc)
                    idx = end
                    while idx < len(payload) and payload[idx].isspace():
                        idx += 1
                if docs:
                    return docs[-1]
        return {}
    finally:
        proc.stdin.close()
        proc.wait(timeout=10)


# ---------------------------------------------------------------- helpers


def run_git(corpus: Path, *args: str) -> str:
    return subprocess.check_output(
        ["git", "-C", str(corpus), "-c", "user.name=bakeoff",
         "-c", "user.email=bakeoff@invalid", *args], text=True)


def timed_git_status(corpus: Path) -> tuple[float, str]:
    t0 = time.perf_counter()
    out = run_git(corpus, "status", "--porcelain")
    return (time.perf_counter() - t0) * 1e3, out


def verify_git_status(out: str, expected: set[str], label: str) -> None:
    got = {line[3:] for line in out.splitlines() if line.startswith(" M ")}
    extra = [line for line in out.splitlines() if not line.startswith(" M ")]
    if got != expected or extra:
        raise SystemExit(
            f"INTEGRITY: git status ({label}) reported {sorted(got)[:5]}..+{extra[:5]}, "
            f"expected exactly {len(expected)} ' M' entries")


def touch_files(corpus: Path, rels: list[str], marker: str) -> None:
    for i, rel in enumerate(rels):
        with (corpus / rel).open("a") as f:
            f.write(f"pub fn {marker}_{i}() {{ let {marker} = {i}; }}\n")


def wm_query(wm: WatchmanClient, watch: str, since: str) -> list[dict]:
    q = {"since": since, "fields": ["name", "exists", "new"],
         "expression": ["allof", ["type", "f"], ["suffix", "rs"]]}
    return wm.cmd("query", watch, q).get("files", [])


def one_shot(corpus: Path, n_files: int) -> tuple[float, dict]:
    """Timed one-shot trivial plan; returns (wall_ms, phase_json)."""
    t0 = time.perf_counter()
    r = subprocess.run(
        [fszero_bin(), "codemode", "return{ok:true}", "--root", str(corpus)],
        capture_output=True, text=True, timeout=600, cwd=corpus,
        env=fszero_env(corpus, n_files),
    )
    wall_ms = (time.perf_counter() - t0) * 1e3
    ack = r.stdout.strip().splitlines()[0] if r.stdout.strip() else ""
    if r.returncode != 0 or ack.startswith("X0"):
        raise SystemExit(f"INTEGRITY: fszero one-shot failed (ack={ack})")
    phase_line = next(
        (line for line in r.stderr.splitlines() if line.startswith('{"index_phases_ms"')), None)
    if phase_line is None:
        raise SystemExit("INTEGRITY: no index phase JSON on fszero stderr")
    return wall_ms, json.loads(phase_line)


# The index walk sees every non-binary file: the N generated .rs files
# plus the corpus .gitignore this harness adds for the git competitor.
WALK_EXTRA = 1


def check_counts(data: dict, n_files: int, dirty: int, incremental: bool, label: str) -> None:
    c = data["counts"]
    want_walk = n_files + WALK_EXTRA
    if c["files_walked"] != want_walk or c["incremental"] != incremental or c["dirty"] != dirty:
        raise SystemExit(
            f"INTEGRITY: fszero {label}: counts={c}, expected files_walked={want_walk} "
            f"dirty={dirty} incremental={incremental}")


MIN_MEASURED_RUNS = 20


def median(xs: list[float]) -> float:
    return statistics.median(xs)


def percentiles(values: list[float]) -> dict[str, float]:
    ordered = sorted(values)

    def at(fraction: float) -> float:
        rank = (len(ordered) - 1) * fraction
        lower = int(rank)
        upper = min(lower + 1, len(ordered) - 1)
        return ordered[lower] + (ordered[upper] - ordered[lower]) * (rank - lower)

    return {"p50": at(0.50), "p95": at(0.95), "p99": at(0.99)}


# ---------------------------------------------------------------- phases


def setup_corpus(tmp: Path, n_files: int) -> tuple[Path, dict]:
    corpus = tmp / "corpus"
    print(f"=== N={n_files}: generating corpus ...", flush=True)
    subprocess.run(
        ["python3", str(ROOT / "benchmarks" / "gen_corpus.py"),
         "--files", str(n_files), "--out", str(corpus), "--seed", "42"], check=True)
    (corpus / ".gitignore").write_text("".join(f"{d}/\n" for d in STORE_DIRS))
    setup: dict = {}

    t0 = time.perf_counter()
    run_git(corpus, "init", "-q")
    run_git(corpus, "add", "-A")
    run_git(corpus, "commit", "-qm", "baseline")
    setup["git_init_commit_s"] = round(time.perf_counter() - t0, 3)

    t0 = time.perf_counter()
    wall_ms, data = one_shot(corpus, n_files)
    c = data["counts"]
    if (c["files_walked"] != n_files + WALK_EXTRA or c["incremental"]
            or c["ingested"] != n_files + WALK_EXTRA):
        raise SystemExit(f"INTEGRITY: cold build wrong: {c}")
    setup["fszero_cold_index_ms"] = round(data["total_ms"], 1)
    setup["fszero_cold_oneshot_wall_ms"] = round(wall_ms, 1)

    return corpus, setup


def phase_change_detection(
    corpus: Path, n_files: int, all_files: list[str],
    wm: WatchmanClient, watch: str, trials: int, k: int,
) -> dict:
    out: dict = {"trials": []}
    # Files touched earlier in this run: FSEvents for them may still be in
    # flight when a later since-window opens. Their reappearance is correct
    # watchman behavior, not noise; only files we never touched are strays.
    touched_so_far: set[str] = set()
    # Detection-only floor: warm one-shot with zero dirty files.
    walls = []
    for _ in range(MIN_MEASURED_RUNS):
        wall_ms, data = one_shot(corpus, n_files)
        check_counts(data, n_files, 0, True, "no-change one-shot")
        walls.append(wall_ms)
    out["fszero_nochange_oneshot_wall_ms"] = round(median(walls), 1)
    out["fszero_nochange_oneshot_wall_samples_ms"] = walls
    out["fszero_nochange_oneshot_wall_percentiles_ms"] = percentiles(walls)

    for t in range(trials):
        rng = random.Random(1000 * n_files + t)
        rels = rng.sample(all_files, k)
        expected = set(rels)
        trial: dict = {"files": rels}

        # -- git: cold-ish then warm status
        touch_files(corpus, rels, f"bkg_{n_files}_{t}")
        cold_ms, cold_out = timed_git_status(corpus)
        verify_git_status(cold_out, expected, f"cold t={t}")
        warm_ms, warm_out = timed_git_status(corpus)
        verify_git_status(warm_out, expected, f"warm t={t}")
        trial["git_cold_ms"] = cold_ms
        trial["git_warm_ms"] = warm_ms

        # -- watchman: event latency (poll until all K), then settled query wall
        clk = wm.cmd("clock", watch)["clock"]
        touch_files(corpus, rels, f"bkw_{n_files}_{t}")
        t0 = time.perf_counter()
        lat_ms = None
        while time.perf_counter() - t0 < POLL_TIMEOUT_S:
            names = {f["name"] for f in wm_query(wm, watch, clk)}
            if expected <= names:
                lat_ms = (time.perf_counter() - t0) * 1e3
                stray = names - expected - touched_so_far
                if stray:
                    raise SystemExit(f"INTEGRITY: watchman reported stray files: {sorted(stray)}")
                break
            time.sleep(POLL_SLEEP_S)
        if lat_ms is None:
            raise SystemExit(f"INTEGRITY: watchman never saw all {k} files in {POLL_TIMEOUT_S}s")
        trial["watchman_event_latency_ms"] = lat_ms
        q_walls = []
        for _ in range(3):
            t0 = time.perf_counter()
            names = {f["name"] for f in wm_query(wm, watch, clk)}
            q_walls.append((time.perf_counter() - t0) * 1e3)
            stray = names - expected - touched_so_far
            if not (expected <= names) or stray:
                raise SystemExit(
                    f"INTEGRITY: settled watchman query wrong: missing="
                    f"{sorted(expected - names)} stray={sorted(stray)}")
        trial["watchman_query_ms"] = median(q_walls)
        trial["watchman_query_samples_ms"] = q_walls

        # -- fszero: one-shot warm refresh applies the deltas
        touch_files(corpus, rels, f"bkf_{n_files}_{t}")
        wall_ms, data = one_shot(corpus, n_files)
        check_counts(data, n_files, k, True, f"warm t={t}")
        trial["fszero_oneshot_wall_ms"] = wall_ms
        trial["fszero_internal_refresh_ms"] = data["total_ms"]
        trial["fszero_phases_ms"] = data["index_phases_ms"]

        # Reset git baseline so the next trial's status is exactly its K files.
        run_git(corpus, "add", "-u")
        run_git(corpus, "commit", "-qm", f"trial {t}")
        touched_so_far |= expected
        out["trials"].append(trial)
        print(f"  detect t={t}: git {cold_ms:.0f}/{warm_ms:.0f}ms  "
              f"wm {trial['watchman_event_latency_ms']}ms/{trial['watchman_query_ms']}ms  "
              f"fz {wall_ms:.0f}ms (int {data['total_ms']:.0f}ms)", flush=True)

    ts = out["trials"]
    latency_fields = (
        "git_cold_ms", "git_warm_ms", "watchman_event_latency_ms",
        "watchman_query_ms", "fszero_oneshot_wall_ms", "fszero_internal_refresh_ms",
    )
    out["medians"] = {field: median([x[field] for x in ts]) for field in latency_fields}
    out["percentiles"] = {
        field: percentiles([x[field] for x in ts]) for field in latency_fields
    }
    return out


def phase_watch_latency(
    corpus: Path, n_files: int, all_files: list[str], trials: int, k: int,
) -> tuple[dict, FszeroServer]:
    """End-to-end watch latency through the long-lived server. Returns the
    (still running) server for the fidelity phase."""
    srv = FszeroServer(corpus, n_files, watch=True)
    # Warm-up plan + verify the watcher is actually ACTIVE, not the per-op
    # stat-scan fallback (which would make us publish a lie): the watch/feed
    # store key is only ever written by the watch-drain apply path, so a
    # sentinel touch must show up in the feed. The feed is empty at this
    # point, so the plan-level expand returns it inline.
    srv.search_hits("bakeoff_warmup_query")
    sentinel = all_files[0]
    touch_files(corpus, [sentinel], f"bks_{n_files}")
    deadline = time.perf_counter() + POLL_TIMEOUT_S
    watch_proven = False
    while time.perf_counter() < deadline:
        srv.search_hits(f"bks_{n_files}")  # op boundary drives the drain
        feed = srv.expand_json("watch/feed")
        if feed.get("last_seq", 0) >= 1:
            watch_proven = True
            break
        time.sleep(POLL_SLEEP_S)
    if not watch_proven:
        raise SystemExit(
            "INTEGRITY: watch/feed never updated after sentinel touch — the "
            "server is running on the stat-scan fallback, not FSEvents; refusing "
            "to publish stat-scan numbers as watch latency")

    out: dict = {"trials": [], "poll_sleep_ms": POLL_SLEEP_S * 1e3}
    # Poll overhead floor: no-change search plan round-trip.
    poll_walls = []
    for _ in range(MIN_MEASURED_RUNS):
        t0 = time.perf_counter()
        srv.search_hits("bakeoff_absent_marker")
        poll_walls.append((time.perf_counter() - t0) * 1e3)
    out["fszero_poll_wall_ms"] = round(median(poll_walls), 2)
    out["fszero_poll_wall_samples_ms"] = poll_walls
    out["fszero_poll_wall_percentiles_ms"] = percentiles(poll_walls)

    for t in range(trials):
        rng = random.Random(2000 * n_files + t)
        rels = rng.sample(all_files, k)
        marker = f"bkl_{n_files}_{t}"
        touch_files(corpus, rels, marker)
        t0 = time.perf_counter()
        lat_ms = None
        while time.perf_counter() - t0 < POLL_TIMEOUT_S:
            hits = srv.search_hits(marker)
            if hits >= k:
                lat_ms = (time.perf_counter() - t0) * 1e3
                if hits != k:
                    raise SystemExit(f"INTEGRITY: fszero saw {hits} hits, expected {k}")
                break
            time.sleep(POLL_SLEEP_S)
        if lat_ms is None:
            raise SystemExit(f"INTEGRITY: fszero watch never saw all {k} files in {POLL_TIMEOUT_S}s")
        out["trials"].append({"files": rels, "fszero_e2e_ms": lat_ms})
        print(f"  watch t={t}: fszero e2e {lat_ms:.1f}ms", flush=True)

    e2e_values = [x["fszero_e2e_ms"] for x in out["trials"]]
    out["medians"] = {"fszero_e2e_ms": median(e2e_values)}
    out["percentiles"] = {"fszero_e2e_ms": percentiles(e2e_values)}
    return out, srv


def feed_events_after(
    corpus: Path, srv: FszeroServer, cursor: int, want: set[tuple[str, str]],
) -> list[dict]:
    """Poll watch/feed until every (kind, file) in `want` appears past cursor.
    Plan executions on `srv` drive the drains; the feed itself is read through
    the per-op oracle because it exceeds the CodeMode inline payload limit."""
    deadline = time.perf_counter() + POLL_TIMEOUT_S
    evs: list[dict] = []
    while time.perf_counter() < deadline:
        srv.search_hits("bakeoff_drain_pump")  # op boundary drives the drain
        feed = oracle_expand_json(corpus, "watch/feed")
        evs = [e for e in feed.get("events", []) if e["seq"] > cursor]
        got = {(e["kind"], e["file"]) for e in evs}
        if want <= got:
            return evs
        time.sleep(0.1)
    return evs


def feed_cursor(corpus: Path, srv: FszeroServer) -> int:
    srv.search_hits("bakeoff_drain_pump")  # apply anything pending first
    return oracle_expand_json(corpus, "watch/feed").get("last_seq", 0)


def phase_fidelity(
    corpus: Path, wm: WatchmanClient, watch: str, srv: FszeroServer,
) -> list[dict]:
    """create / modify / delete / rename: observed classification per system."""
    results: list[dict] = []
    subject = "mod_000/sub_000/f_000.rs"

    def observe(op: str, expected: dict, git_expect_lines: set[str],
                wm_expect: dict[str, tuple[bool, bool]], fz_expect: set[tuple[str, str]],
                clk: str, cursor: int) -> None:
        # git (no strip: porcelain status codes include a leading space)
        git_out = run_git(corpus, "status", "--porcelain")
        git_lines = {line for line in git_out.splitlines() if line}
        # watchman: poll until every expected path appears; compare on the
        # expected paths (earlier touches may still flush into this window).
        wm_got: dict[str, tuple[bool, bool]] = {}
        deadline = time.perf_counter() + POLL_TIMEOUT_S
        while time.perf_counter() < deadline:
            files = wm_query(wm, watch, clk)
            wm_got = {f["name"]: (f["exists"], f.get("new", False)) for f in files
                      if f["name"] in wm_expect}
            if set(wm_expect) <= set(wm_got):
                break
            time.sleep(POLL_SLEEP_S)
        # fszero feed (compare on the expected files; earlier-touch events may
        # still flush into the window, same tolerance as watchman above)
        evs = feed_events_after(corpus, srv, cursor, fz_expect)
        fz_files = {f for _, f in fz_expect}
        fz_got = {(e["kind"], e["file"]) for e in evs if e["file"] in fz_files}
        results.append({
            "op": op,
            "expected": expected,
            "git": {"observed": sorted(git_lines), "match": git_lines == git_expect_lines},
            "watchman": {
                "observed": {k: {"exists": v[0], "new": v[1]} for k, v in sorted(wm_got.items())},
                "match": wm_got == wm_expect,
            },
            "fszero_feed": {"observed": sorted(f"{k}:{f}" for k, f in fz_got),
                            "match": fz_got == fz_expect},
        })
        # reset git baseline for the next op
        run_git(corpus, "add", "-A")
        run_git(corpus, "commit", "-qm", f"fidelity {op}")
        time.sleep(0.3)  # let stray events settle before the next op

    # CREATE
    clk = wm.cmd("clock", watch)["clock"]
    cur = feed_cursor(corpus, srv)
    new_rel = "mod_000/sub_000/zz_bakeoff_new.rs"
    (corpus / new_rel).write_text("pub fn bakeoff_created() { let created_marker = 1; }\n")
    observe("create", {"desc": "new file appears as a creation"},
            {f"?? {new_rel}"}, {new_rel: (True, True)}, {("changed", new_rel)}, clk, cur)

    # MODIFY
    clk = wm.cmd("clock", watch)["clock"]
    cur = feed_cursor(corpus, srv)
    with (corpus / subject).open("a") as f:
        f.write("pub fn bakeoff_modified() { let modified_marker = 1; }\n")
    observe("modify", {"desc": "existing file appears as a modification"},
            {f" M {subject}"}, {subject: (True, False)}, {("changed", subject)}, clk, cur)

    # DELETE
    clk = wm.cmd("clock", watch)["clock"]
    cur = feed_cursor(corpus, srv)
    (corpus / new_rel).unlink()
    observe("delete", {"desc": "deleted file appears as a deletion"},
            {f" D {new_rel}"}, {new_rel: (False, False)}, {("removed", new_rel)}, clk, cur)

    # RENAME (plain filesystem rename, unstaged)
    clk = wm.cmd("clock", watch)["clock"]
    cur = feed_cursor(corpus, srv)
    renamed = "mod_000/sub_000/zz_bakeoff_renamed.rs"
    (corpus / subject).rename(corpus / renamed)
    observe("rename", {"desc": "old path removed + new path created (no system "
                               "reports a first-class rename for unstaged worktree moves)"},
            {f" D {subject}", f"?? {renamed}"},
            {subject: (False, False), renamed: (True, True)},
            {("removed", subject), ("changed", renamed)}, clk, cur)

    return results


# ---------------------------------------------------------------- report


def render_markdown(result: dict) -> str:
    p = result["provenance"]
    k, trials = p["k"], p["trials"]
    lines = [
        "# Watch bake-off: FSZero vs watchman vs git status (fszero-0l1)",
        "",
        "Generated by `benchmarks/watch_bakeoff.py` — do not hand-edit numbers.",
        "Named competitors, identical corpus/touches/machine, losses published.",
        f"Hardware: {p['hardware']}. Benchmark commit: `{p['git_commit'][:12]}`"
        f" (dirty={str(p['git_dirty']).lower()}). Date: {p['date']}.",
        f"Versions: {p['versions']['git']}; watchman {p['versions']['watchman']};"
        f" fszero release bin sha256[:16] `{p['versions']['fszero_bin_sha256_16']}`.",
        f"Corpora: deterministic synthetic rust trees (`gen_corpus.py`, seed 42);"
        f" K={k} seeded touched files per trial, {trials} trials, medians reported."
        " Every observation integrity-checked to be exactly the touched set.",
        "",
        "## 1. Change detection after K touched files (median ms)",
        "",
        "| files | git status (cold-ish) | git status (warm) | watchman since-query"
        " (settled) | fszero one-shot, no change | fszero one-shot, K dirty |"
        " fszero internal refresh |",
        "| --: | --: | --: | --: | --: | --: | --: |",
    ]
    for s in result["sizes"]:
        m = s["change_detection"]["medians"]
        lines.append(
            f"| {s['files']} | {m['git_cold_ms']:.0f} | {m['git_warm_ms']:.0f} |"
            f" {m['watchman_query_ms']:.1f} |"
            f" {s['change_detection']['fszero_nochange_oneshot_wall_ms']:.0f} |"
            f" {m['fszero_oneshot_wall_ms']:.0f} | {m['fszero_internal_refresh_ms']:.0f} |")
    lines += [
        "",
        "What each column buys you (not the same work):",
        "- git status: detection only (lstat crawl + hash of changed files); cold-ish"
        " = first run after the touches, warm = second run on the refreshed index.",
        "- watchman settled since-query: detection only, from an already-running"
        " daemon over a persistent socket, cookie-synced (guaranteed up to date).",
        "- fszero one-shot: process spawn + store open + walk + manifest sig-diff"
        " **+ full index update of the K dirty files (parse, ingest, store txn) +"
        " searcher rebuild**. The no-change column is the detection-only floor;"
        " the K-dirty column also buys an up-to-date code index, which git and"
        " watchman do not produce.",
        "",
        "## 2. Watch event latency, end-to-end (touch K files → all K visible)",
        "",
        "| files | watchman poll-until-K | fszero watch server poll-until-K |"
        " fszero poll round-trip (floor) |",
        "| --: | --: | --: | --: |",
    ]
    for s in result["sizes"]:
        wm_med = s["change_detection"]["medians"]["watchman_event_latency_ms"]
        wl = s["watch_latency"]
        lines.append(
            f"| {s['files']} | {wm_med:.1f} | {wl['medians']['fszero_e2e_ms']:.1f} |"
            f" {wl['fszero_poll_wall_ms']:.1f} |")
    lines += [
        "",
        f"Both systems measured with the same poll loop (persistent connection,"
        f" {POLL_SLEEP_S * 1e3:.0f}ms sleep between polls): time from last touch until a"
        " query/search returns all K files. Includes FSEvents delivery, daemon/server"
        " processing, and poll quantization. FSZero additionally reindexes each file"
        " before it is visible (watchman only records the change). In-process drain"
        " apply cost is measured separately by the committed gate"
        " `per_save_index_cost_under_1ms_p50` (tests/watch_mode.rs): p50 713us release.",
        "",
        "## 3. Event fidelity (create / modify / delete / rename)",
        "",
        "| op | git status | watchman | fszero watch feed |",
        "| :-- | :-- | :-- | :-- |",
    ]
    for f in result["fidelity"]:
        def cell(d: dict) -> str:
            mark = "correct" if d["match"] else "MISMATCH"
            obs = d["observed"]
            if isinstance(obs, dict):
                obs = "; ".join(f"{k} exists={v['exists']} new={v['new']}" for k, v in obs.items())
            else:
                obs = "; ".join(obs)
            return f"{mark}: `{obs}`"
        lines.append(f"| {f['op']} | {cell(f['git'])} | {cell(f['watchman'])} |"
                     f" {cell(f['fszero_feed'])} |")
    lines += [
        "",
        "Expected classifications: create→new, modify→change, delete→removal,"
        " rename→remove(old)+create(new). No system (git worktree status, watchman"
        " since-queries, fszero feed) reports unstaged renames first-class; all three"
        " report the remove+create pair. Fidelity ran on the"
        f" {result['sizes'][0]['files']}-file corpus.",
        "",
        "## Wins and losses (honest reading)",
        "",
    ]
    lines += result["verdicts"]
    lines += [
        "",
        "## Exclusions",
        "",
        "- git `core.fsmonitor` (builtin fsmonitor daemon) not enabled: measured"
        " stock `git status` defaults. With fsmonitor, warm status would close much"
        " of the crawl gap; not measured here.",
        "- watchman subscriptions (push) not measured: polling used for both systems"
        " so the loops are symmetric; push latency would likely be lower for"
        " watchman than the poll numbers shown.",
        "- fszero in-process apply latency (713us p50) is a committed test gate,"
        " not re-measured here; the end-to-end numbers above include FSEvents"
        " delivery + JSON-RPC poll round-trips on top of it.",
        "",
        "Reproduce: `python3 benchmarks/watch_bakeoff.py` (requires the release-perf"
        " binary `./scripts/profile_build.sh -p fs-zero --bin fszero`, git, and watchman). Raw trials in"
        " `watch-bakeoff.json`.",
        "",
    ]
    return "\n".join(lines)


def verdicts(result: dict) -> list[str]:
    out: list[str] = []
    for s in result["sizes"]:
        n = s["files"]
        m = s["change_detection"]["medians"]
        nochange = s["change_detection"]["fszero_nochange_oneshot_wall_ms"]
        fz_e2e = s["watch_latency"]["medians"]["fszero_e2e_ms"]
        wm_lat = m["watchman_event_latency_ms"]
        # one-shot vs git
        for git_col, git_ms in (("cold-ish", m["git_cold_ms"]), ("warm", m["git_warm_ms"])):
            fz = m["fszero_oneshot_wall_ms"]
            if fz > git_ms:
                out.append(
                    f"- **LOSS ({n} files)**: fszero one-shot ({fz:.0f}ms) is"
                    f" {fz / git_ms:.1f}x slower than {git_col} `git status`"
                    f" ({git_ms:.0f}ms) as a raw change-detection CLI — even though the"
                    " one-shot also rebuilds the index for the changed files, the"
                    " CLI-shaped comparison is a loss.")
            else:
                out.append(
                    f"- **WIN ({n} files)**: fszero one-shot ({fz:.0f}ms) beats {git_col}"
                    f" `git status` ({git_ms:.0f}ms) while also updating the code index.")
        # detection floor vs git warm
        if nochange > m["git_warm_ms"]:
            out.append(
                f"- **LOSS ({n} files)**: fszero's detection-only floor (no-change"
                f" one-shot, {nochange:.0f}ms) is {nochange / m['git_warm_ms']:.1f}x"
                f" slower than warm `git status` ({m['git_warm_ms']:.0f}ms): process"
                " spawn + store open + full walk dominate.")
        # settled query vs anything
        if m["watchman_query_ms"] < min(m["git_warm_ms"], nochange):
            out.append(
                f"- **LOSS to watchman ({n} files)**: a settled watchman since-query"
                f" ({m['watchman_query_ms']:.1f}ms) beats both git warm status"
                f" ({m['git_warm_ms']:.0f}ms) and fszero's one-shot floor"
                f" ({nochange:.0f}ms). A long-lived daemon with an in-memory view wins"
                " the pure detection race; fszero only matches this shape in server"
                " mode (see event latency).")
        # event latency
        if fz_e2e <= wm_lat:
            out.append(
                f"- **WIN ({n} files)**: end-to-end watch latency {fz_e2e:.1f}ms"
                f" (fszero server, includes reindex) vs {wm_lat:.1f}ms (watchman,"
                " detection only) under the identical poll loop.")
        else:
            out.append(
                f"- **LOSS ({n} files)**: end-to-end watch latency {fz_e2e:.1f}ms"
                f" (fszero server) vs {wm_lat:.1f}ms (watchman) under the identical"
                " poll loop — watchman reports the change faster; fszero is also"
                " reindexing before it answers.")
    fid = result["fidelity"]
    bad = [f["op"] for f in fid
           if not (f["git"]["match"] and f["watchman"]["match"] and f["fszero_feed"]["match"])]
    if bad:
        out.append(f"- **Fidelity mismatches** on: {', '.join(bad)} (see table 3).")
    else:
        out.append("- Fidelity: all three systems classified create/modify/delete/rename"
                   " as expected (rename = remove+create pair everywhere).")
    return out


# ---------------------------------------------------------------- main


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--sizes", default=",".join(map(str, DEFAULT_SIZES)))
    ap.add_argument("--trials", type=int, default=MIN_MEASURED_RUNS)
    ap.add_argument("--k", type=int, default=10)
    args = ap.parse_args()
    if args.trials < MIN_MEASURED_RUNS:
        ap.error(f"--trials must be at least {MIN_MEASURED_RUNS}")
    sizes = [int(s) for s in args.sizes.split(",")]

    result: dict = {
        "provenance": {
            "hardware": hardware(),
            "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            **git_provenance(),
            "versions": versions(),
            "seed": 42,
            "k": args.k,
            "trials": args.trials,
            "poll_sleep_ms": POLL_SLEEP_S * 1e3,
        },
        "statistical_profile": {
            "minimum_measured_runs": MIN_MEASURED_RUNS,
            "warmup_policy": "phase-specific unmeasured priming; no measured trial excluded",
            "percentile_method": "linear interpolation over ordered measured samples",
            "outlier_policy": "none; retain every ordered raw trial",
            "sample_size_exceptions": [
                {
                    "metric": "sizes[].setup cold-index timings",
                    "sample_size_exception": True,
                    "sample_count": 1,
                    "reason": "setup validation is reported but is not a comparison or gate",
                    "conservative_tail": True,
                    "tail_status": "unresolved; the single observation is the maximum",
                },
            ],
        },
        "sizes": [],
        "fidelity": [],
    }

    for size_idx, n in enumerate(sizes):
        with tempfile.TemporaryDirectory(prefix=f"fszero_bakeoff_{n}_") as tmp:
            corpus, setup = setup_corpus(Path(tmp), n)
            all_files = sorted(
                str(f.relative_to(corpus)) for f in corpus.rglob("*.rs")
                if not any(part.startswith(".") for part in f.relative_to(corpus).parts))
            if len(all_files) != n:
                raise SystemExit(f"INTEGRITY: corpus has {len(all_files)} .rs files != {n}")

            wm = WatchmanClient()
            watch = wm.cmd("watch-project", str(corpus))["watch"]
            if Path(watch).resolve() != corpus.resolve():
                raise SystemExit(f"INTEGRITY: watchman watch root {watch} != corpus")
            t0 = time.perf_counter()
            crawl = wm.cmd("query", watch, {"expression": ["allof", ["type", "f"],
                           ["suffix", "rs"]], "fields": ["name"]})
            setup["watchman_initial_crawl_ms"] = round((time.perf_counter() - t0) * 1e3, 1)
            if len(crawl.get("files", [])) != n:
                raise SystemExit(
                    f"INTEGRITY: watchman crawl saw {len(crawl.get('files', []))} != {n}")

            entry: dict = {"files": n, "setup": setup}
            entry["change_detection"] = phase_change_detection(
                corpus, n, all_files, wm, watch, args.trials, args.k)
            entry["watch_latency"], srv = phase_watch_latency(
                corpus, n, all_files, args.trials, args.k)
            if size_idx == 0:
                # settle + clean git baseline before fidelity ops
                run_git(corpus, "add", "-A")
                run_git(corpus, "commit", "-qm", "pre-fidelity")
                time.sleep(0.5)
                result["fidelity"] = phase_fidelity(corpus, wm, watch, srv)
            srv.close()
            wm.cmd("watch-del", watch)
            result["sizes"].append(entry)

    result["verdicts"] = verdicts(result)

    out_json = ROOT / "benchmarks" / "watch-bakeoff.json"
    out_json.write_text(json.dumps(result, indent=2) + "\n")
    out_md = ROOT / "benchmarks" / "watch-bakeoff.md"
    out_md.write_text(render_markdown(result))
    print(f"\nwritten: {out_json}\nwritten: {out_md}")


if __name__ == "__main__":
    main()
