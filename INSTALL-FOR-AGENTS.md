# INSTALL-FOR-AGENTS

Audience: an AI agent installing and running the full ZeroStack CodeMode sandbox
on a machine it controls. This file is authored in `ZeroStack` and mirrored
verbatim into `FSZero`, `GraphZero`, and `TokenZero`; edit the ZeroStack copy
and re-mirror, never one mirror alone.

Every path, flag, and environment variable below is taken from code in these
repositories. Nothing here is invented. Source anchors are named inline.

---

## 1. What you are installing

Four executables and one optional CLI wrapper.

| Artifact | Produced by | Role |
| --- | --- | --- |
| `zerostack-codemode-host` | `ZeroStack`, `cargo build --release -p zero-codemode --bin zerostack-codemode-host` | Aggregate CodeMode sidecar. Reads bounded NDJSON frames on stdin, writes them on stdout. |
| `fszero-codemode` | `FSZero`, `cargo build --release -p fszero-worker --bin fszero-codemode` | FSZero planner-free raw worker. |
| `graphzero-codemode` | `GraphZero`, `cargo build --release -p graphzero-worker --bin graphzero-codemode --no-default-features` | GraphZero planner-free raw worker. |
| `tokenzero-codemode` | `TokenZero`, `cargo build --release -p tokenzero-worker --bin tokenzero-codemode --no-default-features` | TokenZero planner-free raw worker. |
| `zs` | `ZeroStack/scripts/install_zs.py` | Python CLI wrapper for shell-only harnesses. |

Binary names and the set of four are fixed in
`crates/zero-codemode/src/discovery.rs` (`HarnessBinary::file_stem`,
`HARNESS_BINARIES`). Binary targets and their required features are declared in
`FSZero/crates/fszero-codemode/Cargo.toml`,
`GraphZero/crates/graphzero-cli/Cargo.toml`, and
`TokenZero/crates/tokenzero/Cargo.toml`.

Surfaces are mutually exclusive per engine: install the CodeMode surface **or**
the engine's MCP surface, never both. This is the ZeroStack-wide rule in
`docs/mcp-compatibility-policy.md` and `docs/adr/0001-codemode-execution-boundary.md`,
and it is enforced at compile time in FSZero (`surface-mcp`+`surface-codemode`
fails to build; see `FSZero/README.md`).

---

## 2. Prerequisites

- **Rust toolchain.** Each engine repository pins it in `rust-toolchain.toml`;
  building from a clean checkout with `cargo build --release` picks up the pin.
- **Python 3**, only if you want the `zs` wrapper. It has no third-party
  dependencies and supports Linux and macOS (`docs/codemode.md`).
- **Node.js**, only if your harness uses the JavaScript raw-worker runtime
  (`raw-runtime.js` / `substrates.js`). The Rust sidecar itself does not need it.
- **FSZero build access.** FSZero builds require access to the `ast-sgrep`
  repository; until its public flip, `cargo build` there needs GitHub auth that
  can read `AdityaVG13/ast-sgrep` (`FSZero/README.md`).
- **Platforms.** macOS and Linux are the supported development platforms. The
  discovery grammar also has a Windows branch (`.exe` suffix, `LOCALAPPDATA` /
  Program Files install roots) in `discovery.rs`.

---

## 3. Build

Clone the four repositories as siblings under one parent directory. The sibling
layout is not cosmetic: it is what `ZEROSTACK_DEV_ROOT` resolves against, and the
directory names are fixed by `HarnessBinary::dev_repo_dir` in `discovery.rs`:
`ZeroStack`, `FSZero`, `GraphZero`, `TokenZero`.

```sh
# <parent>/ZeroStack, <parent>/FSZero, <parent>/GraphZero, <parent>/TokenZero
cd <parent>/ZeroStack   && cargo build --release -p zero-codemode --bin zerostack-codemode-host
cd <parent>/FSZero      && cargo build --release -p fszero-worker --bin fszero-codemode
cd <parent>/GraphZero   && cargo build --release -p graphzero-worker --bin graphzero-codemode --no-default-features
cd <parent>/TokenZero   && cargo build --release -p tokenzero-worker --bin tokenzero-codemode --no-default-features
```

The host uses ZeroStack's restricted in-process AST interpreter. It needs no
JavaScript VM feature or external JavaScript runtime.

Each build lands its artifact at `<repo>/target/release/<binary>`, which is
exactly what the `dev_checkout` discovery rule looks for.

---

## 4. Binary discovery

Never write an absolute binary path into a shipped harness config. A config that
names one developer's worktree resolves nothing on another machine and fails at
spawn with a bare `ENOENT`. That is the stated rationale in the module docs of
`crates/zero-codemode/src/discovery.rs`.

