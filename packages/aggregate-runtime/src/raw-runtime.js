import { execFile, spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const PROTOCOL = "zerostack.raw_worker.v2";
const MAX_FRAME_BYTES = 1_048_576;
const DEFAULT_DEADLINE_MS = 30_000;
const DEFAULT_IDLE_TTL_MS = 1_800_000;
const DEFAULT_MAX_WORKERS = 12;

const ENGINE_BY_SURFACE = Object.freeze({ fs: "fszero", graph: "graphzero", token: "tokenzero" });
const SAFE_RAW_OPS = Object.freeze({
  fs: new Set(["fs.ls", "fs.read", "fs.search", "fs.readMany", "fs.searchMany", "fs.stat", "fs.statMany", "fs.expand", "fs.history", "fs.resolve"]),
  graph: new Set(["orient", "search", "snap", "recall", "expand", "blast", "verify", "query", "query_many", "defs", "callers", "ctx_ref"]),
  token: new Set(["read", "find", "tree", "grep", "glob", "recall", "expand", "mem", "rewrite"]),
});

function digest(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function writeJsonDurable(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = `${file}.${process.pid}.${crypto.randomUUID()}.tmp`;
  const descriptor = fs.openSync(temporary, "wx", 0o600);
  try {
    fs.writeFileSync(descriptor, JSON.stringify(value, null, 2), "utf8");
    fs.fsyncSync(descriptor);
  } finally {
    fs.closeSync(descriptor);
  }
  fs.renameSync(temporary, file);
  const directory = fs.openSync(path.dirname(file), "r");
  try { fs.fsyncSync(directory); } finally { fs.closeSync(directory); }
}

const BINARY_REVISIONS = new Map();
function binaryRevision(binary) {
  const stat = fs.statSync(binary);
  const signature = `${stat.dev}:${stat.ino}:${stat.size}:${stat.mtimeMs}`;
  const cached = BINARY_REVISIONS.get(binary);
  if (cached?.signature === signature) return cached.revision;
  const revision = `sha256:${digest(fs.readFileSync(binary))}`;
  BINARY_REVISIONS.set(binary, { signature, revision });
  return revision;
}

function recursivelyFind(value, key) {
  if (!value || typeof value !== "object") return undefined;
  if (typeof value[key] === "string" && value[key]) return value[key];
  for (const child of Object.values(value)) {
    const found = recursivelyFind(child, key);
    if (found) return found;
  }
  return undefined;
}

function collectRefs(value, output = new Set()) {
  if (typeof value === "string") {
    for (const match of value.matchAll(/\b(?:fz|gz|tz):\/\/[^\s"'`<>{}\[\],)]+/g)) output.add(match[0]);
  } else if (Array.isArray(value)) {
    for (const child of value) collectRefs(child, output);
  } else if (value && typeof value === "object") {
    for (const child of Object.values(value)) collectRefs(child, output);
  }
  return [...output];
}

function cacheRefPayloads(value, cache) {
  if (Array.isArray(value)) {
    for (const child of value) cacheRefPayloads(child, cache);
    return;
  }
  if (!value || typeof value !== "object") return;
  if (typeof value.ref === "string" && /^(?:fz|gz|tz):\/\//.test(value.ref)) {
    if (Object.hasOwn(value, "payload")) cache.set(value.ref, value.payload);
    else if (Object.hasOwn(value, "value")) cache.set(value.ref, value.value);
    else if (typeof value.payload_utf8 === "string") cache.set(value.ref, value.payload_utf8);
  }
  for (const child of Object.values(value)) cacheRefPayloads(child, cache);
}

function workerError(frame, fallback = "raw worker failed") {
  const detail = frame?.error;
  const error = new Error(detail?.message || fallback);
  error.kind = detail?.kind || "raw_worker";
  error.retryable = detail?.retryable === true;
  error.details = detail?.details;
  return error;
}

function abortError() {
  return Object.assign(new Error("aggregate raw-worker call cancelled"), {
    name: "AbortError",
    kind: "cancelled",
    retryable: true,
  });
}

function waitForExit(child, timeoutMs) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("raw-worker capability probe timed out"));
    }, timeoutMs);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function probeCapability(spec, root, storeRoot, revision) {
  const child = spawn(spec.binary, spec.argv(root), {
    cwd: root,
    env: {
      ...process.env,
      ...spec.env(root, storeRoot),
      ZEROSTACK_RAW_WORKER_PROTOCOL: "v1",
      ZEROSTACK_WORKER_REVISION: revision,
    },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = Buffer.alloc(0);
  let stderr = "";
  child.stdout.on("data", (chunk) => {
    stdout = Buffer.concat([stdout, chunk]);
    if (stdout.length > MAX_FRAME_BYTES) child.kill("SIGKILL");
  });
  child.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-8192); });
  // v1 envelopes differ across engines. TokenZero accepts a control verb;
  // FSZero/GraphZero accept a tagged handshake and disclose compatibility on
  // the intentional empty-digest rejection. Probe both without dispatching.
  child.stdin.end(`${JSON.stringify({ control: "handshake" })}\n${JSON.stringify({ kind: "handshake", request: {} })}\n`);
  await waitForExit(child, 10_000);
  const frames = stdout.toString("utf8").split(/\r?\n/).filter((entry) => entry.trim()).map((line) => JSON.parse(line));
  if (frames.length === 0) throw new Error(`raw-worker capability probe returned no frame${stderr.trim() ? `: ${stderr.trim()}` : ""}`);
  const contractDigest = frames.map((frame) => recursivelyFind(frame, "semantic_contract_digest")).find(Boolean);
  const registryDigest = frames.map((frame) => recursivelyFind(frame, "operation_registry_digest")).find(Boolean);
  if (!contractDigest) throw new Error("raw-worker capability probe omitted semantic_contract_digest");
  return { contractDigest, registryDigest };
}

class RawWorkerClient {
  constructor(supervisor, surface, root, storeRoot, spec, pin) {
    this.supervisor = supervisor;
    this.surface = surface;
    this.root = root;
    this.storeRoot = storeRoot;
    this.spec = spec;
    this.pin = pin;
    this.child = null;
    this.stderr = "";
    this.buffer = Buffer.alloc(0);
    this.frames = [];
    this.waiters = [];
    this.tail = Promise.resolve();
    this.started = null;
    this.lastUsed = Date.now();
  }

  async start() {
    if (this.started) return this.started;
    const started = this.startProcess();
    this.started = started;
    try {
      await started;
    } catch (error) {
      this.terminate(error);
      throw error;
    }
  }

  async startProcess() {
    const child = spawn(this.spec.binary, this.spec.argv(this.root), {
      cwd: this.root,
      env: {
        ...process.env,
        ...this.spec.env(this.root, this.storeRoot),
        ZEROSTACK_RAW_WORKER_PROTOCOL: PROTOCOL,
        ZEROSTACK_SESSION_ID: this.supervisor.sessionId,
        ZEROSTACK_WORKER_REVISION: this.pin.revision,
      },
      stdio: ["pipe", "pipe", "pipe"],
    });
    this.child = child;
    child.stdin.on("error", () => {});
    child.stdout.on("data", (chunk) => this.onData(chunk));
    child.stderr.on("data", (chunk) => { this.stderr = (this.stderr + chunk).slice(-8192); });
    child.once("error", (error) => this.terminate(error));
    child.once("close", (code, signal) => {
      if (this.child !== child) return;
      this.terminate(new Error(
        `raw worker exited (code=${code ?? "null"}, signal=${signal ?? "null"})${this.stderr.trim() ? `: ${this.stderr.trim()}` : ""}`,
      ));
    });
    this.send({
      kind: "handshake",
      request: {
        protocol_version: PROTOCOL,
        root: this.root,
        session_id: this.supervisor.sessionId,
        expected_engine: ENGINE_BY_SURFACE[this.surface],
        expected_worker_revision: this.pin.revision,
        expected_contract_digest: this.pin.contractDigest,
        ...(this.pin.registryDigest ? { expected_registry_digest: this.pin.registryDigest } : {}),
      },
    });
    const response = await this.nextFrame(10_000);
    if (response.kind !== "handshake_ack") throw workerError(response, "raw-worker handshake rejected");
    const binding = response.ack?.binding || {};
    if (binding.root !== this.root
      || binding.session_id !== this.supervisor.sessionId
      || binding.worker_revision !== this.pin.revision
      || binding.semantic_contract_digest !== this.pin.contractDigest) {
      throw new Error("raw-worker handshake acknowledgement violated the pinned binding");
    }
    this.supervisor.bindProtocolDigest(response.ack?.protocol_digest);
    this.limits = response.ack?.limits || {};
  }

  onData(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    if (this.buffer.length > MAX_FRAME_BYTES && !this.buffer.includes(0x0a)) {
      this.terminate(new Error(`raw-worker frame exceeds ${MAX_FRAME_BYTES} bytes`));
      return;
    }
    while (true) {
      const newline = this.buffer.indexOf(0x0a);
      if (newline < 0) {
        if (this.buffer.length > MAX_FRAME_BYTES) this.terminate(new Error(`raw-worker frame exceeds ${MAX_FRAME_BYTES} bytes`));
        return;
      }
      const line = this.buffer.subarray(0, newline);
      this.buffer = this.buffer.subarray(newline + 1);
      if (line.length === 0) continue;
      if (line.length > MAX_FRAME_BYTES) {
        this.terminate(new Error(`raw-worker frame exceeds ${MAX_FRAME_BYTES} bytes`));
        return;
      }
      let frame;
      try { frame = JSON.parse(line.toString("utf8")); }
      catch (error) {
        this.terminate(new Error(`raw-worker emitted invalid JSON: ${error.message}`));
        return;
      }
      const waiter = this.waiters.shift();
      if (waiter) waiter.resolve(frame);
      else this.frames.push(frame);
    }
  }

  nextFrame(timeoutMs, signal) {
    if (this.frames.length) return Promise.resolve(this.frames.shift());
    return new Promise((resolve, reject) => {
      let timer;
      const waiter = {
        resolve: (value) => { cleanup(); resolve(value); },
        reject: (error) => { cleanup(); reject(error); },
      };
      const onAbort = () => {
        this.terminate(abortError());
        waiter.reject(abortError());
      };
      const cleanup = () => {
        if (timer) clearTimeout(timer);
        signal?.removeEventListener("abort", onAbort);
        const index = this.waiters.indexOf(waiter);
        if (index >= 0) this.waiters.splice(index, 1);
      };
      timer = setTimeout(() => {
        const error = Object.assign(new Error("raw-worker response deadline exceeded"), { kind: "deadline_exceeded", retryable: true });
        this.terminate(error);
        waiter.reject(error);
      }, timeoutMs);
      signal?.addEventListener("abort", onAbort, { once: true });
      this.waiters.push(waiter);
    });
  }

  send(frame) {
    const bytes = Buffer.from(`${JSON.stringify(frame)}\n`);
    if (bytes.length - 1 > MAX_FRAME_BYTES) throw new Error(`raw-worker request exceeds ${MAX_FRAME_BYTES} bytes`);
    if (!this.child?.stdin.writable) throw new Error("raw worker is not running");
    this.child.stdin.write(bytes);
  }

  invoke(op, args, context, signal) {
    const call = this.tail.then(async () => {
      if (signal?.aborted) throw abortError();
      await this.start();
      if (signal?.aborted) throw abortError();
      const requestId = crypto.randomUUID();
      const traceId = context?.toolCallId || crypto.randomUUID();
      const deadlineMs = Math.max(1_000, Number(process.env.ZERO_RAW_WORKER_DEADLINE_MS || this.limits?.default_deadline_ms || DEFAULT_DEADLINE_MS));
      const trace = {
        runtime_id: this.supervisor.runtimeId,
        cell_id: String(context?.cellId || context?.toolCallId || `cell-${traceId}`),
        request_id: requestId,
        trace_id: String(traceId),
        ...(context?.aggregateExecutionId ? { parent_span_id: String(context.aggregateExecutionId) } : {}),
        worker_revision: this.pin.revision,
        contract_digest: this.pin.contractDigest,
      };
      const frame = {
        kind: "call",
        request: {
          request_id: requestId,
          op,
          args: args ?? {},
          deadline_unix_ms: Date.now() + deadlineMs,
          trace,
        },
      };
      this.supervisor.journal("call_started", { surface: this.surface, root: this.root, op, request_id: requestId, trace, args_digest: digest(JSON.stringify(args ?? {})) });
      this.send(frame);
      const response = await this.nextFrame(deadlineMs + 1_000, signal);
      this.lastUsed = Date.now();
      if (response.request_id && response.request_id !== requestId) throw new Error("raw-worker response request_id mismatch");
      if (response.kind === "error") {
        this.supervisor.journal("call_failed", { surface: this.surface, op, request_id: requestId, error: response.error });
        throw workerError(response);
      }
      if (response.kind !== "result") throw new Error(`unexpected raw-worker response kind: ${String(response.kind)}`);
      const value = response.result?.value ?? response.result;
      this.supervisor.recordResult(response.result, value);
      this.supervisor.journal("call_finished", {
        surface: this.surface,
        op,
        request_id: requestId,
        result_digest: digest(JSON.stringify(value)),
        metadata: response.result?.metadata,
        refs: collectRefs(response.result),
      });
      const metadata = response.result?.metadata || {};
      if (metadata.approval?.state === "denied") {
        throw Object.assign(new Error(`approval denied for ${this.surface}.${op}`), {
          kind: "approval_denied",
          retryable: false,
          approval: metadata.approval,
          revert: metadata.revert,
        });
      }
      if ((metadata.approval?.state === "required" || metadata.effect === "approval_required_mutation") && metadata.approval?.state !== "granted") {
        throw Object.assign(new Error(`approval required for ${this.surface}.${op}`), {
          kind: "approval_required",
          retryable: false,
          approval: metadata.approval,
          revert: metadata.revert,
        });
      }
      if (metadata.effect && metadata.effect !== "read_only") {
        const provenance = {
          effect: metadata.effect,
          approval: metadata.approval,
          revert: metadata.revert,
          snapshot: metadata.ownership?.snapshot,
          trace: metadata.trace,
        };
        return value && typeof value === "object" && !Array.isArray(value)
          ? { ...value, _zerostack: provenance }
          : { value, _zerostack: provenance };
      }
      return value;
    });
    this.tail = call.catch(() => {});
    return call;
  }

  terminate(reason = new Error("raw worker terminated")) {
    const child = this.child;
    this.child = null;
    this.started = null;
    if (child && !child.killed) child.kill("SIGKILL");
    for (const waiter of this.waiters.splice(0)) waiter.reject(reason);
    this.frames.length = 0;
    this.supervisor.dropClient(this);
  }

  close(reason = new Error("raw worker shut down")) {
    const child = this.child;
    this.child = null;
    this.started = null;
    if (child && !child.killed) {
      try {
        const bytes = Buffer.from(`${JSON.stringify({ kind: "shutdown", request: { reason: reason.message } })}\n`);
        child.stdin.end(bytes);
      } catch {
        try { child.stdin.end(); } catch {}
      }
      const timer = setTimeout(() => { if (!child.killed && child.exitCode === null) child.kill("SIGKILL"); }, 250);
      timer.unref?.();
    }
    for (const waiter of this.waiters.splice(0)) waiter.reject(reason);
    this.frames.length = 0;
    this.supervisor.dropClient(this);
  }
}

export class RawWorkerSupervisor {
  constructor({ journalDir, approvalHandler } = {}) {
    this.runtimeId = crypto.randomUUID();
    this.executionSecret = crypto.randomBytes(32);
    this.sessionId = `pi-${process.pid}-${crypto.randomUUID()}`;
    this.specs = new Map();
    this.pins = new Map();
    this.clients = new Map();
    this.refCache = new Map();
    this.storeRoots = new Map();
    this.protocolDigest = null;
    this.journalDir = journalDir || process.env.ZERO_AGGREGATE_JOURNAL_DIR || path.join(os.homedir(), ".pi", "agent", "zerostack", "aggregate-journal");
    this.journalPath = path.join(this.journalDir, `${this.sessionId}.ndjson`);
    this.idleTtlMs = Math.max(1_000, Number(process.env.ZERO_RAW_WORKER_IDLE_TTL_MS || DEFAULT_IDLE_TTL_MS));
    this.maxWorkers = Math.max(3, Number(process.env.ZERO_RAW_WORKER_MAX_PROCESSES || DEFAULT_MAX_WORKERS));
    this.maxOpsPerCell = Math.max(1, Number(process.env.ZERO_AGGREGATE_MAX_OPS || 1_000));
    this.maxFanoutPerCell = Math.max(1, Number(process.env.ZERO_AGGREGATE_MAX_FANOUT || 64));
    this.maxRefs = Math.max(1, Number(process.env.ZERO_AGGREGATE_MAX_REFS || 8_192));
    this.maxJournalBytes = Math.max(1_048_576, Number(process.env.ZERO_AGGREGATE_MAX_JOURNAL_BYTES || 64 * 1024 * 1024));
    this.cellOps = new Map();
    this.cellInFlight = new Map();
    this.effectsByExecution = new Map();
    this.approvalHandler = approvalHandler;
  }

  configureSubstrates(substrates) {
    for (const surface of Object.keys(ENGINE_BY_SURFACE)) {
      const config = substrates?.[surface];
      if (!config) continue;
      this.specs.set(surface, {
        get binary() { return (config.rawBinary || config.binary)(); },
        argv: (root) => config.rawArgv ? config.rawArgv(root) : surface === "graph" ? [] : surface === "fs" ? ["--raw-worker", "--root", root] : ["raw-worker", "--root", root],
        env: (root, storeRoot) => ({
          [config.rootEnv]: root,
          ...(storeRoot ? { ZEROSTACK_STORE_ROOT: storeRoot, ZERO_STACK_STORE_ROOT: storeRoot } : {}),
          ...(surface === "graph" ? {
            GRAPHZERO_ROOT: root,
            GRAPHZERO_REPO: root,
            GRAPHZERO_STORE: path.join(storeRoot || root, "graphzero"),
            GZ_REPO_ROOT: root,
          } : {}),
          ...(surface === "token" ? {
            TOKENZERO_ROOT: storeRoot ? path.join(storeRoot, "tokenzero") : root,
            ...(storeRoot ? { TOKENZERO_CACHE_PATH: path.join(storeRoot, "tokenzero", "recovery-cache.json") } : {}),
          } : {}),
          ...(surface === "fs" ? { FSZERO_ROOT: root, FSZERO_SKIP_STARTUP_INDEX: "1" } : {}),
        }),
      });
    }
  }

  bindProtocolDigest(protocolDigest) {
    if (!protocolDigest) throw new Error("raw-worker handshake omitted protocol_digest");
    if (this.protocolDigest && this.protocolDigest !== protocolDigest) {
      throw new Error(`raw-worker protocol digest mismatch: ${this.protocolDigest} != ${protocolDigest}`);
    }
    this.protocolDigest = protocolDigest;
  }

  issueExecutionId() {
    const id = crypto.randomUUID();
    const signature = crypto.createHmac("sha256", this.executionSecret).update(id).digest("hex");
    return `${id}.${signature}`;
  }

  requireExecutionId(value) {
    const [id, signature, ...extra] = String(value || "").split(".");
    if (extra.length || !id || !signature) throw Object.assign(new Error("aggregate worker call is missing its signed execution identity"), { kind: "execution_identity_invalid", retryable: false });
    const expected = crypto.createHmac("sha256", this.executionSecret).update(id).digest("hex");
    const actual = Buffer.from(signature, "utf8");
    const wanted = Buffer.from(expected, "utf8");
    if (actual.length !== wanted.length || !crypto.timingSafeEqual(actual, wanted)) {
      throw Object.assign(new Error("aggregate worker call has an invalid execution identity"), { kind: "execution_identity_invalid", retryable: false });
    }
    return String(value);
  }

  bindStore(root, storeRoot) {
    const resolvedRoot = path.resolve(root);
    const canonicalRoot = fs.existsSync(resolvedRoot) ? fs.realpathSync.native(resolvedRoot) : resolvedRoot;
    const resolvedStore = storeRoot ? path.resolve(storeRoot) : null;
    if (resolvedStore) fs.mkdirSync(resolvedStore, { recursive: true });
    this.storeRoots.set(canonicalRoot, resolvedStore);
  }

  async pin(surface, root) {
    const spec = this.specs.get(surface);
    if (!spec) throw new Error(`raw-worker substrate is not configured: ${surface}`);
    const binary = spec.binary;
    if (!path.isAbsolute(binary) || !fs.existsSync(binary)) throw new Error(`missing raw-worker binary: ${binary}`);
    const revision = binaryRevision(binary);
    const existing = this.pins.get(surface);
    if (existing) {
      if (existing.binary !== binary || existing.revision !== revision) {
        throw Object.assign(new Error(`${surface} raw-worker revision changed inside aggregate runtime`), { kind: "worker_revision_changed", retryable: false });
      }
      return { spec: { ...spec, binary }, pin: existing };
    }
    const capability = await probeCapability({ ...spec, binary }, root, this.storeRoots.get(root), revision);
    const pin = { binary, revision, ...capability };
    this.pins.set(surface, pin);
    this.journal("worker_pinned", { surface, ...pin });
    return { spec: { ...spec, binary }, pin };
  }

  async client(surface, root) {
    this.reapIdle();
    const resolvedRoot = path.resolve(root);
    const canonicalRoot = fs.existsSync(resolvedRoot) ? fs.realpathSync.native(resolvedRoot) : resolvedRoot;
    const storeRoot = this.storeRoots.get(canonicalRoot) || null;
    const key = `${surface}\0${canonicalRoot}\0${storeRoot || ""}`;
    const existing = this.clients.get(key);
    if (existing) return existing;
    while (this.clients.size >= this.maxWorkers) {
      const oldest = [...this.clients.values()].sort((left, right) => left.lastUsed - right.lastUsed)[0];
      if (!oldest) break;
      oldest.close(new Error("raw worker reclaimed for process capacity"));
    }
    const { spec, pin } = await this.pin(surface, canonicalRoot);
    const client = new RawWorkerClient(this, surface, canonicalRoot, storeRoot, spec, pin);
    this.clients.set(key, client);
    return client;
  }

  async invoke(surface, op, args, root, context, signal) {
    if (args && typeof args === "object" && Object.hasOwn(args, "_zerostack_approval")) {
      throw Object.assign(new Error("approval grants are supervisor-owned transport metadata"), { kind: "approval_grant_forged", retryable: false });
    }
    if (context?.aggregateExecutionId && !context?.cellId) {
      throw Object.assign(new Error("aggregate worker call is missing its native host cell provenance; apply the pinned pi-codex-conversion-lite transport patch"), { kind: "cell_provenance_missing", retryable: false });
    }
    const cellId = String(context?.cellId || context?.toolCallId || "unbound-cell");
    this.admitOperation(cellId);
    const inFlight = (this.cellInFlight.get(cellId) || 0) + 1;
    if (inFlight > this.maxFanoutPerCell) {
      throw Object.assign(new Error(`aggregate cell ${cellId} exceeded ${this.maxFanoutPerCell} concurrent raw operations`), { kind: "fanout_quota_exceeded", retryable: false });
    }
    this.cellInFlight.set(cellId, inFlight);
    this.journal("call_admitted", {
      surface,
      root,
      op,
      admission_id: crypto.randomUUID(),
      trace: {
        cell_id: cellId,
        ...(context?.aggregateExecutionId ? { parent_span_id: String(context.aggregateExecutionId) } : {}),
      },
      args_digest: digest(JSON.stringify(args ?? {})),
    });
    try {
      try {
        const value = await (await this.client(surface, root)).invoke(op, args, context, signal);
        this.recordEffect(surface, op, root, context, value);
        return value;
      } catch (error) {
        if (error?.kind === "approval_required") {
          const granted = await this.requestApproval({ surface, op, root, approval: error.approval, revert: error.revert }, context);
          if (!granted) {
            this.journal("approval_denied", { surface, root, op, approval_id: error.approval?.approval_id });
            throw Object.assign(new Error(`approval denied for ${surface}.${op}`), { kind: "approval_denied", retryable: false, approval: error.approval, revert: error.revert });
          }
          const approval = {
            state: "granted",
            approval_id: error.approval?.approval_id,
            execution_id: context?.aggregateExecutionId,
          };
          this.journal("approval_granted", { surface, root, op, approval_id: approval.approval_id });
          const value = await (await this.client(surface, root)).invoke(op, { ...(args || {}), _zerostack_approval: approval }, context, signal);
          this.recordEffect(surface, op, root, context, value);
          return value;
        }
        if (signal?.aborted || error?.retryable === false || !SAFE_RAW_OPS[surface]?.has(op)) throw error;
        this.journal("read_retry", { surface, root, op, reason: error instanceof Error ? error.message : String(error) });
        const value = await (await this.client(surface, root)).invoke(op, args, context, signal);
        this.recordEffect(surface, op, root, context, value);
        return value;
      }
    } finally {
      const remaining = (this.cellInFlight.get(cellId) || 1) - 1;
      if (remaining <= 0) this.cellInFlight.delete(cellId);
      else this.cellInFlight.set(cellId, remaining);
    }
  }

  async requestApproval(details, context) {
    if (typeof this.approvalHandler === "function") return Boolean(await this.approvalHandler(details, context));
    const ui = context?.extensionContext?.ui || context?.ui;
    if (typeof ui?.confirm !== "function") {
      throw Object.assign(new Error(`approval required for ${details.surface}.${details.op}, but no interactive approval channel is available`), {
        kind: "approval_required",
        retryable: false,
        approval: details.approval,
        revert: details.revert,
      });
    }
    return Boolean(await ui.confirm(
      `Approve ZeroStack mutation: ${details.surface}.${details.op}`,
      details.approval?.policy || `Approval ID: ${details.approval?.approval_id || "unspecified"}`,
    ));
  }

  recordEffect(surface, op, root, context, value) {
    const metadata = value?._zerostack;
    const executionId = context?.aggregateExecutionId;
    if (!executionId || !metadata?.effect || metadata.effect === "read_only") return;
    const effects = this.effectsByExecution.get(executionId) || [];
    effects.push({
      surface,
      op,
      root,
      effect: metadata.effect,
      approval: metadata.approval,
      revert: metadata.revert,
      snapshot: metadata.snapshot,
      trace: metadata.trace,
    });
    this.effectsByExecution.set(executionId, effects);
  }

  async rollbackExecution(executionId, context = {}, signal) {
    const effects = [...(this.effectsByExecution.get(executionId) || [])].reverse();
    this.effectsByExecution.delete(executionId);
    const outcomes = [];
    for (const effect of effects) {
      if (!effect.revert?.supported || !effect.revert?.rollback_op) {
        outcomes.push({ surface: effect.surface, op: effect.op, status: "unsupported" });
        continue;
      }
      const rollbackArgs = {
        ...(effect.revert.journal_id ? { journal_id: effect.revert.journal_id } : {}),
        ...(effect.snapshot ? { snapshot: effect.snapshot } : {}),
      };
      this.journal("rollback_started", { surface: effect.surface, root: effect.root, op: effect.revert.rollback_op, journal_id: effect.revert.journal_id });
      try {
        const value = await (await this.client(effect.surface, effect.root)).invoke(
          effect.revert.rollback_op,
          rollbackArgs,
          { ...context, aggregateExecutionId: executionId },
          signal,
        );
        this.journal("rollback_finished", { surface: effect.surface, root: effect.root, op: effect.revert.rollback_op, result_digest: digest(JSON.stringify(value)) });
        outcomes.push({ surface: effect.surface, op: effect.revert.rollback_op, status: "reverted", value });
      } catch (error) {
        this.journal("rollback_failed", { surface: effect.surface, root: effect.root, op: effect.revert.rollback_op, error: error instanceof Error ? error.message : String(error) });
        outcomes.push({ surface: effect.surface, op: effect.revert.rollback_op, status: "failed", error });
      }
    }
    return outcomes;
  }

  completeExecution(executionId) {
    this.effectsByExecution.delete(executionId);
  }

  restoreEffectsFromJournal(executionId, journalPath, cellId) {
    if (this.effectsByExecution.has(executionId)) return this.effectsByExecution.get(executionId).length;
    let records;
    try {
      records = fs.readFileSync(journalPath, "utf8").trim().split("\n").filter(Boolean).map((line) => JSON.parse(line));
    } catch {
      return 0;
    }
    const belongs = (trace) => String(trace?.cell_id) === String(cellId) && trace?.parent_span_id === executionId;
    const started = new Map(records
      .filter((record) => record.event === "call_started" && belongs(record.trace))
      .map((record) => [record.request_id, record]));
    const effects = [];
    for (const record of records) {
      if (record.event !== "call_finished") continue;
      const call = started.get(record.request_id);
      const metadata = record.metadata;
      if (!call || !metadata || metadata.effect === "read_only") continue;
      effects.push({
        surface: call.surface,
        op: call.op,
        root: call.root,
        effect: metadata.effect,
        approval: metadata.approval,
        revert: metadata.revert,
        snapshot: metadata.ownership?.snapshot,
        trace: metadata.trace,
      });
    }
    if (effects.length > 0) this.effectsByExecution.set(executionId, effects);
    return effects.length;
  }

  async expand(ref, options, root, context, signal) {
    if (this.refCache.has(ref)) return this.refCache.get(ref);
    if (String(ref).startsWith("tz://")) return this.invoke("token", "expand", { ref, ...(options || {}) }, root, context, signal);
    if (String(ref).startsWith("gz://")) return this.invoke("graph", "expand", { reference: ref, ...(options || {}) }, root, context, signal);
    if (String(ref).startsWith("fz://")) return this.invoke("fs", "fs.expand", { ref, ...(options || {}) }, root, context, signal);
    throw new Error("expand takes a tz://, fz://, or gz:// ref");
  }

  recordResult(result, value) {
    cacheRefPayloads(value, this.refCache);
    while (this.refCache.size > this.maxRefs) this.refCache.delete(this.refCache.keys().next().value);
  }

  admitOperation(cellId) {
    const count = (this.cellOps.get(cellId) || 0) + 1;
    if (count > this.maxOpsPerCell) {
      throw Object.assign(new Error(`aggregate cell ${cellId} exceeded ${this.maxOpsPerCell} raw operations`), { kind: "operation_quota_exceeded", retryable: false });
    }
    this.cellOps.delete(cellId);
    this.cellOps.set(cellId, count);
    while (this.cellOps.size > 8_192) this.cellOps.delete(this.cellOps.keys().next().value);
  }

  releaseCell(cellId) {
    this.cellOps.delete(String(cellId));
    if (!this.cellInFlight.get(String(cellId))) this.cellInFlight.delete(String(cellId));
  }

  provenance() {
    return Object.fromEntries([...this.pins].map(([surface, pin]) => [surface, {
      binary: pin.binary,
      revision: pin.revision,
      contractDigest: pin.contractDigest,
      registryDigest: pin.registryDigest,
    }]));
  }

  replaySafety(journalPath, cellId, executionId) {
    let records = [];
    try {
      records = fs.readFileSync(journalPath, "utf8")
        .trim()
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line))
        .filter((record) => {
          const trace = record.trace || record.metadata?.trace;
          return trace?.cell_id === cellId && trace?.parent_span_id === executionId;
        });
    } catch {}
    const admitted = records.filter((record) => record.event === "call_admitted");
    const startedRecords = records.filter((record) => record.event === "call_started");
    const started = new Map(startedRecords.map((record) => [record.request_id, record]));
    const operationCount = Math.max(admitted.length, startedRecords.length);
    const surfaces = [...new Set([...admitted, ...startedRecords].map((record) => record.surface))].sort();
    for (const record of admitted) {
      if (!SAFE_RAW_OPS[record.surface]?.has(record.op)) {
        return { safe: false, reason: `admitted ${record.surface}.${record.op} has uncertain effects` };
      }
    }
    for (const record of records) {
      if (record.event === "call_failed") started.delete(record.request_id);
      if (record.event !== "call_finished") continue;
      started.delete(record.request_id);
      if (record.metadata?.effect && record.metadata.effect !== "read_only") {
        return { safe: false, reason: `completed ${record.metadata.effect} operation ${record.op} cannot be replayed automatically` };
      }
    }
    for (const record of started.values()) {
      if (!SAFE_RAW_OPS[record.surface]?.has(record.op)) {
        return { safe: false, reason: `in-flight ${record.surface}.${record.op} has uncertain effects` };
      }
    }
    return { safe: true, pending_read_only: started.size, operation_count: operationCount, worker_started_count: startedRecords.length, surfaces };
  }

  journal(event, data) {
    fs.mkdirSync(this.journalDir, { recursive: true, mode: 0o700 });
    let journalBytes = 0;
    try { journalBytes = fs.statSync(this.journalPath).size; } catch {}
    const line = `${JSON.stringify({ ts: new Date().toISOString(), event, runtime_id: this.runtimeId, session_id: this.sessionId, ...data })}\n`;
    if (journalBytes + Buffer.byteLength(line) > this.maxJournalBytes) {
      throw Object.assign(new Error(`aggregate journal exceeded ${this.maxJournalBytes} bytes`), { kind: "journal_quota_exceeded", retryable: false });
    }
    const descriptor = fs.openSync(this.journalPath, "a", 0o600);
    try {
      fs.writeSync(descriptor, line);
      fs.fsyncSync(descriptor);
    } finally {
      fs.closeSync(descriptor);
    }
    this.onJournal?.(event, data);
  }

  dropClient(client) {
    for (const [key, value] of this.clients) if (value === client) this.clients.delete(key);
  }

  reapIdle() {
    const now = Date.now();
    const clients = [...this.clients.values()].sort((left, right) => left.lastUsed - right.lastUsed);
    for (const client of clients) {
      if (now - client.lastUsed > this.idleTtlMs || this.clients.size > this.maxWorkers) client.close(new Error("raw worker reclaimed"));
    }
  }

  shutdown() {
    for (const client of [...this.clients.values()]) client.close(new Error("aggregate runtime shut down"));
    this.clients.clear();
  }
}

