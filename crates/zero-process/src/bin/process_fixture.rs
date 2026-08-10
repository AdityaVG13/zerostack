//! Test fixture for cfg(windows) process-tree tests.
//!
//! Modes:
//! - `sleep`: run forever (owner/root that must be killed by tests).
//! - `exit-0`: exit immediately with status 0.
//! - `spawn-sleeper --pid-file PATH`: spawn a detached `sleep` descendant,
//!   write its pid to PATH, then run forever. The descendant inherits the
//!   caller's Windows Job Object membership, which is what tree tests assert.
use std::process::Command;
use std::time::Duration;

fn forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[allow(
    clippy::zombie_processes,
    reason = "fixture leaves a live descendant for Job Object containment tests"
)]
fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("sleep") => forever(),
        Some("exit-0") => {}
        Some("spawn-sleeper") => {
            if args.next().as_deref() != Some("--pid-file") {
                panic!("spawn-sleeper requires --pid-file PATH");
            }
            let pid_file = args
                .next()
                .unwrap_or_else(|| panic!("spawn-sleeper requires --pid-file PATH"));
            let exe = std::env::current_exe().expect("current exe");
            let child = Command::new(exe)
                .arg("sleep")
                .spawn()
                .expect("spawn sleeper");
            std::fs::write(&pid_file, child.id().to_string()).expect("write pid file");
            forever();
        }
        _ => std::process::exit(2),
    }
}
