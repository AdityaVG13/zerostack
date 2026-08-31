//! Trigram extraction and postings.

use std::collections::BTreeMap;

use super::format::{TrigramPosting, pack_trigram};

/// Extract distinct trigrams from blob content, recording the first byte
/// offset of each. Deterministic: BTreeMap keyed by packed trigram.
pub fn extract_trigrams(content: &[u8]) -> BTreeMap<u32, u32> {
    let mut out = BTreeMap::new();
    if content.len() < 3 {
        return out;
    }
    for i in 0..content.len() - 2 {
        let t = pack_trigram([content[i], content[i + 1], content[i + 2]]);
        out.entry(t).or_insert(i as u32);
    }
    out
}

/// Build postings for one blob, identified by its index into the shard's
/// coverage blob table.
pub fn postings_for_blob(blob_idx: u32, content: &[u8]) -> Vec<TrigramPosting> {
    let mut postings: Vec<TrigramPosting> = extract_trigrams(content)
        .into_iter()
        .map(|(trigram, offset)| TrigramPosting {
            trigram,
            blob_idx,
            offset,
        })
        .collect();
    sort_postings(&mut postings);
    postings
}

/// Deterministic shard posting order: trigram key, then blob_idx, then offset.
pub fn sort_postings(postings: &mut [TrigramPosting]) {
    postings.sort_by(|a, b| {
        let at = { a.trigram };
        let bt = { b.trigram };
        let abi = { a.blob_idx };
        let bbi = { b.blob_idx };
        let ao = { a.offset };
        let bo = { b.offset };
        at.cmp(&bt).then(abi.cmp(&bbi)).then(ao.cmp(&bo))
    });
}

/// Search sorted postings for an exact trigram (3 ASCII bytes).
pub fn find_trigram(
    postings: &[TrigramPosting],
    needle: [u8; 3],
) -> impl Iterator<Item = &TrigramPosting> {
    let key = pack_trigram(needle);
    postings.iter().filter(move |p| {
        let t = p.trigram;
        t == key
    })
}
