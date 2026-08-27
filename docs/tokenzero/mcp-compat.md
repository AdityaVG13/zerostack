# TokenZero MCP compatibility policy

Classic MCP remains an explicit compatibility surface. It is not the default multi-engine path and does not expose an engine-local CodeMode planner.

Supported behavior receives security and correctness fixes for framing, cancellation, validation, corruption, crashes, hangs, schemas, and documented tools. Compatibility does not promise parity with every ZeroKernel workflow or removal after a fixed number of releases.

## Versioning

FSZero, GraphZero, and TokenZero will adopt one coordinated version when joint releases begin. Compatibility changes follow that engine version. There is no independent release-count removal schedule.

Removing a documented capability requires an explicit owner decision, client migration matrix, rollback evidence, and release notes.

## Migration

1. Inventory direct tools used by the client.
2. Map the workflow to the six-operation ZeroKernel surface.
3. Verify refs, projection, cancellation, and state.
4. Stop the client and remove classic registration.
5. Enable ZeroKernel as the only aggregate model surface.

Rollback avoids dual registration: stop work, remove the new route, restore the classic configuration, then resume.
