//! Lexical semantic tier v1 (graphzero-nmf): BM25 over symbol-span chunks.
//!
//! Every document is one def span, so every hit maps to an existing
//! `gz://blob/<sha256>#B<start>-<end>` evidence ref that round-trips through
//! `expand` byte-for-byte. Documents carry identifier-split tokens from the
//! symbol name, path segments, leading comment/doc lines, and a capped body
//! prefix. No embedding model; scoring is BM25 plus a graph-proximity boost
//! over call/import/ref edges shared between candidates.
//!
//! Publish-time sidecar `semantic_lexical_{id:08}.bin` (GZLX v1) is written
//! during `graphzero index`; legacy snapshots build lazily on first semantic
//! query and persist the same sidecar (zero-config, offline).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow, bail};

use super::super::blob_store::BlobStore;
use super::super::csr::CsrAdjacency;
use super::super::symbol_table::SymbolTable;
use super::snapshot::Snapshot;

/// On-disk magic for the lexical semantic sidecar.
pub const LEXICAL_SEMANTIC_MAGIC: [u8; 4] = *b"GZLX";
/// Sidecar wire version (independent of GZSH/GZNB).
pub const LEXICAL_SEMANTIC_VERSION: u8 = 1;

/// Snapshot-relative sidecar filename.
pub fn lexical_semantic_file_name(snapshot_id: u64) -> String {
    format!("semantic_lexical_{snapshot_id:08}.bin")
}

const K1: f64 = 1.2;
const B: f64 = 0.75;

const NAME_WEIGHT: u32 = 4;
const PATH_WEIGHT: u32 = 2;
const COMMENT_WEIGHT: u32 = 2;
const BODY_WEIGHT: u32 = 1;
/// Body tokens kept per doc (post-filter); bounds index size on large fns.
const BODY_TOKEN_CAP: usize = 192;
/// Body bytes tokenized per doc; bounds build cost on huge spans.
const BODY_BYTE_CAP: usize = 8192;
/// Comment lines scanned above a def span.
const COMMENT_LINE_CAP: usize = 12;

/// Per shared candidate-set edge, multiplicative score boost.
const GRAPH_BOOST_STEP: f32 = 0.05;
const GRAPH_BOOST_CAP: u32 = 4;

const STOPWORDS: &[&str] = &[
    "the", "a", "an", "of", "for", "to", "in", "on", "and", "or", "with", "how", "do", "does",
    "is", "are", "was", "be", "what", "where", "which", "that", "this", "when", "into", "from",
    "by", "at", "as", "it", "its", "all", "any", "can", "not",
];

fn is_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

fn push_token(out: &mut Vec<String>, piece: &str) {
    if piece.len() < 2 {
        return;
    }
    if !piece.bytes().any(|b| b.is_ascii_alphabetic()) {
        return;
    }
    let lower = piece.to_ascii_lowercase();
    if is_stopword(&lower) {
        return;
    }
    out.push(lower);
}

/// Split one identifier word at camelCase, acronym, and letter/digit
/// boundaries. `word` contains only ASCII alphanumerics.
fn split_word(word: &str, out: &mut Vec<String>) {
    let bytes = word.as_bytes();
    let mut start = 0usize;
    for i in 1..bytes.len() {
        let prev = bytes[i - 1];
        let cur = bytes[i];
        let next_lower = bytes.get(i + 1).is_some_and(|b| b.is_ascii_lowercase());
        let boundary = (prev.is_ascii_lowercase() && cur.is_ascii_uppercase())
            || (prev.is_ascii_uppercase() && cur.is_ascii_uppercase() && next_lower)
            || (prev.is_ascii_digit() != cur.is_ascii_digit()
                && (prev.is_ascii_alphabetic() || cur.is_ascii_alphabetic()));
        if boundary {
            push_token(out, &word[start..i]);
            start = i;
        }
    }
    push_token(out, &word[start..]);
}

/// Identifier-aware tokenizer: splits on non-alphanumerics (snake_case,
/// kebab-case, paths, `::`), then camelCase/digit boundaries; lowercases;
/// drops stopwords, single chars, and pure numbers.
pub fn tokenize_into(text: &str, out: &mut Vec<String>) {
    let mut start = None;
    for (i, b) in text.bytes().enumerate() {
        if b.is_ascii_alphanumeric() {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            split_word(&text[s..i], out);
        }
    }
    if let Some(s) = start {
        split_word(&text[s..], out);
    }
}

