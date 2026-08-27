//! Manifest-driven append-only recovery segments.
use crate::shared_cas::SharedCas;
use fs4::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
pub const SEGMENT_FORMAT_VERSION: u32 = 1;
pub const MANIFEST_VERSION: u32 = 1;
pub const DEFAULT_SEGMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAGIC: &[u8; 8] = b"TZSEG001";
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SegmentMigrationPhase {
    Legacy,
    Shadow,
    Active,
    Retired,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentDescriptor {
    pub generation: u64,
    pub data_file: String,
    pub index_file: String,
    pub written_bytes: u64,
    pub ref_count: u64,
    pub index_hash: String,
    pub sealed_at_epoch_ms: Option<u64>,
    pub min_lease_deadline_epoch_ms: Option<u64>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentManifest {
    pub manifest_version: u32,
    pub store_format_version: u32,
    pub generation: u64,
    pub segment_bytes: u64,
    pub phase: SegmentMigrationPhase,
    pub hot: SegmentDescriptor,
    pub cold: Vec<SegmentDescriptor>,
    #[serde(default)]
    pub checksum: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentEntry {
    pub ref_id: String,
    pub offset: u64,
    pub len: u64,
    pub sha256: String,
    pub lease_deadline_epoch_ms: u64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SegmentIndex {
    pub entries: BTreeMap<String, SegmentEntry>,
}
#[derive(Debug, Error)]
pub enum SegmentStoreError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid manifest")]
    InvalidManifest,
    #[error("invalid index: {0}")]
    InvalidIndex(String),
    #[error("corrupt payload: {0}")]
    CorruptPayload(String),
    #[error("entry is {entry_bytes} bytes, exceeding segment bound {segment_bytes}")]
    EntryTooLarge {
        entry_bytes: u64,
        segment_bytes: u64,
    },
    #[error("overflow")]
    Overflow,
    #[error("lock timeout")]
    LockTimeout,
}
#[derive(Debug)]
pub struct SegmentStore {
    cache_path: PathBuf,
    root: PathBuf,
    manifest: SegmentManifest,
    hot_index: SegmentIndex,
    cold_indexes: BTreeMap<u64, SegmentIndex>,
    shared_cas: Option<SharedCas>,
    segment_bytes_override: Option<u64>,
}
impl SegmentStore {
    pub fn manifest_path(cache: &Path) -> PathBuf {
        cache
            .parent()
            .unwrap_or(Path::new("."))
            .join("recovery.manifest.json")
    }
    pub fn exists(cache: &Path) -> bool {
        let p = Self::manifest_path(cache);
        p.is_file() || bak(&p).is_file()
    }
    pub fn create_shadow(
        cache: impl Into<PathBuf>,
        cas: Option<SharedCas>,
    ) -> Result<Self, SegmentStoreError> {
        let cache = cache.into();
        if crate::unexpanded_tilde_path(&cache) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpanded ~ store path: {}", cache.display()),
            )
            .into());
        }
        let root = cache.parent().unwrap_or(Path::new(".")).to_path_buf();
        fs::create_dir_all(&root)?;
        // Hold the store lock across create+publish so a concurrent open()
        // cannot treat the new hot segment as an orphan and unlink it.
        let _lock = Lock::get(lock_path(&cache))?;
        let mut s = Self {
            cache_path: cache,
            root,
            manifest: SegmentManifest {
                manifest_version: MANIFEST_VERSION,
                store_format_version: SEGMENT_FORMAT_VERSION,
                generation: 1,
                segment_bytes: DEFAULT_SEGMENT_BYTES,
                phase: SegmentMigrationPhase::Shadow,
                hot: desc(1),
                cold: vec![],
                checksum: String::new(),
            },
            hot_index: SegmentIndex::default(),
            cold_indexes: BTreeMap::new(),
            shared_cas: cas,
            segment_bytes_override: None,
        };
        s.init_hot()?;
        s.publish()?;
        Ok(s)
    }
    pub fn migrate_legacy(
        cache: impl Into<PathBuf>,
        legacy: &mut crate::RecoveryStore,
        cas: Option<SharedCas>,
    ) -> Result<Self, SegmentStoreError> {
        let cache = cache.into();
        if Self::exists(&cache) {
            return Self::open(cache, cas);
        }
        let mut store = Self::create_shadow(cache, cas)?;
        for ref_id in legacy.blob_ref_ids() {
            let expanded = legacy.expand(&ref_id, Some("raw"), None, None, None, None);
            if expanded.found {
                store.put(&ref_id, expanded.content.as_bytes(), u64::MAX)?;
            }
        }
        store.activate()?;
        Ok(store)
    }
    pub fn open(
        cache: impl Into<PathBuf>,
        cas: Option<SharedCas>,
    ) -> Result<Self, SegmentStoreError> {
        let cache = cache.into();
        if crate::unexpanded_tilde_path(&cache) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpanded ~ store path: {}", cache.display()),
            )
            .into());
        }
        let root = cache.parent().unwrap_or(Path::new(".")).to_path_buf();
        // Recovery can truncate the hot segment and rewrite its index, so it must
        // serialize with writers and use a manifest re-read under that lock.
        let _lock = Lock::get(lock_path(&cache))?;
        let manifest = load_manifest_fallback(&Self::manifest_path(&cache))?;
        cleanup_orphan_segments(&root, &manifest)?;
        let hot_index = load_or_recover_hot(&root, &manifest.hot)?;
        Ok(Self {
            cache_path: cache,
            root,
            manifest,
            hot_index,
            cold_indexes: BTreeMap::new(),
            shared_cas: cas,
            segment_bytes_override: None,
        })
    }
    pub fn manifest(&self) -> &SegmentManifest {
        &self.manifest
    }
    pub fn cold_indexes_loaded(&self) -> usize {
        self.cold_indexes.len()
    }
    pub fn set_segment_bytes(&mut self, n: u64) {
        let n = n.max(64);
        self.manifest.segment_bytes = n;
        self.segment_bytes_override = Some(n);
    }
    pub fn activate(&mut self) -> Result<(), SegmentStoreError> {
        self.phase(SegmentMigrationPhase::Active)
    }
    pub fn rollback(&mut self) -> Result<(), SegmentStoreError> {
        self.phase(SegmentMigrationPhase::Legacy)
    }
    pub fn retire(&mut self) -> Result<(), SegmentStoreError> {
        self.phase(SegmentMigrationPhase::Retired)
    }
    fn phase(&mut self, p: SegmentMigrationPhase) -> Result<(), SegmentStoreError> {
        let _l = Lock::get(lock_path(&self.cache_path))?;
        self.reload()?;
        self.manifest.phase = p;
        self.publish()
    }
    pub fn put(&mut self, r: &str, b: &[u8], lease: u64) -> Result<(), SegmentStoreError> {
        if r.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty segment ref").into());
        }
        if crate::unexpanded_tilde_path(&self.cache_path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unexpanded ~ store path: {}", self.cache_path.display()),
            )
            .into());
        }
        let _l = Lock::get(lock_path(&self.cache_path))?;
        self.reload()?;
        let content_hash = hash(b);
        let now = now_ms();
        if let Some(existing) = self.hot_index.entries.get(r).cloned()
            && existing.lease_deadline_epoch_ms > now
            && existing.len == b.len() as u64
            && existing.sha256 == content_hash
        {
            if lease > existing.lease_deadline_epoch_ms {
                self.hot_index
                    .entries
                    .get_mut(r)
                    .expect("entry was just found")
                    .lease_deadline_epoch_ms = lease;
                write_index(&self.root, &mut self.manifest.hot, &self.hot_index)?;
                self.manifest.hot.min_lease_deadline_epoch_ms = self
                    .hot_index
                    .entries
                    .values()
                    .map(|entry| entry.lease_deadline_epoch_ms)
                    .min();
                self.manifest.generation = next_generation(self.manifest.generation)?;
                self.publish()?;
            }
            return Ok(());
        }
        for index in (0..self.manifest.cold.len()).rev() {
            let descriptor = self.manifest.cold[index].clone();
            if !self.cold_indexes.contains_key(&descriptor.generation) {
                self.cold_indexes
                    .insert(descriptor.generation, load_index(&self.root, &descriptor)?);
            }
            let entries = self
                .cold_indexes
                .get_mut(&descriptor.generation)
                .expect("cold index was just loaded");
            let Some(existing) = entries.entries.get(r).cloned() else {
                continue;
            };
            if existing.lease_deadline_epoch_ms <= now
                || existing.len != b.len() as u64
                || existing.sha256 != content_hash
            {
                continue;
            }
            if lease > existing.lease_deadline_epoch_ms {
                entries
                    .entries
                    .get_mut(r)
                    .expect("entry was just found")
                    .lease_deadline_epoch_ms = lease;
                let descriptor = &mut self.manifest.cold[index];
                write_index(&self.root, descriptor, entries)?;
                descriptor.min_lease_deadline_epoch_ms = entries
                    .entries
                    .values()
                    .map(|entry| entry.lease_deadline_epoch_ms)
                    .min();
                self.manifest.generation = next_generation(self.manifest.generation)?;
                self.publish()?;
            }
            return Ok(());
        }
        let m = Meta {
            ref_id: r.into(),
            sha256: content_hash,
            lease_deadline_epoch_ms: lease,
        };
        let mb = serde_json::to_vec(&m)?;
        let segment_bytes = self.manifest.segment_bytes;
        let size = 12u64
            .checked_add(mb.len() as u64)
            .and_then(|n| n.checked_add(b.len() as u64))
            .ok_or(SegmentStoreError::EntryTooLarge {
                entry_bytes: u64::MAX,
                segment_bytes,
            })?;
        let entry_bytes =
            size.checked_add(MAGIC.len() as u64)
                .ok_or(SegmentStoreError::EntryTooLarge {
                    entry_bytes: u64::MAX,
                    segment_bytes,
                })?;
        if entry_bytes > segment_bytes {
            return Err(SegmentStoreError::EntryTooLarge {
                entry_bytes,
                segment_bytes,
            });
        }
        if self.manifest.hot.written_bytes > 8
            && self
                .manifest
                .hot
                .written_bytes
                .checked_add(size)
                .is_none_or(|next_hot| next_hot > segment_bytes)
        {
            self.seal_inner()?
        }
        let next_manifest = next_generation(self.manifest.generation)?;
        let p = self.root.join(&self.manifest.hot.data_file);
        let mut f = OpenOptions::new().append(true).read(true).open(p)?;
        let start = f.seek(SeekFrom::End(0))?;
        let meta_len = u32::try_from(mb.len()).map_err(|_| SegmentStoreError::Overflow)?;
        f.write_all(&meta_len.to_le_bytes())?;
        f.write_all(&(b.len() as u64).to_le_bytes())?;
        f.write_all(&mb)?;
        let offset = start
            .checked_add(12)
            .and_then(|n| n.checked_add(mb.len() as u64))
            .ok_or(SegmentStoreError::Overflow)?;
        f.write_all(b)?;
        f.sync_all()?;
        self.hot_index.entries.insert(
            r.into(),
            SegmentEntry {
                ref_id: r.into(),
                offset,
                len: b.len() as u64,
                sha256: m.sha256,
                lease_deadline_epoch_ms: lease,
            },
        );
        write_index(&self.root, &mut self.manifest.hot, &self.hot_index)?;
        self.manifest.hot.written_bytes = f.metadata()?.len();
        self.manifest.hot.ref_count = self.hot_index.entries.len() as u64;
        self.manifest.hot.min_lease_deadline_epoch_ms = self
            .hot_index
            .entries
            .values()
            .map(|e| e.lease_deadline_epoch_ms)
            .min();
        self.manifest.generation = next_manifest;
        self.publish()
    }
    pub fn expand(&mut self, r: &str) -> Result<Option<Vec<u8>>, SegmentStoreError> {
        // Recovery truncates the hot segment and eviction unlinks cold files
        // under this lock; a lockless read can observe a truncated or unlinked
        // payload. Reload so this handle cannot expand from a stale index.
        let _l = Lock::get(lock_path(&self.cache_path))?;
        self.reload()?;
        if let Some(e) = self.hot_index.entries.get(r) {
            return read_entry(&self.root.join(&self.manifest.hot.data_file), e).map(Some);
        }
        for i in (0..self.manifest.cold.len()).rev() {
            let d = self.manifest.cold[i].clone();
            if !self.cold_indexes.contains_key(&d.generation) {
                self.cold_indexes
                    .insert(d.generation, load_index(&self.root, &d)?);
            }
            if let Some(e) = self.cold_indexes[&d.generation].entries.get(r) {
                return read_entry(&self.root.join(&d.data_file), e).map(Some);
            }
        }
        Ok(None)
    }
    pub fn seal(&mut self) -> Result<(), SegmentStoreError> {
        let _l = Lock::get(lock_path(&self.cache_path))?;
        self.reload()?;
        self.seal_inner()?;
        self.publish()
    }
    pub fn evict_expired(&mut self, now: u64) -> Result<usize, SegmentStoreError> {
        let _l = Lock::get(lock_path(&self.cache_path))?;
        self.reload()?;
        let mut n = 0;
        let mut keep = vec![];
        let mut remove_after_publish = vec![];
        for d in self.manifest.cold.drain(..) {
            let idx = load_index(&self.root, &d)?;
            let expired = idx
                .entries
                .values()
                .all(|e| e.lease_deadline_epoch_ms <= now);
            let pinned = idx.entries.values().any(|e| {
                self.shared_cas
                    .as_ref()
                    .is_some_and(|c| portable_hash(&e.ref_id).is_some_and(|h| c.is_pinned(h)))
            });
            if expired && !pinned {
                remove_after_publish.push(d.clone());
                self.cold_indexes.remove(&d.generation);
                n += 1
            } else {
                keep.push(d)
            }
        }
        self.manifest.cold = keep;
        if n > 0 {
            self.manifest.generation = next_generation(self.manifest.generation)?;
            // Both durable manifests must stop naming a segment before either
            // file is unlinked. A crash before unlink leaves harmless orphans.
            self.publish()?;
            self.refresh_manifest_backup()?;
            for d in remove_after_publish {
                remove(&self.root.join(&d.data_file))?;
                remove(&self.root.join(&d.index_file))?;
            }
            sync_dir(&self.root)?;
        }
        Ok(n)
    }
    fn reload(&mut self) -> Result<(), SegmentStoreError> {
        if Self::exists(&self.cache_path) {
            let m = load_manifest_fallback(&Self::manifest_path(&self.cache_path))?;
            self.hot_index = load_or_recover_hot(&self.root, &m.hot)?;
            self.manifest = m;
            if let Some(segment_bytes) = self.segment_bytes_override {
                self.manifest.segment_bytes = segment_bytes;
            }
            self.cold_indexes.clear()
        }
        Ok(())
    }
    fn init_hot(&mut self) -> Result<(), SegmentStoreError> {
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(self.root.join(&self.manifest.hot.data_file))?;
        f.write_all(MAGIC)?;
        f.sync_all()?;
        write_index(&self.root, &mut self.manifest.hot, &self.hot_index)?;
        Ok(())
    }
    fn seal_inner(&mut self) -> Result<(), SegmentStoreError> {
        if self.hot_index.entries.is_empty() {
            return Ok(());
        }
        let next_segment = next_generation(self.manifest.hot.generation)?;
        let next_manifest = next_generation(self.manifest.generation)?;
        let mut d = self.manifest.hot.clone();
        d.sealed_at_epoch_ms = Some(now_ms());
        self.manifest.cold.push(d);
        self.manifest.hot = desc(next_segment);
        self.hot_index = SegmentIndex::default();
        self.init_hot()?;
        self.manifest.generation = next_manifest;
        Ok(())
    }
    fn refresh_manifest_backup(&self) -> Result<(), SegmentStoreError> {
        let p = Self::manifest_path(&self.cache_path);
        fs::copy(&p, bak(&p))?;
        File::open(bak(&p))?.sync_all()?;
        sync_dir(&self.root)?;
        Ok(())
    }

    fn publish(&mut self) -> Result<(), SegmentStoreError> {
        let p = Self::manifest_path(&self.cache_path);
        self.manifest.checksum = checksum(&self.manifest)?;
        let tmp = p.with_extension(format!("tmp-{}", std::process::id()));
        let result = (|| {
            let mut f = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&tmp)?;
            f.write_all(&serde_json::to_vec(&self.manifest)?)?;
            f.sync_all()?;
            drop(f);
            if p.is_file() {
                fs::copy(&p, bak(&p))?;
                File::open(bak(&p))?.sync_all()?
            }
            fs::rename(&tmp, &p)?;
            sync_dir(&self.root)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}
#[derive(Serialize, Deserialize)]
struct Meta {
    ref_id: String,
    sha256: String,
    lease_deadline_epoch_ms: u64,
}
fn desc(g: u64) -> SegmentDescriptor {
    SegmentDescriptor {
        generation: g,
        data_file: format!("recovery.{g}.segment"),
        index_file: format!("recovery.{g}.segment.index"),
        written_bytes: 8,
        ref_count: 0,
        index_hash: String::new(),
        sealed_at_epoch_ms: None,
        min_lease_deadline_epoch_ms: None,
    }
}
fn next_generation(generation: u64) -> Result<u64, SegmentStoreError> {
    generation.checked_add(1).ok_or(SegmentStoreError::Overflow)
}
fn hash(b: &[u8]) -> String {
    crate::migration::full_sha256_hex(b)
}

/// On-disk payload length as an allocation size. Uses the descriptor's written
/// bound rather than `DEFAULT_SEGMENT_BYTES`, so a configured larger segment
/// can still recover. `u64` → `usize` truncation is a hard reject.
fn recover_payload_len(dl: u64, written_bytes: u64) -> Option<usize> {
    let len = usize::try_from(dl).ok()?;
    (dl <= written_bytes).then_some(len)
}
fn checksum(m: &SegmentManifest) -> Result<String, serde_json::Error> {
    let mut c = m.clone();
    c.checksum.clear();
    Ok(hash(&serde_json::to_vec(&c)?))
}
fn load_manifest(p: &Path) -> Result<SegmentManifest, SegmentStoreError> {
    let m: SegmentManifest = serde_json::from_slice(&fs::read(p)?)?;
    if m.manifest_version != MANIFEST_VERSION
        || m.store_format_version != SEGMENT_FORMAT_VERSION
        || checksum(&m)? != m.checksum
    {
        return Err(SegmentStoreError::InvalidManifest);
    }
    Ok(m)
}
fn load_manifest_fallback(p: &Path) -> Result<SegmentManifest, SegmentStoreError> {
    load_manifest(p).or_else(|_| load_manifest(&bak(p)))
}

fn cleanup_orphan_segments(
    root: &Path,
    manifest: &SegmentManifest,
) -> Result<(), SegmentStoreError> {
    let mut live = std::collections::BTreeSet::new();
    for descriptor in std::iter::once(&manifest.hot).chain(manifest.cold.iter()) {
        live.insert(descriptor.data_file.as_str());
        live.insert(descriptor.index_file.as_str());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_data = name
            .strip_prefix("recovery.")
            .and_then(|rest| rest.strip_suffix(".segment"))
            .is_some_and(|generation| generation.parse::<u64>().is_ok());
        let is_index = name
            .strip_prefix("recovery.")
            .and_then(|rest| rest.strip_suffix(".segment.index"))
            .is_some_and(|generation| generation.parse::<u64>().is_ok());
        if (is_data || is_index) && !live.contains(name) {
            remove(&entry.path())?;
        }
    }
    sync_dir(root)?;
    Ok(())
}
fn bak(p: &Path) -> PathBuf {
    PathBuf::from(format!("{}.bak", p.display()))
}
fn lock_path(p: &Path) -> PathBuf {
    PathBuf::from(format!("{}.lock", p.display()))
}
fn write_index(
    root: &Path,
    d: &mut SegmentDescriptor,
    i: &SegmentIndex,
) -> Result<(), SegmentStoreError> {
    let b = serde_json::to_vec(i)?;
    d.index_hash = hash(&b);
    let p = root.join(&d.index_file);
    let t = p.with_extension("index.tmp");
    let result = (|| {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&t)?;
        f.write_all(&b)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&t, &p)?;
        sync_dir(root)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&t);
    }
    result
}
fn load_index(root: &Path, d: &SegmentDescriptor) -> Result<SegmentIndex, SegmentStoreError> {
    let b = fs::read(root.join(&d.index_file))?;
    if hash(&b) != d.index_hash {
        return Err(SegmentStoreError::InvalidIndex(d.index_file.clone()));
    }
    Ok(serde_json::from_slice(&b)?)
}
fn load_or_recover_hot(
    root: &Path,
    d: &SegmentDescriptor,
) -> Result<SegmentIndex, SegmentStoreError> {
    load_index(root, d).or_else(|_| recover(root, d))
}
fn recover(root: &Path, d: &SegmentDescriptor) -> Result<SegmentIndex, SegmentStoreError> {
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(&d.data_file))?;
    let mut magic = [0; 8];
    f.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(SegmentStoreError::InvalidIndex(d.data_file.clone()));
    }
    let mut idx = SegmentIndex::default();
    let mut valid = 8;
    loop {
        let start = f.stream_position()?;
        let mut a = [0; 4];
        if f.read_exact(&mut a).is_err() {
            break;
        }
        let ml = match usize::try_from(u32::from_le_bytes(a)) {
            Ok(ml) => ml,
            Err(_) => break,
        };
        let mut z = [0; 8];
        if f.read_exact(&mut z).is_err() {
            break;
        }
        let dl = u64::from_le_bytes(z);
        let Some(payload_len) = recover_payload_len(dl, d.written_bytes) else {
            break;
        };
        if ml > 1_048_576
            || start
                .checked_add(12)
                .and_then(|n| n.checked_add(ml as u64))
                .and_then(|n| n.checked_add(dl))
                .is_none_or(|end| end > d.written_bytes)
        {
            break;
        }
        let mut mb = vec![0; ml];
        if f.read_exact(&mut mb).is_err() {
            break;
        }
        let Ok(m) = serde_json::from_slice::<Meta>(&mb) else {
            break;
        };
        let mut b = vec![0; payload_len];
        if f.read_exact(&mut b).is_err() || hash(&b) != m.sha256 {
            break;
        }
        valid = f.stream_position()?;
        let Some(offset) = start.checked_add(12).and_then(|n| n.checked_add(ml as u64)) else {
            break;
        };
        idx.entries.insert(
            m.ref_id.clone(),
            SegmentEntry {
                ref_id: m.ref_id,
                offset,
                len: dl,
                sha256: m.sha256,
                lease_deadline_epoch_ms: m.lease_deadline_epoch_ms,
            },
        );
    }
    if f.metadata()?.len() != valid {
        f.set_len(valid)?;
        f.sync_all()?
    }
    let mut copy = d.clone();
    write_index(root, &mut copy, &idx)?;
    Ok(idx)
}
fn read_entry(p: &Path, e: &SegmentEntry) -> Result<Vec<u8>, SegmentStoreError> {
    let mut f = File::open(p)?;
    f.seek(SeekFrom::Start(e.offset))?;
    let len = usize::try_from(e.len).map_err(|_| SegmentStoreError::Overflow)?;
    let mut b = vec![0; len];
    f.read_exact(&mut b)?;
    if hash(&b) != e.sha256 {
        return Err(SegmentStoreError::CorruptPayload(e.ref_id.clone()));
    }
    Ok(b)
}
fn portable_hash(r: &str) -> Option<&str> {
    let h = r
        .strip_prefix("tz://blob/")
        .or_else(|| r.strip_prefix("fz://blob/"))
        .or_else(|| r.strip_prefix("gz://blob/"))?;
    zero_ref::is_full_lower_hex(h).then_some(h)
}
fn remove(p: &Path) -> io::Result<()> {
    match fs::remove_file(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        x => x,
    }
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}
fn sync_dir(p: &Path) -> io::Result<()> {
    match File::open(p) {
        Ok(f) => crate::tolerate_unsupported_sync(f.sync_all()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
struct Lock(File);
impl Lock {
    fn get(p: PathBuf) -> Result<Self, SegmentStoreError> {
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(p)?;
        for _ in 0..240 {
            if FileExt::try_lock(&f).is_ok() {
                return Ok(Self(f));
            }
            thread::sleep(Duration::from_millis(25))
        }
        Err(SegmentStoreError::LockTimeout)
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
