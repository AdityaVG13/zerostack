# Local semantic resolve tier (`fszero-semantic-local`)

Supported **opt-in** feature (fszero-9wn). Default builds leave it **off**.

## What it is

When enabled, FSZero wires [frankensearch](https://crates.io/crates/frankensearch) **0.3.2**
`hash` + `lexical` into:

1. **`build_index`** — for dirty/new files, chunk text and memoize FNV hash
   embeddings per chunk SHA-256 digest in the shared recovery CAS
   (`semantic/emb/v1/<digest>`). Bounded by the same
   `FSZERO_INDEX_THREADS` / `FSZERO_INGEST_THREADS` caps as structural ingest.
2. **`fs.resolve` with `engine: "hybrid-local"`** (aliases: `hybrid`, `semantic`) —
   reranks top lexical survivors with hash cosine + lexical overlap; each
   candidate carries a `tier` field: `lexical` | `hash` | `lexical+hash` | `coaccess`.

Optional real ML embeddings (model2vec / fastembed) remain behind frankensearch's
own feature flags and are **not** enabled by FSZero's default opt-in. No cloud
embedding providers are configured.

## Enable

```bash
cargo build --features fszero-semantic-local
cargo test --features fszero-semantic-local --test resolve_semantic_local
```

## Default-off guarantee

With the feature disabled (default), the semantic ingest phase is compiled out.
Producer fingerprint records `semantic_local=0` / `semantic_embedder=off`, so
the cold-index path and the 100k gate are unchanged. Enabling the feature flips
the fingerprint (`semantic_embedder=frankensearch-hash-256`) and invalidates
derived AST/semantic rows cleanly (fszero-1it).

## Dependency pin

| Crate | Version | Features used |
| :-- | :-- | :-- |
| `frankensearch` | `0.3.2` (Cargo.lock) | `hash`, `lexical` (`default-features = false`) |

Local-first: hash embedder needs no model download. Audit with
`cargo audit` when shipping dependency bumps. Known transitive notes at
ship time (fszero-9wn): `crossbeam-epoch` RUSTSEC-2026-0204 is already
reachable via `fsqlite`/rayon on default builds; frankensearch's tantivy
path does not introduce a new advisory class. Track bumps separately.
