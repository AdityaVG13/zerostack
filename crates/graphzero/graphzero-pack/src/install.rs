//! Verify signature, dedup blobs/shards, register pack in store.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ed25519_dalek::VerifyingKey;
use graphzero_store::ContentHash;
use graphzero_store::store::blob_store::BlobStore;
use graphzero_store::store::pack_registry::{InstalledPackRecord, PackRegistry};

use crate::build::verify_pack_artifacts;
use crate::manifest::PackManifest;
use crate::semantic_index::install_semantic_sidecars;
use crate::sign::verify_manifest_signature;
use graphzero_store::store::path_safety::validate_pack_path_component;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallReport {
    pub pack_id: String,
    pub shard_count: u32,
    pub blobs_linked: u32,
    pub blobs_skipped_dedup: u32,
}

pub fn install_pack(
    store_root: &Path,
    manifest_path: &Path,
    verify_key: &VerifyingKey,
) -> Result<InstallReport> {
    install_pack_with_before_register(store_root, manifest_path, verify_key, || Ok(()))
}

fn install_pack_with_before_register<F>(
    store_root: &Path,
    manifest_path: &Path,
    verify_key: &VerifyingKey,
    before_register: F,
) -> Result<InstallReport>
where
    F: FnOnce() -> Result<()>,
{
    let (manifest, pack_root) = load_and_validate_pack(manifest_path, verify_key)?;
    let mut registry = PackRegistry::load(store_root)?;
    ensure_pack_not_installed(&registry, &manifest.pack_id, &manifest.version)?;

    let (linked, skipped) = dedup_link_pack_blobs(store_root, &pack_root)?;
    let dest_pack_dir = materialize_pack_tree(store_root, &pack_root, manifest_path, &manifest)?;
    if let Err(error) = before_register().and_then(|()| {
        register_installed_pack(&mut registry, store_root, &manifest, &dest_pack_dir)
    }) {
        fs::remove_dir_all(&dest_pack_dir).with_context(|| {
            format!(
                "roll back failed pack install directory {} after: {error:#}",
                dest_pack_dir.display()
            )
        })?;
        return Err(error);
    }

    Ok(install_report_from(&manifest, linked, skipped))
}

pub fn list_installed(store_root: &Path) -> Result<Vec<InstalledPackRecord>> {
    Ok(PackRegistry::load(store_root)?.packs)
}

pub fn uninstall_pack(store_root: &Path, pack_id: &str) -> Result<bool> {
    let mut registry = PackRegistry::load(store_root)?;
    let Some(rec) = registry
        .packs
        .iter()
        .find(|p| p.pack_id == pack_id)
        .cloned()
    else {
        return Ok(false);
    };
    registry.remove(pack_id);
    registry.save(store_root)?;
    if let Some(d) = PathBuf::from(&rec.shard_dir).parent() {
        let _ = fs::remove_dir_all(d);
    }
    Ok(true)
}

fn load_and_validate_pack(
    manifest_path: &Path,
    verify_key: &VerifyingKey,
) -> Result<(PackManifest, PathBuf)> {
    let manifest = PackManifest::read_json(manifest_path)?;
    validate_pack_path_component(&manifest.pack_id, "pack_id")?;
    validate_pack_path_component(&manifest.version, "version")?;
    for shard in &manifest.shards {
        validate_pack_path_component(&shard.file_name, "shard file_name")?;
    }
    for sidecar in &manifest.semantic_sidecars {
        validate_pack_path_component(&sidecar.gzsh_file_name, "semantic gzsh_file_name")?;
        validate_pack_path_component(&sidecar.file_name, "semantic file_name")?;
    }
    verify_manifest_signature(&manifest, verify_key)?;
    let pack_root = manifest_path
        .parent()
        .context("manifest has no parent directory")?
        .to_path_buf();
    verify_pack_artifacts(&pack_root, &manifest)?;
    Ok((manifest, pack_root))
}

fn install_report_from(manifest: &PackManifest, linked: u32, skipped: u32) -> InstallReport {
    InstallReport {
        pack_id: manifest.pack_id.clone(),
        shard_count: manifest.shards.len() as u32,
        blobs_linked: linked,
        blobs_skipped_dedup: skipped,
    }
}

