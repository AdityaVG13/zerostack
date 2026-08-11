# INSTALL-FOR-AGENTS

Install one native ZeroStack runtime. Do not install engine workers or a session sidecar.

## Requirements

- Rust toolchain and `rch`
- Node.js 22 or newer for Pi and OMP
- Sibling `ZeroStack`, `FSZero`, `GraphZero`, and `TokenZero` checkouts
- One reviewed immutable ZeroStack revision pinned by every engine

## Build the universal CLI

From the ZeroStack checkout:

```sh
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo build --locked -p zsx
/tmp/rch_target_zerostack/debug/zsx --version
printf '%s\n' 'return await zero.fs.read({path:"Cargo.toml"});' \
  | /tmp/rch_target_zerostack/debug/zsx exec -C "$PWD"
```

`zsx` embeds FSZero, GraphZero, and TokenZero in one process. It creates no worker process, socket, or daemon.

## Build the Node binding

```sh
./scripts/build-node-prebuild.sh
```

The script uses the size-tuned `release-node` profile, strips symbols, copies the addon, and rejects files at or above 20 MB. Set `ZSX_NATIVE_ADDON` to the built `zsx_node` dynamic library during local development. A packaged install uses the matching file under `bindings/node/prebuilds/<platform>-<arch>/zsx_node.node`.

The loader fails when no exact addon exists. It never invokes Cargo, Git, or a sibling repository.

## Install one harness adapter

Use only the adapter for the active harness:

- Pi: `pi-stack/pi/packages/pi-zsx`
- OMP: `pi-stack/omp/packages/omp-zsx-native`

Both packages import `@zerostack/zsx-native` and call the same native session. They own only harness registration, cancellation forwarding, result rendering, and shutdown.

Never install the retired `pi-zerostack`, `omp-zerostack`, `zerostack-session`, or `zerostack-codemode-host` paths.

## Check the cutover

```sh
uv run python tests/scripts/check_surface_substrate.py --strict-engines \
  "$PWD" "$PWD/../FSZero" "$PWD/../GraphZero" "$PWD/../TokenZero"
uv run python tests/scripts/check_semantic_ownership.py
```

The strict guard must pass before packaging or dogfood.
