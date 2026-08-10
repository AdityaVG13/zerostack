//! Native child-tree resource policy and truthful enforcement receipts.

use std::io;
use std::process::Command;

pub const DEFAULT_IDLE_TREE_RSS_BYTES: u64 = 128 * 1024 * 1024;
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
            schema: "zerostack.process.resource_receipt.v1",
            source: "zero-process/native",
            platform: std::env::consts::OS,
            profile: "aggregate-default",
            idle_tree_rss_bytes: policy.idle_tree_rss_bytes,
            active_tree_rss_bytes: policy.active_tree_rss_bytes,
            cpu_seconds: policy.cpu_seconds,
            enforcement: if cfg!(windows) {
                ResourceEnforcement::WindowsJobObject
            } else if cfg!(all(unix, not(target_os = "macos"))) {
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
    _command: &mut Command,
    policy: ProcessResourcePolicy,
) -> io::Result<ResourceReceipt> {
    let receipt = ResourceReceipt::for_policy(policy.validate()?);
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("native aggregate process-tree resource enforcement unsupported: {receipt:?}"),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_defaults_are_exact_and_bounded() {
        let policy = ProcessResourcePolicy::active_default().validate().unwrap();
        assert_eq!(policy.idle_tree_rss_bytes, 128 * 1024 * 1024);
        assert_eq!(policy.active_tree_rss_bytes, 256 * 1024 * 1024);
        let receipt = ResourceReceipt::for_policy(policy);
        assert_eq!(receipt.schema, "zerostack.process.resource_receipt.v1");
        if cfg!(any(windows, all(unix, not(target_os = "macos")))) {
            assert_ne!(receipt.enforcement, ResourceEnforcement::Unsupported);
        } else {
            assert_eq!(receipt.enforcement, ResourceEnforcement::Unsupported);
        }
    }

    #[test]
    fn oversized_or_inverted_profiles_fail_closed() {
        assert!(
            ProcessResourcePolicy {
                idle_tree_rss_bytes: DEFAULT_ACTIVE_TREE_RSS_BYTES,
                active_tree_rss_bytes: DEFAULT_IDLE_TREE_RSS_BYTES,
                cpu_seconds: 1,
            }
            .validate()
            .is_err()
        );
        assert!(
            ProcessResourcePolicy {
                active_tree_rss_bytes: DEFAULT_ACTIVE_TREE_RSS_BYTES + 1,
                ..ProcessResourcePolicy::default()
            }
            .validate()
            .is_err()
        );
    }
}
