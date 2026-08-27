//! Compatibility re-exports for hub-owned process identity primitives.
//!
//! New code depends on `zero-process` directly. This module preserves the
//! existing `zero_machine_permit::session_owner` source API while the
//! machine-permit crate remains focused on permit acquisition and recovery.

pub use zero_process::{OwnerWatchError, OwnerWatcher, ProcessIdentity};
#[cfg(unix)]
pub use zero_process::{current_euid, peer_euid};
