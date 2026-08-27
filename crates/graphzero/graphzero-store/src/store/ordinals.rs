//! Durable generation-qualified ordinal aliases for GraphZero snapshots.
//!
//! The snapshot's symbol table already provides stable, lexicographically sorted
//! zero-based `u32` IDs. This sidecar gives those IDs (and the semantically
//! sorted CSR edges) canonical one-based `gz://o/<generation>/<ordinal>` refs.
//! It is part of the pre-manifest snapshot set: a published snapshot without a
//! valid sidecar is never admitted by the strict loader.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zero_gauge::{EngineScheme, Gauge, OrdinalRef};

use super::hot_path::ShardView;
use super::indexer::EdgeRecord;

pub const ORDINAL_SCHEMA: &str = "graphzero.ordinal_sidecar.v1";
/// GraphZero symbol and edge IDs are `u32`; one-based ordinal coordinates have
/// exactly one additional value, so a generation has 2^32 available slots.
pub const ORDINAL_CAPACITY: u64 = 1u64 << 32;

fn gauge() -> Result<Gauge> {
    Gauge::new(ORDINAL_CAPACITY).map_err(|e| anyhow::anyhow!("ordinal gauge: {e}"))
}

pub fn ordinal_file_name(snapshot_id: u64) -> String {
    format!("ordinals_{snapshot_id:08}.json")
}

