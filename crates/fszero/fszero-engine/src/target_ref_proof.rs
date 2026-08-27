//! INCONTROVERTIBLE ONE-CALL PROOF (bead fszero-snap-to-file-targets-99q7).
//!
//! Simulates a fresh subagent with zero prior context: it issues EXACTLY ONE
//! substrate discovery call and must then locate file + lines + content +
//! intent, and apply a journaled edit, using nothing but that one response.

use super::read_ops::parse_read_arg;
use super::target_ref::{LineWindow, TARGET_INLINE_MAX_BYTES, parse_target_ref};
use crate::FSZeroSession;
use serde_json::json;

/// Everything a subagent may carry out of the single response.
struct ParsedHit {
    target: String,
    path: String,
    window: LineWindow,
    kind: String,
    symbol: String,
    inlined: Vec<(usize, String)>,
}

/// Parse the FIRST canonical hit record out of a raw substrate response.
/// Uses only the response text — no filesystem, no second call.
fn parse_first_hit(response: &str) -> ParsedHit {
    let mut lines = response.lines().skip_while(|l| !l.starts_with("HIT "));
    let header = lines
        .next()
        .unwrap_or_else(|| panic!("no HIT record in: {response}"));
    let target = header["HIT ".len()..]
        .split_whitespace()
        .next()
        .expect("target ref")
        .to_string();
    let kind = header
        .split("kind=")
        .nth(1)
        .expect("kind")
        .split_whitespace()
        .next()
        .expect("kind value")
        .to_string();
    let symbol = header.split("sym=").nth(1).expect("sym").to_string();
    let (path, window) = parse_target_ref(&target).expect("canonical target ref");
    let inlined = lines
        .take_while(|l| l.starts_with("| "))
        .map(|l| {
            let (no, text) = l[2..].split_once(": ").expect("numbered content line");
            (no.parse::<usize>().expect("line no"), text.to_string())
        })
        .collect();
    ParsedHit {
        path: path.to_string(),
        target,
        window,
        kind,
        symbol,
        inlined,
    }
}

#[test]
fn one_substrate_call_yields_target_content_intent_and_a_clean_edit() {
    let root = std::env::temp_dir().join("fszero_one_call_proof_root");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("root");
    let file = root.join("src/widget.rs");
    std::fs::write(&file, FIXTURE).expect("fixture");

    // ---- THE ONE AND ONLY DISCOVERY CALL ----
    let mut discovery_calls = 0usize;
    let mut session = FSZeroSession::with_root(&root);
    let response = {
        discovery_calls += 1;
        session.do_search(Some(&root), Some("OLD_LABEL"))
    };
    assert_eq!(discovery_calls, 1, "discovery must be exactly one call");

    // (b) sub-4KB payloads are inlined whole, never preview-only.
    assert!(
        !response.contains("ref=fz://"),
        "sub-threshold response must not be preview/ref-only: {response}"
    );
    assert!(response.len() <= TARGET_INLINE_MAX_BYTES, "{response}");

    let hit = parse_first_hit(&response);

    // (a) canonical target ref: path + line window, accepted VERBATIM by read.
    assert_eq!(hit.path, "src/widget.rs", "{response}");
    assert!(hit.window.start <= 4 && hit.window.end >= 4, "{response}");
    let (read_path, byte_range) =
        parse_read_arg(&hit.target).expect("fs read accepts the target ref verbatim");
    assert_eq!(read_path, hit.path);
    assert!(
        byte_range.is_none(),
        "line targets resolve as a line window"
    );
    let via_target = session.do_read(Some(&root), Some(&hit.target));
    assert!(
        via_target.starts_with("read:") && !via_target.starts_with("read:0"),
        "verbatim target read failed: {via_target}"
    );

    // (c) intent metadata: match kind + enclosing symbol.
    assert_eq!(hit.kind, "literal", "{response}");
    assert_eq!(hit.symbol, "fn build_widget()", "{response}");

    // Content window inlined in the SAME response.
    let matched = hit
        .inlined
        .iter()
        .find(|(_, text)| text.contains("OLD_LABEL"))
        .expect("matched line inlined in the same response");
    assert_eq!(matched.0, 4, "{response}");

    // Journaled edit derived ONLY from the parsed response. The bare path and
    // canonical window flow through fs.edit; no adapter-local write is allowed.
    assert!(matched.1.contains("OLD_LABEL"));
    let edit = super::dispatch_codemode_method(
        &mut session,
        "fs.edit",
        &json!({
            "path": &hit.path,
            "find": "OLD_LABEL",
            "replace": "NEW_LABEL",
            "start_line": hit.window.start,
            "end_line": hit.window.end
        }),
    )
    .expect("window edit dispatch");
    assert!(
        edit.result.ok && edit.result.mutated,
        "{:?}",
        edit.result.error
    );
    let cert = session
        .expand("last_cert")
        .expect("durable edit certificate");
    assert!(
        String::from_utf8_lossy(&cert)
            .lines()
            .any(|line| line.starts_with("post=fz://"))
    );

    let after = std::fs::read_to_string(&file).expect("read back");
    assert!(
        after.contains("NEW_LABEL"),
        "journaled edit did not apply: {after}"
    );
    assert!(!after.contains("OLD_LABEL"), "{after}");
    let history =
        super::dispatch_codemode_method(&mut session, "fs.history", &json!({"path": &hit.path}))
            .expect("history dispatch");
    assert!(history.result.ok, "{:?}", history.result.error);
    let undo =
        super::dispatch_codemode_method(&mut session, "fs.undo", &json!({"path": &hit.path}))
            .expect("undo dispatch");
    assert!(
        undo.result.ok && undo.result.mutated,
        "{:?}",
        undo.result.error
    );
    assert_eq!(std::fs::read_to_string(&file).expect("undo read"), FIXTURE);
    assert_eq!(discovery_calls, 1, "no second discovery call was needed");

    if let Ok(out) = std::env::var("FSZERO_PROOF_ARTIFACT") {
        let transcript = format!(
            "# one-call proof transcript (fszero-snap-to-file-targets-99q7)\n\
             discovery_calls: {discovery_calls}\n\
             call: fs.search(\"OLD_LABEL\")\n\
             --- response ---\n{response}\n\
             --- derived, with no second discovery call ---\n\
             target_ref (verbatim to fs read): {}\n\
             file: {}\n\
             line_window: L{}-L{}\n\
             match_kind: {}\n\
             enclosing_symbol: {}\n\
             inlined_match_line: {}: {}\n\
             journaled_edit: replace OLD_LABEL -> NEW_LABEL at line {}\n\
             edit_result: certified, recorded, and undo verified\n\
             verbatim_read_ack: {}\n",
            hit.target,
            hit.path,
            hit.window.start,
            hit.window.end,
            hit.kind,
            hit.symbol,
            matched.0,
            matched.1,
            matched.0,
            via_target,
        );
        std::fs::write(out, transcript).expect("transcript");
    }
}

