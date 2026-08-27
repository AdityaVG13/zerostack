//! Zero-copy hot path: every accessor here is a pointer cast over mmap'd
//! bytes via `bytemuck::cast_slice`. No serialization frameworks, no
//! allocation, no copies (FR-002).

use anyhow::{Result, bail};

use super::format::{
    FORMAT_VERSION, FORMAT_VERSION_LEGACY, IdentifierSpanEntry, SEC_COVERAGE, SEC_EDGES,
    SEC_GLOBAL_META, SEC_SPANS, SEC_SYMBOLS, SEC_TRIGRAMS, ShardHeader, SpanEntry, SymbolEntry,
    TrigramPosting,
};

fn decode_span_entries<'a>(
    version: u8,
    data: &'a [u8],
    at: usize,
    count: usize,
) -> Result<std::borrow::Cow<'a, [SpanEntry]>> {
    if count == 0 {
        return Ok(std::borrow::Cow::Borrowed(&[]));
    }
    match version {
        FORMAT_VERSION_LEGACY => {
            let v1: &[IdentifierSpanEntry] = take(data, at, count)?;
            Ok(std::borrow::Cow::Owned(
                v1.iter()
                    .map(|e| SpanEntry::from_identifier_span(*e))
                    .collect(),
            ))
        }
        // v2 is already Pod SpanEntry in the mmap — borrow, do not to_vec.
        // graphzero-4n7's dangling-slice bug was thread-local scratch + from_raw_parts;
        // a true data-lifetime borrow into ShardView's bytes is sound (same as symbols/CSR).
        FORMAT_VERSION => {
            let v2: &[SpanEntry] = take(data, at, count)?;
            Ok(std::borrow::Cow::Borrowed(v2))
        }
        other => bail!("unsupported format version {other} in span section"),
    }
}

/// Zero-copy view over one GZSH file (shard or global), backed by mmap'd or
/// heap-loaded bytes owned by the caller.
pub struct ShardView<'a> {
    data: &'a [u8],
    header: ShardHeader,
}

/// Zero-copy symbol section: entry array + name string region.
pub struct SymbolsView<'a> {
    pub entries: &'a [SymbolEntry],
    pub names: &'a [u8],
}

/// Zero-copy CSR edge section.
pub struct EdgesView<'a> {
    pub offsets: &'a [u32],
    pub targets: &'a [u32],
    pub kinds: &'a [u8],
    pub confidences: &'a [u8],
}

/// Zero-copy coverage section: blob hashes + packed 3-bit tier bitmap.
pub struct CoverageView<'a> {
    pub blob_hashes: &'a [[u8; 32]],
    pub bits: &'a [u8],
}

/// Zero-copy global metadata: CHD perfect-hash arrays.
pub struct MphView<'a> {
    pub seeds: &'a [u32],
    pub values: &'a [u32],
}

fn read_u32(data: &[u8], at: usize) -> Result<u32> {
    if at + 4 > data.len() {
        bail!("truncated section: u32 at {} beyond {}", at, data.len());
    }
    let arr: [u8; 4] = bytemuck::cast_slice::<u8, u8>(&data[at..at + 4])
        .try_into()
        .expect("4-byte slice");
    Ok(u32::from_le_bytes(arr))
}

fn take<T: bytemuck::Pod>(data: &[u8], at: usize, count: usize) -> Result<&[T]> {
    let len = count * std::mem::size_of::<T>();
    if at + len > data.len() {
        bail!(
            "truncated section: {} x {}B at {} beyond {}",
            count,
            std::mem::size_of::<T>(),
            at,
            data.len()
        );
    }
    Ok(bytemuck::cast_slice(&data[at..at + len]))
}

