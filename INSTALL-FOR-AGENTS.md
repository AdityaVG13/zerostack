# INSTALL-FOR-AGENTS

Install one native ZeroStack runtime. Do not install engine workers or a session sidecar.

## Install a signed prebuilt bundle

Production installs do not need Git, Cargo, Rust, or sibling checkouts. They need Python 3.10+ and `minisign` for detached signature verification:

```sh
python3 scripts/install_zerostack.py install \
  --bundle https://example.invalid/zerostack-VERSION-PLATFORM.tar.gz \
  --public-key 'RW...reviewed ZeroStack release key...'
python3 scripts/install_zerostack.py verify
```

The placeholder URL and key above must be replaced by a published release and its reviewed public key. The installer rejects non-HTTPS downloads, missing or invalid signatures, wrong-platform bundles, path traversal, unpinned source heads, artifact digest or size drift, release-ID content collisions, and archives over 1 GiB or 4096 members. It copies only verified prebuilt bytes into versioned release directories and atomically switches the POSIX `current` pointer or Windows `current.txt` pointer. It never invokes Git, Cargo, a package manager, or a source build.

Lifecycle commands use the same prefix and verified release state:

```sh
python3 scripts/install_zerostack.py upgrade --bundle BUNDLE --public-key 'RW...'
python3 scripts/install_zerostack.py status --json
python3 scripts/install_zerostack.py rollback
python3 scripts/install_zerostack.py uninstall
```

`--allow-unsigned` exists only for local fixture development and must never be used for a release. The bundle contract is `tests/contracts/release-bundle-v1.schema.json`; every manifest binds the exact ZeroStack, FSZero, GraphZero, and TokenZero source heads.

## Developer source-build requirements

- Rust toolchain and `rch`
- Node.js 22 or newer for Pi and OMP
- Sibling `ZeroStack`, `FSZero`, `GraphZero`, and `TokenZero` checkouts
- One reviewed immutable ZeroStack revision pinned by every engine

## Build the universal CLI from source

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
