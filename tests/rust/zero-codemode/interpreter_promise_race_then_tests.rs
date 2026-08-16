    use super::*;
    use crate::host::{Connector, ConnectorCompletion, ConnectorError, DispatchContext, Host};
    use crate::limits::HostLimits;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};
    use zero_abi::{CapabilityDescriptor, GlobalRegistration};

    struct NoDispatch;

    impl Connector for NoDispatch {
        fn dispatch(
            &self,
            _capability: &CapabilityDescriptor,
            _args_json: &str,
            _context: DispatchContext,
            _completion: ConnectorCompletion,
        ) -> Result<(), ConnectorError> {
            Err(ConnectorError::new("unexpected connector dispatch"))
        }
    }

    struct DelayedPing {
        delay: Duration,
        payload: String,
        started: AtomicBool,
    }

    impl Connector for DelayedPing {
        fn dispatch(
            &self,
            _capability: &CapabilityDescriptor,
            _args_json: &str,
            _context: DispatchContext,
            completion: ConnectorCompletion,
        ) -> Result<(), ConnectorError> {
            self.started.store(true, Ordering::Release);
            let payload = self.payload.clone();
            let delay = self.delay;
            thread::spawn(move || {
                thread::sleep(delay);
                let _ = completion.complete(Ok(format!(
                    r#"{{"ack":"ok","content":{{"kind":"inline","value":"{payload}"}}}}"#
                )));
            });
            Ok(())
        }
    }

    fn host() -> Host {
        Host::new(
            HostLimits::default(),
            GlobalRegistration::zero(vec![CapabilityDescriptor::new("test", "ping")]),
        )
        .expect("host")
    }

    fn run(plan: &str, connector: Rc<dyn Connector>) -> Result<serde_json::Value, HostError> {
        host().execute_with_cancel_timeout(
            plan,
            connector,
            std::sync::Arc::new(AtomicBool::new(false)),
            Duration::from_millis(400),
        )
    }

    #[test]
    fn race_resolved_then_settles_without_deadline() {
        let value = run(
            "return await Promise.race([Promise.resolve(1).then(x => x + 1)]);",
            Rc::new(NoDispatch),
        )
        .expect("then-only race");
        assert_eq!(value, serde_json::json!(2));
    }

    #[test]
    fn race_then_beats_pending_connector() {
        let connector = Rc::new(DelayedPing {
            delay: Duration::from_millis(80),
            payload: "conn-win".into(),
            started: AtomicBool::new(false),
        });
        let value = run(
            "return await Promise.race([Promise.resolve('then-win').then(x => x), zero.test.ping()]);",
            connector,
        )
        .expect("then should win");
        assert_eq!(value, serde_json::json!("then-win"));
    }

    #[test]
    fn race_of_all_of_resolved_settles() {
        let value = run(
            "return await Promise.race([Promise.all([Promise.resolve(1)])]);",
            Rc::new(NoDispatch),
        )
        .expect("nested all");
        assert_eq!(value, serde_json::json!([1]));
    }

    #[test]
    fn race_fulfilled_sibling_beats_then() {
        let value = run(
            "return await Promise.race([Promise.resolve(1).then(x => x + 1), Promise.resolve('fast')]);",
            Rc::new(NoDispatch),
        )
        .expect("fulfilled sibling");
        assert_eq!(value, serde_json::json!("fast"));
    }

    #[test]
    fn race_fulfilled_sibling_beats_pending_all() {
        let connector = Rc::new(DelayedPing {
            delay: Duration::from_millis(80),
            payload: "all-win".into(),
            started: AtomicBool::new(false),
        });
        let value = run(
            "return await Promise.race([Promise.all([zero.test.ping()]), Promise.resolve('fast')]);",
            connector,
        )
        .expect("fulfilled sibling must beat pending all");
        assert_eq!(value, serde_json::json!("fast"));
    }

    struct SequencedPing {
        delays: Vec<Duration>,
        payloads: Vec<String>,
        calls: AtomicUsize,
    }

    impl Connector for SequencedPing {
        fn dispatch(
            &self,
            _capability: &CapabilityDescriptor,
            _args_json: &str,
            _context: DispatchContext,
            completion: ConnectorCompletion,
        ) -> Result<(), ConnectorError> {
            let index = self.calls.fetch_add(1, Ordering::AcqRel);
            let payload = self
                .payloads
                .get(index)
                .cloned()
                .unwrap_or_else(|| "late".into());
            let delay = self
                .delays
                .get(index)
                .copied()
                .unwrap_or(Duration::from_millis(80));
            thread::spawn(move || {
                thread::sleep(delay);
                let _ = completion.complete(Ok(format!(
                    r#"{{"ack":"ok","content":{{"kind":"inline","value":"{payload}"}}}}"#
                )));
            });
            Ok(())
        }
    }

    #[test]
    fn race_fast_connector_beats_pending_all_of_slow() {
        let connector = Rc::new(SequencedPing {
            delays: vec![Duration::from_millis(80), Duration::from_millis(5)],
            payloads: vec!["all-slow".into(), "sibling-fast".into()],
            calls: AtomicUsize::new(0),
        });
        let value = run(
            "return await Promise.race([Promise.all([zero.test.ping()]), zero.test.ping()]);",
            connector,
        )
        .expect("fast sibling must beat all-of-slow");
        assert_eq!(value, serde_json::json!("sibling-fast"));
    }

    #[test]
    fn race_fast_connector_beats_pending_then_of_slow() {
        let connector = Rc::new(SequencedPing {
            delays: vec![Duration::from_millis(80), Duration::from_millis(5)],
            payloads: vec!["then-slow".into(), "sibling-fast".into()],
            calls: AtomicUsize::new(0),
        });
        let value = run(
            "return await Promise.race([zero.test.ping().then(x => x), zero.test.ping()]);",
            connector,
        )
        .expect("fast sibling must beat then-of-slow");
        assert_eq!(value, serde_json::json!("sibling-fast"));
    }

    #[test]
    fn all_empty_settles() {
        let value = run("return await Promise.all([]);", Rc::new(NoDispatch)).expect("empty all");
        assert_eq!(value, serde_json::json!([]));
    }

    #[test]
    fn all_resolved_siblings_settle() {
        let value = run(
            "return await Promise.all([Promise.resolve(1), Promise.resolve(2)]);",
            Rc::new(NoDispatch),
        )
        .expect("resolved all");
        assert_eq!(value, serde_json::json!([1, 2]));
    }

    #[test]
    fn all_rejected_sibling_beats_pending_connector() {
        let connector = Rc::new(DelayedPing {
            delay: Duration::from_millis(80),
            payload: "conn-win".into(),
            started: AtomicBool::new(false),
        });
        let started = Instant::now();
        let value = run(
            "return await Promise.all([zero.test.ping(), Promise.reject('fast')]).catch(e => e);",
            connector,
        )
        .expect("rejected sibling must win");
        assert_eq!(value, serde_json::json!("fast"));
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "all must not host-wait the pending connector: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn race_of_rejected_all_does_not_hang() {
        let started = Instant::now();
        let value = run(
            "return await Promise.race([Promise.all([Promise.reject('x')])]).catch(e => e);",
            Rc::new(NoDispatch),
        )
        .expect("rejected all must settle the race");
        assert_eq!(value, serde_json::json!("x"));
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "race-of-rejected-all must not loop: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn all_of_rejected_race_does_not_hang() {
        let started = Instant::now();
        let value = run(
            "return await Promise.all([Promise.race([Promise.reject('x')])]).catch(e => e);",
            Rc::new(NoDispatch),
        )
        .expect("rejected race must settle the all");
        assert_eq!(value, serde_json::json!("x"));
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "all-of-rejected-race must not loop: {:?}",
            started.elapsed()
        );
    }

