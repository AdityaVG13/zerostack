import { parseArgs } from "node:util";
import { mkdirSync } from "node:fs";

const { values } = parseArgs({
  options: {
    addon: { type: "string" },
    root: { type: "string", default: process.cwd() },
    "state-root": { type: "string", default: `${process.cwd()}/gc/native-warm-read` },
    path: { type: "string", default: "bench/native-warm-read/fixture.txt" },
    runs: { type: "string", default: "1000" },
    warmup: { type: "string", default: "50" },
  },
});

if (!values.addon) throw new Error("--addon is required");
const runs = Number.parseInt(values.runs, 10);
const warmup = Number.parseInt(values.warmup, 10);
if (!Number.isSafeInteger(runs) || runs < 1) throw new Error("--runs must be positive");
if (!Number.isSafeInteger(warmup) || warmup < 0) throw new Error("--warmup must be nonnegative");

const baselineRssBytes = process.memoryUsage().rss;
const addon = { exports: {} };
process.dlopen(addon, values.addon);
const { NativeZsxSession } = addon.exports;
mkdirSync(values["state-root"], { recursive: true });
const session = new NativeZsxSession(values.root, "native-warm-read", values["state-root"]);
const plan = `return await zero.fs.compound("read",{path:${JSON.stringify(values.path)}});`;

function percentile(samples, fraction) {
  const sorted = [...samples].sort((left, right) => left - right);
  return sorted[Math.ceil(sorted.length * fraction) - 1];
}

async function execute() {
  const result = await session.execute(plan, 30_000);
  if (!result.ok) throw new Error(JSON.stringify(result));
  return result;
}

try {
  for (let index = 0; index < warmup; index += 1) await execute();
  const outer = [];
  const host = [];
  const engine = [];
  const overhead = [];
  const envelopeBytes = [];
  for (let index = 0; index < runs; index += 1) {
    const started = process.hrtime.bigint();
    const result = await execute();
    outer.push(Number(process.hrtime.bigint() - started));
    host.push(result.metrics.host.wall_time_ns);
    engine.push(result.metrics.engine_wall_ns[0]);
    overhead.push(result.metrics.runtime_overhead_lower_bound_ns);
    envelopeBytes.push(Buffer.byteLength(JSON.stringify(result)));
  }
  const summarize = (samples) => ({
    p50: percentile(samples, 0.5),
    p95: percentile(samples, 0.95),
    p99: percentile(samples, 0.99),
  });
  const idleCpuPercent = [];
  const idleRssBytes = [];
  for (let index = 0; index < 20; index += 1) {
    const cpuStarted = process.cpuUsage();
    const wallStarted = process.hrtime.bigint();
    await new Promise((resolve) => setTimeout(resolve, 100));
    const elapsedUs = Number(process.hrtime.bigint() - wallStarted) / 1_000;
    const cpu = process.cpuUsage(cpuStarted);
    idleCpuPercent.push(((cpu.user + cpu.system) / elapsedUs) * 100);
    idleRssBytes.push(process.memoryUsage().rss);
  }
  console.log(JSON.stringify({
    schema: "zerostack.native_warm_read.v1",
    runs,
    warmup,
    outer_ns: summarize(outer),
    host_ns: summarize(host),
    engine_ns: summarize(engine),
    runtime_overhead_lower_bound_ns: summarize(overhead),
    envelope_bytes: summarize(envelopeBytes),
    idle_cpu_percent: summarize(idleCpuPercent),
    rss_bytes: {
      baseline: baselineRssBytes,
      idle_p95: percentile(idleRssBytes, 0.95),
      idle_delta_p95: percentile(idleRssBytes, 0.95) - baselineRssBytes,
    },
    status: session.status(),
  }));
} finally {
  await session.shutdown();
}
