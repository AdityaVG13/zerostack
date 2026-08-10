//! Hub-owned process identity and exact child-tree lifecycle primitives.
//!
//! Engines may expose thin compatibility adapters over this crate. They must
//! not fork these process-lifecycle implementations locally.

mod child;
mod identity;
#[cfg(windows)]
mod pipe;
mod random;
mod resource;

pub use child::{
    ChildBinding, IDENTITY_FILE_NAME, IdentityError, SignalOutcome, VerifiedChild,
    escalate_detached, peer_is_same_user,
};
pub use identity::{OwnerWatchError, OwnerWatcher, ProcessIdentity};
#[cfg(unix)]
pub use identity::{current_euid, peer_euid};
#[cfg(windows)]
pub use pipe::{PipeConnection, PipeListener, PipeListenerCancel, PipeSecurity, Sid};
pub use random::fill_random;
pub use resource::{
    DEFAULT_ACTIVE_CPU_SECONDS, DEFAULT_ACTIVE_TREE_RSS_BYTES, DEFAULT_IDLE_TREE_RSS_BYTES,
    ProcessResourcePolicy, ResourceEnforcement, ResourceReceipt,
};