const FIXTURE: &str = "// header\nfn build_widget() {\n    let mut w = Widget::new();\n    w.set_label(\"OLD_LABEL\");\n    w\n}\n";

/// EVERY discovery route is one-call actionable, not just grep: structural
/// `defs:` / `callers:` / `imports` and `asgrep:` must all emit canonical HIT
/// records carrying `kind=` and `sym=`, so a subagent can act from one response.
#[test]
#[cfg_attr(
    not(feature = "fszero-ast-sgrep"),
    ignore = "callers:/imports/asgrep: routes need the fszero-ast-sgrep AST index"
)]
fn every_discovery_route_emits_actionable_hit_records() {
    let root = std::env::temp_dir().join("fszero_all_routes_proof_root");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).expect("root");
    std::fs::write(root.join("src/widget.rs"), FIXTURE).expect("fixture");
    std::fs::write(root.join("src/caller.rs"), CALLER_FIXTURE).expect("caller fixture");

    let mut session = FSZeroSession::with_root(&root);
    // Index once; each route below is then exactly ONE discovery call.
    let _ = session.do_search(Some(&root), Some("OLD_LABEL"));

    for (query, kind) in [
        ("defs:build_widget", "def"),
        ("callers:build_widget", "caller"),
        ("imports", "import"),
        ("asgrep:build_widget", "asgrep"),
    ] {
        let response = session.do_search(Some(&root), Some(query));
        assert!(
            response.contains("HIT "),
            "route {query} must emit canonical HIT records: {response}"
        );
        let hit = parse_first_hit(&response);
        assert!(
            root.join(&hit.path).exists(),
            "{query}: unknown path {}",
            hit.path
        );
        assert!(
            hit.window.start >= 1 && hit.window.end >= hit.window.start,
            "{query}"
        );
        assert!(!hit.kind.is_empty(), "{query}: kind must be set");
        assert!(!hit.symbol.is_empty(), "{query}: sym must be set");
        // asgrep ranks structural rows above literal ones, so the FIRST hit kind is
        // route-dependent; assert the route's own kind appears in the response.
        assert!(
            response.contains(&format!("kind={kind}")),
            "route {query} must emit kind={kind}: {response}"
        );
        // The target ref is accepted VERBATIM by read -- one follow-up call.
        let (path, window) = parse_target_ref(&hit.target).expect("canonical target ref");
        assert_eq!(path, hit.path);
        assert_eq!(window.start, hit.window.start);
        assert!(
            parse_read_arg(&hit.target).is_ok(),
            "{query}: read must accept {}",
            hit.target
        );
    }
}

const CALLER_FIXTURE: &str = "use crate::widget;\nfn make_all() {\n    build_widget();\n}\n";
