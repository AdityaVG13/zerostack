//! P2.2 CLI control plane for the warm daemon stem.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use graphzero_store::resolve_graphzero_store_root;
use graphzero_store::store::daemon::{
    DaemonStatus, capture_spawned_stem_identity, daemon_owner_session, daemon_status,
    disable_daemon, next_daemon_generation, run_stem, terminate_unregistered_stem,
    write_enabled_state_with,
};

pub fn handle(action: &str, repo: &Path) -> Result<()> {
    let repo = repo.canonicalize().context("repo path")?;
    let store_root = resolve_graphzero_store_root(&repo);
    match action {
        "status" => emit_daemon_status(&store_root)?,
        "enable" => {
            enable(&repo, &store_root)?;
        }
        "disable" => {
            disable_daemon(&store_root)?;
            emit_daemon_status(&store_root)?;
        }
        other => bail!("unknown daemon action: {other}"),
    }
    Ok(())
}

fn emit_daemon_status(store_root: &Path) -> Result<()> {
    emit_status(&daemon_status(store_root))
}

fn emit_status(status: &DaemonStatus) -> Result<()> {
    println!("{}", render_status(status)?);
    Ok(())
}

fn render_status(status: &DaemonStatus) -> Result<String> {
    serde_json::to_string(status).context("serialize daemon status")
}

fn enable(repo: &Path, store_root: &Path) -> Result<()> {
    if daemon_status(store_root).daemon == "warm" {
        emit_daemon_status(store_root)?;
        return Ok(());
    }
    // The parent owns the generation counter: compute the next generation and
    // persist owner + generation exactly once here. The stem child binds the
    // persisted generation without incrementing, so a respawn bumps exactly once.
    let generation = next_daemon_generation(store_root);
    let owner_session = daemon_owner_session();
    write_enabled_state_with(store_root, repo, true, &owner_session, generation)?;
    let exe = std::env::current_exe().context("current_exe")?;
    let child = Command::new(&exe)
        .args(["daemon", "run", "--repo"])
        .arg(repo)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn daemon stem")?;
    if let Err(error) =
        capture_spawned_stem_identity(store_root, child.id(), &owner_session, generation)
    {
        // Reject startup through the shared owned-child teardown path.
        let cleanup = terminate_unregistered_stem(child, &owner_session, generation);
        return Err(error.context(format!(
            "register spawned daemon identity; cleanup={cleanup:?}"
        )));
    }
    drop(child);
    for _ in 0..100 {
        if daemon_status(store_root).daemon == "warm" {
            emit_daemon_status(store_root)?;
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!("daemon stem did not become ready")
}

/// Internal entry: run stem in foreground (spawned by `daemon enable`).
pub fn run_foreground(repo: &Path) -> Result<()> {
    let repo = repo.canonicalize().context("repo path")?;
    let store_root = resolve_graphzero_store_root(&repo);
    run_stem(&store_root, &repo)
}
