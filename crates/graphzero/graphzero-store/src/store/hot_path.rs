//! Zero-copy hot path: every accessor here is a pointer cast over mmap'd
//! bytes via `bytemuck::cast_slice`. No serialization frameworks, no
//! allocation, no copies.

use anyhow::{Result, bail};

use super::format::{
    SEC_COVERAGE, SEC_EDGES, SEC_GLOBAL_META, SEC_SPANS, SEC_SYMBOLS, SEC_TRIGRAMS, ShardHeader,
    SpanEntry, SymbolEntry, TrigramPosting,
};

fn decode_span_entries<'a>(
    data: &'a [u8],
    at: usize,
    count: usize,
) -> Result<std::borrow::Cow<'a, [SpanEntry]>> {
    if count == 0 {
        return Ok(std::borrow::Cow::Borrowed(&[]));
    }
    let entries: &[SpanEntry] = take(data, at, count)?;
    Ok(std::borrow::Cow::Borrowed(entries))
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
    let Some(end) = at.checked_add(4) else {
        bail!("section offset overflow: u32 at {at}");
    };
    let Some(bytes) = data.get(at..end) else {
        bail!("truncated section: u32 at {} beyond {}", at, data.len());
    };
    let arr =
        <[u8; 4]>::try_from(bytes).map_err(|_| anyhow::anyhow!("invalid u32 width at {at}"))?;
    Ok(u32::from_le_bytes(arr))
}

fn take<T: bytemuck::Pod>(data: &[u8], at: usize, count: usize) -> Result<&[T]> {
    let Some(len) = count.checked_mul(std::mem::size_of::<T>()) else {
        bail!("section byte length overflow: {count} entries");
    };
    let Some(end) = at.checked_add(len) else {
        bail!("section end overflow: {len} bytes at {at}");
    };
    let Some(bytes) = data.get(at..end) else {
        bail!(
            "truncated section: {} x {}B at {} beyond {}",
            count,
            std::mem::size_of::<T>(),
            at,
            data.len()
        );
    };
    bytemuck::try_cast_slice(bytes)
        .map_err(|error| anyhow::anyhow!("invalid typed section at {at}: {error}"))
}

