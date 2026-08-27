//! P5.2 semantic reservations: intent footprints, overlap detection, audit ledger.

pub mod footprint;
pub mod ledger;
pub mod schema;
pub mod service;

pub use footprint::{FootprintSnapshot, contract_footprint};
pub use ledger::{ReservationLedger, ledger_state_hash, replay_ledger};
pub use schema::{
    ConflictGraphEdge, DeclareResponse, IntentOperation, IntentReservation,
    ReservationCheckResponse, ReservationQueryResponse, ReservationStatus, SCHEMA_VERSION,
};
pub use service::{
    DeclareRequest, ReserveError, ReserveService, acquire_reservation, check_reservation,
    check_reservation_with_ttl, declare_reservation, list_active_reservations,
    notify_conflict_if_configured, now_ts, release_reservation, test_notify_conflict,
    test_notify_hook_count, test_reset_notify_hook,
};
