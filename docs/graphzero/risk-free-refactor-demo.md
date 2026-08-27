# Evidence-first refactor workflow

This workflow scopes a rename or signature change with GraphZero, binds exact source through FSZero, applies guarded effects through ZeroKernel, and verifies the changed behavior.

## 1. Resolve the symbol and callers

```javascript
const impact = await z.find("record_verification_graph", {
  mode: "callers",
  path: "crates/graphzero-cli/src",
  freshness: "repair",
});

return {
  items: impact.items,
  coverage: impact.coverage,
  freshness: impact.freshness,
};
```

Do not treat an empty caller list as absence unless coverage and freshness support the scope.

## 2. Read exact source

Use the returned paths and spans to request structured snapshots with `z.read`. Graph evidence identifies where to look; FSZero remains authoritative for the bytes that will be edited.

```javascript
const snapshot = await z.read({
  path: "crates/graphzero-cli/src/commands/verify.rs",
  snapshot: true,
});
```

## 3. Apply guarded changes

Use `z.edit` for one file or `z.apply` when the definition and every caller must publish together. The snapshot preimage prevents overwriting intervening work.

```javascript
return await z.edit(snapshot, {
  find: "record_verification_graph",
  replacement: "append_verify_evidence_node",
});
```

## 4. Re-query structure

Run definition and caller queries against the new snapshot. Confirm the old symbol is absent only within the reported covered scope and confirm the new symbol resolves to the intended definition.

## 5. Verify behavior

Run the narrowest compiler or behavioral lane covering the changed contract. GraphZero prioritizes likely break sites and tests; it does not replace compilation or runtime verification.

A completed ZeroKernel cell commits its guarded file effects only after evaluation, output publication, and state publication succeed. Failure or cancellation restores staged effects.
