//! semantic sidecar shard layout (FR-004) — mmap-friendly fixed layout.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bytemuck::{Pod, Zeroable};
use memmap2::Mmap;

use crate::SEMANTIC_DIM;
use crate::index::SemanticRecord;

pub const SEMANTIC_V1_MAGIC: [u8; 4] = *b"GZSV";
pub const SEMANTIC_V1_VERSION: u8 = 1;
pub const SEMANTIC_VERSION: u8 = 2;
const HEADER_LEN: usize = 16;
const INTEGRITY_SCHEME_SHA256: u8 = 1;
const INTEGRITY_DIGEST_LEN: usize = 32;

/// Integrity strength of a semantic shard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticIntegrity {
    /// The writer bound the complete body to an appended SHA-256 digest.
    Verified,
    /// A legacy v1 shard had a zero reserved field and carries no integrity tag.
    LegacyUnverified,
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
struct SemanticHeader {
    magic: [u8; 4],
    version: u8,
    dim: u16,
    record_count: u32,
    reserved: [u8; 5],
}

const _: () = assert!(std::mem::size_of::<SemanticHeader>() == HEADER_LEN);

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C, packed)]
pub struct SemanticRecordWire {
    blob_hash: [u8; 32],
    start: u32,
    end: u32,
    name_len: u16,
    name_pad: u16,
}

const _: () = assert!(std::mem::size_of::<SemanticRecordWire>() == 44);

fn wire_for_record(r: &SemanticRecord) -> Result<SemanticRecordWire> {
    let name = r.span.label.as_bytes();
    if name.len() > u16::MAX as usize {
        bail!("semantic span label too long");
    }
    Ok(SemanticRecordWire {
        blob_hash: r.blob_hash.0,
        start: r.span.start,
        end: r.span.end,
        name_len: name.len() as u16,
        name_pad: 0,
    })
}

fn build_wires_and_names(records: &[SemanticRecord]) -> Result<(Vec<SemanticRecordWire>, Vec<u8>)> {
    let mut name_bytes = Vec::new();
    let mut wires = Vec::with_capacity(records.len());
    for r in records {
        let name = r.span.label.as_bytes();
        name_bytes.extend_from_slice(name);
        wires.push(wire_for_record(r)?);
    }
    Ok((wires, name_bytes))
}

fn header_for_records(record_count: usize) -> SemanticHeader {
    SemanticHeader {
        magic: SEMANTIC_V1_MAGIC,
        version: SEMANTIC_VERSION,
        dim: SEMANTIC_DIM as u16,
        record_count: record_count as u32,
        reserved: [0; 5],
    }
}

fn encode_shard_body(
    header: &SemanticHeader,
    wires: &[SemanticRecordWire],
    records: &[SemanticRecord],
    name_bytes: &[u8],
) -> Vec<u8> {
    use sha2::{Digest, Sha256};

    let mut body = Vec::new();
    body.extend_from_slice(bytemuck::cast_slice(wires));
    for r in records {
        body.extend_from_slice(bytemuck::cast_slice(r.vector.as_slice()));
    }
    body.extend_from_slice(name_bytes);

    let mut header = *header;
    header.reserved = [INTEGRITY_SCHEME_SHA256, 0, 0, 0, 0];
    let digest = Sha256::digest(&body);
    let mut out = Vec::with_capacity(HEADER_LEN + body.len() + INTEGRITY_DIGEST_LEN);
    out.extend_from_slice(bytemuck::bytes_of(&header));
    out.extend_from_slice(&body);
    out.extend_from_slice(&digest);
    out
}

