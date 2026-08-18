//! End-to-end TokenZero background-job lifecycle coverage (audit debt `zerostack-js0z`).

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use zsx_core::ZsxSession;

struct Fixture {
    base: PathBuf,
    session: ZsxSession,
    request_id: u64,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "zsx-long-job-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let root = base.join("project");
        let state_root = base.join("state");
        std::fs::create_dir_all(&root).expect("project root");
        let session = ZsxSession::builder(root)
            .with_state_root(state_root)
            .with_session_id("long-job-test")
            .build_canonical()
            .expect("canonical session");
        Self {
            base,
            session,
            request_id: 1,
        }
    }

    fn execute(&mut self, program: &str) -> Value {
        let request_id = self.request_id;
        self.request_id += 1;
        self.session
            .execute(
                self.session.generation().expect("generation"),
                request_id,
                program,
                Duration::from_secs(10),
            )
            .unwrap_or_else(|error| panic!("program failed: {program}\n{error}"))
            .value
    }

    fn poll_terminal(&mut self, job: &str) -> Value {
        let mut since = 0_u64;
        let mut tail = String::new();
        for _ in 0..50 {
            let job_json = serde_json::to_string(job).expect("job JSON");
            let value = self.execute(&format!(
                "return await zero.token.job({job_json}, {{waitMs:200, since:{since}, tailBytes:4096}});"
            ));
            assert_eq!(value["ack"], "ok", "job poll must be acknowledged: {value}");
            let mut payload = value["content"]["value"].clone();
            if let Some(chunk) = payload["tail"].as_str() {
                tail.push_str(chunk);
            }
            since = payload["cursor"].as_u64().unwrap_or(since);
            if payload["status"] != "running" {
                payload["tail"] = Value::String(tail);
                return payload;
            }
        }
        panic!("background job {job} did not settle within bounded polling");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.session.shutdown();
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn receipt_job(receipt: &Value) -> &str {
    assert_eq!(
        receipt["ack"], "ok",
        "background launch must be acknowledged: {receipt}"
    );
    let receipt = &receipt["content"]["value"];
    assert_eq!(
        receipt["cursor"], 0,
        "new job starts at cursor zero: {receipt}"
    );
    assert_eq!(
        receipt["version"], 0,
        "new job starts at version zero: {receipt}"
    );
    receipt["job"].as_str().expect("job receipt id")
}

#[test]
fn timeout_above_sixty_seconds_auto_backgrounds_and_polls_to_completion() {
    let mut fixture = Fixture::new();
    let receipt = fixture.execute(
        r#"return await zero.token.shell("printf start; sleep 0.05; printf done", {timeoutMs:60001});"#,
    );
    let job = receipt_job(&receipt).to_owned();
    let terminal = fixture.poll_terminal(&job);

    assert_eq!(terminal["status"], "exited", "{terminal}");
    assert_eq!(terminal["exitCode"], 0, "{terminal}");
    let tail = terminal["tail"].as_str().expect("terminal tail");
    assert!(tail.contains("start"), "{terminal}");
    assert!(tail.contains("done"), "{terminal}");
    assert!(terminal["cursor"].as_u64().is_some());
    assert!(terminal["version"].as_u64().is_some());
    std::thread::sleep(Duration::from_millis(100));
    let settled = fixture.poll_terminal(&job);
    assert_eq!(settled["status"], "exited", "{settled}");
    assert_eq!(
        settled["tail"], terminal["tail"],
        "terminal output changed after reap"
    );

    fixture.session.shutdown().expect("session shutdown");
    let error = fixture
        .session
        .execute(1, 99, "return 1;", Duration::from_secs(1))
        .expect_err("shut down session refuses new work");
    assert!(error.to_string().contains("terminat") || error.to_string().contains("shut"));
}

#[test]
fn explicit_background_job_timeout_reaches_terminal_without_late_output() {
    let mut fixture = Fixture::new();
    let receipt = fixture.execute(
        r#"return await zero.token.shell("printf start; sleep 5; printf late", {background:true, timeoutMs:100});"#,
    );
    let job = receipt_job(&receipt).to_owned();
    let terminal = fixture.poll_terminal(&job);

    assert_ne!(terminal["status"], "running", "{terminal}");
    let tail = terminal["tail"].as_str().unwrap_or_default();
    assert!(tail.contains("start"), "{terminal}");
    assert!(
        !tail.contains("late"),
        "timed-out child emitted late output: {terminal}"
    );
    std::thread::sleep(Duration::from_millis(150));
    let settled = fixture.poll_terminal(&job);
    assert_ne!(settled["status"], "running", "{settled}");
    assert!(
        !settled["tail"]
            .as_str()
            .unwrap_or_default()
            .contains("late"),
        "timed-out child produced output after its terminal state: {settled}"
    );
}
