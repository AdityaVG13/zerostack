//! Local semantic tier for `fs.resolve` (`fszero-semantic-local`).
//!
//! frankensearch 0.3.2 lexical+hash (optional ML embeddings stay behind
//! frankensearch's own feature flags; this crate enables only `hash`+`lexical`).
//! Embeddings are memoized per chunk digest in the shared recovery CAS so
//! incremental `build_index` pays once per content digest.
//!
//! Default feature set keeps this module compiled out -- no cold-index cost
//! when disabled (fszero-9wn).

use crate::RecoveryStore;
use frankensearch::HashEmbedder;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

/// CAS key namespace for hash embeddings (v1 = frankensearch HashEmbedder 256d FNV).
pub const EMB_KEY_PREFIX: &str = "semantic/emb/v1/";
/// Stable embedder id recorded in CAS payloads and producer fingerprint.
pub const EMBEDDER_ID: &str = "frankensearch-hash-256";
/// Max chunks stored per file during index (bounds memory / CAS churn).
const MAX_CHUNKS_PER_FILE: usize = 32;
/// Cap concurrent embed workers (mirrors build_index default).
const DEFAULT_EMBED_THREADS: usize = 4;

/// Magic + version + dim for CAS embedding blobs.
const MAGIC: &[u8; 4] = b"FSE1";

fn digest_hex(bytes: &[u8]) -> String {
    crate::hexutil::sha256_hex_of(Sha256::digest(bytes).into())
}

fn emb_key(digest: &str) -> String {
    format!("{EMB_KEY_PREFIX}{digest}")
}

fn encode_payload(values: &[f32]) -> Vec<u8> {
    let dim = values.len() as u16;
    let mut out = Vec::with_capacity(4 + 2 + values.len() * 4);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&dim.to_le_bytes());
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn decode_payload(bytes: &[u8]) -> Option<Vec<f32>> {
    if bytes.len() < 6 || &bytes[..4] != MAGIC {
        return None;
    }
    let dim = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    if bytes.len() != 6 + dim * 4 {
        return None;
    }
    let mut values = Vec::with_capacity(dim);
    for chunk in bytes[6..].chunks_exact(4) {
        values.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Some(values)
}

fn chunk_text(text: &str) -> Vec<&str> {
    super::cdc::content_defined_text_chunks(text, MAX_CHUNKS_PER_FILE)
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for i in 0..a.len() {
        let x = a[i] as f64;
        let y = b[i] as f64;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn lexical_overlap(query: &str, text: &str) -> f64 {
    let q: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() >= 2)
        .collect();
    if q.is_empty() {
        return 0.0;
    }
    let lower = text.to_ascii_lowercase();
    let mut hits = 0usize;
    for t in &q {
        if lower.contains(&t.to_ascii_lowercase()) {
            hits += 1;
        }
    }
    hits as f64 / q.len() as f64
}

/// Thread count for semantic embed phase (bounded; mirrors index ingest).
fn embed_thread_count() -> usize {
    let env = super::budget::env_usize("FSZERO_INDEX_THREADS")
        .or_else(|| super::budget::env_usize("FSZERO_INGEST_THREADS"));
    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let default = std::cmp::min(DEFAULT_EMBED_THREADS, std::cmp::max(1, available / 2));
    env.unwrap_or(default).max(1)
}

fn load_or_none(recovery: &RecoveryStore, digest: &str) -> Option<Vec<f32>> {
    let key = emb_key(digest);
    recovery.expand(&key).and_then(|b| decode_payload(&b))
}

fn store_embedding(recovery: &mut RecoveryStore, digest: &str, values: &[f32]) {
    let key = emb_key(digest);
    recovery.put_key(&key, &encode_payload(values));
    let prov_key = format!("semantic/prov/v1/{digest}");
    recovery.put_key(&prov_key, EMBEDDER_ID.as_bytes());
}

/// `fs::read` on a FIFO/socket blocks; `stat` does not. Skip, do not refuse.
fn is_regular_file(path: &Path) -> bool {
    fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

/// Incremental semantic ingest for dirty/new files during `build_index`.
///
/// Reads CAS sequentially, embeds missing digests with bounded rayon, then
/// writes memoized vectors back. Feature-off builds never call this.
pub fn ingest_semantic_chunks(
    recovery: &mut RecoveryStore,
    _root: &Path,
    files: &[(std::path::PathBuf, String)],
) -> usize {
    if files.is_empty() {
        return 0;
    }

    let mut unique_chunks: HashMap<String, String> = HashMap::new();
    for (path, _file_key) in files {
        if !is_regular_file(path) {
            continue;
        }
        let Ok(bytes) = fs::read(path) else {
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        for chunk in chunk_text(&text) {
            let digest = digest_hex(chunk.as_bytes());
            unique_chunks
                .entry(digest)
                .or_insert_with(|| chunk.to_string());
        }
    }
    if unique_chunks.is_empty() {
        return 0;
    }

    let mut missing: Vec<(String, String)> = Vec::new();
    let mut hit = 0usize;
    for (digest, chunk) in unique_chunks {
        if load_or_none(recovery, &digest).is_some() {
            hit += 1;
        } else {
            missing.push((digest, chunk));
        }
    }

    if missing.is_empty() {
        return hit;
    }

    let threads = embed_thread_count();
    let embedder = HashEmbedder::default_256();
    let computed: Vec<(String, Vec<f32>)> = if threads <= 1 || missing.len() < 8 {
        missing
            .into_iter()
            .map(|(digest, chunk)| (digest, embedder.embed_sync(&chunk)))
            .collect()
    } else {
        use rayon::prelude::*;
        let run = || {
            missing
                .par_iter()
                .map(|(digest, chunk)| (digest.clone(), embedder.embed_sync(chunk)))
                .collect::<Vec<_>>()
        };
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map(|pool| pool.install(run))
            .unwrap_or_else(|_| run())
    };

    let n_new = computed.len();
    for (digest, values) in computed {
        store_embedding(recovery, &digest, &values);
    }
    hit + n_new
}

/// Per-hit tier after hybrid-local rerank.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveTier {
    Lexical,
    LexicalHash,
    Hash,
}

impl ResolveTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lexical => "lexical",
            Self::LexicalHash => "lexical+hash",
            Self::Hash => "hash",
        }
    }
}

