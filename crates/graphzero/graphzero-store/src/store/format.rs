//! GZSH binary format: fixed-offset header and `repr(C)` section entry
//! types. See `docs/contracts/format_change_checklist.md` for version bumps.
//!
//! Layout (all little-endian):
//! ```text
//! 0x00  magic           [u8; 4]  = "GZSH"
//! 0x04  version         u8       = FORMAT_VERSION (0x02 current; 0x01 legacy)
//! 0x05  reserved        [u8; 24] = 0x00..
//! 0x1D  section_count   u8       = 6
//! 0x1E  section_offsets [u64; 6] (absolute file offsets)
//! 0x4E  header_crc      u32      (CRC32c over bytes 0x00..0x4E)
//! 0x52  sections...
//! ```

use anyhow::{Result, bail};
use bytemuck::{Pod, Zeroable};

pub const SHARD_MAGIC: [u8; 4] = *b"GZSH";
pub const DELTA_MAGIC: [u8; 4] = *b"GZDL";
pub const MANIFEST_MAGIC: [u8; 4] = *b"GZMF";
pub const FORMAT_VERSION_LEGACY: u8 = 0x01;
pub const FORMAT_VERSION: u8 = 0x02;
pub const SECTION_COUNT: usize = 6;
pub const HEADER_LEN: usize = 0x52;

/// Section indices within `section_offsets`.
pub const SEC_SYMBOLS: usize = 0;
pub const SEC_SPANS: usize = 1;
pub const SEC_EDGES: usize = 2;
pub const SEC_TRIGRAMS: usize = 3;
pub const SEC_COVERAGE: usize = 4;
pub const SEC_GLOBAL_META: usize = 5;

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
pub struct ShardHeader {
    pub magic: [u8; 4],
    pub version: u8,
    pub reserved: [u8; 24],
    pub section_count: u8,
    pub section_offsets: [u64; SECTION_COUNT],
    pub header_crc: u32,
}

const _: () = assert!(std::mem::size_of::<ShardHeader>() == HEADER_LEN);

impl ShardHeader {
    pub fn new(section_offsets: [u64; SECTION_COUNT]) -> Self {
        let mut h = Self {
            magic: SHARD_MAGIC,
            version: FORMAT_VERSION,
            reserved: [0u8; 24],
            section_count: SECTION_COUNT as u8,
            section_offsets,
            header_crc: 0,
        };
        h.header_crc = h.compute_crc();
        h
    }

    pub fn compute_crc(&self) -> u32 {
        let bytes = bytemuck::bytes_of(self);
        crc32fast::hash(&bytes[..HEADER_LEN - 4])
    }

    /// Parse and validate a header from the start of `data`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < HEADER_LEN {
            bail!("file too small for GZSH header: {} bytes", data.len());
        }
        let header: ShardHeader = bytemuck::pod_read_unaligned(&data[..HEADER_LEN]);
        if header.magic != SHARD_MAGIC {
            bail!("bad magic: expected GZSH, found {:02x?}", header.magic);
        }
        if header.version != FORMAT_VERSION && header.version != FORMAT_VERSION_LEGACY {
            bail!(
                "unsupported format version {}, expected {} or {}",
                header.version,
                FORMAT_VERSION,
                FORMAT_VERSION_LEGACY
            );
        }
        if header.section_count != SECTION_COUNT as u8 {
            bail!(
                "unsupported section count {}, expected {}",
                header.section_count,
                SECTION_COUNT
            );
        }
        if header.header_crc != header.compute_crc() {
            bail!("header CRC mismatch: shard corrupt");
        }
        let offsets = header.section_offsets;
        for (i, off) in offsets.iter().enumerate() {
            if *off as usize > data.len() {
                bail!(
                    "section {} offset {} beyond file end {}",
                    i,
                    off,
                    data.len()
                );
            }
        }
        Ok(header)
    }
}

/// Symbol table entry. Names live in a string region after the entry array.
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq, Eq)]
#[repr(C, packed)]
pub struct SymbolEntry {
    pub symbol_id: u32,
    pub name_offset: u32,
    pub name_len: u16,
    pub kind: u8,
    pub tier: u8,
    pub flags: u16,
}

const _: () = assert!(std::mem::size_of::<SymbolEntry>() == 14);

/// On-disk identifier-only span. Kept for reading legacy shards.
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq, Eq)]
#[repr(C, packed)]
pub struct IdentifierSpanEntry {
    pub blob_idx: u32,
    pub start: u32,
    pub end: u32,
    pub symbol_id: u32,
}

const _: () = assert!(std::mem::size_of::<IdentifierSpanEntry>() == 16);

/// Byte span of a symbol occurrence inside a content-addressed blob.
/// `start`/`end` are the identifier/name extent; `block_start`/`block_end`
/// are the full definition node when tier-A extraction provides them (v2).
/// `blob_idx` references the blob hash table in the coverage section.
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq, Eq)]
#[repr(C, packed)]
pub struct SpanEntry {
    pub blob_idx: u32,
    pub start: u32,
    pub end: u32,
    pub symbol_id: u32,
    pub block_start: u32,
    pub block_end: u32,
}

const _: () = assert!(std::mem::size_of::<SpanEntry>() == 24);

impl Default for SpanEntry {
    fn default() -> Self {
        Zeroable::zeroed()
    }
}

impl SpanEntry {
    /// Upgrade an identifier-only on-disk row.
    pub fn from_identifier_span(entry: IdentifierSpanEntry) -> Self {
        Self {
            blob_idx: entry.blob_idx,
            start: entry.start,
            end: entry.end,
            symbol_id: entry.symbol_id,
            block_start: 0,
            block_end: 0,
        }
    }

    /// Byte range for outline/skeleton line mapping: full block when present.
    pub fn outline_byte_range(&self) -> (u32, u32) {
        if self.block_end > self.block_start {
            (self.block_start, self.block_end)
        } else {
            (self.start, self.end)
        }
    }

    /// Identifier/name extent for evidence refs and symbol locate.
    pub fn name_byte_range(&self) -> (u32, u32) {
        (self.start, self.end)
    }
}

/// Trigram posting: packed 3-byte trigram, blob table index, byte offset.
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq, Eq)]
#[repr(C, packed)]
pub struct TrigramPosting {
    pub trigram: u32,
    pub blob_idx: u32,
    pub offset: u32,
}

const _: () = assert!(std::mem::size_of::<TrigramPosting>() == 12);

/// Pack a 3-byte trigram into a u32 (big-endian byte order, high byte zero).
pub fn pack_trigram(bytes: [u8; 3]) -> u32 {
    ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | bytes[2] as u32
}

pub fn unpack_trigram(t: u32) -> [u8; 3] {
    [(t >> 16) as u8, (t >> 8) as u8, t as u8]
}

/// Symbol kinds (tier-A lexical extraction).
pub mod symbol_kind {
    pub const FUNCTION: u8 = 0;
    pub const TYPE: u8 = 1;
    pub const MODULE: u8 = 2;
    pub const OTHER: u8 = 3;
}

/// Align `offset` up to 8 bytes.
pub fn align8(offset: usize) -> usize {
    (offset + 7) & !7
}