fn advance(at: usize, count: usize, width: usize) -> Result<usize> {
    let bytes = count
        .checked_mul(width)
        .ok_or_else(|| anyhow::anyhow!("section byte length overflow: {count} x {width}"))?;
    at.checked_add(bytes)
        .ok_or_else(|| anyhow::anyhow!("section end overflow: {bytes} bytes at {at}"))
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

    fn section(&self, idx: usize) -> Result<usize> {
        // Copy by value first: ShardHeader is #[repr(C, packed)]; indexing a
        // Use byte offsets because a reference to a packed field can be unaligned.
        let offsets = self.header.section_offsets;
        let offset = offsets
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("missing section offset {idx}"))?;
        usize::try_from(*offset)
            .map_err(|_| anyhow::anyhow!("section offset {offset} does not fit usize"))
    }

    /// Section 1: symbols. Layout: u32 count, u32 names_len, entries, names.
    pub fn symbols(&self) -> Result<SymbolsView<'a>> {
        let base = self.section(SEC_SYMBOLS)?;
        let count = read_u32(self.data, base)? as usize;
        let names_len = read_u32(self.data, advance(base, 1, 4)?)? as usize;
        let entries_at = advance(base, 1, 8)?;
        let entries: &[SymbolEntry] = take(self.data, entries_at, count)?;
        let names_at = advance(entries_at, count, std::mem::size_of::<SymbolEntry>())?;
        let names_end = advance(names_at, names_len, 1)?;
        let names = self
            .data
            .get(names_at..names_end)
            .ok_or_else(|| anyhow::anyhow!("truncated names region"))?;
        Ok(SymbolsView { entries, names })
    }

    /// Section 2: spans. Layout: u32 count, u32 pad, 24-byte entries. Returns a borrowed mmap slice
    /// without copying. The old thread-local + `from_raw_parts` path was unsound because the next
    /// call overwrote scratch; a true `'a` borrow into `self.data` cannot dangle while the view lives.
    pub fn spans(&self) -> Result<std::borrow::Cow<'a, [SpanEntry]>> {
        let base = self.section(SEC_SPANS)?;
        let count = read_u32(self.data, base)? as usize;
        decode_span_entries(self.data, advance(base, 1, 8)?, count)
    }

    /// Section 3: CSR edges. Layout: u32 num_symbols, u32 num_edges,
    /// offsets[n+1], targets[e], kinds[e], confidences[e].
    pub fn edges(&self) -> Result<EdgesView<'a>> {
        let base = self.section(SEC_EDGES)?;
        let num_symbols = read_u32(self.data, base)? as usize;
        let num_edges = read_u32(self.data, advance(base, 1, 4)?)? as usize;
        let mut at = advance(base, 1, 8)?;
        let offset_count = num_symbols
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("symbol offset count overflow"))?;
        let offsets: &[u32] = take(self.data, at, offset_count)?;
        at = advance(at, offset_count, std::mem::size_of::<u32>())?;
        let targets: &[u32] = take(self.data, at, num_edges)?;
        at = advance(at, num_edges, std::mem::size_of::<u32>())?;
        let kinds: &[u8] = take(self.data, at, num_edges)?;
        at = advance(at, num_edges, 1)?;
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
        let base = self.section(SEC_TRIGRAMS)?;
        let count = read_u32(self.data, base)? as usize;
        take(self.data, advance(base, 1, 8)?, count)
    }

    /// Section 5: coverage. Layout: u32 blob_count, u32 pad, hashes, bits.
    pub fn coverage(&self) -> Result<CoverageView<'a>> {
        let base = self.section(SEC_COVERAGE)?;
        let count = read_u32(self.data, base)? as usize;
        let hashes_at = advance(base, 1, 8)?;
        let hashes: &[[u8; 32]] = take(self.data, hashes_at, count)?;
        let bits_at = advance(hashes_at, count, std::mem::size_of::<[u8; 32]>())?;
        let bit_count = count
            .checked_mul(3)
            .ok_or_else(|| anyhow::anyhow!("coverage bit count overflow"))?;
        let bits_len = bit_count.div_ceil(8);
        let bits_end = advance(bits_at, bits_len, 1)?;
        let bits = self
            .data
            .get(bits_at..bits_end)
            .ok_or_else(|| anyhow::anyhow!("truncated coverage bitmap"))?;
        Ok(CoverageView {
            blob_hashes: hashes,
            bits,
        })
    }

    /// Section 6: global metadata (CHD perfect hash + edge evidence).
    /// Layout: u32 n, u32 num_buckets, seeds[num_buckets], values[n],
    /// u32 evidence_count, u32 pad, evidence[SpanEntry].
    pub fn mph(&self) -> Result<MphView<'a>> {
        let base = self.section(SEC_GLOBAL_META)?;
        let n = read_u32(self.data, base)? as usize;
        let num_buckets = read_u32(self.data, advance(base, 1, 4)?)? as usize;
        let seeds_at = advance(base, 1, 8)?;
        let seeds: &[u32] = take(self.data, seeds_at, num_buckets)?;
        let values_at = advance(seeds_at, num_buckets, std::mem::size_of::<u32>())?;
        let values: &[u32] = take(self.data, values_at, n)?;
        Ok(MphView { seeds, values })
    }

    /// Per-edge evidence spans, aligned with CSR edge indices. Zero-copy borrow of mmap Pod
    /// rows. The prior always-owned Vec was a defensive reaction to (thread-local scratch +
    /// `from_raw_parts` that aliased across `QueryEngine::warm`).
    pub fn edge_evidence(&self) -> Result<std::borrow::Cow<'a, [SpanEntry]>> {
        let base = self.section(SEC_GLOBAL_META)?;
        let n = read_u32(self.data, base)? as usize;
        let num_buckets = read_u32(self.data, advance(base, 1, 4)?)? as usize;
        let metadata_count = num_buckets
            .checked_add(n)
            .ok_or_else(|| anyhow::anyhow!("global metadata count overflow"))?;
        let at = advance(
            advance(base, 1, 8)?,
            metadata_count,
            std::mem::size_of::<u32>(),
        )?;
        let count = read_u32(self.data, at)? as usize;
        decode_span_entries(self.data, advance(at, 1, 8)?, count)
    }

    /// Symbol name lookup by entry (zero-copy slice into the names region).
    /// Fails closed on corrupt offsets: out-of-bounds or non-UTF-8 names
    /// render as "" instead of panicking (corruption detection).
    pub fn symbol_name(view: &SymbolsView<'a>, entry: &SymbolEntry) -> &'a str {
        let off = entry.name_offset as usize;
        let len = entry.name_len as usize;
        view.names
            .get(off..off.saturating_add(len))
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .unwrap_or("")
    }
}