/// Rerank top lexical survivors with frankensearch hash embeddings + lexical overlap.
///
/// Returns per-path tier provenance for resolve JSON.
pub fn hybrid_local_rerank(
    recovery: &mut RecoveryStore,
    root: &Path,
    intent: &str,
    ranked: &mut [(String, f64)],
) -> HashMap<String, ResolveTier> {
    let mut tiers: HashMap<String, ResolveTier> = HashMap::new();
    if ranked.is_empty() {
        return tiers;
    }

    let embedder = HashEmbedder::default_256();
    let query_vec = embedder.embed_sync(intent);
    let limit = ranked.len().min(20);

    for item in ranked.iter_mut().take(limit) {
        let path = root.join(&item.0);
        if !is_regular_file(&path) {
            tiers.insert(item.0.clone(), ResolveTier::Lexical);
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            tiers.insert(item.0.clone(), ResolveTier::Lexical);
            continue;
        };
        let Ok(text) = String::from_utf8(bytes) else {
            tiers.insert(item.0.clone(), ResolveTier::Lexical);
            continue;
        };

        let lex = lexical_overlap(intent, &text);
        let mut best_cos = 0.0f64;
        let mut seen: HashSet<String> = HashSet::new();
        for chunk in chunk_text(&text) {
            let digest = digest_hex(chunk.as_bytes());
            if !seen.insert(digest.clone()) {
                continue;
            }
            let vec = match load_or_none(recovery, &digest) {
                Some(v) => v,
                None => {
                    let v = embedder.embed_sync(chunk);
                    store_embedding(recovery, &digest, &v);
                    v
                }
            };
            best_cos = best_cos.max(cosine(&query_vec, &vec));
        }

        let hash_boost = best_cos.max(0.0);
        let lex_boost = lex;
        item.1 *= 1.0 + 0.45 * hash_boost + 0.25 * lex_boost;

        let tier = if hash_boost >= 0.15 && lex_boost >= 0.2 {
            ResolveTier::LexicalHash
        } else if hash_boost >= 0.15 {
            ResolveTier::Hash
        } else {
            ResolveTier::Lexical
        };
        tiers.insert(item.0.clone(), tier);
    }

    for item in ranked.iter().skip(limit) {
        tiers.entry(item.0.clone()).or_insert(ResolveTier::Lexical);
    }

    super::resolve_ident::sort_by_score_desc(ranked, |h| h.1);
    tiers
}

