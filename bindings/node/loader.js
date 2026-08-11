'use strict';

// zsx-node loader.
//
// Selects the native addon from, in order:
//   1. an explicit ZSX_NATIVE_ADDON path, or
//   2. a platform prebuild under bindings/node/prebuilds/<platform>-<arch>/zsx_node.node
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
      return 'zsx_node.dll';
    case 'darwin':
      return 'libzsx_node.dylib';
    default:
      return 'libzsx_node.so';
  }
}

function missingAddonError(candidate, dir) {
  const prebuildPath = path.join('bindings/node/prebuilds', dir, 'zsx_node.node');
  return new Error(
    'zsx-node: native addon not found at ' + candidate + '.\n' +
    'Install a prebuild, or build it yourself:\n' +
    '  cd <ZeroStack repo> && cargo build --release -p zsx-node\n' +
    '  cp target/release/' + builtLibraryName() + ' ' + prebuildPath + '\n' +
    'or set ZSX_NATIVE_ADDON=/absolute/path/to/zsx_node.node'
  );
}

function resolveAddon() {
  const explicit = process.env.ZSX_NATIVE_ADDON;
  if (explicit) {
    if (!fs.existsSync(explicit)) {
      throw missingAddonError(explicit, '');
    }
    return explicit;
  }
  const dir = platformDir();
  if (dir === null) {
    throw new Error(
      'zsx-node: unsupported platform "' + process.platform + '/' + process.arch +
      '". Build the addon and set ZSX_NATIVE_ADDON=/absolute/path/to/zsx_node.node'
    );
  }
  const candidate = path.join(__dirname, 'prebuilds', dir, 'zsx_node.node');
  if (!fs.existsSync(candidate)) {
    throw missingAddonError(candidate, dir);
  }
  return candidate;
}

module.exports = require(resolveAddon());
