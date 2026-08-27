#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '../../..');
const bindingPkgPath = path.join(root, 'bindings/node/package.json');
const loaderPath = path.join(root, 'bindings/node/loader.js');
const typesPath = path.join(root, 'bindings/node/zero-kernel.d.ts');

function fail(message) {
  console.error(`validate: ${message}`);
  process.exit(1);
}

if (!fs.existsSync(bindingPkgPath)) fail(`missing ${bindingPkgPath}`);
if (!fs.existsSync(loaderPath)) fail(`missing ${loaderPath}`);
if (!fs.existsSync(typesPath)) fail(`missing ${typesPath}`);

let pkg;
try {
  pkg = JSON.parse(fs.readFileSync(bindingPkgPath, 'utf8'));
} catch (error) {
  fail(`invalid bindings/node/package.json: ${error.message}`);
}

if (pkg.name !== '@zerostack/zero-kernel') {
  fail(`bindings/node/package.json name must be @zerostack/zero-kernel, got ${JSON.stringify(pkg.name)}`);
}

if (
  !pkg.files ||
  !pkg.files.includes('loader.js') ||
  !pkg.files.includes('zero-kernel.d.ts') ||
  !pkg.files.includes('prebuilds/*/zero_kernel_product.node')
) {
  fail('bindings/node/package.json files must include loader.js, zero-kernel.d.ts, and canonical product prebuilds only');
}


const loader = fs.readFileSync(loaderPath, 'utf8');
if (!loader.includes('resolveAddon')) {
  fail('bindings/node/loader.js does not look like the ZeroKernel loader');
}

const prebuildDir = path.join(root, 'bindings/node/prebuilds');
const hasPrebuild = fs.existsSync(prebuildDir) && fs.readdirSync(prebuildDir).length > 0;

console.log(`validate: ${pkg.name}@${pkg.version} loader and types present`);
console.log(`validate: prebuilds ${hasPrebuild ? 'found' : 'not found (build with build-prebuild.sh)'}`);
console.log('validate: ok - real binding lives in bindings/node, no duplication in packaging/package/npm');
