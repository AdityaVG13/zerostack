// Loader for the `zerostack-codemode-host --locate` manifest (schema
// "zerostack.locate.v1"). Harnesses use this instead of hand-authoring the
// absolute paths the aggregate runtime needs.
//
// HOST RESOLUTION PRECEDENCE (first executable candidate wins):
//   1. options.hostPath                       explicit caller override
//   2. env ZEROSTACK_CODEMODE_HOST            explicit environment override
//   3. $ZEROSTACK_HOME/bin/<exe>              installed home
//   4. <devRoot>/target/release/<exe>         developer checkout (opt-in devRoot)
//   5. $XDG_DATA_HOME|~/.local/share/zerostack/bin/<exe>
//   6. platform data dir (darwin: ~/Library/Application Support/ZeroStack/bin,
//      win32: %LOCALAPPDATA%/ZeroStack/bin, otherwise ~/.zerostack/bin)
//   7. options.extraCandidates                harness-supplied order (pi passes
//      its codeModeBinaryCandidates so there is no second discovery contract)
//   8. bare executable name, resolved through PATH by the execFile probe
//
// The host is frequently NOT on PATH, which is why steps 3-7 exist.
// Dependency-free by design: node:child_process, node:path, node:os, node:fs only.
import { execFile } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export const LOCATE_SCHEMA = "zerostack.locate.v1";

const BINARY_FIELDS = ["fs", "graph", "token"];
const SCALAR_FIELDS = ["node", "runtime_module", "substrate_module", "store_root", "journal_dir"];

function executableName(platform) {
  return platform === "win32" ? "zerostack-codemode-host.exe" : "zerostack-codemode-host";
}

function isExecutableFile(candidate) {
  try {
    fs.accessSync(candidate, fs.constants.X_OK);
    return fs.statSync(candidate).isFile();
  } catch {
    return false;
  }
}

function platformBinDir(home, env, platform) {
  if (platform === "win32") {
    return path.join(env.LOCALAPPDATA || path.join(home, "AppData", "Local"), "ZeroStack", "bin");
  }
  if (platform === "darwin") {
    return path.join(home, "Library", "Application Support", "ZeroStack", "bin");
  }
  return path.join(home, ".zerostack", "bin");
}

// Ordered host candidates. Documented precedence lives in the header comment.
export function hostCandidates(options = {}) {
  const env = options.env || process.env;
  const platform = options.platform || process.platform;
  const home = options.homeDir || os.homedir();
  const exe = executableName(platform);
  const candidates = [options.hostPath, env.ZEROSTACK_CODEMODE_HOST];
  if (env.ZEROSTACK_HOME) candidates.push(path.join(env.ZEROSTACK_HOME, "bin", exe));
  if (options.devRoot) candidates.push(path.join(options.devRoot, "target", "release", exe));
  candidates.push(path.join(env.XDG_DATA_HOME || path.join(home, ".local", "share"), "zerostack", "bin", exe));
  candidates.push(path.join(platformBinDir(home, env, platform), exe));
  for (const extra of options.extraCandidates || []) candidates.push(extra);
  return candidates.filter(
    (value, index, values) => typeof value === "string" && value.length > 0 && values.indexOf(value) === index,
  );
}

function hostUnavailableError(probed, target, cause) {
  const error = new Error(
    "zerostack-codemode-host is unavailable (tried: " + (probed.join(", ") || "none") + ", then " + target +
      " via PATH). Run zerostack-codemode-host --locate to diagnose. Cause: " + (cause?.message || String(cause)),
  );
  error.kind = "codemode_host_unavailable";
  error.retryable = false;
  error.probed = probed;
  throw error;
}

function runLocate(target, options) {
  const env = { ...process.env, ...(options.env || {}) };
  return new Promise((resolve, reject) => {
    execFile(target, ["--locate"], { env, timeout: Number(options.timeoutMs) || 15_000, maxBuffer: 8 * 1024 * 1024 }, (error, stdout) => {
      if (error) reject(error);
      else resolve(String(stdout));
    });
  });
}

// Resolve the host by precedence, run --locate, validate the schema.
export async function loadLocateManifest(options = {}) {
  const platform = options.platform || process.platform;
  const probed = [];
  let resolvedHost = null;
  for (const candidate of hostCandidates(options)) {
    probed.push(candidate);
    if (isExecutableFile(candidate)) {
      resolvedHost = candidate;
      break;
    }
  }
  const target = resolvedHost || executableName(platform);
  let stdout;
  try {
    stdout = await runLocate(target, options);
  } catch (cause) {
    hostUnavailableError(probed, target, cause);
  }
  let manifest;
  try {
    manifest = JSON.parse(stdout);
  } catch (cause) {
    const error = new Error("zerostack-codemode-host --locate did not emit JSON: " + (cause?.message || String(cause)));
    error.kind = "locate_manifest_invalid";
    throw error;
  }
  if (manifest?.schema !== LOCATE_SCHEMA) {
    const error = new Error(
      "unexpected locate manifest schema " + JSON.stringify(manifest?.schema) + ", expected " + LOCATE_SCHEMA,
    );
    error.kind = "locate_manifest_invalid";
    throw error;
  }
  return { manifest, hostPath: target };
}

function unresolvedError(component, entry) {
  const probed = Array.isArray(entry?.probed) ? entry.probed : [];
  const refused = (Array.isArray(entry?.refused) ? entry.refused : []).map((item) =>
    typeof item === "string" ? item : (item?.path || "?") + " (" + (item?.reason || "refused") + ")",
  );
  const error = new Error(
    "locate manifest component " + component + " is unresolved. probed: " + (probed.join(", ") || "none") +
      "; refused: " + (refused.join(", ") || "none") + ". Run zerostack-codemode-host --locate to diagnose.",
  );
  error.kind = "locate_component_unresolved";
  error.retryable = false;
  error.component = component;
  error.probed = probed;
  error.refused = refused;
  return error;
}

function readEntry(manifest, keys) {
  let node = manifest;
  for (const key of keys) {
    if (!node || typeof node !== "object") return undefined;
    node = node[key];
  }
  return node;
}

function entryValue(manifest, keys, component) {
  const entry = readEntry(manifest, keys);
  if (entry === undefined || entry === null) return undefined;
  if (typeof entry === "string") return entry;
  if (entry.resolved === false) throw unresolvedError(component, entry);
  return typeof entry.path === "string" ? entry.path : undefined;
}

// Read one manifest field ("node", "journal_dir", "binaries.fs", ...).
// Throws a doctor-style error when the manifest marked that entry unresolved.
export function manifestField(manifest, field) {
  if (!manifest) return undefined;
  return entryValue(manifest, String(field).split("."), field);
}

// Fill only fields the caller left unset. Caller-provided values always win and
// are never re-validated against the manifest.
export function applyManifestDefaults(bridgeOptions, manifest) {
  const target = bridgeOptions || {};
  if (!manifest) return target;
  const binaries = { ...(target.binaries || {}) };
  for (const name of BINARY_FIELDS) {
    if (binaries[name] !== undefined) continue;
    const value = entryValue(manifest, ["binaries", name], "binaries." + name);
    if (value !== undefined) binaries[name] = value;
  }
  target.binaries = binaries;
  for (const field of SCALAR_FIELDS) {
    if (target[field] !== undefined) continue;
    const value = entryValue(manifest, [field], field);
    if (value !== undefined) target[field] = value;
  }
  return target;
}
