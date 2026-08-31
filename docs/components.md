# Components

ZeroStack is one product. Its files domain (`crates/fszero/`) owns bytes and filesystem effects, its structure domain (`crates/graphzero/`) owns relationships and coverage, and its tokens domain (`crates/tokenzero/`) owns measurement and output economics. Domain crates may depend on crates in their own tree and on shared ZeroStack contracts. They never import another domain tree.

ZeroStack adapters are the only composition boundary. The model sees `z.read`, `z.find`, `z.edit`, `z.apply`, `z.run`, and `z.state`, never a crate or engine selector.

## Dependency rules

1. `zero-abi` defines shared typed contracts and imports no engine.
2. FSZero, GraphZero, and TokenZero implement domain logic independently.
3. `zero-fs`, `zero-graph`, and `zero-token` adapt one domain each to `zero-abi`.
4. `zero-kernel` composes adapters, ZeroGate, ZeroGauge, runtime services, transactions, processes, state, and terminal responses.
5. `zero-mcp` carries one ZeroKernel cell through one MCP tool. It owns no domain catalog.
6. `zerostack-test-support` is the only test-support crate. It lives under root `tests/support` and never becomes a production dependency.

## ZeroStack crates

| Crate | Contract |
| --- | --- |
| `zero-abi` | Engine-independent requests, outcomes, capabilities, cancellation, receipts, and the direct ZeroKernel contract. |
| `zero-ref` | Portable `z://blob/<sha256>` identity, strict fragments, verification, and typed resolution errors. |
| `zero-store` | Canonical immutable CAS layout, verified publication, store roots, and exact object recovery. |
| `zero-process` | Hub-owned child identity, owner-death binding, termination, and verified process-tree reaping. |
| `zero-codemode` | Fresh bounded JavaScript and TypeScript frame evaluation. It does not define a second operation API. |
| `zero-fs` | Typed adapter from ZeroKernel file requests to FSZero. |
| `zero-graph` | Typed adapter from ZeroKernel structural requests to GraphZero. |
| `zero-token` | Typed adapter from operation and response values to TokenZero measurement and projection. |
| `zero-kernel` | Reusable daemonless host, frame lifecycle, budgets, cancellation, transactions, trusted Snap-to-File routing, paired savings reporting, state publication, and terminal response. This crate owns the only product executable, `zero-kernel`. |
| `zero-kernel-node` | Asynchronous N-API embedding of the in-process ZeroKernel host. |
| `zero-mcp` | Lossy single-tool carrier for clients that cannot embed ZeroKernel. |
| `zero-pulse` | Hub-owned append-only accounting ledger and bounded aggregate reports. |
| `zero-gauge` | Comparable native/Zero observations and deterministic exact savings reports. It remains off the authority path. |
| `zero-ledger` | RACC resource, exposure, replay, and phase-dominance receipts. |
| `zero-cert` | Pure verification of proof-carrying RACC evidence. |
| `zero-gate` | Deterministic proof-carrying decisions, including read-only Snap-to-File over host-registered GraphZero completeness evidence. |
| `zerostack-conformance` | Engine-independent executable checks over `zero-abi`; its detailed contract is in the crate's `CONTRACT.md`. |

## FSZero crates

FSZero owns exact bytes, filesystem snapshots, guarded effects, receipts, and restoration. It does not own graph relationships, process lifecycle, or output projection.

| Crate | Contract |
| --- | --- |
| `fszero-core` | Filesystem domain types, edit specifications, and byte-target grammar. |
| `fszero-store` | Byte storage, journals, recovery, snapshots, durable receipts, and filesystem-side CAS integration. |

ZeroKernel exposes FSZero through `z.read`, `z.edit`, and `z.apply`.

## GraphZero crates

GraphZero owns syntax, symbols, relationships, freshness, coverage, impact, and structural evidence. It does not own source bytes or file mutation.

| Crate | Contract |
| --- | --- |
| `graphzero-types` | Stable structural facts, references, evidence, and shared graph value types. |
| `graphzero-core` | Truth classes, coverage, negative knowledge, and certified invalidation rules. |
| `graphzero-extract` | Deterministic syntax parsing and fact extraction. |
| `graphzero-store` | Content-addressed snapshots, indexes, structural evidence, refs, and durability. |
| `graphzero-engine` | Structural query routing, relationships, impact, coverage, and typed domain outcomes. |
| `graphzero-coverage` | Coverage and freshness certification for structural evidence and absence claims. |
| `graphzero-reserve` | Intent reservation, overlap detection, ledger replay, and conflict evidence. |

ZeroKernel exposes GraphZero through `z.find`.

## TokenZero crates

TokenZero owns tokenizer identity, measurement, bounded projection, exact recovery, and accounting facts. It does not own processes, filesystem effects, graph freshness, or Pulse storage.

| Crate | Contract |
| --- | --- |
| `tokenzero-core` | Tokenizer identity, compression models, protected outcomes, measurement, and exact-recovery decisions. |

TokenZero runs automatically at ZeroKernel operation and response boundaries. It is not a seventh operation.

## Test layout

Reusable fixtures and independent oracles live in root `tests/support` as `zerostack-test-support`. All Rust tests live under root `tests/`, including the standalone xtask workspace contract. Product behavior tests exercise `zero-fs`, `zero-graph`, `zero-token`, or the composed `zero-kernel` surface. Domain directories contain no test-support packages or inline test modules.

## Shared contract properties

- **Errors:** stable typed classes cross crate boundaries. Prose remains diagnostic context, not control flow.
- **Determinism:** the same contract, rooted inputs, snapshot, and budget produce the same ordered domain value.
- **Cancellation:** cancellation before publication fails without partial authority. Published durable work is never reported as cancelled.
- **Unsafe code:** each exception must remain a small documented leaf behind a safe facade.
- **No-claim boundary:** conformance proves typed contracts, not operating-system isolation, universal availability, token savings, or graph freshness without the corresponding measured evidence.
