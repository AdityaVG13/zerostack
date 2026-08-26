//! Engine-neutral MCP background-job notification contracts.

use serde_json::{Value, json};
use std::sync::atomic::{AtomicU64, Ordering};
use zero_mcp::job_progress::{
    ClientFamily, JobEvent, NotifyMode, classify_client, notify_mode, observe_job_launch,
    observe_job_poll, plan_notification, remember_client, remember_progress_token_value,
    take_notifications,
};

fn session_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "zero-mcp-job-progress-{}",
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[test]
fn client_modes_are_capability_aware() {
    assert_eq!(classify_client("pi"), ClientFamily::Pi);
    assert_eq!(classify_client("claude-code"), ClientFamily::ClaudeCode);
    assert_eq!(classify_client("Codex"), ClientFamily::Codex);
    assert_eq!(classify_client("OpenCode"), ClientFamily::OpenCode);
    assert_eq!(classify_client("unknown"), ClientFamily::Other);

    assert_eq!(
        notify_mode(ClientFamily::Pi, false, None),
        NotifyMode::Logging
    );
    assert_eq!(
        notify_mode(ClientFamily::Codex, false, None),
        NotifyMode::PollOnly
    );
    assert_eq!(
        notify_mode(ClientFamily::Codex, true, None),
        NotifyMode::Logging
    );
    assert_eq!(
        notify_mode(ClientFamily::Other, false, Some("tok")),
        NotifyMode::Progress
    );
}

#[test]
fn notification_planner_bounds_content_and_terminal_duplicates() {
    let token = json!(7);
    let completed = JobEvent::Completed {
        job_id: "job-1".into(),
        status: "exited".into(),
    };
    let notification = plan_notification(NotifyMode::Progress, Some(&token), &completed, false)
        .expect("terminal progress notification");

    assert_eq!(notification["method"], "notifications/progress");
    assert_eq!(notification["params"]["progressToken"], 7);
    assert_eq!(notification["params"]["progress"], 1);
    assert_eq!(notification["params"]["total"], 1);
    assert!(!notification.to_string().contains("raw job log"));
    assert!(plan_notification(NotifyMode::Progress, Some(&token), &completed, true).is_none());
    assert!(
        plan_notification(
            NotifyMode::PollOnly,
            None,
            &JobEvent::Started {
                job_id: "job-2".into()
            },
            false,
        )
        .is_none()
    );
}

#[test]
fn observed_job_lifecycle_emits_one_terminal() {
    let session = session_id();
    remember_client(&session, "pi", &Value::Null);
    remember_progress_token_value(&session, Some(json!("progress-1")));

    let job = observe_job_launch(
        &session,
        &json!({"structuredContent": {"job": "job-3", "status": "running"}}),
    );
    assert_eq!(job.as_deref(), Some("job-3"));
    assert_eq!(take_notifications(&session).len(), 1);

    observe_job_poll(
        &session,
        "job-3",
        &json!({"cursor": 12, "status": "running"}),
    );
    assert_eq!(take_notifications(&session).len(), 1);

    observe_job_poll(
        &session,
        "job-3",
        &json!({"cursor": 12, "status": "exited"}),
    );
    let terminal = take_notifications(&session);
    assert_eq!(
        terminal.len(),
        2,
        "new cursor and terminal are separate bounded events"
    );

    observe_job_poll(&session, "job-3", &json!({"status": "exited"}));
    assert!(take_notifications(&session).is_empty());
}
