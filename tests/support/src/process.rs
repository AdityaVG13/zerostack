use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// Run one blocking probe on a named worker and fail if it exceeds `budget`.
/// Use this for APIs that must reject FIFOs or sockets without opening them.
pub fn assert_completes_within<T: Send + 'static>(
    name: &str,
    budget: Duration,
    probe: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (sender, receiver) = mpsc::channel();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _ = sender.send(probe());
        })
        .expect("spawn bounded test probe");
    match receiver.recv_timeout(budget) {
        Ok(value) => value,
        Err(RecvTimeoutError::Timeout) => {
            panic!("{name} exceeded its {budget:?} completion budget")
        }
        Err(RecvTimeoutError::Disconnected) => {
            panic!("{name} panicked before returning a result")
        }
    }
}

#[cfg(unix)]
pub fn make_fifo(path: &std::path::Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("spawn mkfifo");
    assert!(
        status.success(),
        "mkfifo {} failed: {status}",
        path.display()
    );
}