fn validate_integrity(
    header: &SemanticHeader,
    mmap: &[u8],
    body_end: usize,
) -> Result<SemanticIntegrity> {
    use sha2::{Digest, Sha256};

    if header.version == SEMANTIC_V1_VERSION {
        if header.reserved != [0; 5] {
            bail!("semantic v1 reserved bytes must be zero");
        }
        if mmap.len() != body_end {
            bail!("legacy semantic shard has trailing bytes");
        }
        return Ok(SemanticIntegrity::LegacyUnverified);
    }
    if header.reserved != [INTEGRITY_SCHEME_SHA256, 0, 0, 0, 0] {
        bail!("unsupported semantic shard integrity scheme");
    }
    let digest_end = checked_section_end(body_end, INTEGRITY_DIGEST_LEN, "integrity digest")?;
    require_len(mmap.len(), digest_end, "integrity digest")?;
    if mmap.len() != digest_end {
        bail!("semantic shard has trailing bytes after integrity digest");
    }
    let expected = Sha256::digest(&mmap[HEADER_LEN..body_end]);
    if mmap[body_end..digest_end] != expected[..] {
        bail!("semantic shard body integrity mismatch");
    }
    Ok(SemanticIntegrity::Verified)
}

fn read_header(mmap: &[u8]) -> Result<SemanticHeader> {
    if mmap.len() < HEADER_LEN {
        bail!("semantic shard too small");
    }
    let header: SemanticHeader = bytemuck::pod_read_unaligned(&mmap[..HEADER_LEN]);
    if header.magic != SEMANTIC_V1_MAGIC {
        bail!("bad semantic magic");
    }
    if !matches!(header.version, SEMANTIC_V1_VERSION | SEMANTIC_VERSION) {
        bail!("unsupported semantic version");
    }
    if header.dim != SEMANTIC_DIM as u16 {
        bail!("unsupported semantic dimension");
    }
    Ok(header)
}

fn checked_section_end(start: usize, len: usize, section: &str) -> Result<usize> {
    start
        .checked_add(len)
        .with_context(|| format!("semantic shard {section} length overflow"))
}

fn require_len(mmap_len: usize, end: usize, section: &str) -> Result<()> {
    if mmap_len < end {
        bail!("truncated semantic {section}");
    }
    Ok(())
}

fn read_wire_records(mmap: &[u8], count: usize) -> Result<(Vec<SemanticRecordWire>, usize)> {
    let wire_len = count
        .checked_mul(std::mem::size_of::<SemanticRecordWire>())
        .context("semantic shard wire length overflow")?;
    let wires_end = checked_section_end(HEADER_LEN, wire_len, "wires")?;
    require_len(mmap.len(), wires_end, "wires")?;
    let wires = mmap[HEADER_LEN..wires_end]
        .chunks_exact(std::mem::size_of::<SemanticRecordWire>())
        .map(bytemuck::pod_read_unaligned)
        .collect();
    Ok((wires, wires_end))
}

fn validate_shard_layout(mmap: &[u8]) -> Result<SemanticIntegrity> {
    let header = read_header(mmap)?;
    let count = header.record_count as usize;
    let (wires, wires_end) = read_wire_records(mmap, count)?;
    let vector_len = count
        .checked_mul(SEMANTIC_DIM)
        .and_then(|items| items.checked_mul(std::mem::size_of::<f32>()))
        .context("semantic shard vector length overflow")?;
    let vec_end = checked_section_end(wires_end, vector_len, "vectors")?;
    require_len(mmap.len(), vec_end, "vectors")?;
    let names_len = wires.iter().try_fold(0usize, |sum, wire| {
        if wire.name_pad != 0 {
            bail!("semantic record padding must be zero");
        }
        sum.checked_add(wire.name_len as usize)
            .context("semantic shard name length overflow")
    })?;
    let names_end = checked_section_end(vec_end, names_len, "names")?;
    require_len(mmap.len(), names_end, "names")?;
    validate_integrity(&header, mmap, names_end)
}

pub struct SemanticShardWriter;

impl SemanticShardWriter {
    pub fn write(path: &Path, records: &[SemanticRecord]) -> Result<()> {
        let temp_path = semantic_temp_path(path);
        Self::write_via_temp(path, &temp_path, records)
    }

