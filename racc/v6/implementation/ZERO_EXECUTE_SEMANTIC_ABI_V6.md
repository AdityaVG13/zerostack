# Zero Execute Semantic ABI - Draft 6

This document defines transport-neutral semantics. An in-process library, Pi package, Codex/Claude/Cursor plugin, local RPC service, CLI/stdio process, or MCP adapter may expose different syntax, but a faithful adapter must preserve these meanings.

## 1. Request

A `ZeroExecuteRequest` contains:

- `abi_version`;
- `task_contract_root` and either the rooted task contract or authority to resolve it;
- `project_root` or workspace binding;
- raw user objective and acceptance criteria;
- optional `continuation_handle`;
- optional model decision or contingent policy;
- requested operation class: inspect, explain, plan, edit, refactor, port, build, test, verify, resume, cancel;
- side-effect policy;
- maximum private-composition depth/time/work;
- required verifier/human authority;
- resource budget and baseline reserve;
- model/harness/tool contract roots;
- requested result-detail level.

The raw user request is real information and must be accounted. A continuation handle avoids retransmitting prior project and execution state; it does not erase the new request.

## 2. Result variants

### `Completed`

Contains:

- protected result or exact reference to it;
- project and successor roots;
- exact delta or result root;
- verification and successor receipts;
- continuation handle;
- decision-view capsule;
- resource ledger;
- cache/Q99 observations;
- provenance and audit-event range.

### `DecisionRequired`

Contains:

- one precisely scoped unresolved semantic question;
- alternatives or an open response schema;
- exact decision-relevant evidence and expansion handles;
- consequence summary;
- continuation handle;
- reason the backend may not choose privately.

### `EvidenceExpansionRequired`

Contains:

- missing proof or evidence class;
- exact objects or operations needed;
- expected cost and reserve effect;
- continuation handle;
- whether the model, backend, user, or external verifier must supply it.

### `VerificationUnknown`

Contains:

- completed candidate;
- failed or unavailable obligations;
- covered and uncovered protected dimensions;
- permitted next actions;
- no commit authority.

### `BaselineFallbackRequired`

Contains:

- reason optimization cannot proceed safely;
- confirmed baseline reserve;
- continuation or native-tool handoff;
- all reusable work already produced;
- no claim of optimized completion.

### `RejectedNoMutation`

Contains:

- counterexample, stale root, expired lease, sandbox violation, verifier rejection, or invalid request;
- proof that authoritative root did not change;
- audit references and recovery options.

## 3. Continuation handles

A handle is an opaque harness-facing identifier resolving to rooted backend state:

- task contract;
- project root;
- causal lens and evidence state;
- candidate/plan state;
- verification state;
- resource ledger;
- model-visible capsule lineage;
- authority and expiration state.

A handle is not authority. It cannot commit. It is invalidated or migrated when its ABI, task, root, model/harness contract, or security scope is incompatible.

## 4. Adapter obligations

A faithful harness adapter must:

1. preserve canonical request semantics;
2. preserve model-visible result content and ordering;
3. not truncate or summarize protected evidence without a certificate;
4. preserve cancellation and timeout semantics;
5. bind session, workspace, and user/project identity;
6. keep opaque handles out of user-controlled authority fields;
7. retain native-tool escape;
8. report model-visible arguments and results for accounting;
9. surface `Unknown` as `Unknown`, not success;
10. pass conformance vectors across supported transports.

## 5. Private composition

The backend may internally perform a sequence of operations in one call when every step is mechanical, deterministic, independently verifiable, or covered by a contingent policy already supplied by the model/user.

It must return to the model when a new observation creates a semantic branch not covered by the policy or verifier.

## 6. Stable model-visible rendering

The adapter should expose a small stable tool schema. Dynamic project state belongs in arguments, continuation handles, and rooted capsules, not in a continually changing tool catalog. Results use deterministic field ordering and canonical formatting to preserve provider prefix reuse where possible.

## 7. Harness portability

Project-semantic L2 objects are independent of the harness rendering. They may be reused across adapters when:

- task/project/contract roots match;
- protected semantics are equivalent;
- the destination adapter faithfully renders required evidence and actions;
- model-specific output constraints are represented as a new rendering contract rather than changing project identity.

## 8. Baseline escape

At any decision boundary, the model may request exact expansion or ordinary native tools. The adapter must not make ZeroStack the only path. If a harness cannot preserve this escape, it cannot claim same-harness capability nondegradation.