function objectSchema(properties, required = []) {
  const mandatory = properties.execution_id && !required.includes("execution_id") ? [...required, "execution_id"] : required;
  return { type: "object", properties, required: mandatory, additionalProperties: false };
}

export function createRawWorkerTools(supervisor) {
  const invoke = (surface, op, buildArgs = (input) => input) => async (input, context, signal) => {
    supervisor.requireExecutionId(input.execution_id);
    const root = path.resolve(input.root || context.cwd || process.cwd());
    return supervisor.invoke(surface, op, buildArgs(input), root, { ...context, aggregateExecutionId: input.execution_id }, signal);
  };
  const root = { root: { type: "string" }, execution_id: { type: "string" } };
  const args = { args: { type: "object", additionalProperties: true } };
  const fsOperations = {
    read: "fs.read",
    search: "fs.search",
    find: "fs.search",
    grep: "fs.search",
    list: "fs.ls",
    tree: "fs.ls",
    inventory: "fs.ls",
    mutate: "fs.edit",
    edit: "fs.edit",
    verifiedEdit: "fs.edit",
    write: "fs.write",
    resolve: "fs.resolve",
  };
  const tools = [
    {
      name: "zero_fs_compound",
      description: "Invoke one planner-free typed FSZero operation",
      parameters: objectSchema({ name: { type: "string" }, ...args, ...root }, ["name"]),
      async invoke(input, context, signal) {
        supervisor.requireExecutionId(input.execution_id);
        const op = fsOperations[input.name];
        if (!op) throw new Error(`FSZero raw worker does not expose planner-only compound operation: ${input.name}`);
        return supervisor.invoke("fs", op, input.args || {}, path.resolve(input.root || context.cwd || process.cwd()), { ...context, aggregateExecutionId: input.execution_id }, signal);
      },
    },
    { name: "zero_fs_plan", description: "Reject planner ownership at the raw-worker boundary", parameters: objectSchema({ goal: { type: "string" }, opts: { type: "object", additionalProperties: true }, ...root }, ["goal"]), async invoke() { throw new Error("zero.fs.plan is planner-owned and unavailable inside aggregate raw workers"); } },
    { name: "zero_fs_structural", description: "Reject planner ownership at the raw-worker boundary", parameters: objectSchema({ query: { type: "string" }, target: { type: "string" }, ...root }, ["query"]), async invoke() { throw new Error("zero.fs.structural is planner-owned and unavailable inside aggregate raw workers"); } },
    ...["resolve", "world", "history", "undo"].map((op) => ({ name: `zero_fs_${op}`, description: `Invoke typed FSZero ${op}`, parameters: objectSchema({ ...args, ...root }), invoke: invoke("fs", `fs.${op}`, ({ args: value }) => value || {}) })),
    ...["snap", "orient", "blast", "query", "recall", "verify", "index", "remember", "reserve"].map((op) => ({ name: `zero_graph_${op}`, description: `Invoke typed GraphZero ${op}`, parameters: objectSchema({ ...args, ...root }), invoke: invoke("graph", op, ({ args: value }) => value || {}) })),
    { name: "zero_graph_query_many", description: "Invoke typed GraphZero query_many", parameters: objectSchema({ ...args, ...root }), invoke: invoke("graph", "query_many", ({ args: value }) => value || {}) },
    ...["read", "find", "tree", "compact", "shell", "grep", "glob", "rewrite", "mem", "recall"].map((op) => ({ name: `zero_token_${op}`, description: `Invoke typed TokenZero ${op}`, parameters: objectSchema({ ...args, ...root }), invoke: invoke("token", op === "compact" ? "ingest" : op, ({ args: value }) => value || {}) })),
    { name: "zero_ref_expand", description: "Expand a ZeroStack ref through its owning worker", parameters: objectSchema({ ref: { type: "string" }, options: { type: "object", additionalProperties: true }, ...root }, ["ref"]), invoke: async (input, context, signal) => {
      supervisor.requireExecutionId(input.execution_id);
      return supervisor.expand(input.ref, input.options, path.resolve(input.root || context.cwd || process.cwd()), { ...context, aggregateExecutionId: input.execution_id }, signal);
    } },
  ];
  return tools.map(({ parameters, ...tool }) => ({
    ...tool,
    kind: "function",
    inputSchema: parameters,
  }));
}

