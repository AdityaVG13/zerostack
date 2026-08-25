#![deny(unsafe_code)]

//! Daemonless in-process ZeroKernel runtime.
//!
//! The crate exposes only typed direct operations. JavaScript binding and cell
//! parsing layer on top of [`ZeroKernel`]; engine routing never uses public
//! command strings or a capability catalog.

mod canonical;
mod host;
mod preparation;
mod runtime;
mod shell;
mod state;
mod transaction;
mod typescript;

pub use canonical::direct_contract_digest;
pub use host::{AtomicCancellation, Cell, HostError, ZeroKernel, typed_error};
pub use preparation::{CellPreparation, PreparedCell};
pub use shell::ShellCommand;
pub use state::{StateError, StateSnapshot, StateStore};
pub use transaction::{
    PreparedEffect, Transaction, TransactionCoordinator, TransactionError, TransactionRecord,
    TransactionState,
};

pub use typescript::{TypeScriptError, erase_typescript};
