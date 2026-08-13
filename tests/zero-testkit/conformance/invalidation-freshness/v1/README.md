# Invalidation and freshness KAT v1

This immutable archive freezes the hub-owned Z3 vectors and JSON Schema. Rust and Python independently replay canonical bytes, certificate digests, typed stale/missing/inflated/replay outcomes, and the wall-clock non-authority rule.

- `vectors.json`: exact fresh certificate plus negative vectors.
- `schema.json`: strict public result envelope.
- `runners/python/verify_v1.py`: independent canonical/digest/outcome replay.
- `index.json`: SHA-256 bindings for every archive input.

Corrections create `v2`; do not rewrite a promoted `v1`. E-FS, E-GRAPH, and E-TOKEN consume this hub schema without peer-engine imports. RCH proves compilation and abstract KAT correspondence only. Native filesystem, crash, performance, packaging, and Windows release claims remain in preregistered engine adoption gates.