/// Comment/doc lines directly above `span_start` (up to [`COMMENT_LINE_CAP`]).
/// Attribute lines (`#[...]`, `@decorator`) are skipped; scan stops at the
/// first non-comment code line.
fn leading_comment_text(content: &[u8], span_start: usize) -> String {
    let mut line_start = content[..span_start.min(content.len())]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |p| p + 1);
    let mut collected: Vec<&str> = Vec::new();
    for _ in 0..COMMENT_LINE_CAP {
        if line_start == 0 {
            break;
        }
        let prev_start = content[..line_start - 1]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |p| p + 1);
        let line = std::str::from_utf8(&content[prev_start..line_start - 1]).unwrap_or("");
        let trimmed = line.trim();
        let is_attr = trimmed.starts_with("#[") || trimmed.starts_with('@');
        let is_comment = trimmed.starts_with("///")
            || trimmed.starts_with("//!")
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with('#')
            || trimmed.starts_with("--")
            || trimmed.starts_with("\"\"\"");
        if is_comment && !is_attr {
            collected.push(trimmed);
        } else if !is_attr && !trimmed.is_empty() {
            break;
        }
        line_start = prev_start;
    }
    collected.reverse();
    collected.join("\n")
}

/// One symbol-span chunk in the index. Evidence span fields mirror the
/// snapshot's `SpanEntry` so hits reuse the symbol route's evidence refs.
#[derive(Clone, Debug, PartialEq)]
pub struct LexicalDoc {
    pub symbol_id: u32,
    pub blob: [u8; 32],
    pub start: u32,
    pub end: u32,
    pub token_count: u32,
}

/// Source material for one doc during index build.
pub struct LexicalDocSource<'a> {
    pub symbol_id: u32,
    pub name: &'a str,
    pub blob: [u8; 32],
    pub start: u32,
    pub end: u32,
    pub block_start: u32,
    pub block_end: u32,
    pub path: Option<&'a str>,
    pub content: Option<&'a [u8]>,
}

/// Streaming builder shared by publish-time and lazy build paths.
#[derive(Default)]
pub struct LexicalIndexBuilder {
    docs: Vec<LexicalDoc>,
    postings: HashMap<String, Vec<(u32, u32)>>,
    total_doc_tokens: u64,
}

impl LexicalIndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_doc(&mut self, src: &LexicalDocSource<'_>) {
        let mut counts: HashMap<String, u32> = HashMap::new();
        let mut toks: Vec<String> = Vec::new();

        tokenize_into(src.name, &mut toks);
        for t in toks.drain(..) {
            *counts.entry(t).or_default() += NAME_WEIGHT;
        }
        if let Some(path) = src.path {
            tokenize_into(path, &mut toks);
            for t in toks.drain(..) {
                *counts.entry(t).or_default() += PATH_WEIGHT;
            }
        }
        if let Some(content) = src.content {
            let (bs, be) = if src.block_end > src.block_start {
                (src.block_start as usize, src.block_end as usize)
            } else {
                (src.start as usize, src.end as usize)
            };
            let bs = bs.min(content.len());
            let comments = leading_comment_text(content, bs);
            tokenize_into(&comments, &mut toks);
            for t in toks.drain(..) {
                *counts.entry(t).or_default() += COMMENT_WEIGHT;
            }
            let be = be.min(content.len()).min(bs + BODY_BYTE_CAP);
            if be > bs {
                let body = String::from_utf8_lossy(&content[bs..be]);
                tokenize_into(&body, &mut toks);
                for t in toks.drain(..).take(BODY_TOKEN_CAP) {
                    *counts.entry(t).or_default() += BODY_WEIGHT;
                }
            }
        }

        let token_count: u32 = counts.values().sum();
        if token_count == 0 {
            return;
        }
        let doc_id = self.docs.len() as u32;
        for (term, tf) in counts {
            self.postings.entry(term).or_default().push((doc_id, tf));
        }
        self.total_doc_tokens += u64::from(token_count);
        self.docs.push(LexicalDoc {
            symbol_id: src.symbol_id,
            blob: src.blob,
            start: src.start,
            end: src.end,
            token_count,
        });
    }

    pub fn finish(self, total_symbols: usize) -> LexicalSemanticIndex {
        let mut terms: Vec<String> = self.postings.keys().cloned().collect();
        terms.sort_unstable();
        let mut postings = Vec::with_capacity(terms.len());
        let mut map = self.postings;
        for term in &terms {
            let mut list = map.remove(term).unwrap_or_default();
            list.sort_unstable_by_key(|&(doc, _)| doc);
            postings.push(list);
        }
        let indexed_symbols = {
            let mut ids: Vec<u32> = self.docs.iter().map(|d| d.symbol_id).collect();
            ids.sort_unstable();
            ids.dedup();
            ids.len() as u32
        };
        LexicalSemanticIndex {
            docs: self.docs,
            terms,
            postings,
            total_symbols: total_symbols as u32,
            indexed_symbols,
            total_doc_tokens: self.total_doc_tokens,
        }
    }
}

