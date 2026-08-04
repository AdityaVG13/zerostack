# Invalidation and freshness v1 Rust correspondence

## Frozen mapping

- Source identity: `zero_abi::FreshnessHeadV1`
- Influence frontier: `zero_abi::CertifiedInfluenceClosure`
- Essential path proof: `zero_abi::EssentialDependencyCertificate`
- Index identity and replay binding: `zero_abi::IndexedThroughCertificate`
- Partial-order verifier: `zero_abi::decide_freshness_v1`
- Typed result: `zero_abi::FreshnessDecisionV1`
- Immutable KAT archive: `crates/zero-testkit/conformance/invalidation-freshness/v1`
- Independent replay: `runners/python/verify_v1.py`

## Engine adoption

E-FS, E-GRAPH, and E-TOKEN consume the same `zero-abi` schema. They retain domain-specific discovery and never import a peer engine. A missing or incomparable proof returns `INDEX_BEHIND` or `UNKNOWN`; it never returns a partial trusted result.

## Evidence boundary

The Rust checker proves canonical identity binding and supplied-closure comparison. It does not prove engine discovery soundness. RCH results are compilation and abstract KAT evidence only. Native filesystem, crash, performance, packaging, and Windows release claims remain deferred to preregistered adoption gates.