### 4.1 Environment variables

| Variable | Source constant | Meaning |
| --- | --- | --- |
| `ZEROSTACK_HOME` | `discovery::HOME_ENV` | One install root. Binaries are looked up at `$ZEROSTACK_HOME/bin/<binary>`. |
| `ZEROSTACK_DEV_ROOT` | `discovery::DEV_ROOT_ENV` | Dev-checkout override: the parent directory holding the sibling engine checkouts. Binaries come from `$ZEROSTACK_DEV_ROOT/<Repo>/target/release/<binary>`. |
| `XDG_DATA_HOME` | read by `DiscoveryEnv::from_process` | When set, replaces the `$HOME/.local/share` default; binaries at `$XDG_DATA_HOME/zerostack/bin/<binary>`. |
| `HOME` / `USERPROFILE` | read by `DiscoveryEnv::from_process` | Supplies the XDG default `$HOME/.local/share/zerostack/bin`. |
| `PATH` | read by `DiscoveryEnv::from_process` | Last resort; only absolute entries are considered. |
| `ZEROSTACK_NODE` | `manifest::NODE_ENV`, `node::NODE_ENV` | Direct file pin for the Node runtime. |
| `ZEROSTACK_RUNTIME_MODULE` | `manifest::RUNTIME_MODULE_ENV` | Direct file pin for the aggregate raw-worker runtime module (`raw-runtime.js`). |
| `ZEROSTACK_SUBSTRATE_MODULE` | `manifest::SUBSTRATE_MODULE_ENV` | Direct file pin for the substrate router module (`substrates.js`). |
| `FNM_DIR` | `node::FNM_DIR_ENV` | Root of an fnm install; the stable `aliases/default` subpath is used, never a per-shell multishell link. |

### 4.2 Resolution order

Highest precedence first (`Source` in `discovery.rs`, extended with
`Source::Explicit` for non-binary artifacts in `manifest.rs`):

| Order | Source label | Location |
| --- | --- | --- |
| 1 | `explicit` | A direct file pin (`ZEROSTACK_NODE`, `ZEROSTACK_RUNTIME_MODULE`, `ZEROSTACK_SUBSTRATE_MODULE`). Non-binary artifacts only. |
| 2 | `zerostack_home` | `$ZEROSTACK_HOME/bin/<binary>` |
| 3 | `dev_checkout` | `$ZEROSTACK_DEV_ROOT/<Repo>/target/release/<binary>` |
| 4 | `xdg_data` | `$XDG_DATA_HOME/zerostack/bin/<binary>`, default `$HOME/.local/share/zerostack/bin` |
| 5 | `platform_install` | `/usr/local/lib/zerostack/bin`, `/opt/zerostack/bin`, `/usr/lib/zerostack/bin`; on Windows the `LOCALAPPDATA`, `PROGRAMFILES`, and `PROGRAMDATA` `ZeroStack/bin` equivalents |
| 6 | `path` | each absolute `PATH` entry, in order |

JavaScript modules live under `<install root>/lib` (`manifest::LIB_DIR`); Node
lives under `<install root>/bin` (`discovery::BIN_DIR`).

### 4.3 Rules that make resolution predictable

From `discovery.rs` and `manifest.rs`:

- An empty or whitespace-only variable is treated as unset.
- A relative install root, a relative file pin, or a blank `PATH` entry (which
  some shells read as "current directory") is discarded rather than
  half-honored, so resolution never depends on the spawning cwd.
- An explicit `XDG_DATA_HOME` replaces the `~/.local/share` default instead of
  adding to it.
- Only a regular file with an execute bit is accepted as an executable, so a
  same-named directory cannot shadow a real binary further down the order.
- Each candidate directory is probed once even if several rules name it.
- Per-shell, pid-keyed Node runtimes are refused with reason
  `ephemeral_path`; the markers are `fnm_multishells` and `nvm_multishells`
  (`node::EPHEMERAL_MARKERS`). Such a path dies with the shell that made it.

### 4.4 Ask the host instead of guessing

```sh
zerostack-codemode-host --locate              # human-readable manifest
zerostack-codemode-host --locate --json       # zerostack.locate.v1 JSON
zerostack-codemode-host --locate-binaries     # zerostack.binary_discovery.v1 JSON
zerostack-codemode-host --version
zerostack-codemode-host --help
```

These are the only accepted argument forms; anything else exits with
`unsupported arguments: ...` (the `match` in
`crates/zero-codemode/src/bin/zerostack-codemode-host.rs`). With no arguments the
process reads bounded NDJSON frames on stdin.

`--locate` reports, per artifact, either the resolved path and the rule that
found it, or every candidate that was probed — so a failed install explains
itself instead of surfacing a bare `ENOENT`.

