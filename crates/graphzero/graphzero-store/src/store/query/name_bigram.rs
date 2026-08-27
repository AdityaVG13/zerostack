//! fff-style bigram candidate index for symbol/path substring search.
//!
//! Spike behind `GRAPHZERO_SEARCH_BIGRAM=1`. Candidate filtering is exact-safe:
//! every UTF-8 string that contains needle (as bytes, len >= 2) contains all
//! consecutive byte bigrams of needle. Fuzzy reranking is intentionally not
//! applied.
//!
//! Selectivity (graphzero-2tee): when the rarest posting is denser than 25% of
//! the universe, or larger than `budget * 64`, return `None` so search falls
//! back to linear scan. Dense common needles (gold `parse*`) fill a small
//! budget after a few hundred `contains` checks; materializing multi-k id sets
//! is strictly slower.
//!
//! Memory layout (graphzero-mba0): packed `u16` byte-bigrams + densified
//! postings (singleton pairs + CSR for len>=2, `u16` doc ids) and binary
//! path hashes. Replaces `HashMap<u64, Vec<u32>>`.
//!
//! Publish-time sidecar (graphzero-lrin): `name_bigram_{id:08}.bin` (GZNB v1)
//! written at index publish so cold `OnceLock` loads instead of rebuilding.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow, bail};

use super::super::symbol_table::SymbolTable;
use super::snapshot::Snapshot;

pub const SEARCH_BIGRAM_ENV: &str = "GRAPHZERO_SEARCH_BIGRAM";

/// On-disk magic for publish-time name-bigram sidecars.
pub const NAME_BIGRAM_MAGIC: [u8; 4] = *b"GZNB";
/// Sidecar wire version (independent of GZSH).
pub const NAME_BIGRAM_VERSION: u8 = 1;

/// Snapshot-relative sidecar filename.
pub fn name_bigram_file_name(snapshot_id: u64) -> String {
    format!("name_bigram_{snapshot_id:08}.bin")
}

/// True when the env flag enables the indexed candidate path.
pub fn search_bigram_enabled() -> bool {
    match std::env::var(SEARCH_BIGRAM_ENV) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "on" | "yes"
        ),
        Err(_) => false,
    }
}

#[inline]
fn pack_bigram(a: u8, b: u8) -> u16 {
    ((a as u16) << 8) | (b as u16)
}

fn bigrams(s: &str) -> impl Iterator<Item = u16> + '_ {
    s.as_bytes().windows(2).map(|w| pack_bigram(w[0], w[1]))
}

fn insert_postings(map: &mut HashMap<u16, Vec<u16>>, text: &str, id: u16) {
    let mut seen = HashSet::new();
    for bg in bigrams(text) {
        if seen.insert(bg) {
            map.entry(bg).or_default().push(id);
        }
    }
}

