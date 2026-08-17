use std::sync::Arc;
use std::time::Duration;
use zsx_core::{ZsxSession, ZsxSessionFailureCode, AdapterBinding, AdapterCall, AdapterError, AdapterResponse, DomainAdapter};
use zero_abi::EngineIdentity;

struct StubAdapter { engine: EngineIdentity, scheme: &'static str }
impl DomainAdapter for StubAdapter {
    fn engine(&self) -> EngineIdentity { self.engine }
    fn binding(&self) -> AdapterBinding {
        AdapterBinding::new(self.engine, "test", "test.v1", "a".repeat(64), "b".repeat(64), self.scheme).expect("stub")
    }
    fn call(&self, _call: AdapterCall<'_>) -> Result<AdapterResponse, AdapterError> {
        Err(AdapterError::new("internal", "stub unused", false, None))
    }
}

fn temp_root(suffix: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("zsx-heavy-{}-{}-{}", suffix, std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

#[test]
fn heavy_shell_cancel_kills_child_group_and_releases_lease() {
    let root = temp_root("cancel");
    let session = ZsxSession::builder(root.clone())
        .with_session_id("test")
        .fszero(Arc::new(StubAdapter { engine: EngineIdentity::FsZero, scheme: "fz://" }))
        .graphzero(Arc::new(StubAdapter { engine: EngineIdentity::GraphZero, scheme: "gz://" }))
        .tokenzero(Arc::new(zsx_core::tokenzero::TokenZeroAdapter::new(&root, "test").expect("tokenzero")))
        .build()
        .expect("session");
    let generation = session.generation().expect("generation");
    let request_id = 1u64;
    let sess = Arc::new(session);
    let sess2 = Arc::clone(&sess);
    let handle = std::thread::spawn(move || {
        sess2.execute(generation, request_id, r#"await zero.token.shell("sleep 10")"#, Duration::from_secs(30))
    });
    std::thread::sleep(Duration::from_millis(900));
    let cancelled = sess.cancellation().cancel_request(generation, request_id);
    assert!(cancelled, "cancel_request must actively cancel");
    let result = handle.join().expect("join");
    let err = result.expect_err("heavy cancel must error");
    assert_eq!(err.code, ZsxSessionFailureCode::Cancelled, "dispatch must return Cancelled, got {err:?}");
    let next_id = 2u64;
    let next = sess.execute(generation, next_id, r#"await zero.token.shell("echo heavy-ok")"#, Duration::from_secs(10));
    if let Err(ref e) = next {
        assert_ne!(e.code, ZsxSessionFailureCode::Backpressure, "permit leaked: {e:?}");
        assert!(!e.detail.contains("permit"), "permit leaked: {e:?}");
    }
    {
        use zero_machine_permit::{MachinePermit, PermitOwnerMetadata};
        let base = zero_machine_permit::try_scoped_permit_base_for("heavy", Some(&root)).expect("permit base");
        let owner = PermitOwnerMetadata::new(root.to_string_lossy().to_string(), "probe".to_string(), "probe".to_string(), "probe".to_string());
        let permit = MachinePermit::acquire_slots_with_owner_metadata(&base, 1, None, owner);
        assert!(permit.is_ok(), "Heavy permit must be acquirable after cancel, got {permit:?}");
    }
    let _ = std::fs::remove_dir_all(&root);
}
