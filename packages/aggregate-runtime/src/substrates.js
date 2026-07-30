import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";

import { GIT } from "./paths.js";

export function platformBinaryRel(binaryRel, platform = process.platform) {
  return platform === "win32" ? `${binaryRel}.exe` : binaryRel;
}

export function originMainBackendConfigs(sourceRepoRoot) {
  return {
    fs: {
      ns: "fz",
      envKey: "ZERO_FSZERO_BIN",
      sourceRepo: path.join(sourceRepoRoot, "FSZero"),
      checkoutName: "fszero",
      binaryRel: "target/release/fszero-codemode",
      rawBinaryRel: "target/release/fszero-codemode",
      buildCommand: "cargo",
      buildArgs: ["build", "--release", "-p", "fszero-codemode", "--bin", "fszero-codemode", "-j", "1"],
    },
    graph: {
      ns: "gz",
      envKey: "ZERO_GRAPHZERO_BIN",
      sourceRepo: path.join(sourceRepoRoot, "GraphZero"),
      checkoutName: "graphzero",
      binaryRel: "target/release/graphzero-codemode",
      rawEnvKey: "ZERO_GRAPHZERO_RAW_BIN",
      rawBinaryRel: "target/release/gz-raw-worker",
      rawBuildArgs: ["build", "--release", "-p", "graphzero-query", "--bin", "gz-raw-worker", "-j", "1"],
      buildCommand: "cargo",
      buildArgs: ["build", "--release", "-p", "graphzero", "--bin", "graphzero-codemode", "--no-default-features", "--features", "tokenzero,surface-codemode", "-j", "1"],
    },
    token: {
      ns: "tz",
      envKey: "ZERO_TOKENZERO_BIN",
      sourceRepo: path.join(sourceRepoRoot, "TokenZero"),
      checkoutName: "TokenZero",
      binaryRel: "target/release/tokenzero-codemode",
      rawBinaryRel: "target/release/tokenzero-codemode",
      buildCommand: "cargo",
      buildArgs: ["build", "--release", "-p", "tokenzero", "--bin", "tokenzero-codemode", "--no-default-features", "--features", "surface-codemode", "-j", "1"],
    },
  };
}
export function refreshLockActive(lockPath, {
  kill = process.kill,
  now = Date.now(),
  maxAgeMs = 960_000,
  ownerGraceMs = 30_000,
} = {}) {
  if (!fs.existsSync(lockPath)) return false;
  let owner;
  let startedAt = 0;
  try {
    owner = JSON.parse(fs.readFileSync(path.join(lockPath, "owner.json"), "utf8"));
    startedAt = Date.parse(owner.startedAt) || 0;
  } catch {}
  if (!startedAt) {
    try { startedAt = fs.statSync(lockPath).mtimeMs; } catch {}
  }
  const ageMs = startedAt ? Math.max(0, now - startedAt) : Number.POSITIVE_INFINITY;
  let active = ageMs <= maxAgeMs;
  if (active && Number.isInteger(owner?.pid) && owner.pid > 0) {
    try {
      kill(owner.pid, 0);
      return true;
    } catch (err) {
      if (err?.code === "EPERM") return true;
      active = false;
    }
  } else if (active && ageMs <= ownerGraceMs) {
    return true;
  } else {
    active = false;
  }
  if (!active) {
    try { fs.rmSync(lockPath, { recursive: true, force: true }); } catch {}
  }
  return active;
}

