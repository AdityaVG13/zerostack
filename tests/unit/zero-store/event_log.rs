use tempfile::tempdir;
use zero_abi::{KernelLedger, ZERO_KERNEL_PROTOCOL, ZeroKernelEvent, ZeroKernelOutcome};
use zero_store::EventLog;

fn event(visible: &[u8]) -> ZeroKernelEvent {
    ZeroKernelEvent {
        protocol: ZERO_KERNEL_PROTOCOL.into(),
        session_id: "session".into(),
        cell_id: "cell".into(),
        source_digest: "source".into(),
        contract_digest: "contract".into(),
        policy_digest: "policy".into(),
        state_root_before: None,
        state_root_after: None,
        input_handles: vec![],
        output_handles: vec![],
        outcome: ZeroKernelOutcome::Completed,
        ledger: KernelLedger::default(),
        model_visible_digest: blake3::hash(visible).to_hex().to_string(),
    }
}

#[test]
fn visible_bytes_are_bound_before_event_is_returned() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    let visible = b"model-visible";
    let publication = log.publish(&event(visible), visible).unwrap();
    assert_eq!(
        publication.model_visible_digest,
        blake3::hash(visible).to_hex().as_str()
    );
    let records = log.records("session").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].event, publication.event);
}

#[test]
fn digest_mismatch_exposes_no_log_record() {
    let root = tempdir().unwrap();
    let log = EventLog::open(root.path());
    assert!(log.publish(&event(b"expected"), b"different").is_err());
    assert!(log.records("session").unwrap().is_empty());
}
