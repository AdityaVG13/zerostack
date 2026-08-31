//! Domain types for FSZero. No session, store, or dispatch.

pub mod canonicalize;
pub mod edit_spec;
pub mod hashline;
pub mod hexutil;
pub mod line_class;
pub mod mutation_outcome;
pub mod target_ref;
pub mod zeroref;

pub use mutation_outcome::{MutationOutcome, MutationState};
pub use zeroref::{EMITTED_SCHEME, ZeroRef};
