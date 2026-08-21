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
    assert_eq!(valid.max_connector_calls, 16);

    let mut invalid = valid;
    invalid.max_connector_calls = 0;
    assert_eq!(
        invalid.validate(),
        Err(LimitError::Zero("max_connector_calls"))
    );

    invalid = valid;
    invalid.wall_timeout = Duration::ZERO;
    assert_eq!(invalid.validate(), Err(LimitError::Zero("wall_timeout")));
}
