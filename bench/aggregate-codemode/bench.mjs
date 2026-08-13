#!/usr/bin/env node
import { createHash } from "node:crypto";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { parseArgs } from "node:util";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const defaultAddon = path.join(
  repoRoot,
  "bindings/node/prebuilds",
  `${process.platform}-${process.arch}`,
  "zsx_node.node",
);
const { values } = parseArgs({
  options: {
    addon: { type: "string", default: defaultAddon },
    output: { type: "string" },
    "cold-runs": { type: "string", default: "2" },
    "warm-runs": { type: "string", default: "10" },
    repetitions: { type: "string", default: "2" },
    warmup: { type: "string", default: "1" },
    "tolerance-relative": { type: "string", default: "0.50" },
    "tolerance-absolute-ns": { type: "string", default: "2000000" },
  },
});

function positiveInteger(name, raw, allowZero = false) {
  const value = Number.parseInt(raw, 10);
  if (!Number.isSafeInteger(value) || value < (allowZero ? 0 : 1)) {
    throw new Error(`--${name} must be ${allowZero ? "nonnegative" : "positive"}`);
  }
  return value;
}
const config = {
  coldRuns: positiveInteger("cold-runs", values["cold-runs"]),
  warmRuns: positiveInteger("warm-runs", values["warm-runs"]),
  repetitions: positiveInteger("repetitions", values.repetitions),
  warmup: positiveInteger("warmup", values.warmup, true),
  toleranceRelative: Number(values["tolerance-relative"]),
  toleranceAbsoluteNs: positiveInteger("tolerance-absolute-ns", values["tolerance-absolute-ns"]),
};
if (!Number.isFinite(config.toleranceRelative) || config.toleranceRelative < 0) {
  throw new Error("--tolerance-relative must be nonnegative");
}
if (config.toleranceRelative >= 0.5) {
  console.error(
    "aggregate-codemode: tolerance-relative>=0.50 is a smoke window, not a keep-gate",
  );
}

const addonPath = path.resolve(values.addon);
const addon = { exports: {} };
process.dlopen(addon, addonPath);
const { NativeZsxSession } = addon.exports;
if (typeof NativeZsxSession !== "function") {
  throw new Error(`NativeZsxSession missing from ${addonPath}`);
}

const MODES = ["aggregate_one_cell", "standalone_per_engine", "per_operation"];
const WORKLOADS = ["mixed", "ref_heavy", "shell", "mutation", "cancellation", "failure"];

const mixedPlans = {
  aggregate_one_cell: [
    `const f=await zero.fs.compound("read",{path:"notes/alpha.md"});
     await zero.graph.index();
     const g=await zero.graph.orient("context","architecture");
     const t=await zero.token.shell("printf mixed-ok");
     return [f.content.value.metadata.ownership.engine,
             g.content.value.metadata.ownership.engine,
             t.content.value.metadata.ownership.engine];`,
  ],
  standalone_per_engine: [
    `const f=await zero.fs.compound("read",{path:"notes/alpha.md"});
     return f.content.value.metadata.ownership.engine;`,
    `await zero.graph.index(); const g=await zero.graph.orient("context","architecture");
     return g.content.value.metadata.ownership.engine;`,
    `const t=await zero.token.shell("printf mixed-ok");
     return t.content.value.metadata.ownership.engine;`,
  ],
  per_operation: [
    `const f=await zero.fs.compound("read",{path:"notes/alpha.md"});
     return f.content.value.metadata.ownership.engine;`,
    `const g=await zero.graph.index(); return g.content.value.metadata.ownership.engine;`,
    `const g=await zero.graph.orient("context","architecture");
     return g.content.value.metadata.ownership.engine;`,
    `const t=await zero.token.shell("printf mixed-ok");
     return t.content.value.metadata.ownership.engine;`,
  ],
};
const shellPlan = `const r=await zero.token.shell("printf shell-cell-ok");
  return String(r.content.value.value.visible).includes("shell-cell-ok");`;
