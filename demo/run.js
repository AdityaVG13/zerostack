'use strict';

const path = require('node:path');
const { ZeroKernel } = require('../bindings/node');

const ROOT = path.resolve(__dirname, '..');

function cellValue(result) {
  if (typeof result.value === 'string') {
    try {
      return JSON.parse(result.value);
    } catch {
      return result.value;
    }
  }
  return result.value;
}

async function main() {
  const kernel = new ZeroKernel({ root: ROOT, sessionId: 'demo' });
  try {
    await kernel.initialize();

    const source = `
const [readme, found] = await Promise.all([
  z.read('README.md'),
  z.find('launch_cell', { mode: 'word', path: 'crates/zerostack/zero-kernel/src/runtime.rs', limit: 5 })
]);
return {
  readmePreview: typeof readme === 'string' ? readme.slice(0, 800) : (readme.view?.text || null),
  readmeVisibleBytes: typeof readme === 'string' ? readme.length : (readme.view?.text?.length || null),
  findHits: found.hits.slice(0, 3),
  findCount: found.hits.length
};
`.trim();

    const result = await kernel.executeCell(source);
    if (result.outcome !== 'Completed') {
      throw new Error(result.error?.detail || 'ZeroKernel demo cell failed');
    }
    const value = cellValue(result);
    if (!value || typeof value.findCount !== 'number' || value.findCount < 1) {
      throw new Error('ZeroKernel demo expected nonzero GraphZero hits');
    }
    if (!Array.isArray(result.handles) || result.handles.length < 1) {
      throw new Error('ZeroKernel demo expected exact recovery handles');
    }
    const afterCell = kernel.status();
    if (afterCell.liveTasks !== 0 || afterCell.liveProcesses !== 0 || afterCell.liveFrames !== 0) {
      throw new Error(
        `ZeroKernel demo leaked resources after cell: liveTasks=${afterCell.liveTasks} liveProcesses=${afterCell.liveProcesses} liveFrames=${afterCell.liveFrames}`,
      );
    }

    process.stdout.write(JSON.stringify(result, null, 2) + '\n');

    await kernel.shutdown();
    const afterShutdown = kernel.status();
    if (
      afterShutdown.liveTasks !== 0 ||
      afterShutdown.liveProcesses !== 0 ||
      afterShutdown.liveFrames !== 0 ||
      afterShutdown.terminated !== true
    ) {
      throw new Error(
        `ZeroKernel demo leaked resources after shutdown: ${JSON.stringify(afterShutdown)}`,
      );
    }
    process.stderr.write(
      `demo: ok outcome=${result.outcome} hits=${value.findCount} handles=${result.handles.length} liveTasks=${afterShutdown.liveTasks}\n`,
    );
  } catch (error) {
    try {
      await kernel.shutdown();
    } catch {
      // Keep the original failure.
    }
    throw error;
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
