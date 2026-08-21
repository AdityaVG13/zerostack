'use strict';

// ZeroKernel loader.
//
// Selects the native addon from, in order:
//   1. an explicit ZERO_KERNEL_NATIVE_ADDON path, or
//   2. a platform prebuild under bindings/node/prebuilds/<platform>-<arch>/zero_kernel_product.node
//
// It never spawns a process, never downloads, and never builds at runtime:
// a missing binary fails with one precise install/build error.

const fs = require('fs');
const path = require('path');

function platformDir() {
  const arch = process.arch;
  switch (process.platform) {
    case 'darwin':
      return 'darwin-' + arch;
    case 'linux':
      return 'linux-' + arch + '-gnu';
    case 'win32':
      return 'win32-' + arch + '-msvc';
    default:
      return null;
  }
}

function builtLibraryName() {
  switch (process.platform) {
    case 'win32':
      return 'zero_kernel_node.dll';
    case 'darwin':
      return 'libzero_kernel_node.dylib';
    default:
      return 'libzero_kernel_node.so';
  }
}

function missingAddonError(candidate, dir) {
  const prebuildPath = path.join('bindings/node/prebuilds', dir, 'zero_kernel_product.node');
  return new Error(
    'ZeroKernel: native addon not found at ' + candidate + '.\n' +
    'Install a prebuild, or build it yourself:\n' +
    '  cd <ZeroStack repo> && cargo build --profile release-node -p zero-kernel-node\n' +
    '  cp target/release-node/' + builtLibraryName() + ' ' + prebuildPath + '\n' +
    'or set ZERO_KERNEL_NATIVE_ADDON=/absolute/path/to/zero_kernel_product.node'
  );
}

function resolveAddon() {
  const explicit = process.env.ZERO_KERNEL_NATIVE_ADDON;
  if (explicit) {
    if (!fs.existsSync(explicit)) {
      throw missingAddonError(explicit, '');
    }
    return explicit;
  }
  const dir = platformDir();
  if (dir === null) {
    throw new Error(
      'ZeroKernel: unsupported platform "' + process.platform + '/' + process.arch +
      '". Build the addon and set ZERO_KERNEL_NATIVE_ADDON=/absolute/path/to/zero_kernel_product.node'
    );
  }
  const candidate = path.join(__dirname, 'prebuilds', dir, 'zero_kernel_product.node');
  if (!fs.existsSync(candidate)) {
    throw missingAddonError(candidate, dir);
  }
  return candidate;
}

module.exports = require(resolveAddon());
