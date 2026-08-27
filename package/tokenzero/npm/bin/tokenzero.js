#!/usr/bin/env node
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const SELF_PREFIX_BYTES = 8192;

const candidate = resolveTokenZero();

if (!candidate) {
  console.error(
    "tokenzero executable not found; install the Rust CLI or set TOKENZERO_BIN"
  );
  process.exit(127);
}

const result = spawnTokenZero(candidate, process.argv.slice(2), {
  stdio: "inherit",
  shell: false
});

if (result.error) {
  console.error(`tokenzero executable failed: ${result.error.message}`);
  process.exit(127);
}

process.exit(result.status ?? 1);

function findTokenZero() {
  const pathEntries = (process.env.PATH || "")
    .split(path.delimiter)
    .filter(Boolean);
  const names =
    process.platform === "win32"
      ? ["tokenzero.exe", "tokenzero.cmd", "tokenzero.bat"]
      : ["tokenzero"];

  for (const dir of pathEntries) {
    for (const name of names) {
      const candidate = path.join(dir, name);
      if (isSelf(candidate)) {
        continue;
      }
      if (isExecutable(candidate)) {
        return candidate;
      }
    }
  }
  return null;
}

function resolveTokenZero() {
  const configured = process.env.TOKENZERO_BIN;
  if (!configured) {
    return findTokenZero();
  }
  // Explicit TOKENZERO_BIN: refuse only realpath identity. Content heuristics
  // falsely reject distinct binaries that merely mention the shim path.
  if (isRealPathSelf(configured)) {
    console.error("TOKENZERO_BIN points to the npm shim; refusing recursive launch");
    process.exit(127);
  }
  if (!isExecutable(configured)) {
    console.error("TOKENZERO_BIN does not point to an executable tokenzero binary");
    process.exit(127);
  }
  return configured;
}

function spawnTokenZero(candidate, args, options) {
  if (process.platform === "win32" && /\.(cmd|bat)$/i.test(candidate)) {
    return spawnSync("cmd.exe", ["/D", "/C", "call", candidate, ...args], options);
  }
  return spawnSync(candidate, args, options);
}

function isExecutable(candidate) {
  try {
    const stat = fs.statSync(candidate);
    if (!stat.isFile()) {
      return false;
    }
    if (process.platform === "win32") {
      return true;
    }
    fs.accessSync(candidate, fs.constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

function isRealPathSelf(candidate) {
  try {
    const realCandidate = fs.realpathSync(candidate);
    const realSelf = fs.realpathSync(__filename);
    return realCandidate === realSelf;
  } catch {
    return false;
  }
}

function isSelf(candidate) {
  if (isRealPathSelf(candidate)) {
    return true;
  }
  // PATH search only: skip npm-generated wrappers that invoke this shim.
  return isNpmGeneratedShim(candidate);
}

function isNpmGeneratedShim(candidate) {
  try {
    const text = readPrefixUtf8(candidate, SELF_PREFIX_BYTES);
    const normalizedText = normalizePathText(text);
    const normalizedSelf = normalizePathText(__filename);
    if (!normalizedText.includes(normalizedSelf)) {
      return false;
    }
    return looksLikeNpmShimInvocation(normalizedText, normalizedSelf);
  } catch {
    return false;
  }
}

function readPrefixUtf8(candidate, maxBytes) {
  const fd = fs.openSync(candidate, "r");
  try {
    const buf = Buffer.alloc(maxBytes);
    const n = fs.readSync(fd, buf, 0, maxBytes, 0);
    return buf.subarray(0, n).toString("utf8");
  } finally {
    fs.closeSync(fd);
  }
}

function looksLikeNpmShimInvocation(normalizedText, normalizedSelf) {
  // npm cmd-shim (node): require("/abs/path/bin/tokenzero.js")
  if (
    normalizedText.includes(`require("${normalizedSelf}")`) ||
    normalizedText.includes(`require('${normalizedSelf}')`)
  ) {
    return true;
  }
  // npm cmd-shim (shell): exec ... "/abs/path/bin/tokenzero.js" "$@"
  if (
    !normalizedText.startsWith("#!") ||
    (!normalizedText.includes(`"${normalizedSelf}"`) &&
      !normalizedText.includes(`'${normalizedSelf}'`))
  ) {
    return false;
  }
  return (
    normalizedText.includes("exec ") ||
    normalizedText.includes("basedir") ||
    /\bnode\b/.test(normalizedText)
  );
}

function normalizePathText(value) {
  return value.replace(/\\/g, "/").toLowerCase();
}
