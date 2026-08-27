//! Installed dependency pack registry under `.graphzero/packs/`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InstalledPackRecord {
    pub pack_id: String,
    pub version: String,
    pub manifest_path: String,
    pub shard_dir: String,
    pub shard_count: u32,
    pub tier_a_coverage: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PackRegistry {
    pub packs: Vec<InstalledPackRecord>,
}

pub fn registry_path(store_root: &Path) -> PathBuf {
    store_root.join("packs").join("registry.json")
}

pub fn packs_root(store_root: &Path) -> PathBuf {
    store_root.join("packs")
}

impl PackRegistry {
    pub fn load(store_root: &Path) -> Result<Self> {
        let path = registry_path(store_root);
        if !path.is_file() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path).context("read pack registry")?;
        serde_json::from_str(&data).context("parse pack registry")
    }

    pub fn save(&self, store_root: &Path) -> Result<()> {
        let root = packs_root(store_root);
        fs::create_dir_all(&root)?;
        let path = registry_path(store_root);
        let tmp = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(self).context("encode pack registry")?;
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn find(&self, pack_id: &str) -> Option<&InstalledPackRecord> {
        self.packs.iter().find(|p| p.pack_id == pack_id)
    }

    pub fn remove(&mut self, pack_id: &str) -> bool {
        let before = self.packs.len();
        self.packs.retain(|p| p.pack_id != pack_id);
        self.packs.len() < before
    }
}