export function wrapZeroPlan(plan, root, executionId) {
  if (typeof executionId !== "string" || executionId.length === 0) {
    throw new TypeError("wrapZeroPlan requires a supervisor-issued execution ID");
  }
  const literalRoot = JSON.stringify(path.resolve(root));
  const literalExecutionId = JSON.stringify(executionId);
  return `
const __zeroRoot = ${literalRoot};
const __zeroExecutionId = ${literalExecutionId};
const __zeroTools = new Proxy(tools, { get: (target, name) => (input = {}) => target[name]({ ...input, execution_id: __zeroExecutionId }) });
const zero = Object.freeze({
  fs: Object.freeze({
    compound: (name, args = {}) => __zeroTools.zero_fs_compound({ name, args, root: __zeroRoot }),
    plan: (goal, opts = {}) => __zeroTools.zero_fs_plan({ goal, opts, root: __zeroRoot }),
    structural: (query, target) => __zeroTools.zero_fs_structural({ query, target, root: __zeroRoot }),
    resolve: (intent, opts = {}) => __zeroTools.zero_fs_resolve({ args: { intent, ...opts }, root: __zeroRoot }),
    world: (actionOrArgs, opts = {}) => __zeroTools.zero_fs_world({ args: typeof actionOrArgs === "string" ? { arg: actionOrArgs, ...opts } : { ...actionOrArgs, ...opts }, root: __zeroRoot }),
    history: (arg = {}) => __zeroTools.zero_fs_history({ args: typeof arg === "string" ? { arg } : arg, root: __zeroRoot }),
    undo: (arg) => __zeroTools.zero_fs_undo({ args: typeof arg === "string" ? { arg } : arg, root: __zeroRoot }),
  }),
  graph: Object.freeze({
    snap: (query, budget) => __zeroTools.zero_graph_snap({ args: { query, ...(budget === undefined ? {} : { budget }) }, root: __zeroRoot }),
    orient: (surface, query) => __zeroTools.zero_graph_orient({ args: { surface, ...(query === undefined ? {} : { query }) }, root: __zeroRoot }),
    blast: (symbol, opts = {}) => __zeroTools.zero_graph_blast({ args: { intent: symbol, ...opts }, root: __zeroRoot }),
    query: (surface, target) => __zeroTools.zero_graph_query({ args: { surface, ...(target === undefined ? {} : { target }) }, root: __zeroRoot }),
    recall: (target) => __zeroTools.zero_graph_recall({ args: { target }, root: __zeroRoot }),
    verify: (target, claim) => __zeroTools.zero_graph_verify({ args: { target, ...(claim === undefined ? {} : { claim }) }, root: __zeroRoot }),
    index: () => __zeroTools.zero_graph_index({ args: {}, root: __zeroRoot }),
    remember: (fact) => __zeroTools.zero_graph_remember({ args: typeof fact === "string" ? { text: fact } : fact, root: __zeroRoot }),
    reserve: (action, args = {}) => __zeroTools.zero_graph_reserve({ args: { action, ...args }, root: __zeroRoot }),
    queryMany: (requests) => __zeroTools.zero_graph_query_many({ args: { requests }, root: __zeroRoot }),
  }),
  token: Object.freeze({
    read: (path, opts = {}) => __zeroTools.zero_token_read({ args: { path, ...opts }, root: __zeroRoot }),
    find: (query, path) => __zeroTools.zero_token_find({ args: { query, ...(path === undefined ? {} : { path }) }, root: __zeroRoot }),
    tree: (path = ".", opts = {}) => __zeroTools.zero_token_tree({ args: { path, ...opts }, root: __zeroRoot }),
    compact: (data) => __zeroTools.zero_token_compact({ args: { text: typeof data === "string" ? data : JSON.stringify(data) }, root: __zeroRoot }),
    shell: (command, opts = {}) => {
      if (opts.background) throw new Error("background shell jobs are replaced by aggregate exec/wait cells");
      return __zeroTools.zero_token_shell({ args: { command, ...opts }, root: __zeroRoot });
    },
    grep: (query, path) => __zeroTools.zero_token_grep({ args: { query, ...(path === undefined ? {} : { path }) }, root: __zeroRoot }),
    glob: (pattern, path) => __zeroTools.zero_token_glob({ args: { pattern, ...(path === undefined ? {} : { path }) }, root: __zeroRoot }),
    rewrite: (command, opts = {}) => __zeroTools.zero_token_rewrite({ args: { command, ...opts }, root: __zeroRoot }),
    mem: () => __zeroTools.zero_token_mem({ args: {}, root: __zeroRoot }),
    recall: (query, opts = {}) => __zeroTools.zero_token_recall({ args: { query, ...opts }, root: __zeroRoot }),
    job: () => { throw new Error("zero.token.job is replaced by aggregate exec/wait cell lifecycle"); },
    expand: (ref, options = {}) => __zeroTools.zero_ref_expand({ ref, options, root: __zeroRoot }),
    expandMany: (items) => Promise.all(items.map((item) => {
      const ref = typeof item === "string" ? item : item.ref;
      const options = typeof item === "string" ? {} : item;
      return __zeroTools.zero_ref_expand({ ref, options, root: __zeroRoot });
    })).then((items) => ({ count: items.length, items })),
  }),
});
const __zeroResult = await (async () => {
${plan}
})();
if (__zeroResult !== undefined) text(__zeroResult);
`;
}

