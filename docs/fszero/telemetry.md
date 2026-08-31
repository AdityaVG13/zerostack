# FSZero telemetry boundary

FSZero records filesystem-domain measurements needed to diagnose reads, indexing, cache behavior, mutations, and recovery. TokenZero owns token accounting and output projection. ZeroStack owns response-wide reporting.

## Local measurements

Domain measurements may include:

- logical and physical filesystem operations;
- bytes read, materialized, or restored;
- cache hits and misses;
- elapsed time with explicit units;
- recovery and durability outcomes;
- measured or unmeasured coverage for each reported field.

These values are local evidence. ZeroStack may attach them to a structured operation receipt or doctor report. FSZero does not calculate product-wide token savings and does not rank itself against other engines.

## Privacy and export

Telemetry is local and default-off unless the embedding application enables an explicit aggregate sink. FSZero has no network exporter and never sends content, paths, queries, refs, commands, project identity, user identity, machine identity, or IP-derived identity.

An unavailable measurement stays `unknown` or `unmeasured`. It is never converted to zero. A zero value is valid only when the measurement ran and observed zero.

## Token accounting

TokenZero measures serialized operation and response values. Every estimate names its tokenizer or estimator and carries the measured byte and token counts. Savings claims require paired observations with the same task, measurement method, and denominator. Missing or incomparable evidence produces no percentage claim.

FSZero supplies byte and operation facts only. It does not expose a separate telemetry CLI, MCP tool, or model-facing catalog.