pub fn ordinal_sidecar_path(shards_dir: &Path, snapshot_id: u64) -> PathBuf {
    shards_dir.join(ordinal_file_name(snapshot_id))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrdinalCounts {
    pub symbols: u64,
    pub edges: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SymbolRecord {
    id: u32,
    name: String,
    ordinal: u64,
    reference: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrdinalEdge {
    pub source_id: u32,
    pub target_id: u32,
    pub kind: u8,
    pub confidence: u8,
    pub blob: String,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EdgeRecordSidecar {
    ordinal: u64,
    source_id: u32,
    target_id: u32,
    source: String,
    target: String,
    kind: u8,
    confidence: u8,
    blob: String,
    start: u32,
    end: u32,
    reference: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SidecarFile {
    schema: String,
    snapshot_generation: u64,
    capacity: u64,
    counts: OrdinalCounts,
    symbols: Vec<SymbolRecord>,
    edges: Vec<EdgeRecordSidecar>,
    integrity_sha256: String,
}

/// Validated, immutable ordinal sidecar loaded for one published snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrdinalSidecar {
    snapshot_generation: u64,
    counts: OrdinalCounts,
    symbol_refs: Vec<OrdinalRef>,
    edge_refs: Vec<OrdinalRef>,
    edge_keys: Vec<OrdinalEdge>,
    symbol_names: Vec<String>,
}

impl OrdinalSidecar {
    /// Build a sidecar from the already-resolved symbol IDs and index records.
    /// `symbol_names` is the global lexicographic symbol-ID order.
    pub fn build(
        snapshot_generation: u64,
        symbol_names: &[String],
        edges: &[EdgeRecord],
    ) -> Result<Self> {
        validate_generation(snapshot_generation)?;
        let gauge = gauge()?;
        let symbol_count = u64::try_from(symbol_names.len())
            .context("symbol count exceeds ordinal integer range")?;
        if symbol_count > ORDINAL_CAPACITY {
            bail!("symbol count {symbol_count} exceeds ordinal capacity {ORDINAL_CAPACITY}");
        }
        let mut symbol_refs = Vec::with_capacity(symbol_names.len());
        for (id, _name) in symbol_names.iter().enumerate() {
            let id = u32::try_from(id).context("symbol ID exceeds u32 format")?;
            let ordinal = u64::from(id)
                .checked_add(1)
                .context("symbol ordinal overflow")?;
            let reference = ordinal_ref(snapshot_generation, ordinal, gauge)?;
            symbol_refs.push(reference);
        }

        let ids: BTreeMap<&str, u32> = symbol_names
            .iter()
            .enumerate()
            .map(|(id, name)| {
                Ok((
                    name.as_str(),
                    u32::try_from(id).context("symbol ID exceeds u32 format")?,
                ))
            })
            .collect::<Result<_>>()?;
        let mut ordered = Vec::with_capacity(edges.len());
        for edge in edges {
            let source_id = *ids
                .get(edge.src.as_str())
                .with_context(|| format!("edge source {:?} is not in symbol table", edge.src))?;
            let target_id = *ids
                .get(edge.dst.as_str())
                .with_context(|| format!("edge target {:?} is not in symbol table", edge.dst))?;
            // Include every semantic and evidence field in the key. The final
            // fields make equal semantic records deterministic as well.
            ordered.push((
                source_id,
                target_id,
                edge.kind,
                edge.confidence,
                edge.blob.to_hex(),
                edge.start,
                edge.end,
                edge.src.clone(),
                edge.dst.clone(),
            ));
        }
        ordered.sort();
        let edge_count =
            u64::try_from(ordered.len()).context("edge count exceeds ordinal integer range")?;
        let total = symbol_count
            .checked_add(edge_count)
            .context("total ordinal count overflow")?;
        if total > ORDINAL_CAPACITY {
            bail!("ordinal count {total} exceeds ordinal capacity {ORDINAL_CAPACITY}");
        }
        let edge_keys = ordered
            .iter()
            .map(
                |(source_id, target_id, kind, confidence, blob, start, end, _, _)| OrdinalEdge {
                    source_id: *source_id,
                    target_id: *target_id,
                    kind: *kind,
                    confidence: *confidence,
                    blob: blob.clone(),
                    start: *start,
                    end: *end,
                },
            )
            .collect();
        let edge_refs = ordered
            .iter()
            .enumerate()
            .map(|(index, _)| {
                let ordinal = symbol_count
                    .checked_add(u64::try_from(index).unwrap_or(u64::MAX))
                    .and_then(|value| value.checked_add(1))
                    .context("edge ordinal overflow")?;
                ordinal_ref(snapshot_generation, ordinal, gauge)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            snapshot_generation,
            counts: OrdinalCounts {
                symbols: symbol_count,
                edges: edge_count,
                total,
            },
            symbol_refs,
            edge_refs,
            edge_keys,
            symbol_names: symbol_names.to_vec(),
        })
    }

    pub fn snapshot_generation(&self) -> u64 {
        self.snapshot_generation
    }

    pub fn counts(&self) -> OrdinalCounts {
        self.counts
    }

    pub fn symbol_ref(&self, symbol_id: u32) -> Result<OrdinalRef> {
        self.symbol_refs
            .get(symbol_id as usize)
            .copied()
            .with_context(|| format!("symbol ID {symbol_id} is outside ordinal sidecar"))
    }

    pub fn edge_ref(&self, edge_index: usize) -> Result<OrdinalRef> {
        self.edge_refs
            .get(edge_index)
            .copied()
            .with_context(|| format!("edge index {edge_index} is outside ordinal sidecar"))
    }

    pub fn edge_key(&self, edge_index: usize) -> Result<&OrdinalEdge> {
        self.edge_keys
            .get(edge_index)
            .with_context(|| format!("edge index {edge_index} is outside ordinal sidecar"))
    }

    pub fn symbol_name(&self, symbol_id: u32) -> Result<&str> {
        self.symbol_names
            .get(symbol_id as usize)
            .map(String::as_str)
            .with_context(|| format!("symbol ID {symbol_id} is outside ordinal sidecar"))
    }

    /// Serialize and atomically publish the sidecar. The caller must include
    /// the returned path in its pre-manifest fsync barrier.
    pub fn write_published(
        &self,
        shards_dir: &Path,
        snapshot_generation: u64,
        edges: &[EdgeRecord],
    ) -> Result<PathBuf> {
        if self.snapshot_generation != snapshot_generation {
            bail!("ordinal sidecar generation mismatch before publication");
        }
        let gauge = gauge()?;
        let mut symbols = Vec::with_capacity(self.symbol_names.len());
        for (id, name) in self.symbol_names.iter().enumerate() {
            let id = u32::try_from(id).context("symbol ID exceeds u32 format")?;
            let ordinal = u64::from(id)
                .checked_add(1)
                .context("symbol ordinal overflow")?;
            let reference = ordinal_ref(snapshot_generation, ordinal, gauge)?.to_string();
            symbols.push(SymbolRecord {
                id,
                name: name.clone(),
                ordinal,
                reference,
            });
        }
        let ids: BTreeMap<&str, u32> = self
            .symbol_names
            .iter()
            .enumerate()
            .map(|(id, name)| {
                Ok((
                    name.as_str(),
                    u32::try_from(id).context("symbol ID exceeds u32 format")?,
                ))
            })
            .collect::<Result<_>>()?;
        let mut semantic_edges = edges
            .iter()
            .map(|edge| {
                let source_id = *ids
                    .get(edge.src.as_str())
                    .ok_or_else(|| anyhow::anyhow!("edge source is not in symbol table"))?;
                let target_id = *ids
                    .get(edge.dst.as_str())
                    .ok_or_else(|| anyhow::anyhow!("edge target is not in symbol table"))?;
                Ok((
                    source_id,
                    target_id,
                    edge.kind,
                    edge.confidence,
                    edge.blob.to_hex(),
                    edge.start,
                    edge.end,
                    edge.src.clone(),
                    edge.dst.clone(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        semantic_edges.sort();
        if semantic_edges.len() != self.edge_refs.len() {
            bail!("edge count changed before ordinal sidecar publication");
        }
        let edges = semantic_edges
            .into_iter()
            .enumerate()
            .map(
                |(
                    index,
                    (source_id, target_id, kind, confidence, blob, start, end, source, target),
                )| {
                    let ordinal =
                        self.counts.symbols + u64::try_from(index).unwrap_or(u64::MAX) + 1;
                    let reference = ordinal_ref(snapshot_generation, ordinal, gauge)?.to_string();
                    Ok(EdgeRecordSidecar {
                        ordinal,
                        source_id,
                        target_id,
                        source,
                        target,
                        kind,
                        confidence,
                        blob,
                        start,
                        end,
                        reference,
                    })
                },
            )
            .collect::<Result<Vec<_>>>()?;
        let mut file = SidecarFile {
            schema: ORDINAL_SCHEMA.to_string(),
            snapshot_generation,
            capacity: ORDINAL_CAPACITY,
            counts: self.counts,
            symbols,
            edges,
            integrity_sha256: String::new(),
        };
        file.integrity_sha256 = integrity_sha256(&file)?;
        let bytes = serde_json::to_vec_pretty(&file).context("serialize ordinal sidecar")?;
        let path = ordinal_sidecar_path(shards_dir, snapshot_generation);
        super::atomic_write_file(&path, &bytes)
            .with_context(|| format!("publish ordinal sidecar {}", path.display()))?;
        Ok(path)
    }
}

fn validate_generation(snapshot_generation: u64) -> Result<()> {
    if snapshot_generation == 0 {
        bail!("ordinal snapshot generation must be nonzero");
    }
    Ok(())
}

fn ordinal_ref(snapshot_generation: u64, ordinal: u64, gauge: Gauge) -> Result<OrdinalRef> {
    if ordinal == 0 || ordinal > ORDINAL_CAPACITY {
        bail!("ordinal {ordinal} is outside capacity {ORDINAL_CAPACITY}");
    }
    let reference = OrdinalRef::new(EngineScheme::Gz, snapshot_generation, ordinal);
    gauge
        .allocation(reference)
        .map_err(|e| anyhow::anyhow!("invalid ordinal coordinate: {e}"))?;
    Ok(reference)
}

/// Strictly load and validate a published sidecar. Missing, stale, malformed,
/// non-dense, non-GZ, or tampered sidecars are all errors.
pub fn load_published(shards_dir: &Path, snapshot_generation: u64) -> Result<OrdinalSidecar> {
    load_published_for_global(shards_dir, snapshot_generation, None)
}

/// Load and bind a sidecar to the actual published global shard. The binding
/// rejects a valid-but-stale recomputed sidecar whose own digest is consistent
/// but whose symbols or CSR edge rows differ from the opened snapshot.
pub fn load_published_for_global(
    shards_dir: &Path,
    snapshot_generation: u64,
    global: Option<&ShardView<'_>>,
) -> Result<OrdinalSidecar> {
    validate_generation(snapshot_generation)?;
    let path = ordinal_sidecar_path(shards_dir, snapshot_generation);
    let bytes =
        fs::read(&path).with_context(|| format!("read ordinal sidecar {}", path.display()))?;
    let file: SidecarFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse ordinal sidecar {}", path.display()))?;
    if file.schema != ORDINAL_SCHEMA {
        bail!("unsupported ordinal sidecar schema {:?}", file.schema);
    }
    if file.snapshot_generation != snapshot_generation {
        bail!("ordinal sidecar snapshot generation mismatch");
    }
    if file.capacity != ORDINAL_CAPACITY {
        bail!("ordinal sidecar capacity mismatch");
    }
    let expected_integrity = integrity_sha256(&file)?;
    if file.integrity_sha256 != expected_integrity {
        bail!("ordinal sidecar integrity digest mismatch");
    }
    let gauge = gauge()?;
    validate_counts(&file)?;
    if let Some(global) = global {
        let symbols = global.symbols()?;
        if symbols.entries.len() != file.symbols.len() {
            bail!("ordinal sidecar symbol count does not match global shard");
        }
        for (index, symbol) in file.symbols.iter().enumerate() {
            let entry = &symbols.entries[index];
            if entry.symbol_id != symbol.id
                || ShardView::symbol_name(&symbols, entry) != symbol.name
            {
                bail!("ordinal sidecar symbol rows do not match global shard");
            }
        }
        let edges = global.edges()?;
        let evidence = global.edge_evidence()?;
        let coverage = global.coverage()?;
        let actual_edges = edges.targets.len();
        if actual_edges != file.edges.len() || evidence.len() != actual_edges {
            bail!("ordinal sidecar edge count does not match global shard");
        }
        let mut actual_keys = Vec::with_capacity(actual_edges);
        for source_id in 0..symbols.entries.len() as u32 {
            let lo = edges.offsets[source_id as usize] as usize;
            let hi = edges.offsets[source_id as usize + 1] as usize;
            for index in lo..hi {
                let ev = evidence[index];
                let blob = coverage
                    .blob_hashes
                    .get(ev.blob_idx as usize)
                    .ok_or_else(|| {
                        anyhow::anyhow!("ordinal sidecar edge evidence blob index is out of range")
                    })?;
                let source = ShardView::symbol_name(&symbols, &symbols.entries[source_id as usize]);
                let target_id = edges.targets[index];
                let target = symbols
                    .entries
                    .get(target_id as usize)
                    .map(|entry| ShardView::symbol_name(&symbols, entry))
                    .ok_or_else(|| {
                        anyhow::anyhow!("ordinal sidecar edge target is out of range")
                    })?;
                actual_keys.push((
                    source_id,
                    target_id,
                    edges.kinds[index],
                    edges.confidences[index],
                    crate::fast_hex_32(blob),
                    ev.start,
                    ev.end,
                    source.to_owned(),
                    target.to_owned(),
                ));
            }
        }
        actual_keys.sort();
        for (index, actual) in actual_keys.iter().enumerate() {
            let sidecar = &file.edges[index];
            let sidecar_key = (
                sidecar.source_id,
                sidecar.target_id,
                sidecar.kind,
                sidecar.confidence,
                sidecar.blob.clone(),
                sidecar.start,
                sidecar.end,
                sidecar.source.clone(),
                sidecar.target.clone(),
            );
            if actual != &sidecar_key {
                bail!("ordinal sidecar edge rows do not match global shard");
            }
        }
    }
    let mut symbol_refs = Vec::with_capacity(file.symbols.len());
    let mut symbol_names = Vec::with_capacity(file.symbols.len());
    for (index, symbol) in file.symbols.iter().enumerate() {
        if index > 0 && file.symbols[index - 1].name >= symbol.name {
            bail!("symbol names are not strictly lexicographically ordered");
        }
        let id = u32::try_from(index).context("symbol count exceeds u32 format")?;
        if symbol.id != id {
            bail!("symbol IDs are not dense and monotone");
        }
        let ordinal = u64::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .context("symbol ordinal overflow")?;
        if symbol.ordinal != ordinal {
            bail!("symbol ordinals are not dense and monotone");
        }
        let reference = parse_gz_ref(&symbol.reference, snapshot_generation, ordinal, gauge)?;
        symbol_refs.push(reference);
        symbol_names.push(symbol.name.clone());
    }
    let mut edge_refs = Vec::with_capacity(file.edges.len());
    let mut prior_edge_key: Option<(u32, u32, u8, u8, String, u32, u32, String, String)> = None;
    for (index, edge) in file.edges.iter().enumerate() {
        let edge_key = (
            edge.source_id,
            edge.target_id,
            edge.kind,
            edge.confidence,
            edge.blob.clone(),
            edge.start,
            edge.end,
            edge.source.clone(),
            edge.target.clone(),
        );
        if prior_edge_key
            .as_ref()
            .is_some_and(|prior| prior > &edge_key)
        {
            bail!("edge records are not nondecreasing semantically ordered");
        }
        prior_edge_key = Some(edge_key);
        let ordinal = file
            .counts
            .symbols
            .checked_add(u64::try_from(index).context("edge index overflow")?)
            .and_then(|value| value.checked_add(1))
            .context("edge ordinal overflow")?;
        if edge.ordinal != ordinal {
            bail!("edge ordinals are not dense and monotone");
        }
        if edge.source_id as usize >= file.symbols.len()
            || edge.target_id as usize >= file.symbols.len()
        {
            bail!("edge endpoint is outside symbol table");
        }
        if edge.source != file.symbols[edge.source_id as usize].name
            || edge.target != file.symbols[edge.target_id as usize].name
        {
            bail!("edge endpoint name does not match symbol table");
        }
        let reference = parse_gz_ref(&edge.reference, snapshot_generation, ordinal, gauge)?;
        if edge.blob.len() != 64
            || !edge
                .blob
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            bail!("edge evidence blob is not a lowercase 64-hex digest");
        }
        edge_refs.push(reference);
    }
    let edge_keys = file
        .edges
        .iter()
        .map(|edge| OrdinalEdge {
            source_id: edge.source_id,
            target_id: edge.target_id,
            kind: edge.kind,
            confidence: edge.confidence,
            blob: edge.blob.clone(),
            start: edge.start,
            end: edge.end,
        })
        .collect();
    Ok(OrdinalSidecar {
        snapshot_generation,
        counts: file.counts,
        symbol_refs,
        edge_refs,
        edge_keys,
        symbol_names,
    })
}

fn integrity_sha256(file: &SidecarFile) -> Result<String> {
    let mut payload = file.clone();
    payload.integrity_sha256.clear();
    let bytes = serde_json::to_vec(&payload).context("serialize ordinal integrity payload")?;
    Ok(crate::fast_hex(Sha256::digest(bytes).as_slice()))
}

/// Test-only helper: tamper the first edge's evidence blob in a published
/// sidecar, recompute the self-integrity digest so the integrity check alone
/// would pass, and write it back. The returned pair is `(original_blob,
/// tampered_blob)`. Callers should then prove that the global-shard binding
/// in [`load_published_for_global`] still rejects the tampered evidence.
#[doc(hidden)]
pub fn tamper_first_edge_blob_for_test(path: &Path) -> Result<(String, String)> {
    let bytes = fs::read(path).context("read sidecar for test tamper")?;
    let mut file: SidecarFile =
        serde_json::from_slice(&bytes).context("parse sidecar for test tamper")?;
    let original_blob = file.edges[0].blob.clone();
    let tampered_blob = if original_blob.as_bytes()[0] == b'0' {
        format!("1{}", &original_blob[1..])
    } else {
        format!("0{}", &original_blob[1..])
    };
    file.edges[0].blob = tampered_blob.clone();
    file.integrity_sha256.clear();
    file.integrity_sha256 = integrity_sha256(&file)?;
    let out = serde_json::to_vec_pretty(&file).context("serialize tampered sidecar")?;
    fs::write(path, out).context("write tampered sidecar")?;
    Ok((original_blob, tampered_blob))
}

fn validate_counts(file: &SidecarFile) -> Result<()> {
    let symbols = u64::try_from(file.symbols.len()).context("symbol count overflow")?;
    let edges = u64::try_from(file.edges.len()).context("edge count overflow")?;
    if file.counts.symbols != symbols || file.counts.edges != edges {
        bail!("ordinal sidecar count mismatch");
    }
    if file.counts.total != symbols.checked_add(edges).context("total count overflow")? {
        bail!("ordinal sidecar total count mismatch");
    }
    if file.counts.total > ORDINAL_CAPACITY {
        bail!("ordinal sidecar count exceeds capacity");
    }
    Ok(())
}

fn parse_gz_ref(text: &str, generation: u64, ordinal: u64, gauge: Gauge) -> Result<OrdinalRef> {
    let reference = OrdinalRef::from_str(text).context("ordinal reference grammar")?;
    if reference.scheme() != EngineScheme::Gz
        || reference.generation() != generation
        || reference.ordinal() != ordinal
    {
        bail!("ordinal reference is stale, non-GZ, or out of range");
    }
    gauge
        .allocation(reference)
        .map_err(|e| anyhow::anyhow!("ordinal reference gauge validation: {e}"))?;
    Ok(reference)
}
