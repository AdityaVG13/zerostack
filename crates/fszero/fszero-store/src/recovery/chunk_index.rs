use super::{RecoveryStore, int_col, sql_int, sql_text, text_col_opt};
use crate::cdc::{CdcChunk, content_defined_chunks};
use serde::{Deserialize, Serialize};

const SQL_SELECT_CHUNK: &str = "SELECT content_ref, len FROM chunk_blobs WHERE digest = ?1 LIMIT 1";
const SQL_INSERT_CHUNK: &str =
    "INSERT OR IGNORE INTO chunk_blobs (digest, content_ref, len) VALUES (?1, ?2, ?3)";
const SQL_DELETE_FILE_CHUNKS: &str = "DELETE FROM file_chunks WHERE path = ?1";
const SQL_INSERT_FILE_CHUNK: &str = "INSERT INTO file_chunks (path, ordinal, start_byte, end_byte, digest, content_ref) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
const SQL_SELECT_FILE_CHUNKS: &str = "SELECT ordinal, start_byte, end_byte, digest, content_ref FROM file_chunks WHERE path = ?1 ORDER BY ordinal ASC";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredChunk {
    pub ordinal: u32,
    pub start: u64,
    pub end: u64,
    pub digest: String,
    pub content_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkInvalidation {
    pub start: u64,
    pub end: u64,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkIndexReport {
    pub chunks: Vec<StoredChunk>,
    pub invalidated: Vec<ChunkInvalidation>,
    pub dedup_hits: usize,
    pub inserted_chunks: usize,
}

impl RecoveryStore {
    pub fn chunk_manifest(&self, path: &str) -> Result<Vec<StoredChunk>, String> {
        let rows = self
            .conn
            .query_with_params(SQL_SELECT_FILE_CHUNKS, &[sql_text(path)])
            .map_err(|error| format!("chunk manifest query failed: {error}"))?;
        let manifest = rows
            .into_iter()
            .map(|row| {
                let ordinal = u32::try_from(int_col(&row, 0))
                    .map_err(|_| "chunk ordinal out of range".to_string())?;
                let start = u64::try_from(int_col(&row, 1))
                    .map_err(|_| "chunk start out of range".to_string())?;
                let end = u64::try_from(int_col(&row, 2))
                    .map_err(|_| "chunk end out of range".to_string())?;
                let digest =
                    text_col_opt(&row, 3).ok_or_else(|| "chunk digest is not text".to_string())?;
                let content_ref = text_col_opt(&row, 4)
                    .ok_or_else(|| "chunk content ref is not text".to_string())?;
                Ok(StoredChunk {
                    ordinal,
                    start,
                    end,
                    digest,
                    content_ref,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut expected_start = 0u64;
        for (ordinal, chunk) in manifest.iter().enumerate() {
            if chunk.ordinal as usize != ordinal
                || chunk.start != expected_start
                || chunk.end <= chunk.start
                || chunk.digest.len() != 64
                || !chunk
                    .digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                || chunk.content_ref != format!("fz://blob/{}", chunk.digest)
            {
                return Err("chunk manifest invariant violation".to_string());
            }
            expected_start = chunk.end;
        }
        Ok(manifest)
    }

    /// Drop chunk-manifest rows for a path that no longer exists in the tree.
    /// Content-addressed `chunk_blobs` stay; pack/CAS GC reclaims unreferenced
    /// bytes later. Called from incremental index when a file is removed.
    pub fn clear_file_chunks(&mut self, path: &str) -> Result<usize, String> {
        if path.is_empty() {
            return Err("chunk index path is empty".to_string());
        }
        self.conn
            .execute_with_params(SQL_DELETE_FILE_CHUNKS, &[sql_text(path)])
            .map_err(|error| format!("chunk manifest clear failed: {error}"))?;
        Ok(1)
    }

    pub fn index_file_chunks(
        &mut self,
        path: &str,
        bytes: &[u8],
    ) -> Result<ChunkIndexReport, String> {
        if path.is_empty() {
            return Err("chunk index path is empty".to_string());
        }
        let old_manifest = self.chunk_manifest(path)?;
        let chunks = content_defined_chunks(bytes);
        let invalidated_chunks = chunk_invalidations_from_manifest(&old_manifest, &chunks);
        let mut stored = Vec::with_capacity(chunks.len());
        let mut dedup_hits = 0usize;
        let mut inserted_chunks = 0usize;

        for (ordinal, chunk) in chunks.iter().copied().enumerate() {
            let digest = chunk.digest_hex();
            let existing = self
                .conn
                .query_with_params(SQL_SELECT_CHUNK, &[sql_text(&digest)])
                .map_err(|error| format!("chunk lookup failed: {error}"))?
                .into_iter()
                .next()
                .map(|row| {
                    let content_ref = text_col_opt(&row, 0)
                        .ok_or_else(|| "stored chunk content ref is not text".to_string())?;
                    let len = usize::try_from(int_col(&row, 1))
                        .map_err(|_| "stored chunk length out of range".to_string())?;
                    Ok::<_, String>((content_ref, len))
                })
                .transpose()?;
            let content_ref = self.try_put_content_ref(chunk.bytes(bytes))?;
            if let Some((stored_ref, stored_len)) = &existing
                && (stored_ref != &content_ref || *stored_len != chunk.len())
            {
                return Err("stored chunk identity mismatch".to_string());
            }
            let existed = existing.is_some();
            self.conn
                .execute_with_params(
                    SQL_INSERT_CHUNK,
                    &[
                        sql_text(&digest),
                        sql_text(&content_ref),
                        sql_int(chunk.len() as i64),
                    ],
                )
                .map_err(|error| format!("chunk blob index failed: {error}"))?;
            if existed {
                dedup_hits += 1;
            } else {
                inserted_chunks += 1;
            }
            stored.push(StoredChunk {
                ordinal: u32::try_from(ordinal)
                    .map_err(|_| "chunk ordinal overflow".to_string())?,
                start: chunk.start as u64,
                end: chunk.end as u64,
                digest,
                content_ref,
            });
        }

        self.conn
            .execute("BEGIN IMMEDIATE")
            .map_err(|error| format!("chunk manifest begin failed: {error}"))?;
        let result = (|| {
            self.conn
                .execute_with_params(SQL_DELETE_FILE_CHUNKS, &[sql_text(path)])
                .map_err(|error| format!("chunk manifest clear failed: {error}"))?;
            for chunk in &stored {
                self.conn
                    .execute_with_params(
                        SQL_INSERT_FILE_CHUNK,
                        &[
                            sql_text(path),
                            sql_int(i64::from(chunk.ordinal)),
                            sql_int(
                                i64::try_from(chunk.start)
                                    .map_err(|_| "chunk start overflow".to_string())?,
                            ),
                            sql_int(
                                i64::try_from(chunk.end)
                                    .map_err(|_| "chunk end overflow".to_string())?,
                            ),
                            sql_text(&chunk.digest),
                            sql_text(&chunk.content_ref),
                        ],
                    )
                    .map_err(|error| format!("chunk manifest insert failed: {error}"))?;
            }
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => {
                if let Err(error) = self.conn.execute("COMMIT") {
                    let _ = self.conn.execute("ROLLBACK");
                    return Err(format!("chunk manifest commit failed: {error}"));
                }
                self.note_durable_mutation();
            }
            Err(error) => {
                let _ = self.conn.execute("ROLLBACK");
                return Err(error);
            }
        }
        Ok(ChunkIndexReport {
            chunks: stored,
            invalidated: invalidated_chunks,
            dedup_hits,
            inserted_chunks,
        })
    }
}

fn chunk_invalidations_from_manifest(
    old: &[StoredChunk],
    new: &[CdcChunk],
) -> Vec<ChunkInvalidation> {
    let mut available = std::collections::HashMap::<String, usize>::new();
    for chunk in new {
        *available.entry(chunk.digest_hex()).or_default() += 1;
    }
    old.iter()
        .filter_map(|chunk| {
            let count = available.entry(chunk.digest.clone()).or_default();
            if *count > 0 {
                *count -= 1;
                return None;
            }
            Some(ChunkInvalidation {
                start: chunk.start,
                end: chunk.end,
                digest: chunk.digest.clone(),
            })
        })
        .collect()
}