---

## 5. Store roots

Store layout is owned by `ZeroStack/crates/zero-store/src/store_root.rs`. One
algorithm serves all three engines.

| Variable | Constant | Meaning |
| --- | --- | --- |
| `ZEROSTACK_STORE_ROOT` | `STORE_ROOT_ENVS[0]` | Store-root pin. First non-empty of the pin list wins. |
| `ZERO_STACK_STORE_ROOT` | `STORE_ROOT_ENVS[1]` | Accepted alias of the above. |
| `ZEROSTACK_SHARED_STORE` | `SHARED_STORE_OPT_IN_ENV` | Cross-engine shared-store opt-in. Engines additionally accept their own alias, e.g. `TOKENZERO_SHARED_STORE`. Truthy values are exactly `1`, `on`, `true`, `yes` (case-insensitive, trimmed). |

Resolution order (`ResolvedStore::resolve`):

1. `<repo_root>/.zerostack`, when it is a directory, wins unconditionally. A
   project-local marker is an explicit per-repository declaration; the pin is
   ambient process state, so a stray variable in a harness must not relocate an
   engine's store.
2. Otherwise, when opted in **and** the pin is non-empty: the pin, used as-is
   when absolute and joined to the repo root when relative. Existence is not
   required, so a store can be pinned before it is created. A pin outside the
   project root is namespaced by project key under `projects/<key>/`.
3. Otherwise the engine's legacy per-repository directory: `.tokenzero`,
   `.fszero`, or `.graphzero`.

Inside a resolved store root: `<root>/<engine>` where engine is `tokenzero`,
`fszero`, or `graphzero`; `<engine dir>/journal` for the journal
(`manifest::JOURNAL_DIR`); and `<root>/blobs` for content-addressed objects,
which are shared per store root and never project-namespaced.

Confirm what a given checkout resolves to with `zerostack-codemode-host --locate`,
which prints `store_root` and `journal_dir`.

---

## 6. Harness wiring

Pick exactly one of the three paths below. Do not register an engine MCP catalog
alongside CodeMode; `docs/mcp-compatibility-policy.md` forbids it.

### 6.1 pi (programmatic tool calling)

pi loads the `pi-zerostack` package, which exposes the single `zero_execute`
tool and drives the aggregate sidecar. Its backend pins live in
`~/.pi/agent/zerostack/backends.json`, read by
`pi-stack/packages/pi-zerostack/code-mode-binary.js`:

```json
{
  "ZERO_FSZERO_BIN": "<...>/fszero-codemode",
  "ZERO_GRAPHZERO_BIN": "<...>/graphzero-codemode",
  "ZERO_TOKENZERO_BIN": "<...>/tokenzero-codemode",
  "aggregateHostBin": "<...>/zerostack-codemode-host"
}
```

`ZEROSTACK_CODEMODE_HOST_BIN` in the process environment is also consulted for
the aggregate host by that module. Prefer installing the four binaries into one
`ZEROSTACK_HOME/bin`, or exporting `ZEROSTACK_DEV_ROOT`, over hand-writing
absolute paths.

### 6.2 Generic MCP carrier

A generic MCP transport MAY carry aggregate CodeMode for harness compatibility.
Per `docs/mcp-compatibility-policy.md`, such a carrier:

- MAY expose only `zero_execute` and `zero_wait`;
- MUST NOT register, proxy, discover, or advertise the TokenZero, FSZero, or
  GraphZero MCP catalogs;
- does not change the deployment's mode — it is CodeMode transport, with
  aggregate engine calls dispatched only through planner-free raw-worker v2
  workers.

The carrier is not shipped from this repository; the policy above is the
contract any carrier must satisfy.

### 6.3 Raw CLI (shell-only harnesses)

For a harness with no programmatic tool calling, drive the sandbox with the
tracked `zs` wrapper — a plain executable invoked over ordinary shell, not an
MCP server.

```sh
cd <parent>/ZeroStack
python3 scripts/install_zs.py --dry-run
python3 scripts/install_zs.py
python3 scripts/install_zs.py --verify
zs --version
```

The default destination is `~/.kimi-code/bin/zs`; `--prefix DIR` selects another
user bin directory. Installation uses a temporary sibling, fsync, executable-mode
preservation, and atomic replacement (`docs/codemode.md`).

Usage, from the wrapper's own help text in `scripts/zs`:

```text
zs [--json] [--verbose] [-C ROOT] fs|graph|token '<js plan>'
zs [--json] [--verbose] [-C ROOT] <engine>-search '<intent>'
zs [--json] [--verbose] [-C ROOT] <engine>-describe '<method>'
zs --version
```

