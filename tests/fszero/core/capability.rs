//! ZeroRef v1 capability negotiation contract (fszero-c6q.5).
//!
//! Pins: (1) the descriptor is derived from the SAME constants the parser
//! and store use, so a sample of accept/reject cases driven off the
//! descriptor's own claims must match real parser behavior; (2) peer
//! validation refuses incompatible major/hash/layout BEFORE payload work
//! with typed errors, while additive/newer-minor peers validate Ok;
//! (3) every effective state a caller must distinguish — local-only, v1
//! code support, shared CAS attached+writable, read-only CAS, degraded
//! store, legacy peer — is visible in the descriptor or the validator.

#[path = "../common/mod.rs"]
mod common;

use common::TestRoot;
use fs_zero::FSZeroSession;
use fs_zero::core::capability::CAPABILITY_STORE_KEY;
use fs_zero::core::validate_peer_descriptor;
use fs_zero::core::zeroref::{ZeroFragment, ZeroRef, ZeroRefErrorClass, select_fragment};
use serde_json::{Value, json};

fn session_with_store(prefix: &str) -> (TestRoot, FSZeroSession) {
    let root = TestRoot::new(prefix);
    root.write("README.md", "capability fixture\n");
    let sess = FSZeroSession::with_repo_store(root.path());
    (root, sess)
}

/// A workspace whose `.zerostack/blobs/` dir opts in to the shared CAS.
fn session_with_cas(prefix: &str) -> (TestRoot, FSZeroSession) {
    let root = TestRoot::new(prefix);
    root.write("README.md", "capability fixture\n");
    std::fs::create_dir_all(root.join(".zerostack/blobs")).unwrap();
    let sess = FSZeroSession::with_repo_store(root.path());
    (root, sess)
}

