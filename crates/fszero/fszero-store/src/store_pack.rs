//! Warm-store pack export/import. Packs hub-owned content-addressed blobs
//! (`blobs/sha256/...`) into a portable directory plus a manifest. Engine-local
//! mutation_log / access_log / ref-index are never packaged (RACC-R authority: hub CAS identity only).

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

use super::cas::CasStore;

pub const PACK_MANIFEST_NAME: &str = "fszero-store-pack.json";
pub const PACK_SCHEMA: &str = "fszero.store-pack";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorePackError {
    Io(String),
    Manifest(String),
    Tampered { hash: String, detail: String },
    MissingObject(String),
}

impl std::fmt::Display for StorePackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(s) => write!(f, "store pack io: {s}"),
            Self::Manifest(s) => write!(f, "store pack manifest: {s}"),
            Self::Tampered { hash, detail } => {
                write!(f, "tampered pack object sha256/{hash}: {detail}")
            }
            Self::MissingObject(h) => write!(f, "missing pack object sha256/{h}"),
        }
    }
}
impl std::error::Error for StorePackError {}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn content_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex_encode(h.finalize().as_slice())
}

/// Export every object from a CAS blobs root into `pack_dir`.
pub fn export_cas_pack(cas: &CasStore, pack_dir: &Path) -> Result<usize, StorePackError> {
    fs::create_dir_all(pack_dir).map_err(|e| StorePackError::Io(e.to_string()))?;
    let objects_dir = pack_dir.join("objects");
    fs::create_dir_all(&objects_dir).map_err(|e| StorePackError::Io(e.to_string()))?;
    let mut hashes = Vec::new();
    // Walk blobs/sha256/*/*
    let root = cas.blobs_root();
    let sha_root = root.join("sha256");
    if sha_root.is_dir() {
        for shard in fs::read_dir(&sha_root).map_err(|e| StorePackError::Io(e.to_string()))? {
            let shard = shard.map_err(|e| StorePackError::Io(e.to_string()))?;
            if !shard.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            for ent in fs::read_dir(shard.path()).map_err(|e| StorePackError::Io(e.to_string()))? {
                let ent = ent.map_err(|e| StorePackError::Io(e.to_string()))?;
                let name = ent.file_name().to_string_lossy().into_owned();
                if name.len() != 64 {
                    continue;
                }
                let bytes = fs::read(ent.path()).map_err(|e| StorePackError::Io(e.to_string()))?;
                let actual = content_hash(&bytes);
                if actual != name {
                    return Err(StorePackError::Tampered {
                        hash: name,
                        detail: format!("on-disk digest {actual}"),
                    });
                }
                let dest = objects_dir.join(&name);
                fs::write(&dest, &bytes).map_err(|e| StorePackError::Io(e.to_string()))?;
                hashes.push(name);
            }
        }
    }
    hashes.sort();
    let manifest = serde_json::json!({
        "schema": PACK_SCHEMA,
        "object_count": hashes.len(),
        "objects": hashes,
    });
    let man_path = pack_dir.join(PACK_MANIFEST_NAME);
    fs::write(
        &man_path,
        serde_json::to_string_pretty(&manifest).expect("manifest"),
    )
    .map_err(|e| StorePackError::Io(e.to_string()))?;
    Ok(hashes.len())
}

/// Import pack into CAS; verify every object; fail loud on tamper.
pub fn import_cas_pack(cas: &CasStore, pack_dir: &Path) -> Result<usize, StorePackError> {
    let man_path = pack_dir.join(PACK_MANIFEST_NAME);
    let man_bytes = fs::read(&man_path).map_err(|e| StorePackError::Io(e.to_string()))?;
    let man: serde_json::Value =
        serde_json::from_slice(&man_bytes).map_err(|e| StorePackError::Manifest(e.to_string()))?;
    if man.get("schema").and_then(|s| s.as_str()) != Some(PACK_SCHEMA) {
        return Err(StorePackError::Manifest(format!(
            "unexpected schema {:?}",
            man.get("schema")
        )));
    }
    let objects = man
        .get("objects")
        .and_then(|o| o.as_array())
        .ok_or_else(|| StorePackError::Manifest("objects array required".into()))?;
    let objects_dir = pack_dir.join("objects");
    let mut n = 0usize;
    for obj in objects {
        let hash = obj
            .as_str()
            .ok_or_else(|| StorePackError::Manifest("object hash must be string".into()))?;
        let path = objects_dir.join(hash);
        if !path.is_file() {
            return Err(StorePackError::MissingObject(hash.to_string()));
        }
        let bytes = fs::read(&path).map_err(|e| StorePackError::Io(e.to_string()))?;
        let actual = content_hash(&bytes);
        if actual != hash {
            return Err(StorePackError::Tampered {
                hash: hash.to_string(),
                detail: format!("pack digest {actual}"),
            });
        }
        cas.put_prehashed(hash, &bytes)
            .map_err(|e| StorePackError::Io(e.to_string()))?;
        n += 1;
    }
    Ok(n)
}
