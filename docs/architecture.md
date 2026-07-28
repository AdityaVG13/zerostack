# Architecture

ZeroStack is an aggregation layer for three sibling engines. It defines their shared concepts, integration patterns, benchmarks, and conformance contract without merging their implementations.

## Boundaries

| Engine | Owns | Does not own |
| --- | --- | --- |
| TokenZero | Compaction, deduplication, selective recovery, `tz://` refs | Filesystem mutation or code graphs |
| FSZero | Live filesystem access, search, planning, safe mutation, `fz://` refs | Global token policy or structural dependency graphs |
| GraphZero | Repository structure, relationships, impact, recall, `gz://` refs | File bytes or output compaction |
| This hub | Documentation, shared contracts, conformance, benchmark aggregation | Engine product code |

## Composition

A typical workflow uses GraphZero to identify a narrow change surface, FSZero to recover or modify exact files, and TokenZero to compact bulky intermediate results. Typed refs let later steps recover evidence without replaying earlier tools.

~~~text
question
  -> GraphZero: relevant symbols and impact
  -> FSZero: exact source and controlled edits
  -> TokenZero: compact outputs and retain recovery handles
  -> answer: minimal visible evidence
~~~

CodeMode can execute this sequence as one JavaScript plan. Standard MCP mode exposes equivalent engine capabilities as individual tool calls. A deployment selects one mode, never both. [ADR 0001](adr/0001-codemode-execution-boundary.md) proposes the normative one-runtime boundary, aggregate raw-worker topology, ownership, and gates.

## Source and release model

The engines remain separate repositories and release independently. Consumers should build each backend from that engine repository's `origin/main` unless a versioned release says otherwise. This hub remains the canonical place for suite-level status and contracts.