fn dedup_link_pack_blobs(store_root: &Path, pack_root: &Path) -> Result<(u32, u32)> {
    let blob_store = BlobStore::open(store_root)?;
    let mut linked = 0u32;
    let mut skipped = 0u32;
    install_pack_blobs(pack_root, &blob_store, &mut linked, &mut skipped)?;
    Ok((linked, skipped))
}

fn materialize_pack_tree(
    store_root: &Path,
    pack_root: &Path,
    manifest_path: &Path,
    manifest: &PackManifest,
) -> Result<PathBuf> {
    let dest_pack_dir = prepare_dest_pack_dir(store_root, manifest)?;
    copy_shards_into_store(pack_root, &dest_pack_dir, manifest)?;
    install_semantic_sidecars(pack_root, &dest_pack_dir.join("shards"), manifest)?;
    fs::copy(manifest_path, dest_pack_dir.join("manifest.json"))
        .context("copy installed manifest")?;
    Ok(dest_pack_dir)
}

fn ensure_pack_not_installed(registry: &PackRegistry, pack_id: &str, version: &str) -> Result<()> {
    if registry
        .packs
        .iter()
        .any(|pack| pack.pack_id == pack_id && pack.version == version)
    {
        bail!("pack {pack_id}@{version} already installed");
    }
    Ok(())
}

fn prepare_dest_pack_dir(store_root: &Path, manifest: &PackManifest) -> Result<PathBuf> {
    let dest_pack_dir = graphzero_store::store::pack_registry::packs_root(store_root)
        .join(&manifest.pack_id)
        .join(&manifest.version);
    if dest_pack_dir.exists() {
        fs::remove_dir_all(&dest_pack_dir).with_context(|| {
            format!(
                "remove existing pack install directory {}",
                dest_pack_dir.display()
            )
        })?;
    }
    fs::create_dir_all(dest_pack_dir.join("shards"))
        .with_context(|| format!("create pack install directory {}", dest_pack_dir.display()))?;
    Ok(dest_pack_dir)
}

fn copy_shards_into_store(
    pack_root: &Path,
    dest_pack_dir: &Path,
    manifest: &PackManifest,
) -> Result<()> {
    for shard in &manifest.shards {
        let src = pack_root.join("shards").join(&shard.file_name);
        let dst = dest_pack_dir.join("shards").join(&shard.file_name);
        link_or_copy(&src, &dst)?;
    }
    Ok(())
}

fn register_installed_pack(
    registry: &mut PackRegistry,
    store_root: &Path,
    manifest: &PackManifest,
    dest_pack_dir: &Path,
) -> Result<()> {
    registry.packs.push(InstalledPackRecord {
        pack_id: manifest.pack_id.clone(),
        version: manifest.version.clone(),
        manifest_path: dest_pack_dir.join("manifest.json").display().to_string(),
        shard_dir: dest_pack_dir.join("shards").display().to_string(),
        shard_count: manifest.shards.len() as u32,
        tier_a_coverage: manifest.tier_a_coverage,
    });
    registry.save(store_root)
}

fn link_or_copy(src: &Path, dst: &Path) -> Result<()> {
    fs::copy(src, dst).context("copy shard into store")?;
    Ok(())
}

fn install_pack_blobs(
    pack_root: &Path,
    blob_store: &BlobStore,
    linked: &mut u32,
    skipped: &mut u32,
) -> Result<()> {
    let blobs_dir = pack_root.join("blobs");
    if !blobs_dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&blobs_dir)? {
        let path = entry?.path();
        link_or_skip_pack_blob(&path, blob_store, linked, skipped)?;
    }
    Ok(())
}

fn link_or_skip_pack_blob(
    path: &Path,
    blob_store: &BlobStore,
    linked: &mut u32,
    skipped: &mut u32,
) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let bytes = fs::read(path)?;
    let hash = ContentHash::of(&bytes);
    if blob_store.get(&hash)?.is_some() {
        *skipped += 1;
        return Ok(());
    }
    blob_store.put(&bytes)?;
    *linked += 1;
    Ok(())
}