export function runtimeResponseToToolResult(response) {
  const scriptError = response?.kind === "result" ? response.errorText : undefined;
  const status = scriptError
    ? `Script error: ${scriptError}`
    : response?.kind === "yielded"
      ? `Still running. Call wait({ cell_id: "${response.cellId}" })`
      : response?.kind === "terminated"
        ? "Script terminated"
        : "Script completed";
  const content = (response?.contentItems || []).flatMap((item) => {
    if (item?.type === "input_text" && typeof item.text === "string") return [{ type: "text", text: item.text }];
    if (item?.type === "input_image" && typeof item.image_url === "string") {
      const match = item.image_url.match(/^data:([^;,]+);base64,(.+)$/s);
      if (match) return [{ type: "image", mimeType: match[1], data: match[2] }];
    }
    return [];
  });
  const maxChars = Math.min(100_000, Math.max(1, response?.maxOutputTokens || 10_000)) * 4;
  let remaining = maxChars;
  let truncated = false;
  const bounded = [];
  let imageCount = 0;
  let imageChars = 0;
  for (const item of content) {
    if (item.type === "image") {
      if (imageCount >= 4 || imageChars + item.data.length > 16 * 1024 * 1024) {
        truncated = true;
        continue;
      }
      imageCount += 1;
      imageChars += item.data.length;
      bounded.push(item);
      continue;
    }
    if (remaining <= 0) {
      truncated = true;
      continue;
    }
    if (item.text.length <= remaining) {
      bounded.push(item);
      remaining -= item.text.length;
    } else {
      bounded.push({ ...item, text: item.text.slice(0, remaining) });
      remaining = 0;
      truncated = true;
    }
  }
  if (truncated) bounded.push({ type: "text", text: "[Output truncated]" });
  return {
    content: [{ type: "text", text: status }, ...bounded],
    details: {
      codeMode: true,
      aggregateZeroStack: true,
      cellId: response?.cellId,
      status: response?.kind,
      ...(response?.traces ? { traces: response.traces } : {}),
      ...(response?.rollback ? { rollback: response.rollback } : {}),
      ...(scriptError ? { scriptError } : {}),
    },
  };
}

