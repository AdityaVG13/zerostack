//! MMR (Merkle Mountain Range) transparency log for blob store durability
//! (P0 graphzero-ea8i).
//!
//! Every blob written to the store appends a leaf to this append-only log.
//! The MMR root provides a stack-wide conservation law: every gz://blob ref
//! must have a valid inclusion proof against the current root. This makes
//! silent blob deletion or corruption detectable.
//!
//! File format (`.graphzero/transparency.mmr`):
//! ```text
//! magic "GZMM" (4) | version u8 | leaf_count u32 | root [u8;32] |
//! crc u32 | leaves (leaf_count * 32 bytes)
//! ```
//!
//! Internal node hashing uses SHA-256 with domain separation:
//! - leaf node: the blob's content SHA-256 directly (already a hash)
//! - internal node: `SHA-256(0x01 || left || right)`

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

const MMR_MAGIC: [u8; 4] = *b"GZMM";
const MMR_VERSION: u8 = 1;

/// One node in an MMR authentication path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofNode {
    /// True if the sibling is to the right (current node is left child).
    pub left: bool,
    pub hash: [u8; 32],
}

/// Inclusion proof for a single leaf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InclusionProof {
    pub leaf_index: u32,
    pub leaf_hash: [u8; 32],
    /// Authentication path from leaf to its peak (bottom-up).
    pub path: Vec<ProofNode>,
    /// Index of the peak this leaf belongs to.
    pub peak_index: u32,
    /// All peaks except the one at `peak_index`, in left-to-right order.
    /// The verifier reconstructs the proved peak and inserts it at
    /// `peak_index` to bag the full root.
    pub other_peaks: Vec<[u8; 32]>,
}

/// MMR transparency log.
#[derive(Clone, Debug)]
pub struct TransparencyLog {
    leaves: Vec<[u8; 32]>,
    store_root: PathBuf,
}

fn mmr_path(store_root: &Path) -> PathBuf {
    store_root.join("transparency.mmr")
}

fn hash_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([0x01]);
    hasher.update(left);
    hasher.update(right);
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

fn bag_peaks(peaks: &[[u8; 32]]) -> [u8; 32] {
    if peaks.is_empty() {
        return [0u8; 32];
    }
    if peaks.len() == 1 {
        return peaks[0];
    }
    // Bag right-to-left: root = hash(peak[0] || bag(peak[1..]))
    let mut acc = peaks[peaks.len() - 1];
    for i in (0..peaks.len() - 1).rev() {
        acc = hash_pair(&peaks[i], &acc);
    }
    acc
}

/// Mountain heights for `n` leaves, descending order.
/// E.g. n=7 → [2, 1, 0], n=5 → [2, 0], n=4 → [2].
fn mountain_heights(n: u32) -> Vec<u32> {
    let mut heights = Vec::new();
    let mut remaining = n;
    while remaining > 0 {
        let h = (31 - remaining.leading_zeros()) as u32;
        heights.push(h);
        remaining -= 1u32 << h;
    }
    heights
}

/// Leaf ranges for each mountain: (start_index, height).
fn mountain_ranges(n: u32) -> Vec<(u32, u32)> {
    let heights = mountain_heights(n);
    let mut start = 0u32;
    let mut ranges = Vec::with_capacity(heights.len());
    for &h in &heights {
        ranges.push((start, h));
        start += 1u32 << h;
    }
    ranges
}