fn strings_of(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|s| s.as_str().expect("string").to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Descriptor shape
// ---------------------------------------------------------------------------

#[test]
fn descriptor_carries_the_negotiation_contract() {
    let (_root, sess) = session_with_store("cap_shape");
    let d = sess.capability_descriptor();

    assert_eq!(d["contract"]["name"], "zeroref");
    assert_eq!(d["contract"]["major"], 1);
    assert_eq!(d["contract"]["minor"], 0);
    assert_eq!(d["contract"]["version"], "v1");

    assert_eq!(d["hash"]["algo"], "sha256");
    assert_eq!(d["hash"]["hex_len"], 64);
    assert_eq!(d["hash"]["case"], "lower");

    assert_eq!(strings_of(&d["schemes"]["emits"]), ["fz"]);
    assert_eq!(strings_of(&d["schemes"]["reads"]), ["fz", "gz", "tz"]);

    assert_eq!(strings_of(&d["ref_kinds"]["portable"]), ["blob"]);
    assert_eq!(
        strings_of(&d["ref_kinds"]["engine_owned"]),
        ["seq", "file", "codemode"]
    );

    assert_eq!(d["fragments"]["byte"], "#B zero-based half-open");
    assert_eq!(d["fragments"]["line"], "#L one-based inclusive");
    assert_eq!(
        d["fragments"]["clamps"],
        serde_json::json!({ "byte": false, "line_start": false, "line_end": true })
    );
    assert_eq!(
        strings_of(&d["fragments"]["legacy_input_aliases"]),
        ["#B<start>+<len>"]
    );

    assert_eq!(d["legacy"]["mode"], "migration_window");
    let named = strings_of(&d["legacy"]["named_keys"]);
    for key in ["read", "stat", "search", "ls_manifest"] {
        assert!(
            named.contains(&key.to_string()),
            "named key {key}: {named:?}"
        );
    }

    assert_eq!(d["shared_cas"]["layout"], "blobs/sha256/<hh>/<hash>");
    assert_eq!(d["shared_cas"]["version"], 1);

    // Interop note distinguishes SYNTAX support from real foreign reads.
    let note = d["interop"]["note"].as_str().unwrap();
    assert!(note.contains("same-store retag"), "note: {note}");
    assert!(note.contains("SYNTAX"), "note: {note}");
    assert_eq!(d["interop"]["foreign_blob_reads"], "same_store_retag");

    // Size limits: explicitly null, never omitted.
    assert!(
        d["limits"].get("max_object_bytes").is_some(),
        "max_object_bytes must be present"
    );
    assert!(d["limits"]["max_object_bytes"].is_null());

    assert_eq!(strings_of(&d["error_classes"]).len(), 10);
}

#[test]
fn descriptor_contains_no_absolute_private_paths() {
    let (root, sess) = session_with_store("cap_paths");
    let rendered = sess.capability_descriptor().to_string();
    let root_display = root.path().display().to_string();
    assert!(
        !rendered.contains(&root_display),
        "descriptor leaks the workspace path: {rendered}"
    );
    let tmp = std::env::temp_dir().display().to_string();
    assert!(
        !rendered.contains(&tmp),
        "descriptor leaks a temp-dir path: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// Parser behavior matches descriptor claims (anti-drift)
// ---------------------------------------------------------------------------

#[test]
fn parser_behavior_matches_descriptor_claims() {
    let (_root, sess) = session_with_store("cap_parser");
    let d = sess.capability_descriptor();
    let hex_len = d["hash"]["hex_len"].as_u64().unwrap() as usize;
    let good_hash = "a".repeat(hex_len);

    // Every advertised read scheme parses a portable blob ref.
    for scheme in strings_of(&d["schemes"]["reads"]) {
        let r = format!("{scheme}://blob/{good_hash}");
        assert!(
            ZeroRef::parse(&r).is_ok(),
            "advertised scheme rejected: {r}"
        );
    }
    // An unadvertised scheme is unsupported.
    let alien = format!("qq://blob/{good_hash}");
    assert!(!strings_of(&d["schemes"]["reads"]).contains(&"qq".to_string()));
    assert_eq!(
        ZeroRef::parse(&alien).unwrap_err().class,
        ZeroRefErrorClass::Unsupported
    );

    // Only advertised portable kinds parse; others are engine-owned.
    let portable = strings_of(&d["ref_kinds"]["portable"]);
    assert!(!portable.contains(&"node".to_string()));
    assert_eq!(
        ZeroRef::parse(&format!("fz://node/{good_hash}"))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Unsupported
    );

    // hash.case == "lower": uppercase is malformed. hex_len is exact.
    assert_eq!(d["hash"]["case"], "lower");
    assert_eq!(
        ZeroRef::parse(&format!("fz://blob/{}", "A".repeat(hex_len)))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Malformed
    );
    assert_eq!(
        ZeroRef::parse(&format!("fz://blob/{}", "a".repeat(hex_len - 1)))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Malformed
    );

    // Byte fragments: zero-based half-open — empty selection allowed,
    // reversed span never parses.
    let empty = ZeroRef::parse(&format!("fz://blob/{good_hash}#B0-0")).unwrap();
    assert_eq!(empty.fragment, ZeroFragment::Bytes { start: 0, end: 0 });
    assert_eq!(
        ZeroRef::parse(&format!("fz://blob/{good_hash}#B5-2"))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Malformed
    );

    // Line fragments: one-based inclusive — #L0 never parses.
    assert_eq!(
        ZeroRef::parse(&format!("fz://blob/{good_hash}#L0-1"))
            .unwrap_err()
            .class,
        ZeroRefErrorClass::Malformed
    );
    let lines = ZeroRef::parse(&format!("fz://blob/{good_hash}#L1-1")).unwrap();
    assert_eq!(
        select_fragment(b"a\nb\n", &lines.fragment, "test").unwrap(),
        b"a\n"
    );

    // Clamp policy matches the descriptor (fszero-00vq): byte spans stay
    // strict, line-span ends clamp to EOF, line starts past EOF still error.
    assert_eq!(
        d["fragments"]["clamps"],
        serde_json::json!({ "byte": false, "line_start": false, "line_end": true })
    );
    let wide = ZeroRef::parse(&format!("fz://blob/{good_hash}#B0-999")).unwrap();
    assert_eq!(
        select_fragment(b"abc", &wide.fragment, "test")
            .unwrap_err()
            .class,
        ZeroRefErrorClass::RangeOutOfBounds
    );
    let tall = ZeroRef::parse(&format!("fz://blob/{good_hash}#L1-200")).unwrap();
    assert_eq!(
        select_fragment(
            b"a
b
",
            &tall.fragment,
            "test"
        )
        .unwrap(),
        b"a
b
",
        "line-span END past EOF clamps to the last line"
    );
    let past = ZeroRef::parse(&format!("fz://blob/{good_hash}#L50-60")).unwrap();
    assert_eq!(
        select_fragment(
            b"a
b
",
            &past.fragment,
            "test"
        )
        .unwrap_err()
        .class,
        ZeroRefErrorClass::RangeOutOfBounds,
        "line-span START past EOF still errors"
    );

    // Legacy alias accepted on input, canonical form on emission.
    let aliased = ZeroRef::parse(&format!("fz://blob/{good_hash}#B2+3")).unwrap();
    assert_eq!(aliased.fragment, ZeroFragment::Bytes { start: 2, end: 5 });
    assert!(aliased.to_string().ends_with("#B2-5"), "{aliased}");
}

#[test]
fn minted_refs_use_the_advertised_emit_scheme() {
    let (_root, mut sess) = session_with_store("cap_mint");
    let d = sess.capability_descriptor();
    let emits = strings_of(&d["schemes"]["emits"]);
    let r = sess.recovery.put_content_ref(b"capability mint sample");
    assert!(
        r.starts_with(&format!("{}://blob/", emits[0])),
        "mint {r} does not use advertised emit scheme {emits:?}"
    );
}

// ---------------------------------------------------------------------------
// Peer validation fixtures (table-driven)
// ---------------------------------------------------------------------------

enum Expect {
    Ok,
    ErrPrefix(&'static str),
}

#[test]
fn peer_descriptor_fixtures() {
    let (_root, sess) = session_with_store("cap_peer");
    let ours = sess.capability_descriptor();

    let mut disabled = ours.clone();
    disabled["shared_cas"]["attached"] = json!(false);
    disabled["shared_cas"]["writable"] = json!(false);
    disabled["interop"]["shared_interop"] = json!("disabled");

    let mut newer_major = ours.clone();
    newer_major["contract"]["major"] = json!(2);

    let mut additive_minor = ours.clone();
    additive_minor["contract"]["minor"] = json!(7);
    additive_minor["contract"]["experimental_channel"] = json!("beta");
    additive_minor["totally_new_section"] = json!({"future": true});

    let mut wrong_algo = ours.clone();
    wrong_algo["hash"]["algo"] = json!("blake3");

    let mut wrong_hex_len = ours.clone();
    wrong_hex_len["hash"]["hex_len"] = json!(40);

    let mut wrong_layout_version = ours.clone();
    wrong_layout_version["shared_cas"]["version"] = json!(2);

    let mut wrong_layout = ours.clone();
    wrong_layout["shared_cas"]["layout"] = json!("objects/<hash>");

    // A real sibling-engine shape (GraphZero key spellings + placeholder
    // dialect) must validate: same contract, aliased field names.
    let graphzero_shape = json!({
        "schema": "zeroref-capability/v1",
        "contract": {"major": 1, "minor": 0},
        "hash": {"algorithm": "sha256", "hex_length": 64,
                 "accept_uppercase": false, "accept_prefixes": false},
        "schemes": {"accepted": ["fz", "gz", "tz"], "emitted": "gz"},
        "shared_cas": {"layout": "blobs/sha256/<hh>/<hash>", "layout_version": 1,
                        "read": true, "write": true},
        "effective": {"code_support": true, "shared_interop": "disabled"},
    });

    let cases: Vec<(&str, Value, Expect)> = vec![
        ("compatible", ours.clone(), Expect::Ok),
        ("disabled", disabled, Expect::Ok),
        ("graphzero_shape", graphzero_shape, Expect::Ok),
        ("additive_minor", additive_minor, Expect::Ok),
        (
            "missing",
            Value::Null,
            Expect::ErrPrefix("missing_capability"),
        ),
        (
            "malformed_not_object",
            json!("zeroref v1, trust me"),
            Expect::ErrPrefix("malformed_capability"),
        ),
        (
            "malformed_type_broken_major",
            json!({"contract": {"name": "zeroref", "major": "one"},
                   "hash": {"algo": "sha256", "hex_len": 64},
                   "shared_cas": {"version": 1}}),
            Expect::ErrPrefix("malformed_capability"),
        ),
        (
            "malformed_missing_hash",
            json!({"contract": {"name": "zeroref", "major": 1},
                   "shared_cas": {"version": 1}}),
            Expect::ErrPrefix("malformed_capability"),
        ),
        (
            "newer_major",
            newer_major,
            Expect::ErrPrefix("incompatible_capability"),
        ),
        (
            "wrong_hash_algo",
            wrong_algo,
            Expect::ErrPrefix("incompatible_capability"),
        ),
        (
            "wrong_hex_len",
            wrong_hex_len,
            Expect::ErrPrefix("incompatible_capability"),
        ),
        (
            "wrong_layout_version",
            wrong_layout_version,
            Expect::ErrPrefix("incompatible_capability"),
        ),
        (
            "wrong_layout_semantics",
            wrong_layout,
            Expect::ErrPrefix("incompatible_capability"),
        ),
    ];

    for (name, peer, expect) in cases {
        let got = validate_peer_descriptor(&ours, &peer);
        match expect {
            Expect::Ok => assert!(got.is_ok(), "fixture {name}: expected Ok, got {got:?}"),
            Expect::ErrPrefix(prefix) => {
                let err = got.expect_err(&format!("fixture {name}: expected Err"));
                assert!(
                    err.starts_with(prefix),
                    "fixture {name}: expected '{prefix}: …', got: {err}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Effective states a caller must distinguish
// ---------------------------------------------------------------------------

#[test]
fn state_local_only_no_cas() {
    let (_root, sess) = session_with_store("cap_local");
    let d = sess.capability_descriptor();
    // (b) v1 code support is always advertised…
    assert_eq!(d["contract"]["major"], 1);
    // …(a) but without an attached CAS resolution is FSZero-local only.
    assert_eq!(d["shared_cas"]["attached"], false);
    assert_eq!(d["shared_cas"]["writable"], false);
    assert_eq!(d["shared_cas"]["store_state"], "durable");
    assert_eq!(d["interop"]["shared_interop"], "disabled");
    assert_eq!(d["remediation"].as_array().unwrap().len(), 0);
}

#[test]
fn state_shared_cas_attached_and_writable() {
    let (_root, sess) = session_with_cas("cap_shared");
    let d = sess.capability_descriptor();
    // (c) configured shared interoperability.
    assert_eq!(d["shared_cas"]["attached"], true);
    assert_eq!(d["shared_cas"]["writable"], true);
    assert_eq!(d["shared_cas"]["store_state"], "durable");
    assert_eq!(d["interop"]["shared_interop"], "enabled");
}

#[cfg(unix)]
#[test]
fn state_cas_attached_but_read_only_carries_remediation() {
    use std::os::unix::fs::PermissionsExt;
    let (root, sess) = session_with_cas("cap_ro");
    let blobs = root.join(".zerostack/blobs");
    std::fs::set_permissions(&blobs, std::fs::Permissions::from_mode(0o555)).unwrap();
    let d = sess.capability_descriptor();
    // Restore before asserting so TestRoot::drop can clean up either way.
    std::fs::set_permissions(&blobs, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(d["shared_cas"]["attached"], true);
    assert_eq!(d["shared_cas"]["writable"], false);
    assert_eq!(d["interop"]["shared_interop"], "read_only");
    let remediation = strings_of(&d["remediation"]);
    assert!(
        remediation.iter().any(|r| r.contains("not writable")),
        "expected writability remediation, got: {remediation:?}"
    );
}

#[test]
fn state_degraded_store_is_visible_with_remediation() {
    // (d) in-memory recovery (durable fallback / no durable store).
    let root = TestRoot::new("cap_degraded");
    root.write("README.md", "x\n");
    let mut sess = FSZeroSession::with_root(root.path());
    sess.durable_degraded = true;
    let d = sess.capability_descriptor();
    assert_eq!(d["shared_cas"]["store_state"], "degraded");
    let remediation = strings_of(&d["remediation"]);
    assert!(
        remediation
            .iter()
            .any(|r| r.contains("durable store unavailable")),
        "expected degraded-store remediation, got: {remediation:?}"
    );
    // Even an in-memory session without the forced flag reports degraded
    // durability: refs will not survive the process.
    let plain = FSZeroSession::with_root(root.path());
    assert_eq!(
        plain.capability_descriptor()["shared_cas"]["store_state"],
        "degraded"
    );
}

#[test]
fn state_legacy_peer_without_descriptor_is_typed() {
    let (_root, sess) = session_with_store("cap_legacy");
    let ours = sess.capability_descriptor();
    // (e) legacy-only peer: no descriptor at all.
    let err = validate_peer_descriptor(&ours, &Value::Null).unwrap_err();
    assert!(err.starts_with("missing_capability"), "{err}");
    assert!(err.contains("legacy"), "{err}");
}

// ---------------------------------------------------------------------------
// Surfaces: store key + root report
// ---------------------------------------------------------------------------

#[test]
fn capabilities_store_key_is_expandable() {
    let (_root, sess) = session_with_store("cap_expand");
    let bytes = sess
        .expand(CAPABILITY_STORE_KEY)
        .expect("capabilities key published at session init");
    let stored: Value = serde_json::from_slice(&bytes).expect("stored descriptor is JSON");
    assert_eq!(stored, sess.capability_descriptor());
}

#[test]
fn root_report_carries_the_capability_section() {
    let (_root, sess) = session_with_store("cap_report");
    let report = sess.root_report();
    let caps = report.get("capabilities").expect("capabilities section");
    assert_eq!(caps["contract"]["name"], "zeroref");
    assert_eq!(caps["contract"]["major"], 1);
    // The doctor-shaped report and the negotiation descriptor are one thing.
    assert_eq!(*caps, sess.capability_descriptor());
}
