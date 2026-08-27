//! Minimal perfect hash via CHD (compress-hash-displace).
//!
//! ADR-002 supersession note: the PRD bound symbol lookup to the `mph`
//! crate; that crates.io name is an unrelated emacs tool, which fires the
//! ADR-002 reversal trigger. This in-crate CHD construction is deterministic,
//! dependency-free, and serializes to two flat u32 arrays (`seeds`,
//! `values`) that mmap zero-copy — a strictly better fit for GZSH v1.
//!
//! Lookup: `bucket = h(key, 0) % m`, `slot = h(key, seeds[bucket]) % n`,
//! `id = values[slot]`. Caller verifies the key against the stored name to
//! reject unknown keys (CHD maps unknown keys to arbitrary slots).

/// 64-bit FNV-1a with seed mixing; deterministic across platforms.
#[inline]
pub fn hash_seed(key: &[u8], seed: u32) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325 ^ ((seed as u64).wrapping_mul(0x9e3779b97f4a7c15));
    for &b in key {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    // Final avalanche (splitmix64 tail) for better bucket spread.
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^ (h >> 31)
}

/// Built CHD function: `seeds` (one per bucket) and `values` (slot -> id).
pub struct ChdMph {
    pub seeds: Vec<u32>,
    pub values: Vec<u32>,
}

impl ChdMph {
    /// Build a minimal perfect hash over `keys`. `values[slot]` is set to the
    /// key's index in `keys`. Keys must be unique.
    pub fn build(keys: &[&[u8]]) -> Self {
        let n = keys.len();
        if n == 0 {
            return Self {
                seeds: vec![0],
                values: Vec::new(),
            };
        }
        let num_buckets = n.div_ceil(2).max(1);
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); num_buckets];
        for (i, key) in keys.iter().enumerate() {
            let b = (hash_seed(key, 0) % num_buckets as u64) as usize;
            buckets[b].push(i as u32);
        }
        let mut order: Vec<usize> = (0..num_buckets).collect();
        order.sort_by_key(|&b| std::cmp::Reverse(buckets[b].len()));

        let mut seeds = vec![0u32; num_buckets];
        let mut slot_taken = vec![false; n];
        let mut values = vec![0u32; n];

        for &b in &order {
            let bucket = &buckets[b];
            if bucket.is_empty() {
                continue;
            }
            let mut seed: u32 = 1;
            'search: loop {
                let mut claimed: Vec<usize> = Vec::with_capacity(bucket.len());
                for &ki in bucket {
                    let slot = (hash_seed(keys[ki as usize], seed) % n as u64) as usize;
                    if slot_taken[slot] || claimed.contains(&slot) {
                        seed = seed.checked_add(1).expect("CHD seed space exhausted");
                        continue 'search;
                    }
                    claimed.push(slot);
                }
                for (idx, &ki) in bucket.iter().enumerate() {
                    let slot = claimed[idx];
                    slot_taken[slot] = true;
                    values[slot] = ki;
                }
                seeds[b] = seed;
                break;
            }
        }
        Self { seeds, values }
    }
}

/// Zero-copy lookup over serialized CHD arrays.
#[inline]
pub fn lookup(seeds: &[u32], values: &[u32], key: &[u8]) -> Option<u32> {
    let n = values.len();
    if n == 0 {
        return None;
    }
    let b = (hash_seed(key, 0) % seeds.len() as u64) as usize;
    let seed = seeds[b];
    if seed == 0 {
        return None; // empty bucket
    }
    let slot = (hash_seed(key, seed) % n as u64) as usize;
    Some(values[slot])
}
