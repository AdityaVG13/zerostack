use std::time::Duration;

use zero_codemode::{HostLimits, LimitError};

#[test]
fn every_limit_dimension_is_finite_and_positive() {
    let valid = HostLimits::new(
        8 * 1024 * 1024,
        256 * 1024,
        Duration::from_secs(1),
        10_000,
        32,
        4,
        16,
        64 * 1024,
        1024 * 1024,
    )
    .unwrap();
    assert!(
        valid.validate().is_ok(),
        "fully valid configuration must be accepted"
    );
    assert_eq!(valid.max_connector_calls, 16);
    let cases: &[(&str, fn(&mut HostLimits))] = &[
        ("memory_bytes", |l| l.memory_bytes = 0),
        ("stack_bytes", |l| l.stack_bytes = 0),
        ("instruction_budget", |l| l.instruction_budget = 0),
        ("microtask_ceiling", |l| l.microtask_ceiling = 0),
        ("max_inflight_connector_calls", |l| {
            l.max_inflight_connector_calls = 0
        }),
        ("max_connector_calls", |l| l.max_connector_calls = 0),
        ("max_plan_bytes", |l| l.max_plan_bytes = 0),
        ("max_json_bytes", |l| l.max_json_bytes = 0),
        ("wall_timeout", |l| l.wall_timeout = Duration::ZERO),
    ];
    for (name, mutate) in cases {
        let mut invalid = valid;
        mutate(&mut invalid);
        assert_eq!(
            invalid.validate(),
            Err(LimitError::Zero(name)),
            "dimension {name} must be rejected when zero"
        );
    }
}
