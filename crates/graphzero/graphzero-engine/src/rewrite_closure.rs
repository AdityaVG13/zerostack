//! Output-side closure: enumerate the mechanically implied edit sites for a
//! root semantic decision (bead zerostack-racc-caching-output-vz89.8).
//!
//! Given a root symbol and a propagation policy, walk the reverse graph over
//! statically resolvable relations only (calls / refs / imports) and return one
//! snap-to-file HIT per implied edit site, using the same target grammar as
//! def/ref/blast hits (bead 5htnw). No speculative propagation: an edge is
//! either in the index with evidence, or it is not reported.

use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::accounting::{PreventedReadAccounting, accounting_for_evidence_refs};
use crate::blast::BlastError;

use graphzero_store::Snapshot;
use graphzero_store::store::csr::{CsrAdjacency, Edge, ReverseIndex, edge_kind};
use graphzero_store::store::format::SpanEntry;
use graphzero_store::store::refs::blob_span_ref;
use graphzero_store::store::symbol_table::SymbolTable;
use serde::{Deserialize, Serialize};

pub const REWRITE_CLOSURE_SCHEMA_VERSION: u32 = 1;

/// Statically resolvable relations a root decision may propagate along.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    /// Call sites of the decision target (add/remove parameter, rename).
    Calls,
    /// Non-call references (mocks, DI bindings, generated clients, re-exports).
    Refs,
    /// Import/use sites naming the decision target.
    Imports,
}

impl Relation {
    pub fn as_str(self) -> &'static str {
        match self {
            Relation::Calls => "calls",
            Relation::Refs => "refs",
            Relation::Imports => "imports",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "calls" => Some(Relation::Calls),
            "refs" => Some(Relation::Refs),
            "imports" => Some(Relation::Imports),
            _ => None,
        }
    }

    fn from_edge_kind(kind: u8) -> Option<Self> {
        if kind == edge_kind::CALLS {
            Some(Relation::Calls)
        } else if kind == edge_kind::REFS {
            Some(Relation::Refs)
        } else if kind == edge_kind::IMPORTS {
            Some(Relation::Imports)
        } else {
            None
        }
    }
}

/// The propagation policy the model emits alongside the root decision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropagationPolicy {
    /// Relations to follow; empty is an error, not "all".
    pub relations: Vec<Relation>,
    /// Hops of propagation. 1 = direct sites only (rename, signature change).
    pub max_depth: u32,
}

impl Default for PropagationPolicy {
    fn default() -> Self {
        Self {
            relations: vec![Relation::Calls, Relation::Refs, Relation::Imports],
            max_depth: 1,
        }
    }
}

impl PropagationPolicy {
    /// Policy from wire-level relation names.
    pub fn from_names(names: &[String], max_depth: u32) -> Result<Self, BlastError> {
        let mut relations = Vec::new();
        for name in names {
            let relation = Relation::parse(name.as_str())
                .ok_or_else(|| BlastError::Parse(format!("unknown relation: {name}")))?;
            if !relations.contains(&relation) {
                relations.push(relation);
            }
        }
        Ok(Self {
            relations,
            max_depth,
        })
    }

    fn allows(&self, relation: Relation) -> bool {
        self.relations.contains(&relation)
    }
}

/// One mechanically implied edit site, in snap-to-file HIT form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditSite {
    /// Canonical target ref `<path>#L<start>-L<end>`.
    pub target: String,
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    /// HIT kind: the relation that implies this edit.
    pub kind: String,
    /// Enclosing symbol at the target span.
    pub sym: String,
    /// Graph symbol that owns the edit site.
    pub symbol: String,
    /// Hops from the root decision (1 = direct site).
    pub hop: u32,
    pub evidence_ref: String,
    /// Inlined window for the leading sites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

