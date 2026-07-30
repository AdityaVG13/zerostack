import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { applyManifestDefaults, loadLocateManifest, manifestField } from "../src/locate.js";

function fakeHome(manifest) {
  const home = fs.mkdtempSync(path.join(os.tmpdir(), "zerostack-locate-"));
  const binDir = path.join(home, "bin");
  fs.mkdirSync(binDir, { recursive: true });
  const script = path.join(binDir, "zerostack-codemode-host");
  const body = [
    "#!/bin/sh",
    "if [ \"$1\" != \"--locate\" ]; then echo \"unexpected args\" >&2; exit 2; fi",
    "cat <<'ZSJSON'",
    JSON.stringify(manifest),
    "ZSJSON",
    "",
  ].join("\n");
  fs.writeFileSync(script, body, { mode: 0o755 });
  return { home, script };
}

function resolvedManifest(home) {
  const p = (...parts) => path.join(home, ...parts);
  const entry = (value) => ({ resolved: true, path: value, probed: [value], refused: [] });
  return {
    schema: "zerostack.locate.v1",
    binaries: {
      fs: entry(p("bin", "fszero")),
      graph: entry(p("bin", "graphzero")),
      token: entry(p("bin", "tokenzero")),
    },
    node: entry(p("bin", "node")),
    runtime_module: entry(p("lib", "runtime.js")),
    substrate_module: entry(p("lib", "substrates.js")),
    store_root: entry(p("store")),
    journal_dir: entry(p("journal")),
  };
}

test("resolves every field from a synthetic ZEROSTACK_HOME with no hand-written paths", async () => {
  const { home, script } = fakeHome(resolvedManifest("/opt/zs"));
  const { manifest, hostPath } = await loadLocateManifest({ env: { ZEROSTACK_HOME: home }, homeDir: home });
  assert.equal(hostPath, script);
  assert.equal(manifest.schema, "zerostack.locate.v1");
  const applied = applyManifestDefaults({}, manifest);
  assert.equal(applied.binaries.fs, manifestField(manifest, "binaries.fs"));
  assert.equal(applied.binaries.graph, manifestField(manifest, "binaries.graph"));
  assert.equal(applied.binaries.token, manifestField(manifest, "binaries.token"));
  for (const field of ["node", "runtime_module", "substrate_module", "store_root", "journal_dir"]) {
    assert.equal(applied[field], manifestField(manifest, field), field);
    assert.ok(String(applied[field]).startsWith("/opt/zs"), field);
  }
  fs.rmSync(home, { recursive: true, force: true });
});

test("unresolved component produces a doctor-style error naming it", async () => {
  const manifest = resolvedManifest("/opt/zs");
  manifest.binaries.graph = {
    resolved: false,
    probed: ["/opt/zs/bin/graphzero", "/usr/local/bin/graphzero"],
    refused: [{ path: "/tmp/graphzero", reason: "not executable" }],
  };
  const { home } = fakeHome(manifest);
  const loaded = await loadLocateManifest({ env: { ZEROSTACK_HOME: home }, homeDir: home });
  assert.throws(
    () => applyManifestDefaults({}, loaded.manifest),
    (error) => {
      assert.equal(error.component, "binaries.graph");
      assert.match(error.message, /binaries\.graph/);
      assert.match(error.message, /\/usr\/local\/bin\/graphzero/);
      assert.match(error.message, /not executable/);
      return true;
    },
  );
  fs.rmSync(home, { recursive: true, force: true });
});

test("applyManifestDefaults never overwrites caller-set fields", () => {
  const manifest = resolvedManifest("/opt/zs");
  const caller = {
    binaries: { fs: "/caller/fszero" },
    node: "/caller/node",
    journal_dir: "/caller/journal",
  };
  const applied = applyManifestDefaults(caller, manifest);
  assert.equal(applied.binaries.fs, "/caller/fszero");
  assert.equal(applied.node, "/caller/node");
  assert.equal(applied.journal_dir, "/caller/journal");
  assert.equal(applied.binaries.graph, "/opt/zs/bin/graphzero");
  assert.equal(applied.store_root, "/opt/zs/store");
});

test("caller-set fields suppress unresolved-component errors", () => {
  const manifest = resolvedManifest("/opt/zs");
  manifest.node = { resolved: false, probed: ["/opt/zs/bin/node"], refused: [] };
  const applied = applyManifestDefaults({ node: "/caller/node" }, manifest);
  assert.equal(applied.node, "/caller/node");
});