`-C/--root` must precede the engine command, and plan paths stay relative to
that validated root. Use `-` or omit the plan argument to read the plan from
stdin. `--json` emits the complete engine result; normal output preserves typed
status and copyable `fz://`, `gz://`, `tz://`, and `cm://` refs. Binary
overrides are `ZS_FSZERO_BIN`, `ZS_GRAPHZERO_BIN`, `ZS_TOKENZERO_BIN`;
`ZS_TIMEOUT_MS` changes the 120000 ms default.

Engines also accept a root through their own environment, e.g.
`FSZERO_ROOT=/your/repo` (`FSZero/README.md`).

---

## 7. Config discovery order — what to set, in order of preference

1. **Nothing.** If the binaries are installed under a platform install directory
   or are on `PATH`, discovery finds them with no configuration.
2. **`ZEROSTACK_HOME`.** One install root with every binary in `bin/` and both
   JavaScript modules in `lib/`. The single-variable production install.
3. **`ZEROSTACK_DEV_ROOT`.** Point it at the parent of the sibling checkouts and
   their `target/release` builds are used directly. The documented
   dev-checkout override.
4. **`XDG_DATA_HOME`.** Redirects the `~/.local/share/zerostack` lookup.
5. **Direct pins** (`ZEROSTACK_NODE`, `ZEROSTACK_RUNTIME_MODULE`,
   `ZEROSTACK_SUBSTRATE_MODULE`) only for the non-binary artifacts, and only
   when the layout rules cannot find them.
6. **Harness-local config** (`~/.pi/agent/zerostack/backends.json`,
   `ZS_*_BIN`) last, for a harness that must pin a specific build.

Absolute paths baked into a shipped config are the failure mode this grammar
exists to eliminate. Prefer the earliest option that works.

---

## 8. Smoke test

Run these in order from the ZeroStack checkout. Each step is independently
diagnosable.

```sh
# 0. Where the binaries came from. Expect no <unresolved> for the four binaries.
export ZEROSTACK_DEV_ROOT=<parent>          # or: export ZEROSTACK_HOME=<install root>
zerostack-codemode-host --version
zerostack-codemode-host --locate

# 1. Machine-readable executable report (zerostack.binary_discovery.v1).
zerostack-codemode-host --locate-binaries

# 2. Per-engine plans through the shell-only path.
zs --version
zs -C . fs    'return await zero.fs.compound("list", { path: "docs" });'
zs -C . graph 'return await zero.graph.orient("context", "architecture");'
zs -C . token 'return await zero.token.compact("hello world");'

# 3. Discovery capability surface: search then describe before executing.
zs -C . fs-search 'read a file'
```

A healthy `--locate` looks like this (paths reflect your own layout; the
`[dev_checkout]` labels become `[zerostack_home]` for a single-root install):

```text
aggregate_host    <parent>/ZeroStack/target/release/zerostack-codemode-host  [dev_checkout]
binaries.fs       <parent>/FSZero/target/release/fszero-codemode  [dev_checkout]
binaries.graph    <parent>/GraphZero/target/release/graphzero-codemode  [dev_checkout]
binaries.token    <parent>/TokenZero/target/release/tokenzero-codemode  [dev_checkout]
capabilities      ["fs","graph","token"]
journal_dir       <repo>/.zerostack/tokenzero/journal
order             ["explicit","zerostack_home","dev_checkout","xdg_data","platform_install","path"]
schema            zerostack.locate.v1
store_root        <repo>/.zerostack/tokenzero
versions.discovery_schema zerostack.binary_discovery.v1
versions.host     0.1.0
versions.manifest_schema zerostack.locate.v1
versions.protocol zerostack-codemode-host/v2
```

`node`, `runtime_module`, and `substrate_module` report `<unresolved>` when the
JavaScript raw-worker runtime is not installed. That is expected for a
Rust-sidecar-only install and does not block steps 2 and 3.

### If something is unresolved

- **A binary is `<unresolved>`.** `--locate-binaries` lists every probed
  candidate with its rule label. Compare against §4.2: usually the planner-free
  worker build did not run, the wrong package/bin was selected, or the checkout
  directory name does not match `ZeroStack` / `FSZero` / `GraphZero` /
  `TokenZero`.
- **A pin was ignored.** It was relative, empty, or whitespace-only. All three
  are discarded by design.
- **Node was refused with `ephemeral_path`.** The candidate lives in an
  `fnm_multishells` or `nvm_multishells` directory. Pin `ZEROSTACK_NODE` at a
  stable install, or set `FNM_DIR` so the `aliases/default` link is used.
- **The store landed somewhere unexpected.** Re-read §5: a project-local
  `.zerostack` directory outranks any pin, and a pin without the shared-store
  opt-in is reported but not honored.
