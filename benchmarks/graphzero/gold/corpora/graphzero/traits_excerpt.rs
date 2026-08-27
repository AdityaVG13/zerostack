// Excerpt mirroring GraphZero's ExternalStore adapter pattern
// (crates/graphzero-store/src/store/expand.rs): a trait, a concrete
// implementation, an unimplemented trait, and an uncalled helper so the
// gold set carries implements edges and confirmed non-edges.

pub trait EvidenceStore {
    fn label(&self) -> &'static str;
    fn fetch(&self, key: &str) -> Option<Vec<u8>>;
}

pub struct DirEvidence {
    root: std::path::PathBuf,
}

impl EvidenceStore for DirEvidence {
    fn label(&self) -> &'static str {
        "dir"
    }

    fn fetch(&self, key: &str) -> Option<Vec<u8>> {
        std::fs::read(self.root.join(normalize_key(key))).ok()
    }
}

pub trait NeverImplemented {
    fn absent(&self);
}

fn normalize_key(key: &str) -> String {
    key.trim().to_ascii_lowercase()
}

fn never_called_helper() -> u32 {
    41
}

pub fn lookup(store: &DirEvidence, key: &str) -> Option<Vec<u8>> {
    let cleaned = normalize_key(key);
    store.fetch(&cleaned)
}
