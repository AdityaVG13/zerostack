// Public export contract for the ZeroStack aggregate runtime + substrates.
//
// Harnesses MUST import from this package instead of pointing runtime_module /
// substrate_module at a pi-stack checkout (zerostack-x99l). No pi-stack internal
// path is required: everything the aggregate host needs is vendored under ./src.
//
// The named exports below are the stable surface. Adding to it is a minor bump;
// removing or changing a signature is a major bump.

export {
  RawWorkerSupervisor,
  createRawWorkerTools,
  wrapZeroPlan,
  runtimeResponseToToolResult,
  createAggregateRuntimeBridge,
} from "./src/raw-runtime.js";

export {
  platformBinaryRel,
  originMainBackendConfigs,
  refreshLockActive,
  createSubstrateHelpers,
} from "./src/substrates.js";

// The declared export contract, so a harness can assert compatibility at load
// time instead of duplicating a hand-written bridge that probes for symbols.
export const AGGREGATE_RUNTIME_CONTRACT = Object.freeze({
  schema: "zerostack.aggregate-runtime.contract.v1",
  version: "0.1.0",
  runtime: Object.freeze([
    "RawWorkerSupervisor",
    "createRawWorkerTools",
    "wrapZeroPlan",
    "runtimeResponseToToolResult",
    "createAggregateRuntimeBridge",
  ]),
  substrate: Object.freeze([
    "platformBinaryRel",
    "originMainBackendConfigs",
    "refreshLockActive",
    "createSubstrateHelpers",
  ]),
});
