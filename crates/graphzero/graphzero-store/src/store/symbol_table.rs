//! Global symbol table: name-sorted dense u32 IDs, CHD perfect-hash lookup
//! (FR-004), and ordered prefix search (FR-005).

use std::collections::BTreeMap;

use anyhow::{Result, bail};

use super::format::SymbolEntry;
use super::hot_path::{MphView, ShardView, SymbolsView};
use super::mph::{ChdMph, lookup};

/// Builder: collects symbols, assigns dense IDs by lexicographic name order.
#[derive(Default)]
pub struct SymbolTableBuilder {
    // name -> (kind, tier)
    symbols: BTreeMap<String, (u8, u8)>,
}

pub struct BuiltSymbolTable {
    /// Names in ID order (sorted); ID = index.
    pub names: Vec<String>,
    pub entries: Vec<SymbolEntry>,
    pub name_bytes: Vec<u8>,
    pub mph: ChdMph,
}

impl SymbolTableBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: &str, kind: u8, tier: u8) {
        self.symbols.entry(name.to_string()).or_insert((kind, tier));
    }

    pub fn id_of(&self, name: &str) -> Option<u32> {
        self.symbols
            .keys()
            .position(|k| k == name)
            .map(|p| p as u32)
    }

    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    pub fn build(self) -> Result<BuiltSymbolTable> {
        let mut names = Vec::with_capacity(self.symbols.len());
        let mut entries = Vec::with_capacity(self.symbols.len());
        let mut name_bytes = Vec::new();
        for (id, (name, (kind, tier))) in self.symbols.into_iter().enumerate() {
            if name.len() > u16::MAX as usize {
                bail!(
                    "symbol name length {} exceeds u16::MAX (SymbolEntry.name_len)",
                    name.len()
                );
            }
            entries.push(SymbolEntry {
                symbol_id: id as u32,
                name_offset: name_bytes.len() as u32,
                name_len: name.len() as u16,
                kind,
                tier,
                flags: 0,
            });
            name_bytes.extend_from_slice(name.as_bytes());
            names.push(name);
        }
        let key_refs: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
        let mph = ChdMph::build(&key_refs);
        Ok(BuiltSymbolTable {
            names,
            entries,
            name_bytes,
            mph,
        })
    }
}

/// Zero-copy reader over the symbol + mph sections of a GZSH file.
pub struct SymbolTable<'a> {
    symbols: SymbolsView<'a>,
    mph: MphView<'a>,
}

impl<'a> SymbolTable<'a> {
    pub fn from_view(view: &ShardView<'a>) -> anyhow::Result<Self> {
        Ok(Self {
            symbols: view.symbols()?,
            mph: view.mph()?,
        })
    }

    pub fn len(&self) -> usize {
        self.symbols.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.symbols.entries.is_empty()
    }

    pub fn name(&self, id: u32) -> Option<&'a str> {
        let entry = self.symbols.entries.get(id as usize)?;
        Some(ShardView::symbol_name(&self.symbols, entry))
    }

    pub fn entry(&self, id: u32) -> Option<&'a SymbolEntry> {
        self.symbols.entries.get(id as usize)
    }

    /// True when symbol IDs are dense `0..n` and names are strictly ascending
    /// — the invariant behind rank arithmetic in the locate fast path
    /// (`graphzero` locate shell minting). Single allocation-free scan;
    /// callers should cache the verdict per snapshot.
    pub fn entries_dense_and_sorted(&self) -> bool {
        let mut prev: Option<&str> = None;
        for (index, entry) in self.symbols.entries.iter().enumerate() {
            if entry.symbol_id != index as u32 || entry.name_len == 0 {
                return false;
            }
            let cur = ShardView::symbol_name(&self.symbols, entry);
            if cur.is_empty() {
                return false;
            }
            if let Some(prev) = prev
                && prev >= cur
            {
                return false;
            }
            prev = Some(cur);
        }
        true
    }

    /// O(1) exact lookup via CHD perfect hash, verified against the stored
    /// name so unknown keys return `None` (FR-004).
    pub fn get(&self, symbol: &str) -> Option<u32> {
        let id = lookup(self.mph.seeds, self.mph.values, symbol.as_bytes())?;
        if self.name(id)? == symbol {
            Some(id)
        } else {
            None
        }
    }

    /// Ordered prefix search over name-sorted IDs (FR-005). Returns the dense
    /// ID range whose names start with `prefix`, in lexicographic order.
    pub fn prefix_search(&self, prefix: &str) -> impl Iterator<Item = u32> + '_ {
        let n = self.symbols.entries.len();
        let lo = self.partition(|name| name < prefix);
        let mut hi = lo;
        while hi < n {
            match self.name(hi as u32) {
                Some(name) if name.starts_with(prefix) => hi += 1,
                _ => break,
            }
        }
        lo as u32..hi as u32
    }

    /// Binary search: first index where `pred(name)` is false.
    fn partition(&self, pred: impl Fn(&str) -> bool) -> usize {
        let (mut lo, mut hi) = (0usize, self.symbols.entries.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if pred(self.name(mid as u32).unwrap_or("")) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }
}
