# Missing Evidence Register - Draft 6

The following evidence does not yet exist in this cumulative research archive and must be generated from the actual implementation.

## Runtime conformance

- completed mapping of all 130 requirements to repository files/symbols/tests;
- concrete abstraction map from Rust runtime states to the abstract state machine;
- proof or fault evidence that untrusted code cannot construct authority;
- crash/race replay results around every authoritative transition;
- measured baseline reserve under deadlines and budgets.

## Harness performance

- same-model/same-harness native-tool versus Zero Execute traces;
- blinded semantic-decision annotations;
- one-/two-call coverage by task family;
- exact model-visible token and call distributions;
- cross-adapter conformance for Pi/Codex/Claude/Cursor or equivalent transports.

## Q99

- observed L2 logical reuse on real repositories;
- L3 capacity required for 99% demanded mass;
- provider-miss insulation measurements;
- sliding-window Q99 and post-change restoration;
- causal graph omission/counterexample rate;
- equality-boundary early-cutoff frequency;
- cache poisoning/corruption recovery;
- real storage, transfer, and maintenance cost.

## Quality and reasoning

- protected regression rate;
- strict-rescue rate;
- factual/evidential support comparison;
- reasoning allowance parity and native-tool escape evidence;
- human evaluation for subjective dimensions;
- long-horizon successor-state regression tests.

## Operations

- idle CPU and memory;
- incremental indexing overhead;
- multi-project storage growth;
- daemon longevity and recovery;
- security/tenancy isolation;
- release reproducibility on supported platforms.

Until these records exist, Draft 6 is a research and implementation contract, not evidence that the production system already meets the claims.
