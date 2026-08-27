//! Domain types for FSZero. No session, store, or dispatch.

pub mod canonicalize;
pub mod edit_spec;
pub mod filesystem_contract;
pub mod hashline;
pub mod hexutil;
pub mod line_class;
pub mod mutation_outcome;
pub mod operation_abi;
pub mod operation_schemas;
pub mod raw_worker_protocol;
pub mod target_ref;
pub mod zeroref;

pub use filesystem_contract::{
    FILESYSTEM_CONTRACT_JSON, FILESYSTEM_CONTRACT_MAJOR, FILESYSTEM_CONTRACT_MINOR,
    FILESYSTEM_CONTRACT_NAME, FILESYSTEM_CONTRACT_STORE_KEY, FILESYSTEM_CONTRACT_VERSION,
    FilesystemContractError,
};
pub use mutation_outcome::{MutationOutcome, MutationState};
pub use operation_abi::{OPERATION_REGISTRY, Operation};
pub use zeroref::{EMITTED_SCHEME, ZeroRef};
