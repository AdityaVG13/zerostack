# INSTALL-FOR-AGENTS -- local development only

ZeroStack has no published install. Do not publish, download, sign, tag, or
distribute bundles, native prebuilds, registry packages, or harness adapters.
The bundle installer, platform matrix, and lifecycle schemas are retained only
as local conformance fixtures. They do not authorize a release workflow.

`pi-zsx` remains internal. Use it only from a local `pi-stack` checkout unless
the owner gives new explicit publication approval in the future.

## Developer requirements

- Rust toolchain and `rch`
- Node.js 22 or newer for Pi and OMP
- Sibling `ZeroStack`, `FSZero`, `GraphZero`, and `TokenZero` checkouts
- One reviewed immutable ZeroStack revision pinned by every engine

## Build the development CLI from source

From the ZeroStack checkout:

```sh
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo build --locked -p zsx
/tmp/rch_target_zerostack/debug/zsx --version
printf '%s\n' 'return await zero.fs.read({path:"Cargo.toml"});' \
  | /tmp/rch_target_zerostack/debug/zsx exec -C "$PWD"
```

`zsx` is a development surface over the reviewed ZeroStack boundaries. It is
not a distributable product or a release artifact.

## Build the Node binding

```sh
./scripts/build-node-prebuild.sh
```

The script uses the size-tuned `release-node` Cargo profile, strips symbols,
copies the addon, and rejects files at or above 20 MB. Here `release-node`
means an optimized local build profile, not publication. Set `ZSX_NATIVE_ADDON`
to the built `zsx_node` dynamic library during local development.

The loader fails when no exact addon exists. It never invokes Cargo, Git, or a sibling repository.

## Use one internal harness adapter locally

Use only the adapter for the active harness:

- Pi: `pi-stack/pi/packages/pi-zsx`
- OMP: `pi-stack/omp/packages/omp-zsx-native`

Both packages are internal. Do not publish them. They own only harness
registration, cancellation forwarding, result rendering, and shutdown.

Never install the retired `pi-zerostack`, `omp-zerostack`, `zerostack-session`, or `zerostack-codemode-host` paths.

## Check the cutover

```sh
uv run python tests/scripts/check_surface_substrate.py --strict-engines \
  "$PWD" "$PWD/../FSZero" "$PWD/../GraphZero" "$PWD/../TokenZero"
uv run python tests/scripts/check_semantic_ownership.py
```

The strict guard must pass before local dogfood.