impl EditSite {
    /// Canonical hit record, identical in shape to def/ref/blast hits.
    pub fn render(&self) -> String {
        let header = format!("HIT {} kind={} sym={}", self.target, self.kind, self.sym);
        match &self.content {
            Some(content) if !content.is_empty() => format!("{header}\n{content}"),
            _ => header,
        }
    }
}

/// Closure_rewrite(D): every implied edit site for one root decision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RewriteClosure {
    pub schema_version: u32,
    pub root_symbol: String,
    pub policy: PropagationPolicy,
    pub sites: Vec<EditSite>,
    /// Implied edges whose evidence does not resolve to a file target; they are
    /// counted, never guessed at.
    pub unresolved_sites: usize,
    pub accounting: PreventedReadAccounting,
}

fn edge_at(csr: &CsrAdjacency<'_>, src: u32, edge_idx: usize) -> Option<Edge> {
    let offset = edge_idx.checked_sub(csr.edge_base(src))?;
    csr.edges(src).nth(offset)
}

fn evidence_ref_for(
    evidence: &[SpanEntry],
    blob_hashes: &[[u8; 32]],
    edge_idx: usize,
) -> Result<String, BlastError> {
    let span = evidence.get(edge_idx).copied().unwrap_or_default();
    let hash_hex = graphzero_store::hex_blob_hash(blob_hashes, span.blob_idx).map_err(|err| {
        BlastError::MalformedIndex {
            blob_idx: err.blob_idx,
            blob_hash_count: err.blob_hash_count,
        }
    })?;
    Ok(blob_span_ref(&hash_hex, span.start, span.end))
}

/// Byte extents of the root symbol's own definition, per blob. The root edit
/// site is the model's own decision, not a mechanically implied one.
fn root_definition_spans(spans: &[SpanEntry], root_id: u32) -> Vec<(u32, u32, u32)> {
    spans
        .iter()
        .filter(|span| span.symbol_id == root_id)
        .map(|span| {
            if span.block_end > span.block_start {
                (span.blob_idx, span.block_start, span.block_end)
            } else {
                (span.blob_idx, span.start, span.end)
            }
        })
        .collect()
}

fn is_root_definition(root_spans: &[(u32, u32, u32)], span: &SpanEntry) -> bool {
    root_spans.iter().any(|&(blob_idx, start, end)| {
        span.blob_idx == blob_idx && span.start >= start && span.start < end
    })
}

struct ImpliedEdge {
    src: u32,
    edge_idx: usize,
    relation: Relation,
    hop: u32,
}

fn walk_implied_edges(
    csr: &CsrAdjacency<'_>,
    reverse: &ReverseIndex,
    root_id: u32,
    policy: &PropagationPolicy,
) -> Vec<ImpliedEdge> {
    let mut seen_depth: HashMap<u32, u32> = HashMap::new();
    let mut seen_edges: BTreeSet<(u32, usize)> = BTreeSet::new();
    let mut queue: VecDeque<u32> = VecDeque::new();
    let mut implied = Vec::new();
    seen_depth.insert(root_id, 0);
    queue.push_back(root_id);

    while let Some(node) = queue.pop_front() {
        let hop = seen_depth[&node];
        if hop >= policy.max_depth {
            continue;
        }
        for &(src, edge_idx) in reverse.callers(node) {
            let edge_idx = edge_idx as usize;
            let Some(edge) = edge_at(csr, src, edge_idx) else {
                continue;
            };
            let Some(relation) = Relation::from_edge_kind(edge.kind) else {
                continue;
            };
            if !policy.allows(relation) {
                continue;
            }
            if seen_edges.insert((src, edge_idx)) {
                implied.push(ImpliedEdge {
                    src,
                    edge_idx,
                    relation,
                    hop: hop + 1,
                });
            }
            if !seen_depth.contains_key(&src) {
                seen_depth.insert(src, hop + 1);
                queue.push_back(src);
            }
        }
    }
    implied
}

