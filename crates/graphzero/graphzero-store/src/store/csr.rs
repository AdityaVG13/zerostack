//! Compressed Sparse Row adjacency for tier-A edges (FR-006). Every edge
//! carries an evidence span (INV-002); evidence travels with the edge
//! through the CSR sort so `evidence[i]` belongs to edge index `i`.

use super::format::SpanEntry;
use super::hot_path::EdgesView;

pub mod edge_kind {
    pub const CALLS: u8 = 0;
    pub const IMPORTS: u8 = 1;
    pub const REFS: u8 = 2;

    /// Edge kinds included in the blast reverse view (CALLS/REFS/IMPORTS).
    #[inline]
    pub fn is_blast_kind(kind: u8) -> bool {
        kind == CALLS || kind == REFS || kind == IMPORTS
    }
    pub const CO_CHANGED: u8 = 3;
    pub const SESSION_FOLLOWED: u8 = 4;
    /// Tier-C publisher kinds (P5.3).
    pub const RUNTIME_CALLED: u8 = 5;
    pub const LINTER_SMELL: u8 = 6;
    /// Verification run proved a claim about the target.
    pub const VERIFICATION_PASSED: u8 = 7;
    /// Verification run refuted or could not prove a claim about the target.
    pub const VERIFICATION_FAILED: u8 = 8;

    /// Declared build dependency (GRAPH-001): manifest -> package sources the
    /// manifest governs building.
    pub const BUILD_DEPENDS: u8 = 9;
    /// Declared schema/codegen dependency (GRAPH-001): build-time include of a
    /// schema input or generated artifact (consumer -> included file).
    pub const SCHEMA_DEPENDS: u8 = 10;
    /// Conservative effect overapprox (GRAPH-001): an effectful producer may
    /// touch sibling artifacts at build/runtime (over-invalidates, never
    /// under-invalidates).
    pub const EFFECT_MAY_TOUCH: u8 = 11;
}

pub use edge_kind as EdgeKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub target: u32,
    pub kind: u8,
    pub confidence: u8,
}

/// Builder: collects (src, dst, kind, confidence, evidence) tuples and
/// emits CSR arrays plus the aligned evidence array.
#[derive(Default)]
pub struct CsrBuilder {
    edges: Vec<(u32, u32, u8, u8, SpanEntry)>,
}

pub struct BuiltCsr {
    pub offsets: Vec<u32>,
    pub targets: Vec<u32>,
    pub kinds: Vec<u8>,
    pub confidences: Vec<u8>,
    /// `evidence[i]` is the byte-span proof for edge index `i` (INV-002).
    pub evidence: Vec<SpanEntry>,
}

impl BuiltCsr {
    pub fn empty(num_symbols: usize) -> Self {
        Self {
            offsets: vec![0u32; num_symbols + 1],
            targets: Vec::new(),
            kinds: Vec::new(),
            confidences: Vec::new(),
            evidence: Vec::new(),
        }
    }
}

impl CsrBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_edge(&mut self, src: u32, dst: u32, kind: u8, confidence: u8) {
        self.add_edge_with_evidence(src, dst, kind, confidence, SpanEntry::default());
    }

    pub fn add_edge_with_evidence(
        &mut self,
        src: u32,
        dst: u32,
        kind: u8,
        confidence: u8,
        evidence: SpanEntry,
    ) {
        self.edges.push((src, dst, kind, confidence, evidence));
    }

    pub fn len(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Build CSR over `num_symbols` source IDs. Edges are sorted by src;
    /// insertion order is preserved within a src (stable sort).
    pub fn build(mut self, num_symbols: usize) -> BuiltCsr {
        self.edges.sort_by_key(|e| e.0);
        let mut offsets = vec![0u32; num_symbols + 1];
        for &(src, _, _, _, _) in &self.edges {
            offsets[src as usize + 1] += 1;
        }
        for i in 1..offsets.len() {
            offsets[i] += offsets[i - 1];
        }
        let mut targets = Vec::with_capacity(self.edges.len());
        let mut kinds = Vec::with_capacity(self.edges.len());
        let mut confidences = Vec::with_capacity(self.edges.len());
        let mut evidence = Vec::with_capacity(self.edges.len());
        for (_, dst, kind, conf, ev) in self.edges {
            targets.push(dst);
            kinds.push(kind);
            confidences.push(conf);
            evidence.push(ev);
        }
        BuiltCsr {
            offsets,
            targets,
            kinds,
            confidences,
            evidence,
        }
    }
}

/// Precomputed reverse adjacency: for each target node, stores (src, edge_index) pairs.
/// Converts O(V*E) reverse traversal into O(degree).
///
/// Flat CSR representation (graphzero perf): one offsets array + one entries
/// array. The previous `Vec<Vec<(u32, usize)>>` allocated one heap Vec per
/// symbol (~38k allocs on this repo) even though most symbols have no
/// incoming CALLS/REFS/IMPORTS edges; the counting-sort build below produces
/// the identical per-target `(src, global_edge_idx)` sequence in the same
/// source order with two linear passes and no per-node allocations.
pub struct ReverseIndex {
    /// `offsets[t] .. offsets[t+1]` slice `entries`, ascending by t.
    pub offsets: Vec<u32>,
    /// For target `t`, each entry is `(src, global_edge_idx)`, in the same
    /// (src-ascending scan) order the previous per-node Vec build produced.
    pub entries: Vec<(u32, u32)>,
}