    fn write_via_temp(path: &Path, temp_path: &Path, records: &[SemanticRecord]) -> Result<()> {
        let (wires, name_bytes) = build_wires_and_names(records)?;
        let header = header_for_records(wires.len());
        let out = encode_shard_body(&header, &wires, records, &name_bytes);
        let write_result = (|| -> Result<()> {
            let mut f = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(temp_path)
                .with_context(|| format!("create semantic shard temp {}", temp_path.display()))?;
            f.write_all(&out)
                .with_context(|| format!("write semantic shard temp {}", temp_path.display()))?;
            f.sync_data()
                .with_context(|| format!("sync semantic shard temp {}", temp_path.display()))?;
            drop(f);
            fs::rename(temp_path, path).with_context(|| {
                format!(
                    "rename semantic shard temp {} to {}",
                    temp_path.display(),
                    path.display()
                )
            })?;
            sync_parent_dir(path)?;
            Ok(())
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(temp_path);
        }
        write_result
    }
}

fn semantic_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("semantic-shard");
    let unique = format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    parent.join(unique)
}

#[cfg(unix)]
fn sync_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)
            .with_context(|| format!("open semantic shard parent dir {}", parent.display()))?
            .sync_data()
            .with_context(|| format!("sync semantic shard parent dir {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_dir(_path: &Path) -> Result<()> {
    Ok(())
}

enum Backing {
    Mmap(Mmap),
    Heap(Vec<u8>),
}

/// True when mmap must not be used (test hook + degraded environments).
pub fn mmap_disabled() -> bool {
    std::env::var("GRAPHZERO_NO_MMAP")
        .map(|v| v == "1")
        .unwrap_or(false)
}

pub struct SemanticShardReader {
    backing: Backing,
    integrity: SemanticIntegrity,
}

impl SemanticShardReader {
    pub fn open(path: &Path) -> Result<Self> {
        let mut f =
            File::open(path).with_context(|| format!("open semantic shard {}", path.display()))?;
        let backing = if mmap_disabled() {
            Self::read_fallback(&mut f)?
        } else {
            // SAFETY: the mapping is read-only, owned by `SemanticShardReader`, and all
            // public accessors borrow from `self`, so the mapped bytes cannot outlive
            // the reader. We never mutate through this mapping, and `open` validates the
            // complete fixed-width layout before any accessor returns borrowed slices.
            // If mmap is denied by the platform or fails, the heap fallback reads the
            // same bytes and runs the same validation path (parity with store shards).
            match unsafe { Mmap::map(&f) } {
                Ok(m) => Backing::Mmap(m),
                Err(_) => Self::read_fallback(&mut f)?,
            }
        };
        let integrity = validate_shard_layout(Self::backing_bytes(&backing))?;
        Ok(Self { backing, integrity })
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

    fn bytes(&self) -> &[u8] {
        Self::backing_bytes(&self.backing)
    }

    /// Whether this reader is mmap-backed; the heap fallback reports false.
    pub fn used_mmap(&self) -> bool {
        matches!(self.backing, Backing::Mmap(_))
    }

    /// Report whether the complete persisted body was integrity-bound.
    pub const fn integrity(&self) -> SemanticIntegrity {
        self.integrity
    }

    pub fn vector_bytes(&self) -> Result<&[u8]> {
        let bytes = self.bytes();
        let header = read_header(bytes)?;
        let count = header.record_count as usize;
        let wires_end = HEADER_LEN + count * std::mem::size_of::<SemanticRecordWire>();
        let vec_end = wires_end + count * SEMANTIC_DIM * 4;
        if bytes.len() < vec_end {
            bail!("truncated semantic vectors");
        }
        Ok(&bytes[wires_end..vec_end])
    }

    pub fn golden_vector_hash(&self) -> Result<String> {
        use sha2::{Digest, Sha256};
        let bytes = self.vector_bytes()?;
        let mut h = Sha256::new();
        h.update(bytes);
        Ok(graphzero_store::fast_hex(h.finalize().as_slice()))
    }

    pub fn record_count(&self) -> Result<usize> {
        let header = read_header(self.bytes())?;
        Ok(header.record_count as usize)
    }
}