/// Enumerate every mechanically implied edit site for the root decision on
/// `root_symbol` under `policy`.
pub fn rewrite_closure(
    snapshot: &Snapshot,
    root_symbol: &str,
    policy: &PropagationPolicy,
) -> Result<RewriteClosure, BlastError> {
    if root_symbol.trim().is_empty() {
        return Err(BlastError::Parse("empty root symbol".into()));
    }
    if policy.relations.is_empty() {
        return Err(BlastError::Parse(
            "propagation policy names no relations".into(),
        ));
    }

    let view = snapshot
        .global_view()
        .map_err(|e| BlastError::Store(e.to_string()))?;
    let table = SymbolTable::from_view(&view).map_err(|e| BlastError::Store(e.to_string()))?;
    let root_id = table
        .get(root_symbol)
        .ok_or_else(|| BlastError::SymbolNotFound(root_symbol.to_string()))?;
    let csr = CsrAdjacency::new(view.edges().map_err(|e| BlastError::Store(e.to_string()))?);
    let evidence = view
        .edge_evidence()
        .map_err(|e| BlastError::Store(e.to_string()))?;
    let blob_hashes = view
        .coverage()
        .map_err(|e| BlastError::Store(e.to_string()))?
        .blob_hashes;
    let spans = view.spans().map_err(|e| BlastError::Store(e.to_string()))?;
    let root_spans = root_definition_spans(&spans, root_id);
    let reverse = snapshot
        .blast_reverse_index()
        .map_err(|e| BlastError::Store(e.to_string()))?;

    let implied = walk_implied_edges(&csr, reverse, root_id, policy);

    let mut sites: Vec<EditSite> = Vec::new();
    let mut placed: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut unresolved_sites = 0usize;
    for edge in &implied {
        let span = evidence.get(edge.edge_idx).copied().unwrap_or_default();
        if is_root_definition(&root_spans, &span) {
            continue;
        }
        let evidence_ref = evidence_ref_for(&evidence, &blob_hashes, edge.edge_idx)?;
        let symbol = table.name(edge.src).unwrap_or("").to_string();
        let Some(hit) = graphzero_store::file_target_for_evidence(
            snapshot,
            &evidence_ref,
            edge.relation.as_str(),
            None,
            false,
        ) else {
            unresolved_sites += 1;
            continue;
        };
        if !placed.insert((
            hit.target.clone(),
            edge.relation.as_str().to_string(),
            symbol.clone(),
        )) {
            continue;
        }
        sites.push(EditSite {
            target: hit.target,
            path: hit.path,
            start_line: hit.start_line,
            end_line: hit.end_line,
            kind: edge.relation.as_str().to_string(),
            sym: hit.symbol,
            symbol,
            hop: edge.hop,
            evidence_ref,
            content: None,
        });
    }

    sites.sort_by(|a, b| {
        a.hop
            .cmp(&b.hop)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.start_line.cmp(&b.start_line))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.symbol.cmp(&b.symbol))
    });

    // Inline the content window for the leading sites, matching def/ref/blast.
    for site in sites
        .iter_mut()
        .take(graphzero_store::TARGET_INLINE_TOP_HITS)
    {
        if let Some(hit) = graphzero_store::file_target_for_evidence(
            snapshot,
            &site.evidence_ref,
            &site.kind,
            Some(&site.sym),
            true,
        ) && !hit.content.is_empty()
        {
            site.content = Some(hit.content);
        }
    }

    let accounting = accounting_for_evidence_refs(
        snapshot,
        "rewrite_closure_unaffected_files",
        sites.iter().map(|site| site.evidence_ref.clone()),
        "rewrite closure selected the statically implied edit sites for the root decision; other indexed files need no edit",
    );

    Ok(RewriteClosure {
        schema_version: REWRITE_CLOSURE_SCHEMA_VERSION,
        root_symbol: root_symbol.to_string(),
        policy: policy.clone(),
        sites,
        unresolved_sites,
        accounting,
    })
}
