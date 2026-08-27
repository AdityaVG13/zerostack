//! Prints the operation ABI contract digest (adoption-gate check).
fn main() {
    println!("{}", tokenzero_core::operation_abi::contract_digest_hex());
}
