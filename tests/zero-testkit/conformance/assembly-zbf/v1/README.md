# Assembly/ZBF KAT v1

Immutable known-answer vectors for AssemblyManifestV1 and ZBF v1.

- `index.json` is canonical sorted-key JSON with no trailing newline.
- Rust regenerates every positive vector and executes every indexed mutation row.
- C v1 and Python v1 independently hash and parse the checked vectors.
- Python v0 is the archived N-1 digest verifier.
- A correction creates `v2/`; published `v1/` bytes never change.

RCH execution proves compilation and cross-language agreement only. It is not native macOS, Linux, Windows, APFS, ext4/XFS, or NTFS evidence.