#[cfg(all(test, unix))]
mod fifo_tests {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    use std::sync::mpsc::{self, RecvTimeoutError};
    use std::time::Duration;

    const HANG_BUDGET: Duration = Duration::from_millis(1500);
    const NEEDLE: &str = "semantic_fifo_needle_unique";

    fn mkfifo(path: &Path) {
        let status = std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .expect("spawn mkfifo");
        assert!(
            status.success(),
            "mkfifo {} failed: {status}",
            path.display()
        );
    }

    fn within_timeout<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("semantic-local-fifo".into())
            .spawn(move || {
                let _ = tx.send(f());
            })
            .expect("spawn timeout worker");
        match rx.recv_timeout(HANG_BUDGET) {
            Ok(value) => value,
            Err(RecvTimeoutError::Timeout) => {
                panic!(
                    "timed out after {HANG_BUDGET:?}: semantic ingest/rank FIFO op hung instead of skipping"
                )
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("timeout worker panicked before returning a skip result")
            }
        }
    }

    fn assert_still_fifo(path: &Path) {
        let meta = fs::symlink_metadata(path).expect("fifo metadata");
        assert!(
            meta.file_type().is_fifo(),
            "{} must remain a FIFO",
            path.display()
        );
    }

    #[test]
    fn ingest_semantic_chunks_skips_fifo_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("hit.rs"), format!("fn {NEEDLE}() {{}}\n")).unwrap();
        let fifo = root.join("hang.rs");
        mkfifo(&fifo);

        let n = within_timeout({
            let root = root.clone();
            let fifo = fifo.clone();
            move || {
                let mut recovery = crate::RecoveryStore::new();
                let files = vec![
                    (root.join("hit.rs"), "hit.rs".to_string()),
                    (fifo, "hang.rs".to_string()),
                ];
                ingest_semantic_chunks(&mut recovery, &root, &files)
            }
        });
        assert!(
            n > 0,
            "ingest must skip FIFO and still process the regular file, n={n}"
        );
        assert_still_fifo(&fifo);
    }

    #[test]
    fn hybrid_local_rerank_skips_fifo_as_lexical_without_blocking() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::write(root.join("hit.rs"), format!("fn {NEEDLE}() {{}}\n")).unwrap();
        let fifo = root.join("hang.rs");
        mkfifo(&fifo);

        let (tiers, ranked) = within_timeout({
            let root = root.clone();
            move || {
                let mut recovery = crate::RecoveryStore::new();
                let mut ranked = vec![("hit.rs".to_string(), 1.0), ("hang.rs".to_string(), 0.9)];
                let tiers = hybrid_local_rerank(&mut recovery, &root, NEEDLE, &mut ranked);
                (tiers, ranked)
            }
        });
        assert_eq!(
            tiers.get("hang.rs"),
            Some(&ResolveTier::Lexical),
            "FIFO rank must lexical-fallback, tiers={tiers:?}"
        );
        assert!(
            tiers.contains_key("hit.rs"),
            "regular file must still be ranked, tiers={tiers:?}"
        );
        assert!(
            ranked.iter().any(|(p, _)| p == "hit.rs"),
            "regular file must remain in ranked, ranked={ranked:?}"
        );
        assert_still_fifo(&fifo);
    }
}
