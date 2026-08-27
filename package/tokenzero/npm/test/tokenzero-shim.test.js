const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const shim = path.resolve(__dirname, "../bin/tokenzero.js");

test("skips non-executable PATH shadow before launching tokenzero", (t) => {
  if (process.platform === "win32") {
    t.skip("POSIX execute-bit behavior does not apply on Windows");
    return;
  }

  const root = fs.mkdtempSync(path.join(os.tmpdir(), "tokenzero-shim-"));
  t.after(() => fs.rmSync(root, { force: true, recursive: true }));

  const staleDir = path.join(root, "stale");
  const realDir = path.join(root, "real");
  fs.mkdirSync(staleDir);
  fs.mkdirSync(realDir);

  const stale = path.join(staleDir, "tokenzero");
  fs.writeFileSync(stale, "#!/bin/sh\nexit 99\n", { mode: 0o644 });

  const real = path.join(realDir, "tokenzero");
  fs.writeFileSync(real, "#!/bin/sh\nprintf 'real:%s\\n' \"$1\"\n", {
    mode: 0o755,
  });

  const env = {
    ...process.env,
    PATH: [staleDir, realDir].join(path.delimiter),
  };
  delete env.TOKENZERO_BIN;

  const result = spawnSync(process.execPath, [shim, "probe"], {
    encoding: "utf8",
    env,
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "real:probe\n");
});

test("TOKENZERO_BIN launches an explicit executable outside PATH", (t) => {
  if (process.platform === "win32") {
    t.skip("POSIX shell fixture does not apply on Windows");
    return;
  }

  const root = fs.mkdtempSync(path.join(os.tmpdir(), "tokenzero-shim-"));
  t.after(() => fs.rmSync(root, { force: true, recursive: true }));

  const real = path.join(root, "tokenzero-real");
  fs.writeFileSync(real, "#!/bin/sh\nprintf 'env:%s\\n' \"$1\"\n", {
    mode: 0o755,
  });

  const env = {
    ...process.env,
    PATH: "",
    TOKENZERO_BIN: real,
  };

  const result = spawnSync(process.execPath, [shim, "probe"], {
    encoding: "utf8",
    env,
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "env:probe\n");
});

test("TOKENZERO_BIN refuses stale non-executable shadow", (t) => {
  if (process.platform === "win32") {
    t.skip("POSIX execute-bit behavior does not apply on Windows");
    return;
  }

  const root = fs.mkdtempSync(path.join(os.tmpdir(), "tokenzero-shim-"));
  t.after(() => fs.rmSync(root, { force: true, recursive: true }));

  const stale = path.join(root, "tokenzero");
  fs.writeFileSync(stale, "#!/bin/sh\nexit 99\n", { mode: 0o644 });

  const env = {
    ...process.env,
    PATH: "",
    TOKENZERO_BIN: stale,
  };

  const result = spawnSync(process.execPath, [shim, "probe"], {
    encoding: "utf8",
    env,
  });

  assert.equal(result.status, 127);
  assert.match(
    result.stderr,
    /TOKENZERO_BIN does not point to an executable tokenzero binary/
  );
});

test("TOKENZERO_BIN refuses npm shim recursion", () => {
  const env = {
    ...process.env,
    PATH: "",
    TOKENZERO_BIN: shim,
  };

  const result = spawnSync(process.execPath, [shim, "probe"], {
    encoding: "utf8",
    env,
  });

  assert.equal(result.status, 127);
  assert.match(result.stderr, /TOKENZERO_BIN points to the npm shim/);
});

test("TOKENZERO_BIN launches distinct executable that mentions shim path", (t) => {
  if (process.platform === "win32") {
    t.skip("POSIX shell fixture does not apply on Windows");
    return;
  }

  const root = fs.mkdtempSync(path.join(os.tmpdir(), "tokenzero-shim-"));
  t.after(() => fs.rmSync(root, { force: true, recursive: true }));

  const distinct = path.join(root, "tokenzero-distinct");
  fs.writeFileSync(
    distinct,
    `#!/bin/sh\n# distinct executable mentioning ${shim}\nprintf 'distinct-ok\\n'\n`,
    { mode: 0o755 }
  );

  const env = {
    ...process.env,
    PATH: "",
    TOKENZERO_BIN: distinct,
  };

  const result = spawnSync(process.execPath, [shim, "probe"], {
    encoding: "utf8",
    env,
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "distinct-ok\n");
});

test("PATH skips npm cmd-shim wrapper that require()s this package bin", (t) => {
  if (process.platform === "win32") {
    t.skip("POSIX execute-bit behavior does not apply on Windows");
    return;
  }

  const root = fs.mkdtempSync(path.join(os.tmpdir(), "tokenzero-shim-"));
  t.after(() => fs.rmSync(root, { force: true, recursive: true }));

  const shimDir = path.join(root, "npm-bin");
  const realDir = path.join(root, "real");
  fs.mkdirSync(shimDir);
  fs.mkdirSync(realDir);

  const npmShim = path.join(shimDir, "tokenzero");
  fs.writeFileSync(
    npmShim,
    `#!/usr/bin/env node\nrequire(${JSON.stringify(shim)})\n`,
    { mode: 0o755 }
  );

  const real = path.join(realDir, "tokenzero");
  fs.writeFileSync(real, "#!/bin/sh\nprintf 'real:%s\\n' \"$1\"\n", {
    mode: 0o755,
  });

  const env = {
    ...process.env,
    PATH: [shimDir, realDir].join(path.delimiter),
  };
  delete env.TOKENZERO_BIN;

  const result = spawnSync(process.execPath, [shim, "probe"], {
    encoding: "utf8",
    env,
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "real:probe\n");
});

test("PATH does not skip distinct binary that only comments the shim path", (t) => {
  if (process.platform === "win32") {
    t.skip("POSIX execute-bit behavior does not apply on Windows");
    return;
  }

  const root = fs.mkdtempSync(path.join(os.tmpdir(), "tokenzero-shim-"));
  t.after(() => fs.rmSync(root, { force: true, recursive: true }));

  const dir = path.join(root, "bin");
  fs.mkdirSync(dir);

  const distinct = path.join(dir, "tokenzero");
  fs.writeFileSync(
    distinct,
    `#!/bin/sh\n# mentioning ${shim} is not an npm shim\nprintf 'comment-ok:%s\\n' \"$1\"\n`,
    { mode: 0o755 }
  );

  const env = {
    ...process.env,
    PATH: dir,
  };
  delete env.TOKENZERO_BIN;

  const result = spawnSync(process.execPath, [shim, "probe"], {
    encoding: "utf8",
    env,
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "comment-ok:probe\n");
});
