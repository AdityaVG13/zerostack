//! Append-only Merkle Mountain Range transparency proofs for recovery mutations.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type Hash = String;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MmrLog {
    leaves: Vec<Hash>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InclusionProof {
    pub size: usize,
    pub leaf_index: usize,
    pub leaf_hash: Hash,
    pub peak_index: usize,
    pub siblings: Vec<ProofSibling>,
    pub peaks: Vec<Hash>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofSibling {
    pub hash: Hash,
    pub is_left: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsistencyProof {
    pub old_size: usize,
    pub new_size: usize,
    pub old_peaks: Vec<Hash>,
    pub appended_leaf_hashes: Vec<Hash>,
}

impl MmrLog {
    pub fn len(&self) -> usize {
        self.leaves.len()
    }
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }
    pub fn root(&self) -> Hash {
        bag(&self.peaks())
    }
    pub fn append(&mut self, event: &[u8]) -> usize {
        self.append_hash(hash_leaf(event))
    }
    pub fn append_hash(&mut self, hash: Hash) -> usize {
        let index = self.leaves.len();
        self.leaves.push(hash);
        index
    }
    pub fn peaks(&self) -> Vec<Hash> {
        peak_ranges(self.len())
            .into_iter()
            .map(|(start, size)| subtree_root(&self.leaves[start..start + size]))
            .collect()
    }
    pub fn inclusion_proof(&self, leaf_index: usize) -> Option<InclusionProof> {
        if leaf_index >= self.len() {
            return None;
        }
        let ranges = peak_ranges(self.len());
        let (peak_index, (start, size)) = ranges
            .iter()
            .copied()
            .enumerate()
            .find(|(_, (start, size))| leaf_index >= *start && leaf_index < start + size)?;
        let mut siblings = Vec::new();
        inclusion_path(
            &self.leaves[start..start + size],
            leaf_index - start,
            &mut siblings,
        );
        Some(InclusionProof {
            size: self.len(),
            leaf_index,
            leaf_hash: self.leaves[leaf_index].clone(),
            peak_index,
            siblings,
            peaks: self.peaks(),
        })
    }
    pub fn consistency_proof(&self, old_size: usize) -> Option<ConsistencyProof> {
        if old_size > self.len() {
            return None;
        }
        let old = Self {
            leaves: self.leaves[..old_size].to_vec(),
        };
        Some(ConsistencyProof {
            old_size,
            new_size: self.len(),
            old_peaks: old.peaks(),
            appended_leaf_hashes: self.leaves[old_size..].to_vec(),
        })
    }
    pub fn merge_concurrent(&mut self, other: &Self) {
        let common = self
            .leaves
            .iter()
            .zip(&other.leaves)
            .take_while(|(a, b)| a == b)
            .count();
        self.leaves.extend(other.leaves[common..].iter().cloned());
    }
}

impl InclusionProof {
    pub fn verify(&self, expected_root: &str) -> bool {
        if self.size == 0 || self.leaf_index >= self.size || self.peak_index >= self.peaks.len() {
            return false;
        }
        let mut node = self.leaf_hash.clone();
        for sibling in &self.siblings {
            node = if sibling.is_left {
                hash_node(&sibling.hash, &node)
            } else {
                hash_node(&node, &sibling.hash)
            };
        }
        let mut peaks = self.peaks.clone();
        peaks[self.peak_index] = node;
        bag(&peaks) == expected_root
    }
}

impl ConsistencyProof {
    pub fn verify(&self, old_root: &str, new_root: &str) -> bool {
        if self.new_size < self.old_size
            || self.appended_leaf_hashes.len() != self.new_size - self.old_size
            || bag(&self.old_peaks) != old_root
        {
            return false;
        }
        let heights = peak_heights(self.old_size);
        if heights.len() != self.old_peaks.len() {
            return false;
        }
        let mut peaks: Vec<(u32, Hash)> = heights
            .into_iter()
            .zip(self.old_peaks.iter().cloned())
            .collect();
        for leaf in &self.appended_leaf_hashes {
            let mut current = (0, leaf.clone());
            while peaks.last().is_some_and(|(height, _)| *height == current.0) {
                let (_, left) = peaks.pop().expect("checked");
                current = (current.0 + 1, hash_node(&left, &current.1));
            }
            peaks.push(current);
        }
        bag(&peaks.into_iter().map(|(_, hash)| hash).collect::<Vec<_>>()) == new_root
    }
}

fn hash_tagged(tag: u8, parts: &[&[u8]]) -> Hash {
    let mut h = Sha256::new();
    h.update([tag]);
    for part in parts {
        h.update(part);
    }
    hex(h.finalize().as_slice())
}
fn hash_leaf(event: &[u8]) -> Hash {
    hash_tagged(0, &[event])
}
fn hash_node(left: &str, right: &str) -> Hash {
    hash_tagged(1, &[left.as_bytes(), right.as_bytes()])
}
fn bag(peaks: &[Hash]) -> Hash {
    if peaks.is_empty() {
        return hash_tagged(3, &[]);
    }
    peaks[1..].iter().fold(peaks[0].clone(), |acc, peak| {
        hash_tagged(2, &[acc.as_bytes(), peak.as_bytes()])
    })
}
fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(H[(b >> 4) as usize] as char);
        out.push(H[(b & 15) as usize] as char);
    }
    out
}
fn peak_heights(mut size: usize) -> Vec<u32> {
    let mut out = Vec::new();
    while size > 0 {
        let h: u32 = usize::BITS - 1 - size.leading_zeros();
        out.push(h);
        size -= 1usize << h;
    }
    out
}
fn peak_ranges(size: usize) -> Vec<(usize, usize)> {
    let mut start = 0;
    peak_heights(size)
        .into_iter()
        .map(|h| {
            let n = 1usize << h;
            let r = (start, n);
            start += n;
            r
        })
        .collect()
}
fn subtree_root(leaves: &[Hash]) -> Hash {
    if leaves.len() == 1 {
        return leaves[0].clone();
    }
    let mid = leaves.len() / 2;
    hash_node(&subtree_root(&leaves[..mid]), &subtree_root(&leaves[mid..]))
}
fn inclusion_path(leaves: &[Hash], index: usize, out: &mut Vec<ProofSibling>) -> Hash {
    if leaves.len() == 1 {
        return leaves[0].clone();
    }
    let mid = leaves.len() / 2;
    if index < mid {
        let node = inclusion_path(&leaves[..mid], index, out);
        out.push(ProofSibling {
            hash: subtree_root(&leaves[mid..]),
            is_left: false,
        });
        hash_node(&node, &out.last().expect("pushed").hash)
    } else {
        let node = inclusion_path(&leaves[mid..], index - mid, out);
        out.push(ProofSibling {
            hash: subtree_root(&leaves[..mid]),
            is_left: true,
        });
        hash_node(&out.last().expect("pushed").hash, &node)
    }
}
