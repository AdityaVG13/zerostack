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

## Process theorems (Thm 5.1, 6.1, 7.1, 8.1) -- unmapped until checkered

The theorem-to-runtime map now carries four process-theorem rows (Explanation
Evidence Preservation 5.1, Decision-Delimited Refactor 6.1, Port Nonregression
7.1, Greenfield Strategy Preservation 8.1) with checker obligations and direct
falsifiers. Per the seven-element theorem-to-runtime rule, these rows grant NO
production authority until their deterministic checkers exist as code:

- Thm 5.1: no factual-claim expandability checker over compact explanation
  views (TokenZero DecisionView/ModelCapsule remain unconstructed by any
  engine production path; see CROSSWALK finding 10).
- Thm 6.1: the W0 decision gate (`zero.decision.require`) implements the
  private-branch rule; a call-count checker over d+1 interactions does not
  exist yet (requires the continuation layer W3 handle counting).
- Thm 7.1: V == B complete-observational-coverage checkers do not exist;
  `ProtectedScopeObligationsV1` (W1) provides the obligation vocabulary but
  no port verification engine consumes it.
- Thm 8.1: no mandatory-gate audit over the capability set; the capabilities
  subsystem (ZS-CAP-001..006) is still spec-only (W7).

Until a finite checker lands for a row, the corresponding authority
consequence is withheld and the Unknown behavior applies.
