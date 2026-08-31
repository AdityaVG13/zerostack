//! Coverage bitmap: 3 bits per blob, one bit per tier A/B/C.
//! Bit layout per blob i (LSB-first within the packed byte stream) bit
//! (i*3 + 0) = tier A indexed, (i*3 + 1) = tier B, (i*3 + 2) = tier C.

use anyhow::{Result, bail};

use crate::Tier;

pub const BITS_PER_BLOB: usize = 3;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoverageBitmap {
    blob_count: usize,
    bits: Vec<u8>,
}

const TIER_A_MASK_8_BLOBS: u32 = 0x24_92_49;
const TIER_B_MASK_8_BLOBS: u32 = 0x49_24_92;
const TIER_C_MASK_8_BLOBS: u32 = 0x92_49_24;

const _: () = {
    assert!(Tier::A as usize == 0);
    assert!(Tier::B as usize == 1);
    assert!(Tier::C as usize == 2);
};

fn tier_bit(tier: Tier) -> usize {
    tier as usize
}

impl CoverageBitmap {
    pub fn new(blob_count: usize) -> Self {
        Self {
            blob_count,
            bits: vec![0u8; (blob_count * BITS_PER_BLOB).div_ceil(8)],
        }
    }

    /// Reconstruct from packed bytes (e.g. a coverage section read).
    pub fn from_packed(blob_count: usize, bits: &[u8]) -> Self {
        let want = (blob_count * BITS_PER_BLOB).div_ceil(8);
        let mut v = bits.to_vec();
        v.resize(want, 0);
        Self {
            blob_count,
            bits: v,
        }
    }

    pub fn blob_count(&self) -> usize {
        self.blob_count
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
    }

    pub fn set(&mut self, blob_idx: usize, tier: Tier, value: bool) {
        assert!(blob_idx < self.blob_count, "blob index out of range");
        let bit = blob_idx * BITS_PER_BLOB + tier_bit(tier);
        let (byte, off) = (bit / 8, bit % 8);
        if value {
            self.bits[byte] |= 1 << off;
        } else {
            self.bits[byte] &= !(1 << off);
        }
    }

    pub fn get(&self, blob_idx: usize, tier: Tier) -> bool {
        if blob_idx >= self.blob_count {
            return false;
        }
        let bit = blob_idx * BITS_PER_BLOB + tier_bit(tier);
        (self.bits[bit / 8] >> (bit % 8)) & 1 == 1
    }

    pub fn tier_count(&self, tier: Tier) -> usize {
        Self::tier_count_packed(&self.bits, self.blob_count, tier)
    }

    pub fn tier_count_packed(bits: &[u8], blob_count: usize, tier: Tier) -> usize {
        let mask = match tier {
            Tier::A => TIER_A_MASK_8_BLOBS,
            Tier::B => TIER_B_MASK_8_BLOBS,
            Tier::C => TIER_C_MASK_8_BLOBS,
        };
        let mut count = 0usize;
        let full_groups = blob_count / 8;
        for chunk in bits.chunks_exact(3).take(full_groups) {
            let w = u32::from(chunk[0]) | (u32::from(chunk[1]) << 8) | (u32::from(chunk[2]) << 16);
            count += (w & mask).count_ones() as usize;
        }
        let tb = tier_bit(tier);
        for i in (full_groups * 8)..blob_count {
            let bit = i * BITS_PER_BLOB + tb;
            count += ((bits.get(bit / 8).copied().unwrap_or(0) >> (bit % 8)) & 1) as usize;
        }
        count
    }

    pub fn tier_counts_packed(bits: &[u8], blob_count: usize) -> Result<[usize; 3]> {
        let bit_count = blob_count
            .checked_mul(BITS_PER_BLOB)
            .ok_or_else(|| anyhow::anyhow!("coverage blob count overflows packed bit length"))?;
        let required_len = bit_count.div_ceil(8);
        if bits.len() < required_len {
            bail!(
                "coverage bits truncated: need {required_len} bytes for {blob_count} blobs, got {}",
                bits.len()
            );
        }
        let mut counts = [0usize; 3];
        let full_groups = blob_count / 8;
        for chunk in bits.chunks_exact(3).take(full_groups) {
            let w = u32::from(chunk[0]) | (u32::from(chunk[1]) << 8) | (u32::from(chunk[2]) << 16);
            counts[0] += (w & TIER_A_MASK_8_BLOBS).count_ones() as usize;
            counts[1] += (w & TIER_B_MASK_8_BLOBS).count_ones() as usize;
            counts[2] += (w & TIER_C_MASK_8_BLOBS).count_ones() as usize;
        }
        // Remainder
        for i in (full_groups * 8)..blob_count {
            let base_bit = i * BITS_PER_BLOB;
            let byte_idx = base_bit / 8;
            let shift = base_bit % 8;
            let byte = bits.get(byte_idx).copied().unwrap_or(0);
            counts[0] += ((byte >> shift) & 1) as usize;
            let b_bit = base_bit + 1;
            let b_byte = bits.get(b_bit / 8).copied().unwrap_or(0);
            counts[1] += ((b_byte >> (b_bit % 8)) & 1) as usize;
            let c_bit = base_bit + 2;
            let c_byte = bits.get(c_bit / 8).copied().unwrap_or(0);
            counts[2] += ((c_byte >> (c_bit % 8)) & 1) as usize;
        }
        Ok(counts)
    }

    pub fn tier_a_count(&self) -> usize {
        self.tier_count(Tier::A)
    }

    pub fn tier_b_count(&self) -> usize {
        self.tier_count(Tier::B)
    }

    pub fn tier_c_count(&self) -> usize {
        self.tier_count(Tier::C)
    }

    /// Coverage ratio for a tier in [0.0, 1.0].
    pub fn ratio(&self, tier: Tier) -> f64 {
        if self.blob_count == 0 {
            return 0.0;
        }
        self.tier_count(tier) as f64 / self.blob_count as f64
    }
}
