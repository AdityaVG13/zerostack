# graphzero-store safety notes

## Packed `ShardHeader` section offsets

`ShardHeader` is `#[repr(C, packed)]`. Indexing `header.section_offsets[idx]` may form an
unaligned reference (clippy `unaligned_references`, EXP-005/020).

Always copy by value first:

```rust
let offsets = self.header.section_offsets;
offsets[idx]
```

See `src/store/hot_path.rs` (`section`) and `src/store/format.rs` parse path.

### Clippy recipe (RCH)

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero \
  cargo clippy -p graphzero-store --lib -- -W clippy::unaligned_references
```

## Published shards are immutable under open RO mmap readers (audit 0028/0029)

**Design intent (documented; not a separately proven formal model):** once a
snapshot shard is *published* via the manifest, readers may hold a read-only
`memmap2::Mmap` over that file for the life of a `ShardReader` / `Snapshot`.
The soundness argument for `unsafe { Mmap::map(&f) }` in
`src/store/shard.rs` (`ShardReader::open`) is that published shard bytes are
not mutated in place while those maps remain open.

### Grounding in code (what exists today)

1. **Write path (new file, then sync, then publish):**
   - `ShardFile::write_to` / `write_to_with_sync` in `src/store/shard.rs`
     write a complete shard file and optionally `sync_data()` (fdatasync) so
     bytes are durable before the manifest points at them (FR-013/FR-014).
   - Index publish (`src/store/indexer.rs`) takes a durability barrier over
     shard/global writes, then calls `Manifest::atomic_publish`.
   - `Manifest::atomic_publish` in `src/store/manifest.rs`: write
     `.manifest.tmp`, `sync_data`, rename over `manifest`, then directory
     `sync_all`. Readers discover the published snapshot only through this
     manifest (see `src/store/query/mod.rs`: readers never lock; they mmap
     the snapshot named by the manifest).

2. **Read path (RO mmap + validation):**
   - `ShardReader::open` opens the shard path read-only, rejects files
     shorter than the fixed GZSH header before mapping, then maps with
     `Mmap::map` (or heap fallback when `GRAPHZERO_NO_MMAP=1` / mmap denied).
   - The in-code `SAFETY` comment at the mmap site states the immutability
     contract and that magic/version/section/CRC validation runs against the
     mapped length immediately after open.

3. **Related but distinct:** `src/store/publish.rs` is the *external edge
   publish* WAL protocol (`publish_batch`, `WriterLock`). It does not open
   shard mmaps. Do not treat edge-publish as the shard RO-mmap invariant;
   the mmap invariant is shard + manifest publish as above.

### What this document does **not** claim

- No claim that every writer path in the crate has been exhaustively proven
  never to truncate/overwrite a live published shard inode under an open map.
- No claim about remote/NFS semantics, external processes, or operator
  `rm`/`truncate` of store files while readers are live.
- No claim that heap fallback (`GRAPHZERO_NO_MMAP`) changes durability;
  it only changes backing storage for the same validated header path.
- Audit labels **0028/0029** name the residual documentation ask from the
  unsafe audit portfolio; this section records the intended invariant and
  the code sites that implement the publish/mmap protocol. It does not
  invent additional guarantees beyond those sites.
