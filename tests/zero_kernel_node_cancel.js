'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { ZeroKernel } = require('../bindings/node');

async function main() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'zero-kernel-node-cancel-'));
  const kernel = new ZeroKernel({
    root,
    sessionId: 'node-cancel',
    wallMs: 30_000,
  });
  try {
    await kernel.initialize();
    const ac = new AbortController();
    const pending = kernel.executeCell(
      "await new Promise(() => {}); return 'should-not-complete';",
      ac.signal,
    );
    const abortAt = setTimeout(() => ac.abort(), 80);
    let result;
    try {
      result = await pending;
    } catch (error) {
      clearTimeout(abortAt);
      throw new Error(
        `ZeroKernel abort must return a structured terminal outcome, not throw: ${
          error && error.stack ? error.stack : error
        }`,
      );
    }
    clearTimeout(abortAt);
    if (!result || result.outcome !== 'Cancelled') {
      throw new Error(`expected outcome Cancelled, got ${JSON.stringify(result)}`);
    }
    if (
      result.error &&
      typeof result.error.detail === 'string' &&
      result.error.detail.includes('frame did not quiesce')
    ) {
      throw new Error(`cancel outcome replaced by quiescence: ${result.error.detail}`);
    }
    const afterCell = kernel.status();
    if (afterCell.liveTasks !== 0 || afterCell.liveProcesses !== 0 || afterCell.liveFrames !== 0) {
      throw new Error(`leaked after cancelled cell: ${JSON.stringify(afterCell)}`);
    }
    await kernel.shutdown();
    const afterShutdown = kernel.status();
    if (
      afterShutdown.liveTasks !== 0 ||
      afterShutdown.liveProcesses !== 0 ||
      afterShutdown.liveFrames !== 0 ||
      afterShutdown.terminated !== true
    ) {
      throw new Error(`leaked after shutdown: ${JSON.stringify(afterShutdown)}`);
    }
    process.stdout.write(
      JSON.stringify({
        outcome: result.outcome,
        error: result.error || null,
        afterCell,
        afterShutdown,
      }) + '\n',
    );
    process.stderr.write(
      `node-cancel: ok outcome=${result.outcome} liveTasks=${afterShutdown.liveTasks}\n`,
    );
  } catch (error) {
    try {
      await kernel.shutdown();
    } catch {
      // Keep the original failure.
    }
    throw error;
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
