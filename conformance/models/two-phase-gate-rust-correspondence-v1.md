# Two-phase gate v1 Rust correspondence

## Scope

This artifact maps freeze `Z5` to the engine-free Rust kernel. It is not native process, filesystem, crash, performance, or packaging evidence.

## Phase mapping

| Contract phase | Rust authority | Enforced property |
|---|---|---|
| prepare | `zero_gate::prepare` | Evaluates G0-G7 in order, rejects zero predecessors, commits every admission input, and mints opaque `ExecutionPermit`. |
| dispatch | `ExecutionPermit::start` and `BrokeredExecution` | Only a consumed permit starts execution; controller instructions and worker bounds are fixed. |
| stage | `buffer_visible` and `stage_effect` | Bytes and effects stay private; approval-required and irreversible effects require their matching evidence commitments. |
| close | `close_transaction` or `abort` | G8 accepts only closed publication or fully accounted restoration. |
| finalize | `ReadyToFinalize::finalize` | G9 binds admission, predecessor, successor, assembly, sources, plan, evidence, outputs, effects, usage, and trace. |
| publish | `CommitReceipt::publish` | Consumes the final receipt before releasing bytes and approved effects. |

## G0-G9 law

`ExecutionTrace::verify_prefix` checks exact order and each predecessor. `verify_complete` requires ten events. `ExecutionPermit` records exactly G0-G7. G8 is added only after transaction closure. G9 is added only during finalization.

## Failure and compatibility law

Old `DecisionGate::commit` and `PolicySufficiencyWitness::commit` publication paths were removed. Policy selection remains available, but publication must use the two-phase kernel. Typed mutants return `FailureCode` and cannot create a publish capability. `validate_receipt_record` recomputes binding and receipt commitments before external acceptance. Fallback receipts never expose candidate buffers or staged effects.

## Evidence boundary

The Rust and Python KATs prove abstract correspondence only. macOS, Linux, and Windows native broker receipts remain separate immutable DSR evidence. Missing Windows evidence is `NOT_RUN`; it is never inferred from RCH or another host.
