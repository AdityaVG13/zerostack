'use strict';

const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { performance } = require('node:perf_hooks');
const { ZeroKernel } = require('../bindings/node');

const RUNS = 20;
const ROOT = path.resolve(__dirname, '..');
const OUTPUT = path.join(__dirname, 'zero-kernel-reference.json');

function percentile(values, p) {
  const sorted = [...values].sort((a, b) => a - b);
  const index = (sorted.length - 1) * p;
  const lower = Math.floor(index);
  const upper = Math.ceil(index);
  return sorted[lower] + (sorted[upper] - sorted[lower]) * (index - lower);
}

function summarize(samples) {
  return {
    p50_ms: percentile(samples, 0.5),
    p95_ms: percentile(samples, 0.95),
    samples_ms: samples,
  };
}

async function measure(kernel, source) {
  const samples = [];
  for (let i = 0; i < RUNS; i += 1) {
    const started = performance.now();
    try {
      await kernel.executeCell(source);
    } catch (error) {
      throw new Error(`benchmark cell failed at sample ${i + 1}`, { cause: error });
    }
    samples.push(performance.now() - started);
  }
  return summarize(samples);
}

async function main() {
  const kernel = new ZeroKernel({ root: ROOT, sessionId: 'reference-benchmark' });
  let report;
  try {
    const initializedAt = performance.now();
    await kernel.initialize();
    const initializationMs = performance.now() - initializedAt;

    await kernel.executeCell('return 1');
    const noopFrame = await measure(kernel, 'return 1');
    const readFile = await measure(kernel, "return await z.read('Cargo.toml')");
    const fixture = await fs.stat(path.join(ROOT, 'Cargo.toml'));

    report = {
      schema: 'zerokernel.reference-benchmark.v1',
      measured_at: new Date().toISOString(),
      environment: {
        platform: os.platform(),
        architecture: os.arch(),
        cpu: os.cpus()[0]?.model ?? 'unknown',
        memory_bytes: os.totalmem(),
        node: process.version,
        binding: 'packaged platform prebuild',
      },
      method: {
        runs_per_operation: RUNS,
        warmups: 1,
        host_lifecycle: 'one reusable host; one fresh bounded frame per cell',
        concurrency: 'sequential',
        dropped_samples: 0,
      },
      initialization_ms: initializationMs,
      operations: {
        noop_frame: noopFrame,
        read_file: {
          fixture: 'Cargo.toml',
          fixture_bytes: fixture.size,
          ...readFile,
        },
      },
    };
  } finally {
    await kernel.shutdown();
  }

  await fs.writeFile(OUTPUT, JSON.stringify(report, null, 2) + '\n');
  process.stdout.write(JSON.stringify(report) + '\n');
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