export function createAggregateRuntimeBridge(pi, options = {}) {
  let supervisor = new RawWorkerSupervisor(options.rawRuntime);
  let substrates = null;
  const provider = {
    getTools: () => createRawWorkerTools(supervisor),
    isActive: () => process.env.ZERO_AGGREGATE_RUNTIME !== "0",
  };
  let runtime = null;
  let providerId = null;
  let memoryMonitor = null;
  let memoryCheckActive = false;
  let activeExecutions = 0;
  const activeCells = new Set();
  const executionCells = new Map();
  const maxHostRssBytes = Math.max(64 * 1024 * 1024, Number(process.env.ZERO_AGGREGATE_HOST_RSS_BYTES || 768 * 1024 * 1024));
  const cellDir = path.join(supervisor.journalDir, "cells");

  const cellPath = (cellId) => path.join(cellDir, `${digest(String(cellId)).slice(0, 32)}.json`);
  const removeCell = (cellId) => {
    for (const [executionId, ownedCellId] of executionCells) {
      if (ownedCellId === cellId) executionCells.delete(executionId);
    }
    try {
      fs.unlinkSync(cellPath(cellId));
      const directory = fs.openSync(cellDir, "r");
      try { fs.fsyncSync(directory); } finally { fs.closeSync(directory); }
    } catch {}
  };
  function blockCellRecovery(cellId, reason) {
    try {
      const file = cellPath(cellId);
      const record = JSON.parse(fs.readFileSync(file, "utf8"));
      writeJsonDurable(file, { ...record, replay_blocked: reason, blocked_at: new Date().toISOString() });
    } catch {}
  }
  function persistCell(response, plan, root, store, executionId) {
    if (response.kind !== "yielded") {
      removeCell(response.cellId);
      activeCells.delete(response.cellId);
      supervisor.releaseCell(response.cellId);
      return;
    }
    activeCells.add(response.cellId);
    executionCells.set(executionId, response.cellId);
    writeJsonDurable(cellPath(response.cellId), {
      schema: "pi-zerostack.aggregate-cell.v1",
      cell_id: response.cellId,
      execution_id: executionId,
      plan,
      root,
      store,
      journal_path: supervisor.journalPath,
      protocol_digest: supervisor.protocolDigest,
      workers: supervisor.provenance(),
      saved_at: new Date().toISOString(),
    });
  }

  function attachSupervisorHooks(target) {
    target.onJournal = (event, data) => {
      if (event !== "call_admitted" && event !== "call_started") return;
      const executionId = data.trace?.parent_span_id;
      const cellId = executionCells.get(executionId);
      if (!cellId) return;
      const file = cellPath(cellId);
      const record = JSON.parse(fs.readFileSync(file, "utf8"));
      writeJsonDurable(file, {
        ...record,
        protocol_digest: target.protocolDigest,
        workers: target.provenance(),
        provenance_updated_at: new Date().toISOString(),
      });
    };
  }
  attachSupervisorHooks(supervisor);

  async function finalizeExecution(response, executionId, context) {
    if (!response || response.kind === "yielded") return response;
    const failed = response.kind === "terminated" || response.kind === "error" || Boolean(response.errorText);
    if (failed) {
      const rollback = await supervisor.rollbackExecution(executionId, context || {}, undefined);
      if (rollback.length > 0) response.rollback = rollback.map(({ error, ...item }) => ({
        ...item,
        ...(error ? { error: error instanceof Error ? error.message : String(error) } : {}),
      }));
    } else {
      supervisor.completeExecution(executionId);
    }
    return response;
  }

  async function runPlan(plan, root, signal, onUpdate, ctx, store) {
    supervisor.bindStore(root, store.active === false ? null : store.storeRoot);
    const executionId = supervisor.issueExecutionId();
    const source = wrapZeroPlan(plan, root, executionId);
    const yieldMs = Number(process.env.ZERO_AGGREGATE_INITIAL_YIELD_MS || 0);
    activeExecutions += 1;
    let response;
    try {
      response = await (await runtime.getClient()).execute(
        yieldMs > 0 ? `// @exec: ${JSON.stringify({ yield_time_ms: yieldMs })}\n${source}` : source,
        { cwd: root, extensionContext: ctx, onUpdate },
        signal,
        runtime.collectTools(ctx),
      );
    } finally {
      activeExecutions -= 1;
    }
    await finalizeExecution(response, executionId, ctx);
    persistCell(response, plan, root, store, executionId);
    return response;
  }

  async function recoverCell(cellId, context, signal) {
    let record;
    try { record = JSON.parse(fs.readFileSync(cellPath(cellId), "utf8")); } catch { return undefined; }
    if (record?.schema !== "pi-zerostack.aggregate-cell.v1" || record.cell_id !== cellId || typeof record.execution_id !== "string" || typeof record.plan !== "string" || typeof record.root !== "string" || !record.workers || typeof record.workers !== "object") {
      throw Object.assign(new Error(`aggregate cell ${cellId} recovery record is invalid`), { kind: "replay_record_invalid", retryable: false });
    }
    if (record.replay_blocked) {
      throw Object.assign(new Error(`aggregate cell ${cellId} cannot be replayed: ${record.replay_blocked}`), { kind: "replay_unsafe", retryable: false });
    }
    if (Object.keys(record.workers).length > 0 && (typeof record.journal_path !== "string" || !path.isAbsolute(record.journal_path) || !fs.existsSync(record.journal_path))) {
      throw Object.assign(new Error(`aggregate cell ${cellId} recovery journal is unavailable`), { kind: "replay_record_invalid", retryable: false });
    }
    supervisor.bindStore(record.root, record.store?.active === false ? null : record.store?.storeRoot);
    const safety = supervisor.replaySafety(record.journal_path, cellId, record.execution_id);
    if (safety.worker_started_count > 0) {
      if (!record.workers || typeof record.workers !== "object" || typeof record.protocol_digest !== "string") {
        throw Object.assign(new Error(`aggregate cell ${cellId} is missing worker provenance`), { kind: "replay_record_invalid", retryable: false });
      }
      for (const surface of safety.surfaces) {
        if (!record.workers[surface]) throw Object.assign(new Error(`aggregate cell ${cellId} is missing ${surface} worker provenance`), { kind: "replay_record_invalid", retryable: false });
      }
    }
    for (const [surface, expected] of Object.entries(record.workers || {})) {
      const worker = await supervisor.client(surface, record.root);
      await worker.start();
      const pin = worker.pin;
      if (pin.revision !== expected.revision || pin.contractDigest !== expected.contractDigest || pin.registryDigest !== expected.registryDigest) {
        throw Object.assign(new Error(`aggregate cell ${cellId} worker provenance changed for ${surface}`), { kind: "worker_revision_changed", retryable: false });
      }
    }
    if (safety.worker_started_count > 0 && supervisor.protocolDigest !== record.protocol_digest) {
      throw Object.assign(new Error(`aggregate cell ${cellId} raw-worker protocol provenance changed`), { kind: "worker_revision_changed", retryable: false });
    }
    if (!safety.safe) {
      supervisor.restoreEffectsFromJournal(record.execution_id, record.journal_path, cellId);
      const rollback = await supervisor.rollbackExecution(record.execution_id, context?.extensionContext || context || {}, undefined);
      throw Object.assign(new Error(`aggregate cell ${cellId} requires manual recovery: ${safety.reason}`), {
        kind: "replay_unsafe",
        retryable: false,
        rollback: rollback.map(({ error, ...item }) => ({
          ...item,
          ...(error ? { error: error instanceof Error ? error.message : String(error) } : {}),
        })),
      });
    }
    removeCell(cellId);
    activeCells.delete(cellId);
    supervisor.releaseCell(cellId);
    return runPlan(record.plan, record.root, signal, context?.onUpdate, context?.extensionContext, record.store || {});
  }

  function wrapClientRecovery(client) {
    if (!client || client.__piZeroStackRecovery) return client;
    const wait = client.wait.bind(client);
    const terminate = client.terminate.bind(client);
    client.wait = async (cellId, yieldTimeMs, context, signal) => {
      let record;
      try { record = JSON.parse(fs.readFileSync(cellPath(cellId), "utf8")); } catch {}
      const response = await wait(cellId, yieldTimeMs, context, signal);
      if (response.missingCell) {
        const recovered = await recoverCell(cellId, context, signal);
        if (recovered) return recovered;
      }
      if (response.kind !== "yielded") {
        if (record?.execution_id) await finalizeExecution(response, record.execution_id, context?.extensionContext);
        removeCell(cellId);
        activeCells.delete(cellId);
        supervisor.releaseCell(cellId);
      }
      return response;
    };
    client.terminate = async (cellId, context, signal) => {
      let record;
      try { record = JSON.parse(fs.readFileSync(cellPath(cellId), "utf8")); } catch {}
      try {
        const response = await terminate(cellId, context, signal);
        const terminated = { ...(response || {}), kind: "terminated" };
        if (record?.execution_id) await finalizeExecution(terminated, record.execution_id, context?.extensionContext);
        return terminated;
      } finally {
        removeCell(cellId);
        activeCells.delete(cellId);
        supervisor.releaseCell(cellId);
      }
    };
    Object.defineProperty(client, "__piZeroStackRecovery", { value: true });
    return client;
  }

  function monitorHostMemory(client) {
    if (memoryMonitor || !client) return;
    memoryMonitor = setInterval(() => {
      const pid = client.child?.pid;
      if (!pid || memoryCheckActive || (activeExecutions === 0 && activeCells.size === 0)) return;
      memoryCheckActive = true;
      execFile("/bin/ps", ["-o", "rss=", "-p", String(pid)], { timeout: 1_000 }, (error, stdout) => {
        memoryCheckActive = false;
        if (error || client.child?.pid !== pid) return;
        const rssBytes = Number(String(stdout).trim()) * 1024;
        if (!Number.isFinite(rssBytes) || rssBytes <= maxHostRssBytes) return;
        try { supervisor.journal("host_memory_quota_exceeded", { pid, rss_bytes: rssBytes, limit_bytes: maxHostRssBytes }); } catch {}
        for (const cellId of activeCells) blockCellRecovery(cellId, "host memory quota exceeded");
        client.child.kill("SIGKILL");
      });
    }, 500);
    memoryMonitor.unref?.();
  }

  function bind() {
    const state = pi.events?.[Symbol.for("@howaboua/pi-codex-conversion-lite.code-mode")];
    const candidate = state?.runtime || state;
    if (!candidate?.addProvider || !candidate?.getClient || !candidate?.collectTools) return false;
    if (runtime === candidate && providerId) return true;
    if (runtime && providerId) runtime.removeProvider?.(providerId);
    runtime = candidate;
    providerId = runtime.addProvider(provider);
    if (typeof runtime.getClient === "function") {
      void Promise.resolve(runtime.getClient()).then((client) => {
        wrapClientRecovery(client);
        monitorHostMemory(client);
      }).catch(() => {});
    }
    return true;
  }

  return {
    configureSubstrates(value) {
      substrates = value;
      supervisor.configureSubstrates(value);
    },
    bind,
    available() {
      return process.env.ZERO_AGGREGATE_RUNTIME !== "0" && bind();
    },
    async executePlan(plan, root, signal, onUpdate, ctx, store = {}) {
      if (!bind()) throw new Error("shared CodeMode runtime is unavailable");
      const response = await runPlan(plan, root, signal, onUpdate, ctx, store);
      return runtimeResponseToToolResult(response);
    },
    shutdown() {
      if (memoryMonitor) clearInterval(memoryMonitor);
      memoryMonitor = null;
      activeCells.clear();
      executionCells.clear();
      supervisor.shutdown();
      if (runtime && providerId) runtime.removeProvider?.(providerId);
      runtime = null;
      providerId = null;
      supervisor = new RawWorkerSupervisor(options.rawRuntime);
      attachSupervisorHooks(supervisor);
      if (substrates) supervisor.configureSubstrates(substrates);
    },
    get supervisor() {
      return supervisor;
    },
  };
}
