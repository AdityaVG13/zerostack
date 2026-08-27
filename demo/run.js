'use strict';

const path = require('node:path');
const { ZeroKernel } = require('../bindings/node');

const ROOT = path.resolve(__dirname, '..');

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
    process.stdout.write(JSON.stringify(result, null, 2) + '\n');
  } finally {
    await kernel.shutdown();
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
