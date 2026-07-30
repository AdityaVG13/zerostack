// External tool binary paths shared by Pi ZeroStack.
//
// Paths are derived from the current home directory and native executable suffix.
// Legacy code-intel binaries are no longer exposed; all code intelligence and structural work
// now routes through FSZero + GraphZero + TokenZero via CodeMode (zero_execute + zero.* surfaces).
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

export function nativeExecutable(name, platform = process.platform) {
  return platform === "win32" ? `${name}.exe` : name;
}

const AI_ROOT = path.join(os.homedir(), "AI");

function gitCandidates() {
  if (process.platform === "win32") {
    const programFiles = process.env.ProgramFiles || "C:\\Program Files";
    return [
      path.join(programFiles, "Git", "cmd", "git.exe"),
      path.join(programFiles, "Git", "bin", "git.exe"),
    ];
  }
  return ["/opt/homebrew/bin/git", "/usr/bin/git", "/usr/local/bin/git"];
}

function firstExisting(...candidates) {
  return candidates.find((candidate) => fs.existsSync(candidate)) || candidates[0];
}

// Resolve a backend binary from an ordered candidate list. Falls back to the
// first candidate so callers still get a diagnosable path (and a truthy value
// to `fs.existsSync`-check) when nothing is installed.
export function resolveBackendBinary(candidates) {
  return firstExisting(...candidates);
}

// zerostack-j0j: developer `target/` trees are cleanup-eligible by design, so a
// binary inside one must never outrank a protected install for the *active*
// backend. Pruning ~/AI/TokenZero/target once removed target/release/tokenzero
// out from under the live CodeMode session and took zero.token offline, even
// though ~/.tokenzero/bin/tokenzero was healthy the whole time. Protected
// prefixes first; developer targets remain as a last-resort fallback so a
// from-source checkout still works before anything is installed.
const PROTECTED_PREFIXES = [
  path.join(os.homedir(), ".tokenzero", "bin"),
  path.join(os.homedir(), ".fszero", "bin"),
  path.join(os.homedir(), ".graphzero", "bin"),
  path.join(os.homedir(), ".local", "bin"),
];

const DEVELOPER_TARGET_ROOTS = {
  tokenzero: [path.join(AI_ROOT, "TokenZero"), path.join(AI_ROOT, "tokenzero")],
  fszero: [path.join(AI_ROOT, "FSZero"), path.join(AI_ROOT, "ZeroStack", "FSZero")],
  graphzero: [path.join(AI_ROOT, "graphzero"), path.join(AI_ROOT, "GraphZero"), path.join(AI_ROOT, "ZeroStack", "GraphZero")],
};

// Ordered binary candidates for a backend: protected installs, then developer targets.
export function backendBinaryCandidates(name, platform = process.platform) {
  const binary = nativeExecutable(name, platform);
  const protectedPaths = PROTECTED_PREFIXES.map((prefix) => path.join(prefix, binary));
  const developerPaths = (DEVELOPER_TARGET_ROOTS[name] || []).flatMap((root) => [
    path.join(root, "target", "release", binary),
    path.join(root, "target", "debug", binary),
  ]);
  return [...protectedPaths, ...developerPaths];
}

// True when a resolved backend lives in a cleanup-eligible developer target tree.
export function isCleanupEligibleBackend(binaryPath) {
  if (!binaryPath) return false;
  return path.resolve(binaryPath).split(path.sep).includes("target");
}

// Report which resolved backends ordinary `target/` pruning would delete. A
// backend inside a developer target tree is live but cleanup-eligible, so
// pruning artifacts can take the running session offline (zerostack-j0j).
export function artifactCleanupSafety(resolved) {
  const cleanupEligible = Object.entries(resolved)
    .filter(([, binaryPath]) => isCleanupEligibleBackend(binaryPath))
    .map(([backend, binaryPath]) => ({ backend, path: binaryPath }));
  return {
    cleanup_eligible_backends: cleanupEligible,
    safe_to_prune_targets: cleanupEligible.length === 0,
    remediation: cleanupEligible.length === 0
      ? "All resolved backends live outside developer target/ trees; pruning target/ is safe."
      : `Install these backends to a protected prefix (~/.tokenzero/bin, ~/.local/bin) or set the ZERO_*_BIN override before you prune target/: ${cleanupEligible.map((entry) => entry.backend).join(", ")}.`,
  };
}

// TokenZero CLI helpers (ingest/expand) are a distinct executable contract from
// the dedicated CodeMode server selected by ZERO_TOKENZERO_BIN.
export const TOKENZERO = process.env.ZERO_TOKENZERO_CLI_BIN || process.env.ZEROSTACK_TOKENZERO_CLI_BIN
  || resolveBackendBinary(backendBinaryCandidates("tokenzero"));
export const GIT = process.env.GIT_BIN || firstExisting(...gitCandidates());
export function backendRouterLog() {
  return process.env.PI_ZEROSTACK_LOG_PATH || process.env.OMP_BACKEND_ROUTER_LOG_PATH || path.join(os.homedir(), ".pi", "agent", "zerostack", "cache-proof.ndjson");
}

// ZeroStack CodeMode binaries (exact Cloudflare pattern).
// Point to release builds after `cargo build --release` in each crate.
export const FSZERO = process.env.ZERO_FSZERO_BIN || process.env.ZEROSTACK_FSZERO_BIN
  || resolveBackendBinary(backendBinaryCandidates("fszero"));
export const GZERO = process.env.ZERO_GRAPHZERO_BIN || process.env.ZEROSTACK_GRAPHZERO_BIN
  || resolveBackendBinary(backendBinaryCandidates("graphzero"));
export const GRAPHZERO = GZERO;
