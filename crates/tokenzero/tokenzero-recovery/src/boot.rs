//! O(1) session boot manifest and delta persistence.
//! Boot metadata stays separate from the recovery snapshot so session open never
//! deserializes the store or enumerates the repository.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokenzero_core::count_tokens;

const MANIFEST_VERSION: u32 = 1;
const DELTA_VERSION: u32 = 1;
const STORE_VERSION: u32 = 1;
const ID_HEX_LEN: usize = 12;
const EMPTY_ID: &str = "000000000000";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BootManifest {
    version: u32,
    root_digest: String,
    manifest_id: String,
    store_version: u32,
    toc_ref: String,
    working_set_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionDelta {
    version: u32,
    manifest_id: String,
    session_hwm: u64,
    #[serde(default)]
    added_refs: Vec<String>,
    #[serde(default)]
    changed_refs: Vec<String>,
    #[serde(default)]
    deleted_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootTokenComponents {
    pub manifest: usize,
    pub delta: usize,
    pub toc_working_set: usize,
    pub other: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionBoot {
    pub schema: &'static str,
    pub mode: &'static str,
    pub wire: String,
    pub manifest_id: String,
    pub delta_ref: String,
    pub manifest_path: PathBuf,
    pub delta_path: PathBuf,
    pub telemetry: BootTokenComponents,
}

/// Open a session without loading the recovery snapshot or walking the repo.
/// Missing metadata is initialized atomically. Unknown/older/newer/corrupt/
/// unreadable metadata is left untouched with a bounded legacy fallback.
pub fn open_session_boot(
    cache_path: &Path,
    root: &Path,
    allowed_roots: &[PathBuf],
) -> std::io::Result<SessionBoot> {
    let manifest_path = cache_path.with_file_name("boot-manifest.json");
    let delta_path = cache_path.with_file_name("boot-session-delta.json");
    let root_digest = root_digest(root, allowed_roots);

    let (manifest, delta, mode) = match load_manifest(&manifest_path) {
        MetadataLoad::Compatible(manifest) if manifest.root_digest == root_digest => {
            let (delta, mode) = take_delta(&delta_path, &manifest.manifest_id)?;
            (manifest, delta, mode)
        }
        MetadataLoad::Missing => {
            let manifest = new_manifest(root_digest);
            let (delta, mode) = take_delta(&delta_path, &manifest.manifest_id)?;
            if mode == "manifest_delta" {
                atomic_write_json(&manifest_path, &manifest)?;
            }
            (manifest, delta, mode)
        }
        MetadataLoad::Compatible(_) | MetadataLoad::Incompatible => {
            let manifest = new_manifest(root_digest);
            let delta = empty_delta(&manifest.manifest_id);
            (manifest, delta, "legacy_fallback")
        }
    };
    Ok(build_boot(mode, manifest, delta, manifest_path, delta_path))
}

/// Compatible → reuse; Missing → write empty; Incompatible → empty in-memory only.
fn take_delta(path: &Path, manifest_id: &str) -> std::io::Result<(SessionDelta, &'static str)> {
    match load_delta(path, manifest_id) {
        MetadataLoad::Compatible(delta) => Ok((delta, "manifest_delta")),
        MetadataLoad::Missing => {
            let delta = empty_delta(manifest_id);
            atomic_write_json(path, &delta)?;
            Ok((delta, "manifest_delta"))
        }
        MetadataLoad::Incompatible => Ok((empty_delta(manifest_id), "legacy_fallback")),
    }
}

fn build_boot(
    mode: &'static str,
    manifest: BootManifest,
    delta: SessionDelta,
    manifest_path: PathBuf,
    delta_path: PathBuf,
) -> SessionBoot {
    let delta_ref = delta_id(&delta);
    let mut wire = format!(
        "TZ/1 root={} m={} v={}",
        manifest.root_digest, manifest.manifest_id, manifest.store_version
    );
    let m = count_tokens(&wire);
    wire.push_str(&format!(" d={delta_ref}"));
    let d = count_tokens(&wire);
    wire.push_str(&format!(
        " toc={} ws={}",
        manifest.toc_ref, manifest.working_set_ref
    ));
    let t = count_tokens(&wire);
    if mode != "manifest_delta" {
        wire.push_str(" fallback=legacy");
    }
    let total = count_tokens(&wire);
    SessionBoot {
        schema: "tokenzero.session-boot.v1",
        mode,
        wire,
        manifest_id: manifest.manifest_id,
        delta_ref,
        manifest_path,
        delta_path,
        telemetry: BootTokenComponents {
            manifest: m,
            delta: d.saturating_sub(m),
            toc_working_set: t.saturating_sub(d),
            other: total.saturating_sub(t),
            total,
        },
    }
}

fn new_manifest(root_digest: String) -> BootManifest {
    let seed = format!(
        "v={MANIFEST_VERSION}|root={root_digest}|store={STORE_VERSION}|toc={EMPTY_ID}|ws={EMPTY_ID}"
    );
    BootManifest {
        version: MANIFEST_VERSION,
        root_digest,
        manifest_id: short_digest(seed.as_bytes()),
        store_version: STORE_VERSION,
        toc_ref: EMPTY_ID.into(),
        working_set_ref: EMPTY_ID.into(),
    }
}

fn empty_delta(manifest_id: &str) -> SessionDelta {
    SessionDelta {
        version: DELTA_VERSION,
        manifest_id: manifest_id.into(),
        session_hwm: 0,
        added_refs: Vec::new(),
        changed_refs: Vec::new(),
        deleted_refs: Vec::new(),
    }
}

enum MetadataLoad<T> {
    Missing,
    Compatible(T),
    Incompatible,
}

fn read_metadata(path: &Path) -> MetadataLoad<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => MetadataLoad::Compatible(bytes),
        Err(e) if e.kind() == ErrorKind::NotFound => MetadataLoad::Missing,
        Err(_) => MetadataLoad::Incompatible,
    }
}

fn parse_metadata<T>(path: &Path, parse: impl FnOnce(&[u8]) -> Option<T>) -> MetadataLoad<T> {
    match read_metadata(path) {
        MetadataLoad::Missing => MetadataLoad::Missing,
        MetadataLoad::Incompatible => MetadataLoad::Incompatible,
        MetadataLoad::Compatible(bytes) => parse(&bytes)
            .map(MetadataLoad::Compatible)
            .unwrap_or(MetadataLoad::Incompatible),
    }
}

fn load_manifest(path: &Path) -> MetadataLoad<BootManifest> {
    parse_metadata(path, |bytes| {
        let m = serde_json::from_slice::<BootManifest>(bytes).ok()?;
        (m.version == MANIFEST_VERSION
            && m.store_version == STORE_VERSION
            && [
                &m.root_digest,
                &m.manifest_id,
                &m.toc_ref,
                &m.working_set_ref,
            ]
            .into_iter()
            .all(|id| is_fixed_id(id)))
        .then_some(m)
    })
}

fn load_delta(path: &Path, manifest_id: &str) -> MetadataLoad<SessionDelta> {
    parse_metadata(path, |bytes| {
        let d = serde_json::from_slice::<SessionDelta>(bytes).ok()?;
        let refs_ok = d
            .added_refs
            .iter()
            .chain(&d.changed_refs)
            .chain(&d.deleted_refs)
            .all(|r| is_fixed_id(r));
        (d.version == DELTA_VERSION && d.manifest_id == manifest_id && refs_ok).then_some(d)
    })
}

fn delta_id(delta: &SessionDelta) -> String {
    if delta.session_hwm == 0
        && delta.added_refs.is_empty()
        && delta.changed_refs.is_empty()
        && delta.deleted_refs.is_empty()
    {
        return EMPTY_ID.into();
    }
    serde_json::to_vec(delta)
        .map(|b| short_digest(&b))
        .unwrap_or_else(|_| EMPTY_ID.into())
}

fn root_digest(root: &Path, allowed_roots: &[PathBuf]) -> String {
    let mut roots: Vec<_> = allowed_roots.iter().map(|p| normalize(p)).collect();
    roots.push(normalize(root));
    roots.sort();
    roots.dedup();
    short_digest(roots.join("\n").as_bytes())
}

fn normalize(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_fixed_id(value: &str) -> bool {
    value.len() == ID_HEX_LEN && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn short_digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)[..ID_HEX_LEN / 2]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
    zero_store::atomic_write_file(path, &body)
}
