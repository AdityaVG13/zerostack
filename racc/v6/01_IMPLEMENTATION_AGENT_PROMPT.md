# Implementation Agent Prompt

You are implementing the ZeroStack RACC V6 specification into existing repositories. Do not invent a replacement program or assume that missing functionality is absent until you inspect the code.

## Required actions

1. Read `00_START_HERE_FOR_IMPLEMENTATION.md` and every referenced current V6 document.
2. Inspect the actual ZeroStack, FSZero, GraphZero, and TokenZero repositories.
3. Produce a requirement-to-code map using `IMPLEMENTATION_BACKLOG_V6.csv`.
4. Mark each item `implemented`, `partial`, `conflicting`, `missing`, or `not applicable`, with exact file/symbol/test evidence.
5. Preserve the four-repository authority split and prohibit engine-to-engine dependencies.
6. Freeze the same-model/same-harness baseline before optimization.
7. Implement P0 requirements in dependency order, beginning with canonical identity, contracts, Safe/Unsafe/Unknown, exact read-only Zero Execute, and complete ledgers.
8. For each patch, provide tests, fault injection, rollback, benchmark effect, and updated requirement evidence.
9. Never claim Q99, no degradation, one-call completion, or Pareto improvement from algebra alone. Require the runtime certificate and paired data defined by the corpus.
10. Treat historical papers as lineage. Apply current V6 corrections where they conflict.

## First response required

Return:

- repository map;
- current execution flow;
- current cache/index/object identity model;
- current harness integration points;
- top 20 P0 gaps by dependency order;
- first minimal patch proposal;
- explicit uncertainties and files not yet inspected.

Do not start with autonomous edits or capability learning. Establish exact state, evidence, fallback, and accounting first.