/// One scored hit: symbol-span chunk plus BM25(+graph boost) score.
#[derive(Clone, Debug, PartialEq)]
pub struct LexicalHit {
    pub symbol_id: u32,
    pub blob: [u8; 32],
    pub start: u32,
    pub end: u32,
    pub score: f32,
    pub matched_terms: u32,
}

/// BM25 inverted index over symbol-span chunks (GZLX v1).
#[derive(Debug, Default)]
pub struct LexicalSemanticIndex {
    docs: Vec<LexicalDoc>,
    /// Sorted unique terms; `postings[i]` belongs to `terms[i]`.
    terms: Vec<String>,
    /// Per term: ascending `(doc_id, weighted_tf)` pairs.
    postings: Vec<Vec<(u32, u32)>>,
    total_symbols: u32,
    indexed_symbols: u32,
    total_doc_tokens: u64,
}

impl LexicalSemanticIndex {
    pub fn doc_count(&self) -> usize {
        self.docs.len()
    }

    pub fn term_count(&self) -> usize {
        self.terms.len()
    }

    pub fn total_symbols(&self) -> usize {
        self.total_symbols as usize
    }

    pub fn indexed_symbols(&self) -> usize {
        self.indexed_symbols as usize
    }

    /// Honest coverage: fraction of snapshot symbols with >=1 indexed chunk.
    pub fn coverage_percent(&self) -> f64 {
        if self.total_symbols == 0 {
            return 0.0;
        }
        f64::from(self.indexed_symbols) / f64::from(self.total_symbols) * 100.0
    }

