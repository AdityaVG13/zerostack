use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// `tz_fetch`'s TTL index: url → (blob ref, fetch time). Lives beside the
/// recovery cache; bodies themselves are in the content-addressed store.
/// All IO here is fail-open — a lost index only costs a re-fetch.
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct FetchIndex {
    #[serde(default)]
    pub entries: BTreeMap<String, FetchIndexEntry>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct FetchIndexEntry {
    pub blob_ref: String,
    pub fetched_at_secs: u64,
    pub bytes: usize,
}

pub(crate) fn fetch_index_path(cache_path: &Path) -> PathBuf {
    cache_path
        .parent()
        .map(|dir| dir.join("fetch-cache.json"))
        .unwrap_or_else(|| PathBuf::from("fetch-cache.json"))
}

pub fn load_fetch_index(path: &Path) -> FetchIndex {
    fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub(crate) fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn record_fetch(path: &Path, url: &str, blob_ref: &str, bytes: usize) {
    const MAX_FETCH_INDEX_ENTRIES: usize = 200;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // Hold an exclusive advisory lock across the whole read-modify-write so
    // two concurrent fetches cannot each load the same index and clobber the
    // other's entry. Fail-open: a lock we cannot take only costs a re-fetch.
    let Some(_lock) = FetchIndexLock::acquire(path) else {
        return;
    };
    let mut index = load_fetch_index(path);
    index.entries.insert(
        url.to_string(),
        FetchIndexEntry {
            blob_ref: blob_ref.to_string(),
            fetched_at_secs: epoch_secs(),
            bytes,
        },
    );
    while index.entries.len() > MAX_FETCH_INDEX_ENTRIES {
        let oldest = index
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.fetched_at_secs)
            .map(|(url, _)| url.clone());
        match oldest {
            Some(url) => {
                index.entries.remove(&url);
            }
            None => break,
        }
    }
    let _ = atomic_write_fetch_index(path, &index);
}

fn atomic_write_fetch_index(path: &Path, index: &FetchIndex) -> std::io::Result<()> {
    let serialized = serde_json::to_string(index).map_err(std::io::Error::other)?;
    zero_store::atomic_write_file(path, serialized.as_bytes())
}

/// RAII exclusive lock over a sibling lock file for the fetch index.
struct FetchIndexLock {
    _file: fs::File,
}

impl FetchIndexLock {
    fn acquire(index_path: &Path) -> Option<Self> {
        use fs4::FileExt;
        let parent = index_path.parent().unwrap_or_else(|| Path::new("."));
        let lock_path = parent.join("fetch-cache.json.lock");
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .ok()?;
        const LOCK_ATTEMPTS: usize = 300;
        for attempt in 0..LOCK_ATTEMPTS {
            match FileExt::try_lock(&file) {
                Ok(()) => return Some(Self { _file: file }),
                Err(_) if attempt + 1 < LOCK_ATTEMPTS => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return None,
            }
        }
        None
    }
}

impl Drop for FetchIndexLock {
    fn drop(&mut self) {
        use fs4::FileExt;
        let _ = FileExt::unlock(&self._file);
    }
}
