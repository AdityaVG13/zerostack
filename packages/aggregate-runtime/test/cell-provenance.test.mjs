import test from "node:test";
import assert from "node:assert/strict";
import { resolveHostCellProvenance } from "../src/raw-runtime.js";

test("re-attaches cell provenance when the host cell id is dropped mid-session", () => {
  const cache = new Map();
  assert.equal(resolveHostCellProvenance({ aggregateExecutionId: "exec-1", cellId: "cell-a" }, cache), "cell-a");
  assert.equal(resolveHostCellProvenance({ aggregateExecutionId: "exec-1" }, cache), "cell-a");
});

test("derives a stable cell id when provenance was never observed for the execution", () => {
  const a = resolveHostCellProvenance({ aggregateExecutionId: "exec-2" }, new Map());
  const b = resolveHostCellProvenance({ aggregateExecutionId: "exec-2" }, new Map());
  assert.notEqual(a, "unbound-cell");
  assert.equal(a, b);
});

test("falls back to toolCallId, then to the unbound cell", () => {
  assert.equal(resolveHostCellProvenance({ toolCallId: "t1" }, new Map()), "t1");
  assert.equal(resolveHostCellProvenance({}, new Map()), "unbound-cell");
});