fn hex_hash(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_hex_hash(hex: &str) -> Result<[u8; 32]> {
    let hex = hex.as_bytes();
    if hex.len() != 64 {
        return Err(anyhow!(
            "name_bigram: expected 64-hex path hash, got {}",
            hex.len()
        ));
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        let hi = hex_nibble(hex[i * 2])?;
        let lo = hex_nibble(hex[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(anyhow!("name_bigram: bad hex digit")),
    }
}

fn write_uleb128(out: &mut Vec<u8>, mut v: u32) {
    loop {
        let mut b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            b |= 0x80;
        }
        out.push(b);
        if v == 0 {
            break;
        }
    }
}

fn read_uleb128(buf: &[u8], mut i: usize) -> Option<(u32, usize)> {
    let mut result = 0u32;
    let mut shift = 0u32;
    while i < buf.len() {
        let b = buf[i];
        i += 1;
        result |= u32::from(b & 0x7f) << shift;
        if b & 0x80 == 0 {
            return Some((result, i));
        }
        shift += 7;
        if shift > 28 {
            return None;
        }
    }
    None
}

fn encode_id_list(list: &[u16], out: &mut Vec<u8>) {
    debug_assert!(list.len() >= 2);
    out.extend_from_slice(&list[0].to_le_bytes());
    let mut prev = list[0];
    for &id in &list[1..] {
        write_uleb128(out, u32::from(id - prev));
        prev = id;
    }
}

fn decode_id_list(buf: &[u8]) -> Option<Vec<u16>> {
    if buf.len() < 2 {
        return None;
    }
    let first = u16::from_le_bytes([buf[0], buf[1]]);
    let mut out = vec![first];
    let mut prev = first;
    let mut i = 2;
    while i < buf.len() {
        let (delta, next) = read_uleb128(buf, i)?;
        let id = prev.checked_add(delta as u16)?;
        out.push(id);
        prev = id;
        i = next;
    }
    Some(out)
}

/// Densified postings: singleton `(key,id)` pairs + CSR of delta-varint multi-lists.
#[derive(Debug, Default, Clone)]
struct DensePostings {
    /// Sorted by key (unique keys).
    singles: Vec<(u16, u16)>,
    /// Sorted unique keys for multi-id lists.
    keys: Vec<u16>,
    /// Byte offsets into `id_bytes` (length = keys.len() + 1 when non-empty).
    offsets: Vec<u32>,
    /// Delta-varint encoded multi-lists (first id u16-LE, then uleb128 gaps).
    id_bytes: Vec<u8>,
}

impl DensePostings {
    fn from_map(mut map: HashMap<u16, Vec<u16>>) -> Self {
        if map.is_empty() {
            return Self::default();
        }
        let mut keys_all: Vec<u16> = map.keys().copied().collect();
        keys_all.sort_unstable();

        let mut singles = Vec::new();
        let mut keys = Vec::new();
        let mut offsets = Vec::new();
        let mut id_bytes = Vec::new();

        for &k in &keys_all {
            let mut list = map.remove(&k).unwrap_or_default();
            list.sort_unstable();
            list.dedup();
            match list.len() {
                0 => {}
                1 => singles.push((k, list[0])),
                _ => {
                    if keys.is_empty() {
                        offsets.push(0);
                    }
                    keys.push(k);
                    encode_id_list(&list, &mut id_bytes);
                    offsets.push(id_bytes.len() as u32);
                }
            }
        }
        Self {
            singles,
            keys,
            offsets,
            id_bytes,
        }
    }

    fn collect_list(&self, key: u16) -> Option<Vec<u16>> {
        if let Ok(i) = self.singles.binary_search_by_key(&key, |e| e.0) {
            return Some(vec![self.singles[i].1]);
        }
        let i = self.keys.binary_search(&key).ok()?;
        let start = self.offsets[i] as usize;
        let end = self.offsets[i + 1] as usize;
        decode_id_list(&self.id_bytes[start..end])
    }

    fn approx_bytes(&self) -> usize {
        self.singles.len() * 4 + self.keys.len() * 2 + self.offsets.len() * 4 + self.id_bytes.len()
    }

    fn write_to(&self, out: &mut Vec<u8>) {
        write_u32(out, self.singles.len() as u32);
        for &(k, id) in &self.singles {
            out.extend_from_slice(&k.to_le_bytes());
            out.extend_from_slice(&id.to_le_bytes());
        }
        write_u32(out, self.keys.len() as u32);
        for &k in &self.keys {
            out.extend_from_slice(&k.to_le_bytes());
        }
        write_u32(out, self.offsets.len() as u32);
        for &o in &self.offsets {
            out.extend_from_slice(&o.to_le_bytes());
        }
        write_u32(out, self.id_bytes.len() as u32);
        out.extend_from_slice(&self.id_bytes);
    }

    fn read_from(buf: &[u8], mut i: usize) -> Result<(Self, usize)> {
        let (singles_len, ni) = read_u32(buf, i)?;
        i = ni;
        let mut singles = Vec::with_capacity(singles_len as usize);
        for _ in 0..singles_len {
            let (k, ni) = read_u16(buf, i)?;
            let (id, ni) = read_u16(buf, ni)?;
            singles.push((k, id));
            i = ni;
        }
        let (keys_len, ni) = read_u32(buf, i)?;
        i = ni;
        let mut keys = Vec::with_capacity(keys_len as usize);
        for _ in 0..keys_len {
            let (k, ni) = read_u16(buf, i)?;
            keys.push(k);
            i = ni;
        }
        let (offsets_len, ni) = read_u32(buf, i)?;
        i = ni;
        if keys_len > 0 && offsets_len != keys_len + 1 {
            bail!(
                "name_bigram: offsets_len {offsets_len} != keys_len+1 ({})",
                keys_len + 1
            );
        }
        if keys_len == 0 && offsets_len != 0 {
            bail!("name_bigram: empty keys but offsets_len {offsets_len}");
        }
        let mut offsets = Vec::with_capacity(offsets_len as usize);
        for _ in 0..offsets_len {
            let (o, ni) = read_u32(buf, i)?;
            offsets.push(o);
            i = ni;
        }
        let (id_bytes_len, ni) = read_u32(buf, i)?;
        i = ni;
        let end = i
            .checked_add(id_bytes_len as usize)
            .ok_or_else(|| anyhow!("name_bigram: id_bytes overflow"))?;
        if end > buf.len() {
            bail!("name_bigram: id_bytes truncated");
        }
        let id_bytes = buf[i..end].to_vec();
        Ok((
            Self {
                singles,
                keys,
                offsets,
                id_bytes,
            },
            end,
        ))
    }
}

fn write_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn read_u32(buf: &[u8], i: usize) -> Result<(u32, usize)> {
    if i + 4 > buf.len() {
        bail!("name_bigram: truncated u32");
    }
    Ok((
        u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]),
        i + 4,
    ))
}

