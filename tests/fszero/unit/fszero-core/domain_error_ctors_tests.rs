use super::*;
use crate::filesystem_contract::filesystem_contract_descriptor;
use std::collections::BTreeSet;

fn contract_error_classes() -> BTreeSet<String> {
    filesystem_contract_descriptor()
        .get("error_classes")
        .and_then(serde_json::Value::as_object)
        .expect("filesystem-v1 error_classes")
        .keys()
        .cloned()
        .collect()
}

fn named_ctor_errors() -> Vec<(&'static str, DomainError)> {
    let msg = "probe";
    vec![
        ("already_exists", DomainError::already_exists(msg)),
        ("budget_exceeded", DomainError::budget_exceeded(msg)),
        ("cancelled", DomainError::cancelled(msg)),
        ("conflict", DomainError::conflict(msg)),
        ("corrupt_state", DomainError::corrupt_state(msg)),
        ("deadline_exceeded", DomainError::deadline_exceeded(msg)),
        (
            "durability_unavailable",
            DomainError::durability_unavailable(msg),
        ),
        (
            "incompatible_contract",
            DomainError::incompatible_contract(msg),
        ),
        ("internal", DomainError::internal(msg)),
        ("invalid_argument", DomainError::invalid_argument(msg)),
        ("invalid_path", DomainError::invalid_path(msg)),
        ("io_error", DomainError::io_error(msg)),
        ("not_directory", DomainError::not_directory(msg)),
        ("not_file", DomainError::not_file(msg)),
        ("not_found", DomainError::not_found(msg)),
        ("outside_root", DomainError::outside_root(msg)),
        ("permission_denied", DomainError::permission_denied(msg)),
        ("stale_preimage", DomainError::stale_preimage(msg)),
        ("store_unavailable", DomainError::store_unavailable(msg)),
        (
            "unsupported_file_type",
            DomainError::unsupported_file_type(msg),
        ),
    ]
}

#[test]
fn named_ctors_cover_contract_error_classes() {
    let contract = contract_error_classes();
    let named = named_ctor_errors();
    let named_classes: BTreeSet<String> = named.iter().map(|(c, _)| (*c).to_string()).collect();
    assert_eq!(
        named_classes, contract,
        "named DomainError ctors must match filesystem-v1 error_classes"
    );
    for (class, err) in named {
        assert_eq!(err.class, class);
        assert_eq!(err.message, "probe");
        assert_eq!(
            err.retryable,
            error_class_retryable(class),
            "retryable for {class}"
        );
    }
}

#[test]
fn from_detail_stale_preimage_classifies() {
    let err = DomainError::from_detail("stale preimage: expected abc got def");
    assert_eq!(err.class, "stale_preimage");
    assert_eq!(err.retryable, error_class_retryable("stale_preimage"));
}

#[test]
fn from_detail_durability_unavailable_classifies_before_io() {
    for detail in [
        "durability unavailable: barrier failed",
        "fsync failed: Input/output error",
        "fullsync barrier failed",
        "FULLFSYNC failed: os error 5",
        "durability unavailable: fsync failed: io error errno 5",
    ] {
        let err = DomainError::from_detail(detail);
        assert_eq!(err.class, "durability_unavailable", "detail={detail}");
        assert_eq!(
            err.retryable,
            error_class_retryable("durability_unavailable"),
            "retryable for {detail}"
        );
        assert!(err.retryable, "durability_unavailable must be retryable");
    }
}

#[test]
fn from_detail_io_error_does_not_map_to_durability() {
    let err = DomainError::from_detail("io error: errno 5");
    assert_eq!(err.class, "io_error");
    assert_eq!(err.retryable, error_class_retryable("io_error"));
    assert_ne!(err.class, "durability_unavailable");
}