    /// BM25 top-`k` chunks for a natural-language query. Empty when no query
    /// term matches any indexed term.
    pub fn search(&self, query: &str, k: usize) -> Vec<LexicalHit> {
        let mut qterms: Vec<String> = Vec::new();
        tokenize_into(query, &mut qterms);
        qterms.sort_unstable();
        qterms.dedup();
        if qterms.is_empty() || self.docs.is_empty() {
            return Vec::new();
        }
        let n = self.docs.len() as f64;
        let avg = self.total_doc_tokens as f64 / n;
        let mut scores: HashMap<u32, (f64, u32)> = HashMap::new();
        for term in &qterms {
            let Ok(ti) = self.terms.binary_search(term) else {
                continue;
            };
            let list = &self.postings[ti];
            let df = list.len() as f64;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            for &(doc, tf) in list {
                let dl = f64::from(self.docs[doc as usize].token_count);
                let tf = f64::from(tf);
                let contrib = idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * dl / avg));
                let entry = scores.entry(doc).or_insert((0.0, 0));
                entry.0 += contrib;
                entry.1 += 1;
            }
        }
        let mut hits: Vec<LexicalHit> = scores
            .into_iter()
            .map(|(doc, (score, matched))| {
                let d = &self.docs[doc as usize];
                LexicalHit {
                    symbol_id: d.symbol_id,
                    blob: d.blob,
                    start: d.start,
                    end: d.end,
                    score: score as f32,
                    matched_terms: matched,
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.symbol_id.cmp(&b.symbol_id))
        });
        hits.truncate(k);
        hits
    }

    /// Build lazily from an opened snapshot (legacy stores without the
    /// publish-time sidecar). Reads each blob once via the blob store.
    pub fn build_from_snapshot(snapshot: &Snapshot) -> Result<Self> {
        let view = snapshot.global_view()?;
        let table = SymbolTable::from_view(&view)?;
        let spans = view.spans()?;
        let coverage = view.coverage()?;
        let store = BlobStore::open(&snapshot.store_root)?;
        let mut content_cache: HashMap<u32, Option<Vec<u8>>> = HashMap::new();
        let mut builder = LexicalIndexBuilder::new();
        for span in spans.as_ref() {
            let Some(name) = table.name(span.symbol_id) else {
                continue;
            };
            let Some(blob) = coverage.blob_hashes.get(span.blob_idx as usize).copied() else {
                continue;
            };
            let hex = crate::fast_hex_32(&blob);
            let path = snapshot.path_for_blob(&hex).map(|r| r.path.clone());
            let content = content_cache
                .entry(span.blob_idx)
                .or_insert_with(|| store.get_hex(&hex).ok().flatten());
            builder.add_doc(&LexicalDocSource {
                symbol_id: span.symbol_id,
                name,
                blob,
                start: span.start,
                end: span.end,
                block_start: span.block_start,
                block_end: span.block_end,
                path: path.as_deref(),
                content: content.as_deref(),
            });
        }
        Ok(builder.finish(table.len()))
    }

    /// Encode as GZLX v1 bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(64 + self.docs.len() * 48);
        out.extend_from_slice(&LEXICAL_SEMANTIC_MAGIC);
        out.push(LEXICAL_SEMANTIC_VERSION);
        out.extend_from_slice(&[0u8; 3]);
        out.extend_from_slice(&self.total_symbols.to_le_bytes());
        out.extend_from_slice(&self.indexed_symbols.to_le_bytes());
        out.extend_from_slice(&(self.docs.len() as u32).to_le_bytes());
        out.extend_from_slice(&(self.terms.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.total_doc_tokens.to_le_bytes());
        for d in &self.docs {
            out.extend_from_slice(&d.symbol_id.to_le_bytes());
            out.extend_from_slice(&d.blob);
            out.extend_from_slice(&d.start.to_le_bytes());
            out.extend_from_slice(&d.end.to_le_bytes());
            out.extend_from_slice(&d.token_count.to_le_bytes());
        }
        for (term, list) in self.terms.iter().zip(&self.postings) {
            out.extend_from_slice(&(term.len() as u16).to_le_bytes());
            out.extend_from_slice(term.as_bytes());
            out.extend_from_slice(&(list.len() as u32).to_le_bytes());
            let mut prev = 0u32;
            for &(doc, tf) in list {
                write_uleb128(&mut out, doc - prev);
                write_uleb128(&mut out, tf);
                prev = doc;
            }
        }
        out
    }

    /// Decode GZLX v1 bytes.
    pub fn from_bytes(buf: &[u8]) -> Result<Self> {
        let header = parse_header(buf)?;
        let mut i = HEADER_LEN;
        let mut docs = Vec::with_capacity(header.doc_count as usize);
        for _ in 0..header.doc_count {
            if i + 48 > buf.len() {
                bail!("semantic_lexical: truncated doc table");
            }
            let symbol_id = read_u32_at(buf, i);
            let mut blob = [0u8; 32];
            blob.copy_from_slice(&buf[i + 4..i + 36]);
            let start = read_u32_at(buf, i + 36);
            let end = read_u32_at(buf, i + 40);
            let token_count = read_u32_at(buf, i + 44);
            docs.push(LexicalDoc {
                symbol_id,
                blob,
                start,
                end,
                token_count,
            });
            i += 48;
        }
        let mut terms = Vec::with_capacity(header.term_count as usize);
        let mut postings = Vec::with_capacity(header.term_count as usize);
        for _ in 0..header.term_count {
            if i + 2 > buf.len() {
                bail!("semantic_lexical: truncated term length");
            }
            let len = u16::from_le_bytes([buf[i], buf[i + 1]]) as usize;
            i += 2;
            if i + len > buf.len() {
                bail!("semantic_lexical: truncated term bytes");
            }
            let term = std::str::from_utf8(&buf[i..i + len])
                .map_err(|_| anyhow!("semantic_lexical: non-utf8 term"))?
                .to_string();
            i += len;
            if i + 4 > buf.len() {
                bail!("semantic_lexical: truncated posting count");
            }
            let count = read_u32_at(buf, i);
            i += 4;
            let mut list = Vec::with_capacity(count as usize);
            let mut prev = 0u32;
            for _ in 0..count {
                let (delta, ni) = read_uleb128(buf, i)
                    .ok_or_else(|| anyhow!("semantic_lexical: truncated posting delta"))?;
                let (tf, ni) = read_uleb128(buf, ni)
                    .ok_or_else(|| anyhow!("semantic_lexical: truncated posting tf"))?;
                let doc = prev
                    .checked_add(delta)
                    .ok_or_else(|| anyhow!("semantic_lexical: posting doc overflow"))?;
                if doc as usize >= docs.len() {
                    bail!("semantic_lexical: posting doc {doc} out of range");
                }
                list.push((doc, tf));
                prev = doc;
                i = ni;
            }
            terms.push(term);
            postings.push(list);
        }
        Ok(Self {
            docs,
            terms,
            postings,
            total_symbols: header.total_symbols,
            indexed_symbols: header.indexed_symbols,
            total_doc_tokens: header.total_doc_tokens,
        })
    }

    /// Write sidecar next to shards.
    pub fn write_published(shards_dir: &Path, snapshot_id: u64, index: &Self) -> Result<()> {
        let path = shards_dir.join(lexical_semantic_file_name(snapshot_id));
        fs::write(&path, index.to_bytes())
            .map_err(|e| anyhow!("semantic_lexical write {}: {e}", path.display()))
    }

    /// Load published sidecar if present. `Ok(None)` when missing (legacy).
    pub fn try_load_published(shards_dir: &Path, snapshot_id: u64) -> Result<Option<Self>> {
        let path = shards_dir.join(lexical_semantic_file_name(snapshot_id));
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path)
            .map_err(|e| anyhow!("semantic_lexical read {}: {e}", path.display()))?;
        Ok(Some(Self::from_bytes(&bytes)?))
    }

    /// Header-only coverage read so `semantic_tier_percent` stays cheap when
    /// the index is not yet loaded. `None` when the sidecar is absent/invalid.
    pub fn published_coverage_percent(shards_dir: &Path, snapshot_id: u64) -> Option<f64> {
        let path = shards_dir.join(lexical_semantic_file_name(snapshot_id));
        let mut buf = [0u8; HEADER_LEN];
        use std::io::Read;
        let mut f = fs::File::open(path).ok()?;
        f.read_exact(&mut buf).ok()?;
        let header = parse_header(&buf).ok()?;
        if header.total_symbols == 0 {
            return Some(0.0);
        }
        Some(f64::from(header.indexed_symbols) / f64::from(header.total_symbols) * 100.0)
    }
}

