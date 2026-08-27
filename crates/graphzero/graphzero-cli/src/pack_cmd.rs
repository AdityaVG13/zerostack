//! P5.5 `graphzero pack` subcommands.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Subcommand;
use graphzero_pack::{
    PackManifest, PackSignKey, build_fixture_pack, install_pack, list_installed, uninstall_pack,
};

#[derive(Subcommand)]
pub enum PackCommand {
    /// List installed dependency packs
    List {
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Install a pack from manifest.json (verify signature)
    Install {
        manifest: PathBuf,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
    },
    /// Verify manifest signature and shard hashes
    Verify { manifest: PathBuf },
    /// Build fixture pack (dev/test)
    #[command(hide = true)]
    BuildFixture {
        #[arg(long)]
        out: PathBuf,
    },
    /// Remove pack registration (retains shared blobs)
    Uninstall {
        pack_id: String,
        #[arg(long, default_value = ".")]
        repo: PathBuf,
        /// Preview removals without deleting anything
        #[arg(long)]
        dry_run: bool,
    },
}

fn store_root(repo: &Path) -> PathBuf {
    graphzero_store::resolve_graphzero_store_root(repo)
}

fn fixture_sign_key() -> PackSignKey {
    PackSignKey::fixture()
}

fn run_list(repo: PathBuf) -> Result<()> {
    let root = store_root(&repo);
    let packs = list_installed(&root)?;
    println!("{{\"packs\":[");
    for (i, p) in packs.iter().enumerate() {
        if i > 0 {
            print!(",");
        }
        println!(
            "{{\"pack_id\":\"{}\",\"version\":\"{}\",\"shard_count\":{}}}",
            p.pack_id, p.version, p.shard_count
        );
    }
    println!("]}}");
    Ok(())
}

fn run_install(manifest: PathBuf, repo: PathBuf) -> Result<()> {
    let root = store_root(&repo);
    let key = fixture_sign_key();
    let report = install_pack(&root, &manifest, &key.public())?;
    println!(
        "{{\"pack_id\":\"{}\",\"shards\":{},\"linked\":{},\"dedup\":{}}}",
        report.pack_id, report.shard_count, report.blobs_linked, report.blobs_skipped_dedup
    );
    Ok(())
}

fn run_verify(manifest: PathBuf) -> Result<()> {
    let key = fixture_sign_key();
    let m = PackManifest::read_json(&manifest)?;
    graphzero_pack::verify_manifest_signature(&m, &key.public())?;
    let parent = manifest.parent().unwrap_or(Path::new("."));
    graphzero_pack::verify_pack_artifacts(parent, &m)?;
    println!("{{\"status\":\"ok\"}}");
    Ok(())
}

fn run_build_fixture(out: PathBuf) -> Result<()> {
    let key = fixture_sign_key();
    let path = build_fixture_pack(&out, &key)?;
    println!("{{\"manifest\":\"{}\"}}", path.display());
    Ok(())
}

fn run_uninstall(pack_id: String, repo: PathBuf, dry_run: bool) -> Result<()> {
    let root = store_root(&repo);
    if dry_run {
        let packs = list_installed(&root)?;
        let found = packs.iter().find(|p| p.pack_id == pack_id);
        let would_remove = found.map(|p| {
            serde_json::json!({
                "pack_id": p.pack_id,
                "version": p.version,
                "shard_dir": p.shard_dir,
            })
        });
        println!(
            "{}",
            serde_json::json!({
                "dry_run": true,
                "pack_id": pack_id,
                "found": found.is_some(),
                "would_remove": would_remove,
            })
        );
        return Ok(());
    }
    let removed = uninstall_pack(&root, &pack_id)?;
    println!("{{\"removed\":{}}}", removed);
    Ok(())
}

pub fn run(cmd: PackCommand) -> Result<()> {
    match cmd {
        PackCommand::List { repo } => run_list(repo),
        PackCommand::Install { manifest, repo } => run_install(manifest, repo),
        PackCommand::Verify { manifest } => run_verify(manifest),
        PackCommand::BuildFixture { out } => run_build_fixture(out),
        PackCommand::Uninstall {
            pack_id,
            repo,
            dry_run,
        } => run_uninstall(pack_id, repo, dry_run),
    }
}
