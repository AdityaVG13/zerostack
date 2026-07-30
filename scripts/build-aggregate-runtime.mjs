#!/usr/bin/env node
// Build the public @zerostack/aggregate-runtime tarball into dist/ (zerostack-x99l).
//
// npm pack is the whole build: the package ships plain ESM, so there is no
// transpile step. This wrapper exists so the artifact lands at a predictable
// path that the smoke test and harness installers can both point at.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const packageDir = path.join(repoRoot, "packages", "aggregate-runtime");
const distDir = path.join(repoRoot, "dist");

fs.mkdirSync(distDir, { recursive: true });

const packed = JSON.parse(
  execFileSync("npm", ["pack", "--json", "--pack-destination", distDir], {
    cwd: packageDir,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
  }),
);

const filename = packed?.[0]?.filename;
if (!filename) throw new Error("npm pack did not report a filename");

const tarball = path.join(distDir, filename);
if (!fs.existsSync(tarball)) throw new Error(`npm pack reported ${filename} but it is not in ${distDir}`);

process.stdout.write(`${tarball}\n`);
export default tarball;
