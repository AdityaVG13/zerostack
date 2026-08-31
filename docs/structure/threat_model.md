# Structure threat model

## Scope and security posture

This document records trust boundaries implemented by the current GraphZero codebase. It separates controls that exist today from residual risk; it is not a claim that GraphZero is a security boundary against a hostile operating-system account. Assets in scope are repository contents, indexes and facts, content-addressed blobs, daemon state, host-process availability, and tool-result integrity.

The repository scanner policy is security_scan.md. Scanning detects selected dependency, license, source, and secret problems; it does not replace the runtime controls below. The production panic-site ratchet is unwrap_budget.toml. That ratchet constrains growth in explicit production panic sites, but is an availability-maintenance control rather than isolation or input validation.

## 1. ZeroKernel request boundary

**Attacker capability.** A caller can submit malformed structural requests,
patterns, paths, symbols, and budgets through `z.find`.

**Existing mitigations.** ZeroStack owns JavaScript evaluation, finite frame
budgets, cancellation, and process lifecycle. GraphZero receives a typed,
root-confined request. Unsupported modes, malformed patterns, ambiguous symbols,
incomplete coverage, and stale snapshots return explicit error or absence
classes before a claim is published. GraphZero does not evaluate guest
JavaScript and does not expose an engine-local MCP or CodeMode catalog.

**Residual risk.** Syntax parsers, semantic retrieval, and graph construction
consume CPU and memory on attacker-controlled repository content. Host budgets
bound one request but do not make GraphZero a security boundary against the
local operating-system account.

## 2. Project-local and shared CAS namespace / ZeroRef negotiation

**Attacker capability.** A local process, shared-store participant, or peer can supply a digest/reference, place or race filesystem entries in a configured CAS root, provide oversized or corrupt objects, or advertise an incompatible ZeroRef capability descriptor. In shared mode, parties intentionally share blob bytes across project boundaries.

**Existing mitigations.** Project-local CAS is the default; shared CAS requires explicit environment configuration, shares only immutable blob bytes, and keeps facts/project metadata project-keyed ([shared_cas.rs:1-14](../../crates/graphzero/graphzero-store/src/store/shared_cas.rs#L1-L14)). Writes enforce a 256 MiB ceiling, derive the destination from the complete-content SHA-256, reject non-directory substitutions, use create-new sibling temporaries, sync, and atomically publish ([shared_cas.rs:24-36](../../crates/graphzero/graphzero-store/src/store/shared_cas.rs#L24-L36), [shared_cas.rs:98-177](../../crates/graphzero/graphzero-store/src/store/shared_cas.rs#L98-L177)). Reads require a full digest, reject non-regular/oversized objects, and verify complete bytes before returning them ([shared_cas.rs:180-235](../../crates/graphzero/graphzero-store/src/store/shared_cas.rs#L180-L235)). Capability descriptors are generated from the same hash/layout constants as the store ([zeroref_capability.rs:154-187](../../crates/graphzero/graphzero-store/src/store/zeroref_capability.rs#L154-L187)); peer validation rejects missing/malformed descriptors and mismatched contract-major, hash identity, or layout version before payload work, and enables shared interop only when both sides report it enabled ([zeroref_capability.rs:240-325](../../crates/graphzero/graphzero-store/src/store/zeroref_capability.rs#L240-L325)).

**Residual risk.** A shared CAS is not confidential: a principal with filesystem access to the shared root can inspect stored bytes and object names, and digest-addressing can reveal equality. Capability negotiation checks compatibility and effective state, not peer identity or authorization. Filesystem checks still rely on integrity and permissions of the configured root; a same-privilege adversary can race or remove entries, causing denial of service even though returned bytes are digest-verified. The 256 MiB policy is per object, not a total quota.

## 3. Git-sourced hub contract dependencies

**Attacker capability.** A compromised upstream repository, maintainer account,
reviewed commit, transitive dependency, or local dependency cache can place code
in a pinned ZeroStack contract that builds with GraphZero's privileges.

**Existing mitigations.** Dependencies pin exact revisions and release work
reviews the upstream diff, dependency graph, build scripts, unsafe code, and
contract changes. The scanner cadence is defined in
security_scan.md.

**Residual risk.** A commit pin gives reproducibility, not provenance or safety.
Git dependencies may not map cleanly to registry advisory data, and review can
miss malicious or subtle behavior.


## 4. Subprocess invocations

**Attacker capability.** A caller or environment can influence PATH, GraphZero's analyzer override variables, executable contents, repository metadata, and subprocess output. Any selected executable runs native code with the invoking user's permissions.

**Existing mitigations.** Live analyzer probes invoke an executable directly, not through a shell, with the fixed `--version` argument. They capture output and report nonzero or launch failures as unavailable: [rust_analyzer.rs](../../crates/graphzero/graphzero-extract/src/rust_analyzer.rs) and [tsserver.rs](../../crates/graphzero/graphzero-extract/src/tsserver.rs). Their executable overrides are explicit environment variables, so CI selection is visible rather than interpolated into a command line.

**Residual risk.** Direct argv avoids shell expansion but does not validate the executable. `PATH`, `GRAPHZERO_RUST_ANALYZER_BIN`, and `GRAPHZERO_TSSERVER_BIN` are trust decisions. These probes have no timeout or output-size bound, so a malicious executable can hang, flood captured output, or perform arbitrary side effects before returning a version.

## Review triggers

Re-review this threat model when a typed domain operation or ZeroKernel adapter is added,
worker composition changes, shared-store layout or negotiation changes, a pinned dependency
changes, or a new subprocess is introduced. Run the checks in `security_scan.md` for
dependency and secret evidence. Evaluate panic-site changes against `unwrap_budget.toml`;
neither control replaces boundary-specific tests or mitigations.
