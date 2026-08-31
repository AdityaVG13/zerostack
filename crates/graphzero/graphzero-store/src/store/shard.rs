//! Shard writer and reader for the GZSH container.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result};
use memmap2::Mmap;

use super::coverage::CoverageBitmap;
use super::csr::BuiltCsr;
use super::format::{HEADER_LEN, SECTION_COUNT, ShardHeader, SpanEntry, TrigramPosting, align8};
use super::hot_path::ShardView;
use super::symbol_table::BuiltSymbolTable;

/// Target shard payload size (hybrid partitioning, ~4MB).
pub const TARGET_SHARD_SIZE: usize = 4 * 1024 * 1024;

/// In-memory shard content, serialized to a GZSH file by `write_to`.
pub struct ShardBuilder {
    pub symbols: BuiltSymbolTable,
    pub spans: Vec<SpanEntry>,
    pub csr: BuiltCsr,
    pub trigrams: Vec<TrigramPosting>,
    pub coverage_blobs: Vec<[u8; 32]>,
    pub coverage: CoverageBitmap,
}

fn pad_to8(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(8) {
        buf.push(0);
    }
}

impl ShardBuilder {
    /// Serialize all six sections plus header into GZSH bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut offsets = [0u64; SECTION_COUNT];
        let mut body: Vec<u8> = Vec::new();
        let base = align8(HEADER_LEN);

        // Section 1: symbols — u32 count, u32 names_len, entries, names.
        offsets[0] = (base + body.len()) as u64;
        body.extend_from_slice(&(self.symbols.entries.len() as u32).to_le_bytes());
        body.extend_from_slice(&(self.symbols.name_bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(bytemuck::cast_slice(&self.symbols.entries));
        body.extend_from_slice(&self.symbols.name_bytes);
        pad_to8(&mut body);

        // Section 2: spans — u32 count, u32 pad, entries.
        offsets[1] = (base + body.len()) as u64;
        body.extend_from_slice(&(self.spans.len() as u32).to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(bytemuck::cast_slice(&self.spans));
        pad_to8(&mut body);

        // Section 3: CSR — u32 num_symbols, u32 num_edges, arrays.
        offsets[2] = (base + body.len()) as u64;
        let num_symbols = self.csr.offsets.len().saturating_sub(1);
        body.extend_from_slice(&(num_symbols as u32).to_le_bytes());
        body.extend_from_slice(&(self.csr.targets.len() as u32).to_le_bytes());
        body.extend_from_slice(bytemuck::cast_slice(&self.csr.offsets));
        body.extend_from_slice(bytemuck::cast_slice(&self.csr.targets));
        body.extend_from_slice(&self.csr.kinds);
        body.extend_from_slice(&self.csr.confidences);
        pad_to8(&mut body);

        // Section 4: trigrams — u32 count, u32 pad, postings.
        offsets[3] = (base + body.len()) as u64;
        body.extend_from_slice(&(self.trigrams.len() as u32).to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(bytemuck::cast_slice(&self.trigrams));
        pad_to8(&mut body);

        // Section 5: coverage — u32 blob_count, u32 pad, hashes, bits.
        offsets[4] = (base + body.len()) as u64;
        body.extend_from_slice(&(self.coverage_blobs.len() as u32).to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        for h in &self.coverage_blobs {
            body.extend_from_slice(h);
        }
        body.extend_from_slice(self.coverage.as_bytes());
        pad_to8(&mut body);

        // Section 6: global metadata — u32 n, u32 num_buckets, seeds,
        // values, u32 evidence_count, u32 pad, evidence spans.
        offsets[5] = (base + body.len()) as u64;
        body.extend_from_slice(&(self.symbols.mph.values.len() as u32).to_le_bytes());
        body.extend_from_slice(&(self.symbols.mph.seeds.len() as u32).to_le_bytes());
        body.extend_from_slice(bytemuck::cast_slice(&self.symbols.mph.seeds));
        body.extend_from_slice(bytemuck::cast_slice(&self.symbols.mph.values));
        body.extend_from_slice(&(self.csr.evidence.len() as u32).to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        body.extend_from_slice(bytemuck::cast_slice(&self.csr.evidence));
        pad_to8(&mut body);

        let header = ShardHeader::new(offsets);
        let mut out = Vec::with_capacity(base + body.len());
        out.extend_from_slice(bytemuck::bytes_of(&header));
        out.resize(base, 0);
        out.extend_from_slice(&body);
        out
    }

    /// Write the shard to `path` and sync its data before manifest publication.
    pub fn write_to(&self, path: &Path) -> Result<u64> {
        self.write_to_with_sync(path, true)
    }

    /// Write the shard; when `sync` is false the caller must fsync before
    /// publishing the manifest (batch durability for multi-shard publishes).
    pub fn write_to_with_sync(&self, path: &Path, sync: bool) -> Result<u64> {
        let bytes = self.to_bytes();
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("create shard {}", path.display()))?;
        f.write_all(&bytes)?;
        if sync {
            f.sync_data()?;
        }
        Ok(file_hash64(&bytes))
    }
}

/// 64-bit integrity hash of a shard file, stored in the manifest.
/// CRC32c of the bytes in the low word, length in the high word.
pub fn file_hash64(bytes: &[u8]) -> u64 {
    ((bytes.len() as u64) << 32) | crc32fast::hash(bytes) as u64
}

enum Backing {
    Mmap(Mmap),
    Heap(Vec<u8>),
}

/// Opened shard: mmap-backed by default, heap fallback when mmap is
/// unavailable. `view()` returns the zero-copy section reader.
pub struct ShardReader {
    backing: Backing,
    header: ShardHeader,
}

/// True when mmap must not be used (test hook + degraded environments).
pub fn mmap_disabled() -> bool {
    std::env::var("GRAPHZERO_NO_MMAP")
        .map(|v| v == "1")
        .unwrap_or(false)
}

impl ShardReader {
    pub fn open(path: &Path) -> Result<Self> {
        let mut f = File::open(path).with_context(|| format!("open shard {}", path.display()))?;
        let len = f
            .metadata()
            .with_context(|| format!("stat shard {}", path.display()))?
            .len();
        if len < HEADER_LEN as u64 {
            anyhow::bail!("file too small for GZSH header: {len} bytes");
        }
        let backing = if mmap_disabled() {
            Self::read_fallback(&mut f)?
        } else {
            // SAFETY: mmap is sound here because GraphZero treats shard files as immutable content-addressed
            // artifacts after publication: writers write a complete new file, fdatasync it, then atomically
            // publish a manifest that points at it.
            match unsafe { Mmap::map(&f) } {
                Ok(m) => Backing::Mmap(m),
                Err(_) => Self::read_fallback(&mut f)?,
            }
        };
        let header = ShardHeader::parse(Self::backing_bytes(&backing))?;
        Ok(Self { backing, header })
    }

    fn read_fallback(f: &mut File) -> Result<Backing> {
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(Backing::Heap(buf))
    }

    fn backing_bytes(backing: &Backing) -> &[u8] {
        match backing {
            Backing::Mmap(m) => m,
            Backing::Heap(v) => v,
        }
    }

    pub fn used_mmap(&self) -> bool {
        matches!(self.backing, Backing::Mmap(_))
    }

    pub fn bytes(&self) -> &[u8] {
        Self::backing_bytes(&self.backing)
    }

    pub fn view(&self) -> Result<ShardView<'_>> {
        Ok(ShardView::from_header(self.bytes(), self.header))
    }
}
