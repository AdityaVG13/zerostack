# Second-pass lexical extract (intentional)

After tier-A tree-sitter extract, the indexer always runs `extract_edges_with_known`: tokenize every line and probe the repo-global known def-name set for `ident(` call edges. On `known_sig` drift, reused files are re-read for the same pass (tree-sitter still skipped).

This is **graph coverage**, not leftover work. Tree-sitter extract in the first pass cannot emit cross-file calls to names that were not yet in the known set. Skipping or fusing the pass can drop edges.

Phases already exist: `extract_ms` vs `scan_ms`. A fusion/optimization needs:

1. Measured `scan_ms` vs `extract_ms` on cold index and known-signature-miss incremental runs (Spark, not a laptop guess).
2. A retained edge-set / golden quality gate so fusion cannot silently lose calls.
3. An explicit product decision to fuse.

Until those exist, keep the second pass.