const HEADER_LEN: usize = 32;

struct LexicalHeader {
    total_symbols: u32,
    indexed_symbols: u32,
    doc_count: u32,
    term_count: u32,
    total_doc_tokens: u64,
}

fn parse_header(buf: &[u8]) -> Result<LexicalHeader> {
    if buf.len() < HEADER_LEN {
        bail!("semantic_lexical: file too small");
    }
    if buf[0..4] != LEXICAL_SEMANTIC_MAGIC {
        bail!("semantic_lexical: bad magic {:02x?}", &buf[0..4]);
    }
    if buf[4] != LEXICAL_SEMANTIC_VERSION {
        bail!(
            "semantic_lexical: unsupported version {}, expected {}",
            buf[4],
            LEXICAL_SEMANTIC_VERSION
        );
    }
    Ok(LexicalHeader {
        total_symbols: read_u32_at(buf, 8),
        indexed_symbols: read_u32_at(buf, 12),
        doc_count: read_u32_at(buf, 16),
        term_count: read_u32_at(buf, 20),
        total_doc_tokens: u64::from_le_bytes(
            buf[24..32].try_into().expect("header slice is 8 bytes"),
        ),
    })
}

fn read_u32_at(buf: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]])
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

/// Graph-proximity rerank: candidates sharing call/import/ref edges with
/// other candidates get a bounded multiplicative boost, then re-sort.
pub fn graph_proximity_boost(hits: &mut [LexicalHit], csr: &CsrAdjacency<'_>) {
    if hits.len() < 2 {
        return;
    }
    let index_of: HashMap<u32, usize> = hits
        .iter()
        .enumerate()
        .map(|(i, h)| (h.symbol_id, i))
        .collect();
    let mut shared = vec![0u32; hits.len()];
    for (i, h) in hits.iter().enumerate() {
        for edge in csr.edges(h.symbol_id) {
            if let Some(&j) = index_of.get(&edge.target)
                && j != i
            {
                shared[i] += 1;
                shared[j] += 1;
            }
        }
    }
    for (i, h) in hits.iter_mut().enumerate() {
        h.score *= 1.0 + GRAPH_BOOST_STEP * shared[i].min(GRAPH_BOOST_CAP) as f32;
    }
    hits.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.symbol_id.cmp(&b.symbol_id))
    });
}
