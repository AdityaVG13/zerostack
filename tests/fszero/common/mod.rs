#![allow(dead_code)]

// schema_drift.rs is not on disk; do not declare a vanished module.

use fs_zero::{FSZeroSession, estimate_visible_tokens};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, Once};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: Mutex<()> = Mutex::new(());

static HERMETIC_ENV: Once = Once::new();

fn ensure_hermetic_env() {
    // Once serializes first init — do NOT take ENV_LOCK here. Nested callers
    // (env_vars / env_var) already hold ENV_LOCK; re-locking deadlocked
    // wire_contract (fszero-ivee.5).
    HERMETIC_ENV.call_once(|| unsafe {
        // The parent session leaks these into cargo test; clear them once
        // per test process so every repo-scoped session gets an isolated
        // durable store. FSZERO_REF_INDEX is disabled for the same reason
        // the zeroref fixture does: the per-user ref index is intentionally
        // cross-session, so tests must not pollute it.
        std::env::remove_var("ZEROSTACK_STORE_ROOT");
        std::env::remove_var("ZERO_STACK_STORE_ROOT");
        std::env::remove_var("TOKENZERO_CACHE_PATH");
        std::env::set_var("FSZERO_REF_INDEX", "0");
    });
}

pub struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    pub fn new(prefix: &str) -> Self {
        ensure_hermetic_env();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "fszero_{prefix}_{}_{}_{}",
            std::process::id(),
            stamp,
            seq
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, rel: impl AsRef<Path>) -> PathBuf {
        self.path.join(rel)
    }

    pub fn write(&self, rel: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
        let path = self.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }
}

impl AsRef<Path> for TestRoot {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn assert_one_token(tok: &str) {
    assert_eq!(estimate_visible_tokens(tok), 1, "ack not 1 token: {tok}");
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Platform-appropriate test xattr name: Linux restricts unprivileged
/// xattrs to the `user.` namespace; macOS allows arbitrary names.
pub fn xattr_ns(suffix: &str) -> String {
    if cfg!(target_os = "linux") {
        format!("user.example.{suffix}")
    } else {
        format!("com.example.{suffix}")
    }
}

/// Prefix that [`xattr_ns`] names share, for filtering walks.
pub fn xattr_ns_prefix() -> &'static str {
    if cfg!(target_os = "linux") {
        "user.example."
    } else {
        "com.example."
    }
}

pub const SAMPLE_MAIN: &str = r#"fn main() {
    println!("hello from main");
    let _ = process_request("foo");
}
fn process_request(input: &str) -> Result<String, String> {
    if input.is_empty() { return Err("empty".to_string()); }
    Ok(format!("processed:{input}"))
}
pub fn helper() {}
"#;

pub const CODEMODE_MAIN: &str = r#"fn main() { let _ = process_request("x"); }
fn process_request(input: &str) -> String { format!("ok:{input}") }
"#;

pub fn sample_workspace() -> (TestRoot, String) {
    let root = TestRoot::new("sample");
    root.write("src/main.rs", SAMPLE_MAIN);
    (root, SAMPLE_MAIN.to_string())
}

pub fn codemode_tree() -> TestRoot {
    let root = TestRoot::new("cm");
    root.write("src/main.rs", CODEMODE_MAIN);
    root.write("src/lib.rs", "pub fn helper() -> i32 { 1 }\n");
    root
}

pub struct EnvGuard {
    key: &'static str,
    _lock: MutexGuard<'static, ()>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe { std::env::remove_var(self.key) };
    }
}

pub fn env_var(key: &'static str, value: &'static str) -> EnvGuard {
    ensure_hermetic_env();
    let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    unsafe { std::env::set_var(key, value) };
    EnvGuard { key, _lock: lock }
}

pub struct EnvVarsGuard {
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _lock: MutexGuard<'static, ()>,
}

impl Drop for EnvVarsGuard {
    fn drop(&mut self) {
        for (key, value) in self.previous.drain(..).rev() {
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

pub fn env_vars(vars: &[(&'static str, Option<String>)]) -> EnvVarsGuard {
    ensure_hermetic_env();
    let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let previous = vars
        .iter()
        .map(|(key, _)| (*key, std::env::var_os(key)))
        .collect::<Vec<_>>();
    for (key, value) in vars {
        unsafe {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
    EnvVarsGuard {
        previous,
        _lock: lock,
    }
}

pub fn expand_text(sess: &FSZeroSession, key: &str) -> String {
    String::from_utf8_lossy(&sess.expand(key).expect(key)).into_owned()
}