impl ReverseIndex {
    fn from_pairs(n: usize, pairs: Vec<(u32, (u32, u32))>) -> Self {
        // Two-pass counting placement (O(V+E), no comparison sort): count
        // per-target, prefix-sum into offsets, then place each pair at its
        // slot in forward-emission order. Placement is stable because pairs
        // arrive grouped by nothing but are emitted in src-scan order; equal
        // targets keep their relative scan order exactly as the previous
        // per-node Vec build produced.
        let mut offsets = vec![0u32; n + 1];
        for &(t, _) in &pairs {
            let slot = t as usize + 1;
            if slot <= n {
                offsets[slot] += 1;
            }
        }
        for i in 1..=n {
            offsets[i] += offsets[i - 1];
        }
        let mut entries: Vec<(u32, u32)> = vec![(0, 0); pairs.len()];
        let mut cursor = offsets[..n].to_vec();
        for &(t, e) in &pairs {
            let slot = cursor[t as usize] as usize;
            cursor[t as usize] += 1;
            if slot < entries.len() {
                entries[slot] = e;
            }
        }
        Self { offsets, entries }
    }

    #[inline]
    fn slice_for(&self, target: u32) -> &[(u32, u32)] {
        let t = target as usize;
        if t + 1 >= self.offsets.len() {
            return &[];
        }
        let lo = self.offsets[t] as usize;
        let hi = self.offsets[t + 1] as usize;
        self.entries.get(lo..hi).unwrap_or(&[])
    }
}

/// Optional multi-view reverse build (tests / offline). Production Snapshot
/// materializes each view lazily so blast-only sessions pay for one table, not
/// three (see graphzero-8n6k6).
pub struct ReverseIndexBundle {
    pub all: ReverseIndex,
    pub blast: ReverseIndex,
    pub calls: ReverseIndex,
}

impl ReverseIndexBundle {
    /// One O(E) pass filling all three views. Prefer Snapshot's lazy per-view
    /// APIs on the warm query path.
    pub fn build(csr: &CsrAdjacency<'_>) -> Self {
        let n = csr.num_symbols();
        let mut all_pairs: Vec<(u32, (u32, u32))> = Vec::new();
        let mut blast_pairs: Vec<(u32, (u32, u32))> = Vec::new();
        let mut calls_pairs: Vec<(u32, (u32, u32))> = Vec::new();
        for src in 0..n as u32 {
            let base = csr.edge_base(src);
            for (offset, edge) in csr.edges(src).enumerate() {
                if edge.target as usize >= n {
                    continue;
                }
                let entry = (src, (base + offset) as u32);
                all_pairs.push((edge.target, entry));
                if edge.kind == edge_kind::CALLS {
                    calls_pairs.push((edge.target, entry));
                }
                if edge_kind::is_blast_kind(edge.kind) {
                    blast_pairs.push((edge.target, entry));
                }
            }
        }
        Self {
            all: ReverseIndex::from_pairs(n, all_pairs),
            blast: ReverseIndex::from_pairs(n, blast_pairs),
            calls: ReverseIndex::from_pairs(n, calls_pairs),
        }
    }
}

impl ReverseIndex {
    pub fn build(csr: &CsrAdjacency<'_>, filter_kind: Option<u8>) -> Self {
        match filter_kind {
            Some(kind) => Self::build_filtered(csr, |k| k == kind),
            None => Self::build_filtered(csr, |_| true),
        }
    }

    /// Build a reverse adjacency retaining only edges whose kind matches `keep`.
    pub fn build_filtered(csr: &CsrAdjacency<'_>, keep: impl Fn(u8) -> bool) -> Self {
        let n = csr.num_symbols();
        let mut pairs: Vec<(u32, (u32, u32))> = Vec::new();
        for src in 0..n as u32 {
            let base = csr.edge_base(src);
            for (i, edge) in csr.edges(src).enumerate() {
                if !keep(edge.kind) {
                    continue;
                }
                if (edge.target as usize) < n {
                    pairs.push((edge.target, (src, (base + i) as u32)));
                }
            }
        }
        Self::from_pairs(n, pairs)
    }

    /// Incoming `(src, global_edge_idx)` pairs for `target`, in forward-scan
    /// order — identical sequence to the previous per-node-Vec build.
    pub fn callers(&self, target: u32) -> &[(u32, u32)] {
        self.slice_for(target)
    }
}

/// Zero-copy CSR reader over an mmap'd edge section.
pub struct CsrAdjacency<'a> {
    view: EdgesView<'a>,
}

impl<'a> CsrAdjacency<'a> {
    pub fn new(view: EdgesView<'a>) -> Self {
        Self { view }
    }

    pub fn num_symbols(&self) -> usize {
        self.view.offsets.len().saturating_sub(1)
    }

    pub fn num_edges(&self) -> usize {
        self.view.targets.len()
    }

    /// Iterate outgoing edges of `src` in insertion order. Out-of-bounds
    /// sources yield an empty iterator (degraded mode per interface
    /// contract).
    pub fn edges(&self, src: u32) -> impl Iterator<Item = Edge> + 'a {
        let (lo, hi) = self.range(src);
        let targets = self.view.targets;
        let kinds = self.view.kinds;
        let confidences = self.view.confidences;
        (lo..hi).map(move |i| Edge {
            target: targets[i],
            kind: kinds[i],
            confidence: confidences[i],
        })
    }

    /// Global edge index of the first edge of `src` (evidence alignment).
    pub fn edge_base(&self, src: u32) -> usize {
        self.range(src).0
    }

    fn range(&self, src: u32) -> (usize, usize) {
        let s = src as usize;
        if s + 1 >= self.view.offsets.len() {
            return (0, 0);
        }
        let lo = self.view.offsets[s] as usize;
        let hi = self.view.offsets[s + 1] as usize;
        if lo > hi || hi > self.view.targets.len() {
            return (0, 0);
        }
        (lo, hi)
    }
}
