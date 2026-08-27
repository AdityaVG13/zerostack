//! Native child-tree resource policy and truthful enforcement receipts.

use std::io;
use std::process::Command;

pub const DEFAULT_IDLE_TREE_RSS_BYTES: u64 = 96 * 1024 * 1024;
pub const DEFAULT_ACTIVE_TREE_RSS_BYTES: u64 = 256 * 1024 * 1024;
pub const DEFAULT_ACTIVE_CPU_SECONDS: u64 = 300;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessResourcePolicy {
    pub idle_tree_rss_bytes: u64,
    pub active_tree_rss_bytes: u64,
    pub cpu_seconds: u64,
}

impl ProcessResourcePolicy {
    pub const fn active_default() -> Self {
        Self {
            idle_tree_rss_bytes: DEFAULT_IDLE_TREE_RSS_BYTES,
            active_tree_rss_bytes: DEFAULT_ACTIVE_TREE_RSS_BYTES,
            cpu_seconds: DEFAULT_ACTIVE_CPU_SECONDS,
        }
    }

    /// Fail-closed share for one active child while the other prewarmed child
    /// trees may remain at their idle shares. This keeps the worst case of
    /// `workers` active children plus `workers - 1` idle prewarms within the
    /// aggregate active budget.
    pub fn share(self, workers: u64) -> io::Result<Self> {
        if workers == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "resource budget worker count must be nonzero",
            ));
        }
        let idle_tree_rss_bytes = self.idle_tree_rss_bytes / workers;
        let reserved_idle = idle_tree_rss_bytes
            .checked_mul(workers.saturating_sub(1))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "idle share overflow"))?;
        let active_tree_rss_bytes = self
            .active_tree_rss_bytes
            .checked_sub(reserved_idle)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "active budget below idle reserve",
                )
            })?
            / workers;
        Self {
            idle_tree_rss_bytes,
            active_tree_rss_bytes,
            cpu_seconds: self.cpu_seconds / workers,
        }
        .validate()
    }

    pub fn validate(self) -> io::Result<Self> {
        if self.idle_tree_rss_bytes == 0
            || self.active_tree_rss_bytes == 0
            || self.idle_tree_rss_bytes > self.active_tree_rss_bytes
            || self.active_tree_rss_bytes > DEFAULT_ACTIVE_TREE_RSS_BYTES
            || self.cpu_seconds == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid process resource policy",
            ));
        }
        Ok(self)
    }
}

impl Default for ProcessResourcePolicy {
    fn default() -> Self {
        Self::active_default()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceEnforcement {
    WindowsJobObject,
    UnixInheritedPerProcess,
    MacOsInheritedCpu,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceReceipt {
    pub schema: &'static str,
    pub source: &'static str,
    pub platform: &'static str,
    pub profile: &'static str,
    pub idle_tree_rss_bytes: u64,
    pub active_tree_rss_bytes: u64,
    pub cpu_seconds: u64,
    pub enforcement: ResourceEnforcement,
}

impl ResourceReceipt {
    pub fn for_policy(policy: ProcessResourcePolicy) -> Self {
        Self {
            schema: "zerostack.process.resource_receipt",
            source: "zero-process/native",
            platform: std::env::consts::OS,
            profile: "aggregate-default",
            idle_tree_rss_bytes: policy.idle_tree_rss_bytes,
            active_tree_rss_bytes: policy.active_tree_rss_bytes,
            cpu_seconds: policy.cpu_seconds,
            enforcement: if cfg!(windows) {
                ResourceEnforcement::WindowsJobObject
            } else if cfg!(target_os = "macos") {
                ResourceEnforcement::MacOsInheritedCpu
            } else if cfg!(unix) {
                ResourceEnforcement::UnixInheritedPerProcess
            } else {
                ResourceEnforcement::Unsupported
            },
        }
    }

    pub const fn is_tree_enforced(&self) -> bool {
        matches!(self.enforcement, ResourceEnforcement::WindowsJobObject)
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn configure_command(
    command: &mut Command,
    policy: ProcessResourcePolicy,
) -> io::Result<ResourceReceipt> {
    use std::os::unix::process::CommandExt;

    let policy = policy.validate()?;
    // SAFETY: the closure only invokes async-signal-safe setrlimit before exec.
    unsafe {
        command.pre_exec(move || {
            let memory = libc::rlimit {
                rlim_cur: policy.active_tree_rss_bytes as libc::rlim_t,
                rlim_max: policy.active_tree_rss_bytes as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &memory) != 0 {
                return Err(io::Error::last_os_error());
            }
            let cpu = libc::rlimit {
                rlim_cur: policy.cpu_seconds as libc::rlim_t,
                rlim_max: policy.cpu_seconds as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(ResourceReceipt::for_policy(policy))
}

#[cfg(target_os = "macos")]
pub(crate) fn configure_command(
    command: &mut Command,
    policy: ProcessResourcePolicy,
) -> io::Result<ResourceReceipt> {
    use std::os::unix::process::CommandExt;

    let policy = policy.validate()?;
    // Darwin does not enforce RLIMIT_AS/RLIMIT_RSS. Enforce the inherited CPU
    // limit and report that narrower native guarantee truthfully.
    // SAFETY: the closure only invokes async-signal-safe setrlimit before exec.
    unsafe {
        command.pre_exec(move || {
            let cpu = libc::rlimit {
                rlim_cur: policy.cpu_seconds as libc::rlim_t,
                rlim_max: policy.cpu_seconds as libc::rlim_t,
            };
            if libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(ResourceReceipt::for_policy(policy))
}

#[cfg(windows)]
pub(crate) fn configure_command(
    _command: &mut Command,
    policy: ProcessResourcePolicy,
) -> io::Result<ResourceReceipt> {
    Ok(ResourceReceipt::for_policy(policy.validate()?))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn configure_command(
    _command: &mut Command,
    policy: ProcessResourcePolicy,
) -> io::Result<ResourceReceipt> {
    let receipt = ResourceReceipt::for_policy(policy.validate()?);
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("native process resource enforcement unsupported: {receipt:?}"),
    ))
}