const mutationPlans = {
  aggregate_one_cell: [
    `await zero.token.shell("printf mutation-ok > changed.txt");
     const r=await zero.fs.compound("read",{path:"changed.txt"});
     return JSON.stringify(r.content.value.value).includes("mutation-ok");`,
  ],
  standalone_per_engine: [
    `await zero.token.shell("printf mutation-ok > changed.txt"); return true;`,
    `const r=await zero.fs.compound("read",{path:"changed.txt"});
     return JSON.stringify(r.content.value.value).includes("mutation-ok");`,
  ],
  per_operation: [
    `await zero.token.shell("printf mutation-ok > changed.txt"); return true;`,
    `const r=await zero.fs.compound("read",{path:"changed.txt"});
     return JSON.stringify(r.content.value.value).includes("mutation-ok");`,
  ],
};
const cancellationPlan = `return await zero.token.shell("sleep 0.2");`;
const failurePlan = `return await zero.fs.compound("read",{path:"definitely-missing.txt"});`;
const refAggregatePlan = `const r=await zero.token.read({path:"large.txt"});
  const refs=r.content.value.metadata.ownership.refs;
  const e=await zero.token.expand(refs[0]);
  return refs[0].startsWith("tz://blob/") && JSON.stringify(e.content.value.value).length>1000;`;
const refReadPlan = `const r=await zero.token.read({path:"large.txt"});
  return r.content.value.metadata.ownership.refs[0];`;
const refExpandPlan = (reference) => `const e=await zero.token.expand(${JSON.stringify(reference)});
  return JSON.stringify(e.content.value.value).length>1000;`;

