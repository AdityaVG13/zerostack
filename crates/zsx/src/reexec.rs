//! Replace this process with a newer plugin `bin/zsx` after a rename-install.
//!
//! Grok keeps `zsx mcp` alive for the whole conversation. `/zerostack-rebuild`
//! swings the directory entry (new inode) without killing the mapped image.
//! The next MCP reply then `execve`s the new inode: same pid, same stdin
//! pipe, same parent. No spawn, no sidecar, no new Grok session.
//! Other live sessions do the same on their next call.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

static RUNNING_IMAGE: OnceLock<File> = OnceLock::new();
static BOOT_UNIX_MS: OnceLock<u64> = OnceLock::new();

pub fn capture_running_image() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let _ = BOOT_UNIX_MS.set(now);
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Ok(file) = File::open(exe) else {
        return;
    };
    let _ = RUNNING_IMAGE.set(file);
}

fn unix_inode_of(file: &File) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        file.metadata().ok().map(|m| m.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        None
    }
}

fn unix_path_inode(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(path).ok().map(|m| m.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

pub fn plugin_bin() -> Option<PathBuf> {
    let root = std::env::var_os("GROK_PLUGIN_ROOT")?;
    let bin = PathBuf::from(root).join("bin/zsx");
    bin.is_file().then_some(bin)
}

pub fn running_inode() -> Option<u64> {
    RUNNING_IMAGE.get().and_then(unix_inode_of)
}

pub fn bin_inode() -> Option<u64> {
    plugin_bin().and_then(|path| unix_path_inode(&path))
}

pub fn image_is_stale() -> bool {
    match (running_inode(), bin_inode()) {
        (Some(run), Some(disk)) => run != disk,
        _ => false,
    }
}

pub fn boot_unix_ms() -> u64 {
    *BOOT_UNIX_MS.get().unwrap_or(&0)
}

pub fn image_payload() -> Value {
    json!({
        "stale": image_is_stale(),
        "running_inode": running_inode(),
        "bin_inode": bin_inode(),
        "boot_unix_ms": boot_unix_ms(),
    })
}

/// After the current MCP frame has been written. Never returns on success.
pub fn reexec_if_plugin_bin_changed() {
    if !image_is_stale() {
        return;
    }
    let Some(bin) = plugin_bin() else {
        return;
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(std::env::args().skip(1));
        let err = cmd.exec();
        eprintln!("zsx mcp: reexec {} failed: {err}", bin.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncaptured_image_is_not_stale() {
        assert!(!image_is_stale() || running_inode().is_some());
        if RUNNING_IMAGE.get().is_none() {
            assert!(!image_is_stale());
        }
    }
}
