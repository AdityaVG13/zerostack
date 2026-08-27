# Journal delta feed

Generated evidence for **fszero-sa2v**.

## Wire schema

Each serde snake_case delta contains `version`, `seq`, `op`, `path`,
`before_hash`, `after_hash`, `byte_range`, and `replacement`.
`op` is `upsert` or `remove`. Hashes are lowercase SHA-256 of the exact
image bytes. `replacement` contains only the changed postimage bytes, so the
wire is self-contained and does not require parsing or expanding an `fz://`,
`gz://`, or `tz://` reference.

`byte_range` is the minimal changed span after removing the common prefix and
suffix. Its three offsets are architecture-neutral `u64` byte offsets and are
half-open: replace `start..before_end` in the
preimage with `start..after_end` from the postimage. A remove covers the full
preimage (`0..len`) and has `after_end = 0`.

## Law and failure model

For every gapless journal page:

`integrate(journal_deltas(sequence)) == batch_render(sequence)` byte-for-byte.

The feed executes one bounded ascending SQL query. It fails closed on query
errors, unavailable journal pre/post blobs, or a first/middle sequence gap.
Integration checks its exclusive cursor, preimage and reconstructed postimage
hashes, operation shape, replacement length, and byte range against a staged
state. It publishes that state only after the whole page validates. Consumers
need only their prior state and the delta bytes; they never read workspace paths
or call an engine-specific ref expander.

## Complexity

For page bound `L`, the SQL scan is `O(L)` rows and one query. Publisher blob
expansion, diffing, and hashing are `O(B)` for total pre/post bytes. Consumer
integration is `O(L + D + S)`, where `D` is replacement bytes and `S` is caller
state cloning required for atomic publication. Consumer memory is `O(D + S)`.

## Detached RCH verification

`rch exec -- cargo test --test journal_delta -- --nocapture`

`rch exec -- cargo check --all-targets`
