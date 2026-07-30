// Smoke test: the aggregate runtime artifact is usable from OUTSIDE the repo
// tree with no pi-stack internal path (zerostack-x99l, zerostack-0pgp).
//
// The test builds the tarball, installs it into a temp dir that is not a
// descendant of the repo, and imports it by its package name. A consumer that
// still needed a pi-stack checkout would fail at import or at the contract
// assertion below.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..");

function buildTarball() {
  const out = execFileSync("node", [path.join(repoRoot, "scripts", "build-aggregate-runtime.mjs")], {
    cwd: repoRoot,
    encoding: "utf8",
  });
  const tarball = out.trim().split("\n").pop();
  assert.ok(fs.existsSync(tarball), `build did not produce a tarball: ${tarball}`);
  return tarball;
}

function installOutsideRepo(tarball) {
  const consumer = fs.mkdtempSync(path.join(fs.realpathSync(os.tmpdir()), "zerostack-agg-consumer-"));
  assert.ok(
    !path.resolve(consumer).startsWith(path.resolve(fs.realpathSync(repoRoot)) + path.sep),
    `consumer dir must live outside the repo tree, got ${consumer}`,
  );
  fs.writeFileSync(
    path.join(consumer, "package.json"),
    JSON.stringify({ name: "zerostack-agg-consumer", private: true, type: "module", version: "0.0.0" }, null, 2),
  );
  execFileSync("npm", ["install", "--no-audit", "--no-fund", "--install-strategy=shallow", tarball], {
    cwd: consumer,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  return consumer;
}

// Run the probe as a child process rooted in the consumer dir so Node resolves
// the package from the temp install, not from anywhere in the repo.
function probe(consumer, source) {
  const probeFile = path.join(consumer, `probe-${Math.random().toString(36).slice(2)}.mjs`);
  fs.writeFileSync(probeFile, source);
  return execFileSync("node", [probeFile], { cwd: consumer, encoding: "utf8" });
}

const tarball = buildTarball();
const consumer = installOutsideRepo(tarball);

test("build produces an installable artifact outside the repo tree", () => {
  const installed = path.join(consumer, "node_modules", "@zerostack", "aggregate-runtime");
  assert.ok(fs.existsSync(path.join(installed, "index.js")), "index.js is missing from the install");
  assert.ok(fs.existsSync(path.join(installed, "src", "raw-runtime.js")), "raw-runtime.js is missing");
  assert.ok(fs.existsSync(path.join(installed, "src", "substrates.js")), "substrates.js is missing");
  assert.ok(fs.existsSync(path.join(installed, "src", "paths.js")), "paths.js is missing");
});

test("declared export contract resolves by package name", () => {
  const out = probe(consumer, `
import * as mod from "@zerostack/aggregate-runtime";
const contract = mod.AGGREGATE_RUNTIME_CONTRACT;
const missing = [...contract.runtime, ...contract.substrate].filter((name) => typeof mod[name] !== "function");
console.log(JSON.stringify({ schema: contract.schema, missing }));
`);
  const result = JSON.parse(out.trim().split("\n").pop());
  assert.equal(result.schema, "zerostack.aggregate-runtime.contract.v1");
  assert.deepEqual(result.missing, [], `contract symbols missing from the artifact: ${result.missing.join(", ")}`);
});

test("subpath entry points resolve by package name", () => {
  const out = probe(consumer, `
const runtime = await import("@zerostack/aggregate-runtime/raw-runtime");
const substrates = await import("@zerostack/aggregate-runtime/substrates");
console.log(JSON.stringify({
  bridge: typeof runtime.createAggregateRuntimeBridge,
  supervisor: typeof runtime.RawWorkerSupervisor,
  helpers: typeof substrates.createSubstrateHelpers,
}));
`);
  const result = JSON.parse(out.trim().split("\n").pop());
  assert.equal(result.bridge, "function");
  assert.equal(result.supervisor, "function");
  assert.equal(result.helpers, "function");
});

test("the installed artifact needs no pi-stack internal path", () => {
  const installed = path.join(consumer, "node_modules", "@zerostack", "aggregate-runtime");
  for (const rel of ["index.js", "src/raw-runtime.js", "src/substrates.js", "src/paths.js"]) {
    const text = fs.readFileSync(path.join(installed, rel), "utf8");
    const specifiers = [...text.matchAll(/(?:^|[^\w])(?:import|from)\s*\(?\s*["']([^"']+)["']/g)].map((m) => m[1]);
    for (const specifier of specifiers) {
      assert.ok(!specifier.includes("pi-stack"), `${rel} imports a pi-stack path: ${specifier}`);
      assert.ok(!specifier.includes("pi-zerostack"), `${rel} imports a pi-zerostack path: ${specifier}`);
      // A relative escape out of the package would reach back into a checkout.
      assert.ok(!specifier.startsWith("../"), `${rel} imports outside the package: ${specifier}`);
    }
  }
});

test("substrate helpers construct without a pi-stack checkout", () => {
  const out = probe(consumer, `
const { createSubstrateHelpers } = await import("@zerostack/aggregate-runtime/substrates");
const helpers = createSubstrateHelpers();
console.log(JSON.stringify({ type: typeof helpers, keys: Object.keys(helpers).length > 0 }));
`);
  const result = JSON.parse(out.trim().split("\n").pop());
  assert.equal(result.type, "object");
  assert.equal(result.keys, true);
});