impl<'a> ShardView<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let header = ShardHeader::parse(data)?;
        Ok(Self { data, header })
    }

    pub fn from_header(data: &'a [u8], header: ShardHeader) -> Self {
        Self { data, header }
    }

    pub fn header(&self) -> &ShardHeader {
        &self.header
    }

    fn section(&self, idx: usize) -> usize {
        {
            // Copy by value first: ShardHeader is #[repr(C, packed)]; indexing a
            // field reference can form an unaligned reference (EXP-005/020).
            let offsets = self.header.section_offsets;
            offsets[idx] as usize
        }
    }

    /// Section 1: symbols. Layout: u32 count, u32 names_len, entries, names.
    pub fn symbols(&self) -> Result<SymbolsView<'a>> {
        let base = self.section(SEC_SYMBOLS);
        let count = read_u32(self.data, base)? as usize;
        let names_len = read_u32(self.data, base + 4)? as usize;
        let entries: &[SymbolEntry] = take(self.data, base + 8, count)?;
        let names_at = base + 8 + count * std::mem::size_of::<SymbolEntry>();
        if names_at + names_len > self.data.len() {
            bail!("truncated names region");
        }
        let names: &[u8] = bytemuck::cast_slice(&self.data[names_at..names_at + names_len]);
        Ok(SymbolsView { entries, names })
    }

    /// Section 2: spans. Layout: u32 count, u32 pad, entries (v1: 16B, v2: 24B).
    ///
    /// Returns a `Cow`: borrowed mmap slice for v2 (zero-copy, FR-002), owned
    /// upgrade for legacy v1. The old thread-local + `from_raw_parts` path
    /// (graphzero-4n7) was unsound because the next call overwrote scratch;
    /// a true `'a` borrow into `self.data` cannot dangle while the view lives.
    pub fn spans(&self) -> Result<std::borrow::Cow<'a, [SpanEntry]>> {
        let base = self.section(SEC_SPANS);
        let count = read_u32(self.data, base)? as usize;
        decode_span_entries(self.header.version, self.data, base + 8, count)
    }

    /// Section 3: CSR edges. Layout: u32 num_symbols, u32 num_edges,
    /// offsets[n+1], targets[e], kinds[e], confidences[e].
    pub fn edges(&self) -> Result<EdgesView<'a>> {
        let base = self.section(SEC_EDGES);
        let num_symbols = read_u32(self.data, base)? as usize;
        let num_edges = read_u32(self.data, base + 4)? as usize;
        let mut at = base + 8;
        let offsets: &[u32] = take(self.data, at, num_symbols + 1)?;
        at += (num_symbols + 1) * 4;
        let targets: &[u32] = take(self.data, at, num_edges)?;
        at += num_edges * 4;
        let kinds: &[u8] = take(self.data, at, num_edges)?;
        at += num_edges;
        let confidences: &[u8] = take(self.data, at, num_edges)?;
        Ok(EdgesView {
            offsets,
            targets,
            kinds,
            confidences,
        })
    }

    /// Section 4: trigram postings. Layout: u32 count, u32 pad, postings.
    pub fn trigrams(&self) -> Result<&'a [TrigramPosting]> {
        let base = self.section(SEC_TRIGRAMS);
        let count = read_u32(self.data, base)? as usize;
        take(self.data, base + 8, count)
    }

    /// Section 5: coverage. Layout: u32 blob_count, u32 pad, hashes, bits.
    pub fn coverage(&self) -> Result<CoverageView<'a>> {
        let base = self.section(SEC_COVERAGE);
        let count = read_u32(self.data, base)? as usize;
        let hashes: &[[u8; 32]] = take(self.data, base + 8, count)?;
        let bits_at = base + 8 + count * 32;
        let bits_len = (count * 3).div_ceil(8);
        if bits_at + bits_len > self.data.len() {
            bail!("truncated coverage bitmap");
        }
        let bits: &[u8] = bytemuck::cast_slice(&self.data[bits_at..bits_at + bits_len]);
        Ok(CoverageView {
            blob_hashes: hashes,
            bits,
        })
    }

    /// Section 6: global metadata (CHD perfect hash + edge evidence).
    /// Layout: u32 n, u32 num_buckets, seeds[num_buckets], values[n],
    /// u32 evidence_count, u32 pad, evidence[SpanEntry].
    pub fn mph(&self) -> Result<MphView<'a>> {
        let base = self.section(SEC_GLOBAL_META);
        let n = read_u32(self.data, base)? as usize;
        let num_buckets = read_u32(self.data, base + 4)? as usize;
        let seeds: &[u32] = take(self.data, base + 8, num_buckets)?;
        let values: &[u32] = take(self.data, base + 8 + num_buckets * 4, n)?;
        Ok(MphView { seeds, values })
    }

    /// Per-edge evidence spans, aligned with CSR edge indices (INV-002).
    ///
    /// v2: zero-copy borrow of mmap Pod rows. v1: owned upgrade. The prior
    /// always-owned Vec was a defensive reaction to graphzero-4n7 (thread-local
    /// scratch + `from_raw_parts` that aliased across `QueryEngine::warm`).
    /// Borrowing `self.data` for `'a` is the same sound pattern as `symbols`
    /// / `edges` and removes the O(|E|) / O(|defs|) warm-path copy (i33n4).
    pub fn edge_evidence(&self) -> Result<std::borrow::Cow<'a, [SpanEntry]>> {
        let base = self.section(SEC_GLOBAL_META);
        let n = read_u32(self.data, base)? as usize;
        let num_buckets = read_u32(self.data, base + 4)? as usize;
        let at = base + 8 + (num_buckets + n) * 4;
        let count = read_u32(self.data, at)? as usize;
        decode_span_entries(self.header.version, self.data, at + 8, count)
    }

    /// Symbol name lookup by entry (zero-copy slice into the names region).
    /// Fails closed on corrupt offsets: out-of-bounds or non-UTF-8 names
    /// render as "" instead of panicking (NFR-007 corruption detection).
    pub fn symbol_name(view: &SymbolsView<'a>, entry: &SymbolEntry) -> &'a str {
        let off = entry.name_offset as usize;
        let len = entry.name_len as usize;
        view.names
            .get(off..off.saturating_add(len))
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .unwrap_or("")
    }
}