impl TransparencyLog {
    /// Open or create the transparency log under `store_root`.
    pub fn open(store_root: &Path) -> Result<Self> {
        let path = mmr_path(store_root);
        let leaves = match fs::read(&path) {
            Ok(data) => Self::decode_leaves(&data)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e).context("read transparency log"),
        };
        Ok(Self {
            leaves,
            store_root: store_root.to_path_buf(),
        })
    }

    fn decode_leaves(data: &[u8]) -> Result<Vec<[u8; 32]>> {
        if data.len() < 45 {
            bail!("transparency log too small");
        }
        if data[0..4] != MMR_MAGIC {
            bail!("bad transparency log magic");
        }
        if data[4] != MMR_VERSION {
            bail!("unsupported transparency log version {}", data[4]);
        }
        let leaf_count = u32::from_le_bytes(data[5..9].try_into().unwrap()) as usize;
        let body_end = 45 + leaf_count * 32;
        if data.len() < body_end {
            bail!(
                "transparency log truncated: expected {body_end} bytes, got {}",
                data.len()
            );
        }
        // CRC over header (magic+version+leaf_count+root = 41 bytes) + leaves.
        let stored_crc = u32::from_le_bytes(data[41..45].try_into().unwrap());
        let mut crc_input = Vec::with_capacity(41 + leaf_count * 32);
        crc_input.extend_from_slice(&data[..41]);
        crc_input.extend_from_slice(&data[45..body_end]);
        let computed_crc = crc32fast::hash(&crc_input);
        if computed_crc != stored_crc {
            bail!("transparency log CRC mismatch");
        }
        let mut leaves = Vec::with_capacity(leaf_count);
        for i in 0..leaf_count {
            let offset = 45 + i * 32;
            let mut leaf = [0u8; 32];
            leaf.copy_from_slice(&data[offset..offset + 32]);
            leaves.push(leaf);
        }
        Ok(leaves)
    }

    /// Number of leaves in the log.
    pub fn leaf_count(&self) -> usize {
        self.leaves.len()
    }

    /// Check whether a blob hash is already recorded.
    pub fn contains(&self, hash: &[u8; 32]) -> bool {
        self.leaves.iter().any(|leaf| leaf == hash)
    }

    /// Append a blob content hash as a new leaf. Returns the leaf index.
    /// Duplicate appends are skipped (idempotent).
    pub fn append(&mut self, hash: [u8; 32]) -> usize {
        if let Some(idx) = self.leaves.iter().position(|leaf| leaf == &hash) {
            return idx;
        }
        let idx = self.leaves.len();
        self.leaves.push(hash);
        idx
    }

    /// Persist the log to disk atomically.
    pub fn flush(&self) -> Result<()> {
        let path = mmr_path(&self.store_root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let root = self.root();
        // CRC over header (magic+version+leaf_count+root = 41 bytes) + leaves.
        let mut crc_input = Vec::with_capacity(41 + self.leaves.len() * 32);
        crc_input.extend_from_slice(&MMR_MAGIC);
        crc_input.push(MMR_VERSION);
        crc_input.extend_from_slice(&(self.leaves.len() as u32).to_le_bytes());
        crc_input.extend_from_slice(&root);
        for leaf in &self.leaves {
            crc_input.extend_from_slice(leaf);
        }
        let crc = crc32fast::hash(&crc_input);
        let mut buf = Vec::with_capacity(45 + self.leaves.len() * 32);
        buf.extend_from_slice(&MMR_MAGIC);
        buf.push(MMR_VERSION);
        buf.extend_from_slice(&(self.leaves.len() as u32).to_le_bytes());
        buf.extend_from_slice(&root);
        buf.extend_from_slice(&crc.to_le_bytes());
        for leaf in &self.leaves {
            buf.extend_from_slice(leaf);
        }
        let tmp = path.with_extension("mmr.tmp");
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&buf)?;
            f.sync_data()?;
        }
        fs::rename(&tmp, &path)?;
        if let Ok(dir) = fs::File::open(&self.store_root) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    /// Compute the hash of a perfect binary tree of height `h` whose leaves
    /// start at `start` in the leaves array.
    fn compute_range_hash(&self, start: u32, h: u32) -> [u8; 32] {
        if h == 0 {
            return self.leaves[start as usize];
        }
        let half = 1u32 << (h - 1);
        let left = self.compute_range_hash(start, h - 1);
        let right = self.compute_range_hash(start + half, h - 1);
        hash_pair(&left, &right)
    }

    /// Compute all peak hashes (left-to-right).
    fn peaks(&self) -> Vec<[u8; 32]> {
        let n = self.leaves.len() as u32;
        mountain_ranges(n)
            .iter()
            .map(|&(start, h)| self.compute_range_hash(start, h))
            .collect()
    }

    /// Compute the MMR root (bagging all peaks).
    pub fn root(&self) -> [u8; 32] {
        bag_peaks(&self.peaks())
    }

    /// Generate an inclusion proof for the leaf at `leaf_index`.
    pub fn inclusion_proof(&self, leaf_index: u32) -> Result<InclusionProof> {
        let n = self.leaves.len() as u32;
        if leaf_index >= n {
            bail!("leaf index {leaf_index} out of range ({n} leaves)");
        }
        let leaf_hash = self.leaves[leaf_index as usize];

        // Find which mountain this leaf belongs to.
        let ranges = mountain_ranges(n);
        let mut mountain_idx = 0u32;
        let mut mountain_start = 0u32;
        let mut mountain_height = 0u32;
        for (i, &(start, h)) in ranges.iter().enumerate() {
            let leaf_count = 1u32 << h;
            if leaf_index < start + leaf_count {
                mountain_idx = i as u32;
                mountain_start = start;
                mountain_height = h;
                break;
            }
        }

        // Build authentication path from leaf to peak.
        // At each level, the node covers 2^level leaves starting at a base.
        // The leaf is at position (leaf_index - mountain_start) within the mountain.
        let pos_in_mountain = leaf_index - mountain_start;
        let mut path = Vec::new();
        let mut current_pos = pos_in_mountain; // position at current level
        for level in 0..mountain_height {
            let leaves_at_level = 1u32 << level;
            let is_left_child = current_pos % 2 == 0;
            // Leaf start of the current node at this level.
            let node_leaf_start = mountain_start + current_pos * leaves_at_level;
            if is_left_child {
                // Sibling is to the right.
                let sibling_start = node_leaf_start + leaves_at_level;
                let sibling_hash = self.compute_range_hash(sibling_start, level);
                path.push(ProofNode {
                    left: true,
                    hash: sibling_hash,
                });
            } else {
                // Sibling is to the left.
                let sibling_start = node_leaf_start - leaves_at_level;
                let sibling_hash = self.compute_range_hash(sibling_start, level);
                path.push(ProofNode {
                    left: false,
                    hash: sibling_hash,
                });
            }
            // Move up: at the next level, position is floor(current_pos / 2).
            current_pos /= 2;
        }

        // All peaks to reconstruct the root via bagging.
        let all_peaks = self.peaks();
        let other_peaks: Vec<[u8; 32]> = all_peaks
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != mountain_idx as usize)
            .map(|(_, &h)| h)
            .collect();

        Ok(InclusionProof {
            leaf_index,
            leaf_hash,
            path,
            peak_index: mountain_idx,
            other_peaks,
        })
    }

    /// Verify an inclusion proof against an expected root.
    pub fn verify(leaf_hash: [u8; 32], proof: &InclusionProof, expected_root: [u8; 32]) -> bool {
        if proof.leaf_hash != leaf_hash {
            return false;
        }
        // Reconstruct peak from leaf + authentication path.
        let mut current = leaf_hash;
        for node in &proof.path {
            current = if node.left {
                hash_pair(&current, &node.hash)
            } else {
                hash_pair(&node.hash, &current)
            };
        }
        // Rebuild the full peak list by inserting the reconstructed peak
        // at its position, then bag all peaks.
        let mut all_peaks = proof.other_peaks.clone();
        let insert_pos = (proof.peak_index as usize).min(all_peaks.len());
        all_peaks.insert(insert_pos, current);
        bag_peaks(&all_peaks) == expected_root
    }

    /// Get all leaf hashes (for durability matrix checks).
    pub fn leaves(&self) -> &[[u8; 32]] {
        &self.leaves
    }
}