fn read_u16(buf: &[u8], i: usize) -> Result<(u16, usize)> {
    if i + 2 > buf.len() {
        bail!("name_bigram: truncated u16");
    }
    Ok((u16::from_le_bytes([buf[i], buf[i + 1]]), i + 2))
}

/// Sorted-merge intersection of two ascending id lists.
fn intersect_sorted(a: &[u16], b: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(a.len().min(b.len()));
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

/// When the rarest bigram posting is this dense, indexed filtering costs more than
/// a budget-capped linear scan on gold-style common needles (graphzero-2tee).
const SELECTIVITY_FALLBACK_NUM: usize = 1;
const SELECTIVITY_FALLBACK_DEN: usize = 4; // >25% of universe → scan
/// If rarest posting exceeds `budget * PROBE`, expected scan visits to fill the
/// budget are far cheaper than materializing the candidate set.
const BUDGET_PROBE_FACTOR: usize = 64;

/// Candidate ids for `needle`, or `None` to fall back to full scan.
///
/// `Some(empty)` means no string can match (missing bigram). `budget` is the
/// search hit cap; when set, over-broad needles fall back to scan so dense
/// common-class queries do not regress vs linear scan.
fn candidate_ids(
    postings: &DensePostings,
    needle: &str,
    universe: usize,
    budget: Option<usize>,
) -> Option<Vec<u32>> {
    let keys: Vec<u16> = bigrams(needle).collect();
    if keys.is_empty() {
        return None;
    }
    let mut owned: Vec<Vec<u16>> = Vec::with_capacity(keys.len());
    for k in &keys {
        match postings.collect_list(*k) {
            Some(list) => owned.push(list),
            None => return Some(Vec::new()),
        }
    }
    owned.sort_by_key(|l| l.len());
    let rarest = owned[0].len();
    if universe > 0
        && rarest > universe.saturating_mul(SELECTIVITY_FALLBACK_NUM) / SELECTIVITY_FALLBACK_DEN
    {
        return None;
    }
    if let Some(cap) = budget {
        let cap = cap.max(1);
        if rarest > cap.saturating_mul(BUDGET_PROBE_FACTOR) {
            return None;
        }
    }
    let mut acc = std::mem::take(&mut owned[0]);
    for list in owned.iter().skip(1) {
        acc = intersect_sorted(&acc, list);
        if acc.is_empty() {
            break;
        }
    }
    Some(acc.into_iter().map(|id| id as u32).collect())
}

/// In-memory bigram postings over symbol names and path strings.
#[derive(Debug, Default)]
pub struct NameBigramIndex {
    symbol_postings: DensePostings,
    path_postings: DensePostings,
    /// Blob content hashes (32 raw bytes), parallel to path posting ids.
    path_hashes: Vec<[u8; 32]>,
    symbol_count: usize,
    path_count: usize,
    approx_bytes: usize,
}

impl NameBigramIndex {
    /// Build from sorted symbol names (id = index) and path (hash_hex, path) pairs.
    pub fn build_from_names_and_paths(
        names: &[String],
        path_pairs: &[(String, String)],
    ) -> Result<Self> {
        let symbol_count = names.len();
        let symbol_postings = if symbol_count > u16::MAX as usize {
            // The bigram sidecar is an optional accelerator. Oversized repositories
            // must keep exact search available through the linear fallback rather
            // than failing the entire GraphZero index build.
            DensePostings::default()
        } else {
            let mut symbol_map: HashMap<u16, Vec<u16>> = HashMap::new();
            for (id, name) in names.iter().enumerate() {
                if name.is_empty() {
                    continue;
                }
                insert_postings(&mut symbol_map, name, id as u16);
            }
            DensePostings::from_map(symbol_map)
        };

        let mut path_pairs: Vec<(String, String)> = path_pairs.to_vec();
        // Stable build order; search still walks snapshot.path_records() order.
        path_pairs.sort_by(|a, b| a.0.cmp(&b.0));
        let path_postings_enabled = path_pairs.len() <= u16::MAX as usize;
        let mut path_map: HashMap<u16, Vec<u16>> = HashMap::new();
        let mut path_hashes = Vec::with_capacity(path_pairs.len());
        for (idx, (hash, path)) in path_pairs.iter().enumerate() {
            path_hashes.push(parse_hex_hash(hash)?);
            if path_postings_enabled {
                insert_postings(&mut path_map, path, idx as u16);
            }
        }
        let path_postings = DensePostings::from_map(path_map);

        let path_count = path_hashes.len();
        let approx_bytes = estimate_bytes(&symbol_postings, &path_postings, path_count);

        Ok(Self {
            symbol_postings,
            path_postings,
            path_hashes,
            symbol_count,
            path_count,
            approx_bytes,
        })
    }

    pub fn build(snapshot: &Snapshot) -> Result<Self> {
        let view = snapshot
            .global_view()
            .map_err(|e| anyhow!("name_bigram view: {e}"))?;
        let table = SymbolTable::from_view(&view).map_err(|e| anyhow!("name_bigram table: {e}"))?;

        let mut names = Vec::with_capacity(table.len());
        for id in 0..table.len() as u32 {
            names.push(table.name(id).unwrap_or("").to_string());
        }
        let path_pairs: Vec<(String, String)> = snapshot
            .path_records()
            .map(|(h, r)| (h.to_hex(), r.path.clone()))
            .collect();
        Self::build_from_names_and_paths(&names, &path_pairs)
    }

    /// Encode densified index as GZNB v1 bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.approx_bytes);
        out.extend_from_slice(&NAME_BIGRAM_MAGIC);
        out.push(NAME_BIGRAM_VERSION);
        out.extend_from_slice(&[0u8; 3]); // reserved
        write_u32(&mut out, self.symbol_count as u32);
        write_u32(&mut out, self.path_count as u32);
        self.symbol_postings.write_to(&mut out);
        self.path_postings.write_to(&mut out);
        for h in &self.path_hashes {
            out.extend_from_slice(h);
        }
        out
    }

    /// Decode GZNB v1 bytes.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        if buf.len() < 16 {
            bail!("name_bigram: file too small");
        }
        if buf[0..4] != NAME_BIGRAM_MAGIC {
            bail!("name_bigram: bad magic {:02x?}", &buf[0..4]);
        }
        if buf[4] != NAME_BIGRAM_VERSION {
            bail!(
                "name_bigram: unsupported version {}, expected {}",
                buf[4],
                NAME_BIGRAM_VERSION
            );
        }
        let mut i = 8;
        let (symbol_count, ni) = read_u32(buf, i)?;
        let (path_count, ni) = read_u32(buf, ni)?;
        i = ni;
        let (symbol_postings, ni) = DensePostings::read_from(buf, i)?;
        let (path_postings, ni) = DensePostings::read_from(buf, ni)?;
        i = ni;
        let need = (path_count as usize)
            .checked_mul(32)
            .ok_or_else(|| anyhow!("name_bigram: path_hashes overflow"))?;
        let end = i
            .checked_add(need)
            .ok_or_else(|| anyhow!("name_bigram: path_hashes end overflow"))?;
        if end > buf.len() {
            bail!("name_bigram: path_hashes truncated");
        }
        let mut path_hashes = Vec::with_capacity(path_count as usize);
        for off in (0..need).step_by(32) {
            let mut h = [0u8; 32];
            h.copy_from_slice(&buf[i + off..i + off + 32]);
            path_hashes.push(h);
        }
        if path_hashes.len() != path_count as usize {
            bail!("name_bigram: path_hash count mismatch");
        }
        let approx_bytes = estimate_bytes(&symbol_postings, &path_postings, path_count as usize);
        Ok(Self {
            symbol_postings,
            path_postings,
            path_hashes,
            symbol_count: symbol_count as usize,
            path_count: path_count as usize,
            approx_bytes,
        })
    }

    /// Write GZNB sidecar next to shards.
    pub fn write_published(shards_dir: &Path, snapshot_id: u64, index: &Self) -> Result<()> {
        let path = shards_dir.join(name_bigram_file_name(snapshot_id));
        fs::write(&path, index.to_bytes())
            .map_err(|e| anyhow!("name_bigram write {}: {e}", path.display()))
    }

    /// Load published sidecar if present. `Ok(None)` when missing (legacy snapshot).
    pub fn try_load_published(shards_dir: &Path, snapshot_id: u64) -> Result<Option<Self>> {
        let path = shards_dir.join(name_bigram_file_name(snapshot_id));
        if !path.is_file() {
            return Ok(None);
        }
        let bytes =
            fs::read(&path).map_err(|e| anyhow!("name_bigram read {}: {e}", path.display()))?;
        Ok(Some(Self::from_bytes(&bytes)?))
    }

    pub fn symbol_count(&self) -> usize {
        self.symbol_count
    }

    pub fn path_count(&self) -> usize {
        self.path_count
    }

    pub fn approx_bytes(&self) -> usize {
        self.approx_bytes
    }

    /// Sorted candidate symbol ids. `None` means fall back to full scan
    /// (short needle, or over-broad postings vs universe/budget).
    pub fn candidate_symbol_ids(&self, needle: &str) -> Option<Vec<u32>> {
        self.candidate_symbol_ids_for_budget(needle, None)
    }

    /// Like [`Self::candidate_symbol_ids`] with a search budget hint for
    /// selectivity early-out (graphzero-2tee).
    pub fn candidate_symbol_ids_for_budget(
        &self,
        needle: &str,
        budget: Option<usize>,
    ) -> Option<Vec<u32>> {
        if self.symbol_count > u16::MAX as usize {
            return None;
        }
        candidate_ids(&self.symbol_postings, needle, self.symbol_count, budget)
    }

    /// Candidate path blob-hash hex strings. `None` => full scan.
    pub fn candidate_path_hashes(&self, needle: &str) -> Option<HashSet<String>> {
        self.candidate_path_hashes_for_budget(needle, None)
    }

    /// Like [`Self::candidate_path_hashes`] with budget-aware selectivity.
    pub fn candidate_path_hashes_for_budget(
        &self,
        needle: &str,
        budget: Option<usize>,
    ) -> Option<HashSet<String>> {
        if self.path_count > u16::MAX as usize {
            return None;
        }
        let ids = candidate_ids(&self.path_postings, needle, self.path_count, budget)?;
        Some(
            ids.into_iter()
                .filter_map(|id| self.path_hashes.get(id as usize).map(hex_hash))
                .collect(),
        )
    }

    /// Diagnostics: rarest posting length + whether search would fall back to scan.
    pub fn candidate_selectivity(
        &self,
        needle: &str,
        budget: Option<usize>,
    ) -> Option<(usize, bool)> {
        let keys: Vec<u16> = bigrams(needle).collect();
        if keys.is_empty() {
            return None;
        }
        let mut rarest = usize::MAX;
        for k in &keys {
            match self.symbol_postings.collect_list(*k) {
                Some(list) => rarest = rarest.min(list.len()),
                None => return Some((0, false)),
            }
        }
        if rarest == usize::MAX {
            return None;
        }
        let fallback = self
            .candidate_symbol_ids_for_budget(needle, budget)
            .is_none();
        Some((rarest, fallback))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_symbol_domain_falls_back_instead_of_failing() {
        let names = vec!["LargeRepoSymbol".to_owned(); usize::from(u16::MAX) + 1];
        let index = NameBigramIndex::build_from_names_and_paths(&names, &[]).unwrap();
        assert_eq!(index.symbol_count(), names.len());
        assert!(index.candidate_symbol_ids("LargeRepoSymbol").is_none());
    }
}

fn estimate_bytes(
    symbol_postings: &DensePostings,
    path_postings: &DensePostings,
    path_count: usize,
) -> usize {
    symbol_postings.approx_bytes() + path_postings.approx_bytes() + path_count * 32
}