export function createSubstrateHelpers() {
  // Binary paths resolve lazily with env overrides. By default, CodeMode substrates run
  // from private origin/main checkouts under ~/.pi/agent so Pi picks up backend fixes after
  // they are pushed, without clobbering the developer's editable source checkouts.
  const ORIGIN_MAIN_ROOT = process.env.ZERO_BACKEND_ORIGIN_MAIN_ROOT || path.join(os.homedir(), ".pi", "agent", "zerostack-origin-main");
  const SOURCE_REPO_ROOT = process.env.ZERO_SOURCE_REPO_ROOT || path.join(os.homedir(), "AI");
  const ORIGIN_MAIN_REFRESH_MS = Math.max(0, Number(process.env.ZERO_BACKEND_ORIGIN_MAIN_REFRESH_MS || 300_000));
  const ORIGIN_MAIN_BUILD_TIMEOUT_MS = Math.max(1_000, Number(process.env.ZERO_BACKEND_ORIGIN_MAIN_BUILD_TIMEOUT_MS || 3_600_000));
  const ORIGIN_MAIN_BUILD_ENABLED = process.env.ZERO_BACKEND_ORIGIN_MAIN_BUILD !== "0";
  const ORIGIN_MAIN_BUILD_LOCK = process.env.ZERO_BACKEND_ORIGIN_MAIN_BUILD_LOCK
    || path.join(os.tmpdir(), "pi-zerostack-cargo-build.lock");
  const BACKEND_CONFIG_PATH = process.env.ZERO_BACKEND_CONFIG
    || path.join(os.homedir(), ".pi", "agent", "zerostack", "backends.json");
  const REBUILD_INSTRUCTION = ORIGIN_MAIN_BUILD_ENABLED
    ? "Automatic origin/main rebuild failed; inspect the backend status error, then restart Pi to retry."
    : "Remove ZERO_BACKEND_ORIGIN_MAIN_BUILD=0 and restart Pi to enable automatic serial origin/main rebuilds.";
  const ORIGIN_MAIN_REFRESH_STATE = new Map();
  const ORIGIN_MAIN_STATUS = new Map();

  function stampPathForCheckout(checkoutPath) {
    return path.join(checkoutPath, ".pi-origin-main-build.json");
  }

  function originMainCheckoutPaths(config) {
    const checkout = path.join(ORIGIN_MAIN_ROOT, config.checkoutName);
    return {
      checkout,
      binary: path.join(checkout, platformBinaryRel(config.binaryRel)),
      rawBinary: path.join(checkout, platformBinaryRel(config.rawBinaryRel || config.binaryRel)),
      stamp: stampPathForCheckout(checkout),
      refreshStamp: path.join(checkout, ".pi-origin-main-refresh.json"),
      lock: `${checkout}.lock`,
    };
  }

  function spawnOriginMainRefresh(config, paths) {
    const worker = String.raw`
  import fs from "node:fs";
  import os from "node:os";
  import path from "node:path";
  import { spawnSync } from "node:child_process";
  const cfg = JSON.parse(process.env.ZERO_REFRESH_CONFIG || "{}");
  const timeout = Number(process.env.ZERO_REFRESH_TIMEOUT_MS || 900000);
  const sleepCell = new Int32Array(new SharedArrayBuffer(4));
  function run(command, args, opts = {}) {
    const res = spawnSync(command, args, {
      cwd: opts.cwd,
      encoding: "utf8",
      env: opts.env || process.env,
      timeout: opts.timeout || timeout,
      maxBuffer: 8 * 1024 * 1024,
    });
    if (res.error) throw res.error;
    if (res.status !== 0) throw new Error(command + " " + args.join(" ") + " failed (" + res.status + "): " + String(res.stderr || res.stdout || "").slice(0, 1200));
    return String(res.stdout || "").trim();
  }
  function rmLock(lockPath) {
    try { fs.rmSync(lockPath, { recursive: true, force: true }); } catch {}
  }
  function lockIsActive(lockPath) {
    let owner;
    let startedAt = 0;
    try {
      owner = JSON.parse(fs.readFileSync(path.join(lockPath, "owner.json"), "utf8"));
      startedAt = Date.parse(owner.startedAt) || 0;
    } catch {}
    if (!startedAt) {
      try { startedAt = fs.statSync(lockPath).mtimeMs; } catch {}
    }
    if (!startedAt || Date.now() - startedAt > timeout + 60000) return false;
    if (!Number.isInteger(owner?.pid) || owner.pid <= 0) return true;
    try {
      process.kill(owner.pid, 0);
      return true;
    } catch (err) {
      return err?.code === "EPERM";
    }
  }
  function acquireBuildLock() {
    const deadline = Date.now() + timeout;
    for (;;) {
      try {
        fs.mkdirSync(cfg.buildLockPath, { recursive: false });
        fs.writeFileSync(path.join(cfg.buildLockPath, "owner.json"), JSON.stringify({
          pid: process.pid,
          ns: cfg.ns,
          startedAt: new Date().toISOString(),
        }));
        return;
      } catch (err) {
        if (err?.code !== "EEXIST") throw err;
        if (!lockIsActive(cfg.buildLockPath)) {
          rmLock(cfg.buildLockPath);
          continue;
        }
        if (Date.now() >= deadline) throw new Error("timed out waiting for machine-wide Rust build lock " + cfg.buildLockPath);
        Atomics.wait(sleepCell, 0, 0, 100);
      }
    }
  }
  function readJson(filePath) {
    try { return JSON.parse(fs.readFileSync(filePath, "utf8")); } catch { return null; }
  }
  function writeJson(filePath, value) {
    fs.writeFileSync(filePath, JSON.stringify(value, null, 2));
  }
  let ownsBuildLock = false;
  try {
    fs.mkdirSync(cfg.root, { recursive: true });
    fs.mkdirSync(cfg.lockPath, { recursive: false });
    fs.writeFileSync(path.join(cfg.lockPath, "owner.json"), JSON.stringify({ pid: process.pid, startedAt: new Date().toISOString() }));
    const remote = run(cfg.git, ["-C", cfg.sourceRepo, "remote", "get-url", "origin"], { timeout: 15000 });
    if (!fs.existsSync(path.join(cfg.checkout, ".git"))) {
      // Non-destructive bootstrap: clone into a temp dir and swap in only on success,
      // preserving any pre-existing target/ (e.g. locally provisioned binaries) so a
      // failed clone (offline, missing auth) never destroys a working checkout.
      const tmp = cfg.checkout + ".clone-tmp";
      fs.rmSync(tmp, { recursive: true, force: true });
      run(cfg.git, ["clone", "--branch", "main", "--single-branch", remote, tmp], { cwd: cfg.root, timeout: 300000 });
      if (fs.existsSync(cfg.checkout)) {
        const oldTarget = path.join(cfg.checkout, "target");
        if (fs.existsSync(oldTarget) && !fs.existsSync(path.join(tmp, "target"))) fs.renameSync(oldTarget, path.join(tmp, "target"));
        fs.rmSync(cfg.checkout, { recursive: true, force: true });
      }
      fs.renameSync(tmp, cfg.checkout);
    } else {
      run(cfg.git, ["-C", cfg.checkout, "remote", "set-url", "origin", remote], { timeout: 15000 });
      run(cfg.git, ["-C", cfg.checkout, "fetch", "--prune", "origin", "main"], { timeout: 180000 });
      run(cfg.git, ["-C", cfg.checkout, "reset", "--hard", "origin/main"], { timeout: 60000 });
      run(cfg.git, ["-C", cfg.checkout, "clean", "-fdx", "-e", "target", "-e", ".pi-origin-main-build.json", "-e", ".pi-origin-main-refresh.json"], { timeout: 60000 });
    }
    const rev = run(cfg.git, ["-C", cfg.checkout, "rev-parse", "origin/main"], { timeout: 15000 });
    let stamp = readJson(cfg.stampPath);
    let binaryPresent = fs.existsSync(cfg.binaryPath);
    let rawBinaryPresent = fs.existsSync(cfg.rawBinaryPath);
    let fresh = binaryPresent && rawBinaryPresent && stamp?.rev === rev && stamp?.binaryRel === cfg.binaryRel && stamp?.rawBinaryRel === cfg.rawBinaryRel;
    let built = false;
    if (!fresh && cfg.buildEnabled) {
      acquireBuildLock();
      ownsBuildLock = true;
      try { os.setPriority(0, 10); } catch {}
      run(cfg.buildCommand, cfg.buildArgs, {
        cwd: cfg.checkout,
        timeout,
        env: { ...process.env, CARGO_BUILD_JOBS: "1" },
      });
      if (Array.isArray(cfg.rawBuildArgs) && cfg.rawBuildArgs.length > 0) {
        run(cfg.buildCommand, cfg.rawBuildArgs, {
          cwd: cfg.checkout,
          timeout,
          env: { ...process.env, CARGO_BUILD_JOBS: "1" },
        });
      }
      built = true;
      binaryPresent = fs.existsSync(cfg.binaryPath);
      rawBinaryPresent = fs.existsSync(cfg.rawBinaryPath);
      if (!binaryPresent) throw new Error("Rust build completed without producing " + cfg.binaryPath);
      if (!rawBinaryPresent) throw new Error("Rust build completed without producing " + cfg.rawBinaryPath);
      stamp = {
        rev,
        binaryRel: cfg.binaryRel,
        rawBinaryRel: cfg.rawBinaryRel,
        builtAt: new Date().toISOString(),
        build: [cfg.buildCommand, ...cfg.buildArgs],
        cargoBuildJobs: 1,
        machineWideLock: cfg.buildLockPath,
        priority: "low",
        async: true,
      };
      writeJson(cfg.stampPath, stamp);
      fresh = true;
    }
    const refreshedAt = new Date().toISOString();
    writeJson(cfg.refreshStampPath, { rev, binaryRel: cfg.binaryRel, rawBinaryRel: cfg.rawBinaryRel, refreshedAt, built, buildEnabled: cfg.buildEnabled });
    const mode = fresh ? "origin/main-ready" : binaryPresent ? "origin/main-stale" : "origin/main-missing";
    writeJson(cfg.statusPath, {
      mode,
      ns: cfg.ns,
      checkout: cfg.checkout,
      binary: binaryPresent ? cfg.binaryPath : null,
      rawBinary: rawBinaryPresent ? cfg.rawBinaryPath : null,
      rev,
      refreshedAt,
      built,
      buildEnabled: cfg.buildEnabled,
      buildRequired: !fresh,
      ...(fresh ? {} : { rebuildInstruction: cfg.rebuildInstruction }),
    });
  } catch (err) {
    try {
      writeJson(cfg.statusPath, {
        mode: "refresh-failed",
        ns: cfg.ns,
        checkout: cfg.checkout,
        binary: fs.existsSync(cfg.binaryPath) ? cfg.binaryPath : null,
        error: String(err.message || err),
        refreshedAt: new Date().toISOString(),
        buildEnabled: cfg.buildEnabled,
        rebuildInstruction: cfg.rebuildInstruction,
      });
    } catch {}
    process.exitCode = 1;
  } finally {
    if (ownsBuildLock) rmLock(cfg.buildLockPath);
    rmLock(cfg.lockPath);
  }`;
    const statusPath = path.join(ORIGIN_MAIN_ROOT, `${config.checkoutName}.status.json`);
    const child = spawn(process.execPath, ["--input-type=module", "-e", worker], {
      detached: true,
      stdio: "ignore",
      env: {
        ...process.env,
        ZERO_REFRESH_TIMEOUT_MS: String(ORIGIN_MAIN_BUILD_TIMEOUT_MS),
        ZERO_REFRESH_CONFIG: JSON.stringify({
          ns: config.ns,
          root: ORIGIN_MAIN_ROOT,
          sourceRepo: config.sourceRepo,
          checkout: paths.checkout,
          binaryPath: paths.binary,
          binaryRel: config.binaryRel,
          rawBinaryPath: paths.rawBinary,
          rawBinaryRel: config.rawBinaryRel || config.binaryRel,
          stampPath: paths.stamp,
          refreshStampPath: paths.refreshStamp,
          statusPath,
          lockPath: paths.lock,
          buildLockPath: ORIGIN_MAIN_BUILD_LOCK,
          git: GIT,
          buildCommand: config.buildCommand,
          buildArgs: config.buildArgs,
          rawBuildArgs: config.rawBuildArgs || [],
          buildEnabled: ORIGIN_MAIN_BUILD_ENABLED,
          rebuildInstruction: REBUILD_INSTRUCTION,
        }),
      },
    });
    child.unref?.();
    return statusPath;
  }

  function readOriginMainStatusFile(config) {
    try { return JSON.parse(fs.readFileSync(path.join(ORIGIN_MAIN_ROOT, `${config.checkoutName}.status.json`), "utf8")); } catch { return null; }
  }

  function scheduleOriginMainRefresh(config, paths, selectedBinary) {
    const now = Date.now();
    const cached = ORIGIN_MAIN_REFRESH_STATE.get(config.ns);
    if (cached && now - cached.checkedAt < ORIGIN_MAIN_REFRESH_MS) return cached;
    fs.mkdirSync(ORIGIN_MAIN_ROOT, { recursive: true });
    const locked = refreshLockActive(paths.lock, { maxAgeMs: ORIGIN_MAIN_BUILD_TIMEOUT_MS + 60_000 });
    const statusFile = readOriginMainStatusFile(config);
    const binaryPresent = fs.existsSync(paths.binary);
    const mode = binaryPresent ? "origin/main-ready-refreshing" : "origin/main-missing-refreshing";
    const status = {
      mode,
      checkout: paths.checkout,
      binary: selectedBinary,
      refresh: locked ? "already-running" : "scheduled",
      previous: statusFile,
      buildEnabled: ORIGIN_MAIN_BUILD_ENABLED,
      ...(!binaryPresent ? { rebuildInstruction: REBUILD_INSTRUCTION } : {}),
    };
    ORIGIN_MAIN_STATUS.set(config.ns, status);
    ORIGIN_MAIN_REFRESH_STATE.set(config.ns, { checkedAt: now, binaryPath: selectedBinary, status });
    if (!locked) spawnOriginMainRefresh(config, paths);
    return ORIGIN_MAIN_REFRESH_STATE.get(config.ns);
  }

  function originMainBinaryFast(config) {
    const paths = originMainCheckoutPaths(config);
    const privateReady = fs.existsSync(paths.binary);
    const selectedBinary = paths.binary;
    scheduleOriginMainRefresh(config, paths, selectedBinary);
    const current = ORIGIN_MAIN_STATUS.get(config.ns) || {};
    const statusFile = readOriginMainStatusFile(config);
    ORIGIN_MAIN_STATUS.set(config.ns, {
      ...current,
      mode: current.mode || (privateReady ? "origin/main-ready" : "origin/main-missing-refreshing"),
      checkout: paths.checkout,
      binary: selectedBinary,
      latest: statusFile,
      non_blocking: true,
      buildEnabled: ORIGIN_MAIN_BUILD_ENABLED,
      ...(!privateReady ? { rebuildInstruction: REBUILD_INSTRUCTION } : {}),
    });
    return selectedBinary;
  }

  // True when a configured path names the very binary the origin/main mechanism
  // manages, so it must be refreshed rather than treated as an external override
  // the user pinned deliberately.
  //
  // Compared case-insensitively because the checkout directories are not
  // consistently cased (the tz checkout is "TokenZero" while its sibling status
  // file is "tokenzero.status.json"), and on a case-insensitive filesystem an
  // exact compare would miss and silently freeze the refresh.
  function isManagedOriginMainBinary(config, candidate) {
    const paths = originMainCheckoutPaths(config);
    const normalize = (value) => path.resolve(value).split(path.sep).join("/").toLowerCase();
    return normalize(candidate) === normalize(paths.binary);
  }

  function configuredBackendBinary(config) {
    if (!fs.existsSync(BACKEND_CONFIG_PATH)) return null;
    let values;
    try {
      values = JSON.parse(fs.readFileSync(BACKEND_CONFIG_PATH, "utf8"));
    } catch (error) {
      throw new Error(`invalid ZeroStack backend config ${BACKEND_CONFIG_PATH}: ${error.message}`);
    }
    const alias = config.envKey.replace(/^ZERO_/, "ZEROSTACK_");
    const selected = values?.[config.envKey] ?? values?.[alias];
    if (selected === undefined) return null;
    if (typeof selected !== "string" || !path.isAbsolute(selected)) {
      throw new Error(`${config.envKey} in ${BACKEND_CONFIG_PATH} must be an absolute path`);
    }
    return selected;
  }

  function configuredRawWorkerBinary(config) {
    if (!config.rawEnvKey || !fs.existsSync(BACKEND_CONFIG_PATH)) return null;
    let values;
    try {
      values = JSON.parse(fs.readFileSync(BACKEND_CONFIG_PATH, "utf8"));
    } catch (error) {
      throw new Error(`invalid ZeroStack backend config ${BACKEND_CONFIG_PATH}: ${error.message}`);
    }
    const alias = config.rawEnvKey.replace(/^ZERO_/, "ZEROSTACK_");
    const selected = values?.[config.rawEnvKey] ?? values?.[alias];
    if (selected === undefined) return null;
    if (typeof selected !== "string" || !path.isAbsolute(selected)) {
      throw new Error(`${config.rawEnvKey} in ${BACKEND_CONFIG_PATH} must be an absolute path`);
    }
    return selected;
  }

  function substrateBinary(config) {
    return () => {
      const override = process.env[config.envKey] || process.env[config.envKey.replace(/^ZERO_/, "ZEROSTACK_")];
      if (override) {
        ORIGIN_MAIN_STATUS.set(config.ns, { mode: "env-override", env: config.envKey, binary: override });
        return override;
      }
      const configured = configuredBackendBinary(config);
      if (configured) {
        // A config entry pointing INSIDE the origin/main root names the binary
        // this mechanism itself manages, so it must still be refreshed. Taking
        // the plain override path here skipped scheduleOriginMainRefresh, whose
        // only caller is originMainBinaryFast, so the checkout never advanced,
        // refreshedAt froze, and pushes to origin/main silently never went live
        // while status cheerfully reported "config-override".
        if (isManagedOriginMainBinary(config, configured)) {
          const managed = originMainBinaryFast(config);
          const current = ORIGIN_MAIN_STATUS.get(config.ns) || {};
          ORIGIN_MAIN_STATUS.set(config.ns, {
            ...current,
            config: BACKEND_CONFIG_PATH,
            key: config.envKey,
            config_points_at_managed_checkout: true,
          });
          return managed;
        }
        ORIGIN_MAIN_STATUS.set(config.ns, {
          mode: "config-override",
          config: BACKEND_CONFIG_PATH,
          key: config.envKey,
          binary: configured,
        });
        return configured;
      }
      return originMainBinaryFast(config);
    };
  }

  function rawWorkerBinary(config, primaryBinary) {
    if (!config.rawBinaryRel || config.rawBinaryRel === config.binaryRel) return primaryBinary;
    return () => {
      const override = process.env[config.rawEnvKey] || process.env[config.rawEnvKey.replace(/^ZERO_/, "ZEROSTACK_")];
      if (override) return override;
      const configured = configuredRawWorkerBinary(config);
      if (configured) return configured;
      const primary = primaryBinary();
      if (isManagedOriginMainBinary(config, primary)) return originMainCheckoutPaths(config).rawBinary;
      return path.join(path.dirname(primary), path.basename(platformBinaryRel(config.rawBinaryRel)));
    };
  }

  function originMainStatusSnapshot() {
    return Object.fromEntries(Object.values(backendConfigs).map((config) => {
      const current = ORIGIN_MAIN_STATUS.get(config.ns);
      if (!current) {
        return [config.ns, { mode: "pending-first-use", buildEnabled: ORIGIN_MAIN_BUILD_ENABLED }];
      }
      if (current.mode === "env-override" || current.mode === "config-override") return [config.ns, current];
      const paths = originMainCheckoutPaths(config);
      const latest = readOriginMainStatusFile(config);
      if (latest && !fs.existsSync(paths.lock)) {
        return [config.ns, { ...current, ...latest, latest, non_blocking: true }];
      }
      return [config.ns, current];
    }));
  }

  const backendConfigs = originMainBackendConfigs(SOURCE_REPO_ROOT);
  // Exact rebuild command per substrate. A bare "cargo build --release" does
  // NOT produce the codemode binaries (MCP and CodeMode surface features are
  // mutually exclusive), so diagnostics must carry the full feature flags.
  const rebuildHint = (cfg) => [cfg.buildCommand, ...cfg.buildArgs].join(" ");
  const fsBinary = substrateBinary(backendConfigs.fs);
  const graphBinary = substrateBinary(backendConfigs.graph);
  const tokenBinary = substrateBinary(backendConfigs.token);
  const SUBSTRATES = {
    fs: { surface: "fs", ns: backendConfigs.fs.ns, label: "fszero", binary: fsBinary, rawBinary: rawWorkerBinary(backendConfigs.fs, fsBinary), rawArgv: (root) => ["--raw-worker", "--root", root], argv: [], rootEnv: "FSZERO_ROOT", rebuildHint: rebuildHint(backendConfigs.fs) },
    graph: { surface: "graph", ns: backendConfigs.graph.ns, label: "graphzero", binary: graphBinary, rawBinary: rawWorkerBinary(backendConfigs.graph, graphBinary), rawArgv: () => [], argv: [], rootEnv: "GZ_REPO_ROOT", rebuildHint: rebuildHint(backendConfigs.graph) },
    token: { surface: "token", ns: backendConfigs.token.ns, label: "tokenzero", binary: tokenBinary, rawBinary: rawWorkerBinary(backendConfigs.token, tokenBinary), rawArgv: (root) => ["raw-worker", "--root", root], argv: [], rootEnv: "TOKENZERO_ROOT", rebuildHint: rebuildHint(backendConfigs.token) },
  };

  return {
    BACKEND_CONFIG_PATH,
    ORIGIN_MAIN_BUILD_ENABLED,
    ORIGIN_MAIN_BUILD_LOCK,
    ORIGIN_MAIN_BUILD_TIMEOUT_MS,
    ORIGIN_MAIN_REFRESH_MS,
    ORIGIN_MAIN_ROOT,
    REBUILD_INSTRUCTION,
    SUBSTRATES,
    originMainStatusSnapshot,
  };
}