const corpus = {
  mixed: mixedPlans,
  ref_heavy: {
    aggregate_one_cell: [refAggregatePlan],
    standalone_per_engine: [refAggregatePlan],
    per_operation: [refReadPlan, "<ref-dependent-expand>"],
  },
  shell: Object.fromEntries(MODES.map((mode) => [mode, [shellPlan]])),
  mutation: mutationPlans,
  cancellation: Object.fromEntries(MODES.map((mode) => [mode, [cancellationPlan]])),
  failure: Object.fromEntries(MODES.map((mode) => [mode, [failurePlan]])),
};

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}
function git(args, cwd) {
  try {
    return execFileSync("git", args, {
      cwd,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return null;
  }
}
function repositoryIdentity(name, cwd) {
  const head = git(["rev-parse", "HEAD"], cwd);
  const status = git(["status", "--porcelain=v1", "--untracked-files=normal"], cwd);
  const trackedDiff = git(["diff", "--binary", "HEAD"], cwd) ?? "";
  return {
    name,
    head,
    dirty: status === null ? null : status.length > 0,
    dirty_entry_count: status === null || status === "" ? 0 : status.split("\n").length,
    worktree_state_sha256: status === null ? null : sha256(`${status}\n${trackedDiff}`),
  };
}
function fixtureRoot(prefix) {
  const root = mkdtempSync(path.join(os.tmpdir(), `zsx-e2e-${prefix}-`));
  mkdirSync(path.join(root, "notes"), { recursive: true });
  writeFileSync(path.join(root, "notes/alpha.md"), "alpha architecture benchmark\nline2\n");
  writeFileSync(path.join(root, "large.txt"), "ref-heavy benchmark payload\n".repeat(1600));
  return root;
}
function fixtureContentDigest(root) {
  const digest = createHash("sha256");
  for (const relative of ["notes/alpha.md", "large.txt"]) {
    digest.update(relative);
    digest.update("\0");
    digest.update(readFileSync(path.join(root, relative)));
    digest.update("\0");
  }
  return digest.digest("hex");
}
function percentile(values, fraction) {
  const sorted = [...values].sort((left, right) => left - right);
  return sorted[Math.max(0, Math.ceil(sorted.length * fraction) - 1)];
}
function summarize(values) {
  const mean = values.reduce((sum, value) => sum + value, 0) / values.length;
  const variance = values.length < 2
    ? 0
    : values.reduce((sum, value) => sum + ((value - mean) ** 2), 0) / (values.length - 1);
  const standardDeviation = Math.sqrt(variance);
  return {
    count: values.length,
    min: Math.min(...values),
    max: Math.max(...values),
    mean,
    standard_deviation: standardDeviation,
    coefficient_of_variation: mean === 0 ? 0 : standardDeviation / mean,
    p50: percentile(values, 0.50),
    p95: percentile(values, 0.95),
    p99: percentile(values, 0.99),
  };
}
function errorKind(envelope) {
  const detail = String(envelope?.error?.detail ?? "");
  if (detail.includes("deadline exceeded")) return "cancelled";
  if (detail.includes("not_found") || detail.includes("not found")) return "not_found";
  return envelope?.error?.code ?? "unknown_error";
}
function canonicalize(workload, envelopes) {
  if (workload === "cancellation" || workload === "failure") {
    return errorKind(envelopes.at(-1));
  }
  const results = envelopes.map((envelope) => envelope.result);
  if (workload === "mixed") {
    const engines = results.flat().filter((value) => typeof value === "string");
    return [...new Set(engines)].sort();
  }
  return results.every((value) => value === true || (typeof value === "string" && value.startsWith("tz://blob/")));
}
function expectedCanonical(workload) {
  if (workload === "mixed") return ["fszero", "graphzero", "tokenzero"];
  if (workload === "cancellation") return "cancelled";
  if (workload === "failure") return "not_found";
  return true;
}

async function executeLogical(session, workload, mode) {
  let plans = corpus[workload][mode];
  const envelopes = [];
  const executedPlans = [];
  for (let index = 0; index < plans.length; index += 1) {
    let plan = plans[index];
    if (plan === "<ref-dependent-expand>") {
      const reference = envelopes[0]?.result;
      if (typeof reference !== "string" || !reference.startsWith("tz://blob/")) {
        throw new Error(`ref-heavy producer did not return a canonical ref: ${JSON.stringify(reference)}`);
      }
      plan = refExpandPlan(reference);
    }
    executedPlans.push(plan);
    const timeoutMs = workload === "cancellation" ? 2 : 30_000;
    const envelope = await session.execute(plan, timeoutMs);
    envelopes.push(envelope);
    if (!envelope.ok && workload !== "cancellation" && workload !== "failure") break;
  }
  return { envelopes, plans: executedPlans, canonical: canonicalize(workload, envelopes) };
}

let trialSequence = 0;
async function runTrial({ repetition, phase, index, workload, mode, session, ownedRoot }) {
  const fixtureContentSha256 = fixtureContentDigest(ownedRoot);
  const cpuBefore = process.cpuUsage();
  const rssBefore = process.memoryUsage().rss;
  const started = process.hrtime.bigint();
  const execution = await executeLogical(session, workload, mode);
  const wallNs = Number(process.hrtime.bigint() - started);
  const cpu = process.cpuUsage(cpuBefore);
  const rssAfter = process.memoryUsage().rss;
  const encoded = JSON.stringify(execution.envelopes);
  const inputBytes = execution.plans.reduce((sum, plan) => sum + Buffer.byteLength(plan), 0);
  const outputBytes = Buffer.byteLength(encoded);
  const innerWallNs = execution.envelopes.reduce(
    (sum, envelope) => sum + Number(envelope?.metrics?.host?.wall_time_ns ?? 0),
    0,
  );
  const canonicalJson = JSON.stringify(execution.canonical);
  const expectedJson = JSON.stringify(expectedCanonical(workload));
  return {
    sequence: ++trialSequence,
    repetition,
    phase,
    index,
    workload,
    mode,
    wall_ns: wallNs,
    inner_wall_ns: innerWallNs,
    process_cpu_user_us: cpu.user,
    process_cpu_system_us: cpu.system,
    rss_before_bytes: rssBefore,
    rss_after_bytes: rssAfter,
    rss_delta_bytes: rssAfter - rssBefore,
    input_bytes: inputBytes,
    output_bytes: outputBytes,
    model_visible_turns: execution.plans.length,
    token_accounting: {
      kind: "conservative_utf8_bytes_div_4",
      upper_bound_tokens: Math.ceil((inputBytes + outputBytes) / 4),
      exact_model_tokens: null,
    },
    envelope_digest: sha256(encoded),
    envelope_statuses: execution.envelopes.map((envelope) =>
      envelope.ok ? "ok" : errorKind(envelope)),
    result_digest: sha256(canonicalJson),
    canonical_result: execution.canonical,
    expected_result: expectedCanonical(workload),
    differential_correct: canonicalJson === expectedJson,
    fixture_identity_sha256: sha256(ownedRoot),
    fixture_content_sha256: fixtureContentSha256,
  };
}

const trials = [];
const runtimeStatuses = [];
for (let repetition = 1; repetition <= config.repetitions; repetition += 1) {
  for (const workload of WORKLOADS) {
    for (const mode of MODES) {
      for (let index = 1; index <= config.coldRuns; index += 1) {
        const root = fixtureRoot(`${repetition}-${workload}-${mode}-cold-${index}`);
        const session = new NativeZsxSession(root, `cold-${repetition}-${workload}-${mode}-${index}`, null);
        try {
          trials.push(await runTrial({ repetition, phase: "cold", index, workload, mode, session, ownedRoot: root }));
          runtimeStatuses.push(session.status());
        } finally {
          await session.shutdown();
          rmSync(root, { recursive: true, force: true });
        }
      }
      const root = fixtureRoot(`${repetition}-${workload}-${mode}-warm`);
      const session = new NativeZsxSession(root, `warm-${repetition}-${workload}-${mode}`, null);
      try {
        for (let index = 0; index < config.warmup; index += 1) {
          await executeLogical(session, workload, mode);
        }
        for (let index = 1; index <= config.warmRuns; index += 1) {
          trials.push(await runTrial({ repetition, phase: "warm", index, workload, mode, session, ownedRoot: root }));
        }
        runtimeStatuses.push(session.status());
      } finally {
        await session.shutdown();
        rmSync(root, { recursive: true, force: true });
      }
    }
  }
}

const summaries = [];
for (const workload of WORKLOADS) {
  for (const mode of MODES) {
    for (const phase of ["cold", "warm"]) {
      const selected = trials.filter((trial) => trial.workload === workload && trial.mode === mode && trial.phase === phase);
      summaries.push({
        workload,
        mode,
        phase,
        wall_ns: summarize(selected.map((trial) => trial.wall_ns)),
        cpu_user_us: summarize(selected.map((trial) => trial.process_cpu_user_us)),
        cpu_system_us: summarize(selected.map((trial) => trial.process_cpu_system_us)),
        rss_after_bytes: summarize(selected.map((trial) => trial.rss_after_bytes)),
        rss_delta_bytes: summarize(selected.map((trial) => trial.rss_delta_bytes)),
        input_bytes: summarize(selected.map((trial) => trial.input_bytes)),
        output_bytes: summarize(selected.map((trial) => trial.output_bytes)),
        model_visible_turns: summarize(selected.map((trial) => trial.model_visible_turns)),
        conservative_tokens: summarize(selected.map((trial) => trial.token_accounting.upper_bound_tokens)),
      });
    }
  }
}

const comparisons = [];
for (const workload of WORKLOADS) {
  for (const phase of ["cold", "warm"]) {
    const rows = summaries.filter((summary) => summary.workload === workload && summary.phase === phase);
    const best = Math.min(...rows.map((row) => row.wall_ns.p50));
    const winners = rows.filter((row) => row.wall_ns.p50 <= best * 1.05).map((row) => row.mode);
    comparisons.push({
      workload,
      phase,
      outcome: winners.length > 1 ? "tie_within_5_percent" : "winner",
      winners,
      p50_wall_ns: Object.fromEntries(rows.map((row) => [row.mode, row.wall_ns.p50])),
      loss_ratio_to_best: Object.fromEntries(rows.map((row) => [row.mode, row.wall_ns.p50 / best])),
    });
  }
}

const rerunTolerance = [];
for (const workload of WORKLOADS) {
  for (const mode of MODES) {
    for (const phase of ["cold", "warm"]) {
      const p50s = [];
      for (let repetition = 1; repetition <= config.repetitions; repetition += 1) {
        const selected = trials.filter((trial) => trial.repetition === repetition && trial.workload === workload && trial.mode === mode && trial.phase === phase);
        p50s.push(summarize(selected.map((trial) => trial.wall_ns)).p50);
      }
      const baseline = p50s[0];
      const allowed = Math.max(config.toleranceAbsoluteNs, baseline * config.toleranceRelative);
      const spread = Math.max(...p50s) - Math.min(...p50s);
      rerunTolerance.push({ workload, mode, phase, p50_wall_ns_by_repetition: p50s, spread_ns: spread, allowed_spread_ns: allowed, passed: spread <= allowed });
    }
  }
}

const repoParent = path.dirname(repoRoot);
const receipt = {
  schema_version: 1,
  schema: "zerostack.aggregate_codemode_benchmark.v1",
  bead_key: "zerostack-codemode-first-execution-jgm.6",
  captured_at: new Date().toISOString(),
  benchmark_semantics: {
    canonical_runtime: "zsx-core in-process through @zerostack/zsx-native",
    retired_sidecar: "not measured because it is no longer a supported execution authority",
    aggregate_one_cell: "one model-visible cell composes all logical operations",
    standalone_per_engine: "one model-visible cell per participating engine",
    per_operation: "one model-visible cell per logical operation",
    provider_use: "none",
  },
  config,
  corpus: {
    workloads: WORKLOADS,
    modes: MODES,
    sha256: sha256(JSON.stringify(corpus)),
    definition: corpus,
  },
  provenance: {
    source_repositories: [
      repositoryIdentity("ZeroStack", repoRoot),
      repositoryIdentity("FSZero", path.join(repoParent, "FSZero")),
      repositoryIdentity("GraphZero", path.join(repoParent, "GraphZero")),
      repositoryIdentity("TokenZero", path.join(repoParent, "TokenZero")),
    ],
    addon: {
      path: path.relative(repoRoot, addonPath),
      sha256: sha256(readFileSync(addonPath)),
      bytes: readFileSync(addonPath).length,
      source_binding: "digest_only; this run does not infer a source revision from the binary",
    },
    machine: {
      hostname_sha256: sha256(os.hostname()),
      platform: process.platform,
      architecture: process.arch,
      release: os.release(),
      cpu: os.cpus()[0]?.model ?? "unknown",
      logical_cpu_count: os.cpus().length,
      total_memory_bytes: os.totalmem(),
      node: process.version,
    },
    runtime_status_digests: [...new Set(runtimeStatuses.map((status) => sha256(JSON.stringify(status))))],
  },
  trials,
  summaries,
  comparisons,
  rerun_tolerance: {
    semantics: "two repetitions in one invocation; every repetition uses fresh cold sessions and an independently warmed session",
    declared: {
      relative_to_first_repetition: config.toleranceRelative,
      absolute_floor_ns: config.toleranceAbsoluteNs,
    },
    checks: rerunTolerance,
    passed: rerunTolerance.every((entry) => entry.passed),
  },
  differential_correctness: {
    expected_by_workload: Object.fromEntries(WORKLOADS.map((workload) => [workload, expectedCanonical(workload)])),
    failures: trials.filter((trial) => !trial.differential_correct).map((trial) => trial.sequence),
    passed: trials.every((trial) => trial.differential_correct),
  },
  accepted_optimizations: [],
  result: rerunTolerance.every((entry) => entry.passed) && trials.every((trial) => trial.differential_correct) ? "passed" : "failed",
  residual_assumptions: [
    "The committed addon is measured by digest; this benchmark does not infer or fabricate its source revision.",
    "Conservative token counts are UTF-8 request plus JSON response bytes divided by four; exact provider/model tokens are unavailable because this provider-free benchmark makes no model call.",
    "Process RSS is sampled before and after each trial, not continuously, so sub-trial peaks may be missed.",
    "macOS process.cpuUsage values cover the Node process and in-process engine work; user shell descendants are not included.",
    "The repository AGENTS.md still names q6am as an open sidecar frontier even though the shipped install/runtime path retires sidecar authority; this receipt measures the shipped in-process successor only.",
  ],
};

const rendered = `${JSON.stringify(receipt, null, 2)}\n`;
if (values.output) {
  const outputPath = path.resolve(values.output);
  mkdirSync(path.dirname(outputPath), { recursive: true });
  writeFileSync(outputPath, rendered);
  writeFileSync(`${outputPath}.sha256`, `${sha256(rendered)}  ${path.basename(outputPath)}\n`);
  console.log(outputPath);
} else {
  process.stdout.write(rendered);
}
if (receipt.result !== "passed") process.exitCode = 1;
