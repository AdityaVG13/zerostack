//! ZeroRef v1 capability negotiation contract (ADR 002 §11, bead
//! graphzero-zeroref-v1-shared-cas-1ghi.6): one descriptor source derived
//! from parser/store constants, strict peer validation against the shared
//! golden fixtures, and no secrets or private paths in the output.

use std::path::PathBuf;

use graphzero_store::store::shared_cas::{CAS_LAYOUT, CAS_LAYOUT_VERSION};
use graphzero_store::store::zeroref::{HASH_ALGORITHM, HASH_HEX_LEN, ZEROREF_MAJOR};
use graphzero_store::{
    CAS_MAX_OBJECT_BYTES, EffectiveState, SharedCas, SharedInteropState, ZeroRefDescriptor,
    validate_peer_descriptor,
};
use serde::Deserialize;

fn enabled_state() -> EffectiveState {
    EffectiveState {
        code_support: true,
        shared_interop: SharedInteropState::Enabled,
        shared_root_configured: true,
        detail: None,
    }
}

#[test]
fn descriptor_derives_from_parser_and_store_constants() {
    let descriptor = ZeroRefDescriptor::current(enabled_state());
    assert_eq!(descriptor.schema, "zeroref-capability/v1");
    assert_eq!(descriptor.contract.major, ZEROREF_MAJOR);
    assert_eq!(descriptor.hash.algorithm, HASH_ALGORITHM);
    assert_eq!(descriptor.hash.hex_length, HASH_HEX_LEN as u64);
    assert!(!descriptor.hash.accept_prefixes);
    assert!(!descriptor.hash.accept_uppercase);
    assert_eq!(descriptor.shared_cas.layout, CAS_LAYOUT);
    assert_eq!(descriptor.shared_cas.layout_version, CAS_LAYOUT_VERSION);
    assert_eq!(descriptor.shared_cas.max_object_bytes, CAS_MAX_OBJECT_BYTES);
    assert!(!descriptor.fragments.clamps);
    assert_eq!(descriptor.error_classes.len(), 10);
}

/// The advertised layout template and the real object path must agree.
#[test]
fn advertised_layout_matches_object_path() {
    let hash = "ab".repeat(32);
    let cas = SharedCas::open("/cas-root");
    let path = cas.object_path(&hash);
    let expected = CAS_LAYOUT
        .replace("<hh>", &hash[..2])
        .replace("<hash>", &hash);
    assert_eq!(
        path,
        std::path::Path::new("/cas-root").join(expected),
        "capability output would drift from the real layout"
    );
}

/// A caller must distinguish local-only support from configured cross-engine
/// interop without attempting an expand.
#[test]
fn effective_state_distinguishes_local_only_from_shared() {
    let descriptor = ZeroRefDescriptor::current(EffectiveState {
        code_support: true,
        shared_interop: SharedInteropState::Disabled,
        shared_root_configured: false,
        detail: Some("shared-store opt-in not set; operating local-only".to_string()),
    });
    assert!(descriptor.effective.code_support);
    assert_eq!(
        descriptor.effective.shared_interop,
        SharedInteropState::Disabled
    );
    let json = descriptor.to_json();
    assert_eq!(
        json.pointer("/effective/shared_interop").unwrap(),
        "disabled"
    );
    assert_eq!(json.pointer("/effective/code_support").unwrap(), true);
}

/// No secrets and no absolute private paths in capability output, in any
/// effective state the env probe can produce.
#[test]
#[serial_test::serial]
fn descriptor_never_leaks_paths() {
    let _lock = graphzero_test_support::lock_env();
    let shared = tempfile::tempdir().unwrap();
    let secret_path = shared.path().to_string_lossy().to_string();
    // SAFETY: shared ENV_LOCK + serial; vars restored below.
    unsafe {
        std::env::set_var("GRAPHZERO_SHARED_STORE", "1");
        std::env::set_var("ZEROSTACK_STORE_ROOT", &secret_path);
    }
    let rendered = serde_json::to_string(&ZeroRefDescriptor::from_env()).unwrap();
    unsafe {
        std::env::remove_var("GRAPHZERO_SHARED_STORE");
        std::env::remove_var("ZEROSTACK_STORE_ROOT");
    }
    assert!(
        !rendered.contains(&secret_path),
        "descriptor must not embed the shared root path"
    );
    assert!(rendered.contains("\"shared_root_configured\":true"));
}

#[derive(Deserialize)]
struct Fixture {
    fixture_version: u32,
    peers: Vec<PeerCase>,
}

#[derive(Deserialize)]
struct PeerCase {
    name: String,
    expect: String,
    #[serde(default)]
    error_class: Option<String>,
    #[serde(default)]
    shared_interop: Option<bool>,
    descriptor: serde_json::Value,
}

#[test]
fn golden_peer_fixtures_validate_with_stable_classes() {
    let raw = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/contracts/zeroref-capability-fixtures.json"),
    )
    .expect("read capability fixtures");
    let fixture: Fixture = serde_json::from_str(&raw).expect("parse capability fixtures");
    assert_eq!(fixture.fixture_version, 1);

    let ours = ZeroRefDescriptor::current(enabled_state());
    for case in &fixture.peers {
        let outcome = validate_peer_descriptor(&case.descriptor, &ours);
        match case.expect.as_str() {
            "compatible" => {
                let compat =
                    outcome.unwrap_or_else(|e| panic!("peer '{}' should validate: {e}", case.name));
                assert_eq!(
                    Some(compat.shared_interop),
                    case.shared_interop,
                    "peer '{}' shared_interop verdict (notes: {:?})",
                    case.name,
                    compat.notes
                );
                if !compat.shared_interop {
                    assert!(
                        !compat.notes.is_empty(),
                        "peer '{}' restriction must be explained",
                        case.name
                    );
                }
            }
            "error" => {
                let err = outcome.expect_err(&format!("peer '{}' must fail", case.name));
                assert_eq!(
                    err.class.as_str(),
                    case.error_class.as_deref().unwrap(),
                    "peer '{}' error class (message: {})",
                    case.name,
                    err.message
                );
                assert!(
                    !err.message.is_empty(),
                    "peer '{}' error must be actionable",
                    case.name
                );
            }
            other => panic!("peer '{}' has unknown expectation '{other}'", case.name),
        }
    }
}

/// When the local side is not enabled, a compatible peer still validates but
/// the combination is precisely explained and shared interop stays off.
#[test]
fn local_misconfiguration_is_explained_not_hidden() {
    let ours = ZeroRefDescriptor::current(EffectiveState {
        code_support: true,
        shared_interop: SharedInteropState::Misconfigured,
        shared_root_configured: false,
        detail: Some("shared-store opt-in set but no store-root env is configured".to_string()),
    });
    let peer = ZeroRefDescriptor::current(enabled_state()).to_json();
    let compat = validate_peer_descriptor(&peer, &ours).unwrap();
    assert!(!compat.shared_interop);
    assert!(
        compat
            .notes
            .iter()
            .any(|n| n.contains("misconfigured") && n.contains("no store-root env")),
        "notes must explain the local restriction precisely: {:?}",
        compat.notes
    );
}
