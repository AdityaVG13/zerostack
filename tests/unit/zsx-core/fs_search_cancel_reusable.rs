//! Regression: cancelled fs.compound search leaves dispatch reusable.
//!
//! Papercut pc_f6bbdf6db29f: fs.compound("search", query="pc_2178c942f3ff", path=".papercuts.jsonl")
//! with aggregate dispatch cancellation must stop cleanly, then the same session
//! must service the next FSZero and TokenZero dispatches with no outstanding
//! aggregate request and no heavy permit live. This is the exact follow-up to
//! generic heavy-cancel bead zerostack-qks9 covering the fs.search edge.

use std::sync::Arc;
use std::time::Duration;

use zsx_core::{ZsxSession, ZsxSessionFailureCode};

fn temp_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "zsx-zibb-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

fn heavy_permit_holders(root: &std::path::Path) -> usize {
    let base = match zero_machine_permit::try_scoped_permit_base_for("heavy", Some(root)) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    // Heavy class uses 1 slot (see dispatch_permit_slots).
    match zero_machine_permit::permit_status(&base, 1) {
        Ok(holders) => holders.len(),
        Err(_) => 0,
    }
}

#[test]
fn fs_compound_search_cancel_leaves_dispatch_reusable() {
    let root = temp_root("search-cancel");
    // Minimal project file so FSZero has a repo.
    std::fs::write(root.join(".papercuts.jsonl"), "pc_2178c942f3ff one-line\n")
        .expect("write papercuts");
    // Add extra files to give search a non-zero window for in-flight cancel.
    for i in 0..1200 {
        let path = root.join(format!("filler_{i:04}.txt"));
        let content = if i % 7 == 0 {
            "pc_2178c942f3ff needle\n".repeat(4)
        } else {
            " filler content to expand search IO\n".repeat(8)
        };
        std::fs::write(path, content).expect("filler");
    }

    let session = ZsxSession::builder(root.clone())
        .with_session_id("zibb-session")
        .fszero(Arc::new(zsx_core::fszero::FsZeroAdapter::new(
            &root,
            "zibb-session",
        )))
        .graphzero(Arc::new(zsx_core::graphzero::GraphZeroAdapter::new(
            &root,
            "zibb-session",
        )))
        .tokenzero(Arc::new(
            zsx_core::tokenzero::TokenZeroAdapter::new(&root, "zibb-session").expect("tokenzero"),
        ))
        .build()
        .expect("session");
    let generation = session.generation().expect("generation");
    let session = Arc::new(session);

    // Try to hit an in-flight window. Retry a few times with fresh request_ids
    // because a fast search may settle before cancel arrives.
    let mut cancelled_once = false;
    let mut last_err: Option<String> = None;
    for attempt in 0u64..10 {
        let request_id = 1 + attempt * 3;
        let sess2 = Arc::clone(&session);
        // Use a broad search (no path filter) so the kernel scans many files
        // and the in-flight window is wider than the targeted single-file case
        // from the papercut note.
        let handle = std::thread::spawn(move || {
            sess2.execute(
                generation,
                request_id,
                r#"return await zero.fs.compound("search", {query: "pc_2178c942f3ff"})"#,
                Duration::from_secs(15),
            )
        });
        std::thread::sleep(Duration::from_millis(15));
        let actively = session
            .cancellation()
            .cancel_request(generation, request_id);
        let res = handle.join().expect("join search");
        if actively {
            match res {
                Err(err) => {
                    // Session must surface Cancelled, not a leaked dispatch timeout.
                    assert_eq!(
                        err.code,
                        ZsxSessionFailureCode::Cancelled,
                        "cancelled search must be Cancelled, got {err:?}"
                    );
                    assert!(
                        !err.to_string()
                            .contains("aggregate dispatches did not stop"),
                        "must not leak aggregate-dispatch timeout: {err:?}"
                    );
                    cancelled_once = true;
                    break;
                }
                Ok(ok) => {
                    // Rare race where cancel arrived after settle but before wait.
                    // Outstanding must still be idle; treat as not-yet-cancelled and retry.
                    last_err = Some(format!("unexpected Ok on actively cancelled: {ok:?}"));
                    continue;
                }
            }
        } else {
            // Not actively cancelled (already settled); consume result and retry.
            let _ = res;
            continue;
        }
    }
    assert!(
        cancelled_once,
        "failed to cancel an in-flight fs.search after retries; last_err={last_err:?}"
    );

    // After cancellation, no aggregate dispatch may remain and no heavy permit
    // may be live. Next FSZero and TokenZero dispatches must succeed on the
    // same session/generation.
    assert_eq!(
        heavy_permit_holders(&root),
        0,
        "heavy permit must not be live after cancelled fs.search"
    );

    let fs_next = session.execute(
        generation,
        100,
        r#"return await zero.fs.compound("read", {path: ".papercuts.jsonl"})"#,
        Duration::from_secs(10),
    );
    assert!(
        fs_next.is_ok(),
        "next FSZero dispatch after cancelled search must succeed, got {fs_next:?}"
    );
    let fs_val = fs_next.unwrap().value;
    let fs_str = fs_val.to_string();
    assert!(
        fs_str.contains("pc_2178c942f3ff"),
        "FSZero read must return papercuts content, got {fs_str}"
    );

    let token_next = session.execute(
        generation,
        101,
        r#"return await zero.token.shell("echo token-ok")"#,
        Duration::from_secs(10),
    );
    assert!(
        token_next.is_ok(),
        "next TokenZero dispatch after cancelled search must succeed, got {token_next:?}"
    );
    let token_str = token_next.unwrap().value.to_string();
    assert!(
        token_str.contains("token-ok"),
        "TokenZero shell must return token-ok, got {token_str}"
    );

    assert_eq!(
        heavy_permit_holders(&root),
        0,
        "heavy permit must remain free after follow-up dispatches"
    );

    let _ = std::fs::remove_dir_all(&root);
}
