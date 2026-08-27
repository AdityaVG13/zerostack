//! ZeroRef v1 interoperability conformance gate (bead 1ghi.8).
//!
//! This test audits GraphZero's public claims against the frozen contract and
//! capability fixtures. It fails when broad interoperability wording appears
//! without an evidence marker, or when the capability descriptor drifts from
//! the runtime constants.
//!
//! The full three-binary matrix gate lives in `scripts/zeroref_conformance_gate.py`,
//! which is wired into release/CI and blocks while macOS/Linux/Windows evidence
//! is absent.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use graphzero_store::store::zeroref_capability::{
    EffectiveState, SharedInteropState, ZeroRefDescriptor, validate_peer_descriptor,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct CapabilityFixture {
    name: String,
    expect: String,
    descriptor: serde_json::Value,
}

#[derive(Deserialize)]
struct CapabilityFixtures {
    peers: Vec<CapabilityFixture>,
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/contracts"))
}

fn load_capability_fixtures() -> CapabilityFixtures {
    let path = fixture_dir().join("zeroref-capability-fixtures.json");
    let raw = fs::read_to_string(&path).expect("read zeroref-capability-fixtures.json");
    serde_json::from_str(&raw).expect("parse capability fixtures")
}

#[test]
fn descriptor_matches_runtime_constants() {
    let desc = ZeroRefDescriptor::from_env();
    assert_eq!(desc.schema, "zeroref-capability/v1");
    assert_eq!(desc.contract.major, 1);
    assert_eq!(desc.contract.minor, 0);
    assert_eq!(desc.hash.algorithm, "sha256");
    assert_eq!(desc.hash.hex_length, 64);
    assert!(!desc.hash.accept_uppercase);
    assert!(!desc.hash.accept_prefixes);
    assert_eq!(desc.schemes.accepted, vec!["fz", "gz", "tz"]);
    assert_eq!(desc.schemes.emitted, "gz");
    assert_eq!(
        desc.fragments.canonical,
        vec!["#B<start>-<end>", "#L<start>-<end>"]
    );
    assert_eq!(desc.fragments.byte_span, "zero-based-half-open");
    assert_eq!(desc.fragments.line_span, "one-based-inclusive");
    assert!(!desc.fragments.clamps);
    assert_eq!(desc.shared_cas.layout, "blobs/sha256/<hh>/<hash>");
    assert_eq!(desc.shared_cas.layout_version, 1);
    assert!(desc.shared_cas.read);
    assert!(desc.shared_cas.write);
    assert_eq!(desc.shared_cas.max_object_bytes, 256 * 1024 * 1024);
    let classes: BTreeSet<String> = desc.error_classes.iter().cloned().collect();
    let expected: BTreeSet<String> = [
        "malformed",
        "unsupported",
        "range_out_of_bounds",
        "not_utf8",
        "missing",
        "io",
        "digest_mismatch",
        "policy_denied",
        "incompatible_version",
        "legacy_ambiguity",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(classes, expected);
}

#[test]
fn descriptor_passes_enabled_peer_fixture() {
    let fixtures = load_capability_fixtures();
    let ours = ZeroRefDescriptor::current(EffectiveState {
        code_support: true,
        shared_interop: SharedInteropState::Enabled,
        shared_root_configured: true,
        detail: None,
    });
    let enabled = fixtures
        .peers
        .iter()
        .find(|p| p.name == "compatible_enabled")
        .expect("enabled fixture exists");
    assert_eq!(enabled.expect, "compatible");
    let compat =
        validate_peer_descriptor(&enabled.descriptor, &ours).expect("enabled peer is compatible");
    assert!(compat.shared_interop);
}

#[test]
fn descriptor_rejects_disabled_peer_for_shared_interop() {
    let fixtures = load_capability_fixtures();
    let ours = ZeroRefDescriptor::current(EffectiveState {
        code_support: true,
        shared_interop: SharedInteropState::Enabled,
        shared_root_configured: true,
        detail: None,
    });
    let disabled = fixtures
        .peers
        .iter()
        .find(|p| p.name == "compatible_but_peer_disabled")
        .expect("disabled fixture exists");
    let compat = validate_peer_descriptor(&disabled.descriptor, &ours)
        .expect("disabled peer is still compatible at contract level");
    assert!(
        !compat.shared_interop,
        "shared interop must be false when peer is disabled"
    );
}

#[test]
fn docs_do_not_make_unmarked_broad_interop_claims() {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let docs = [
        root.join("README.md"),
        root.join("docs/adr/002-zeroref-v1.md"),
        root.join("docs/contracts/zeroref-fixture-cli.md"),
    ];
    let broad_phrases = [
        "any scheme resolves anywhere",
        "all schemes resolve",
        "resolves across all engines",
        "universal interoperability",
        "any ref works anywhere",
        "every scheme works everywhere",
    ];
    for doc in &docs {
        if !doc.exists() {
            continue;
        }
        let text = fs::read_to_string(doc).expect("read doc");
        for (line_no, line) in text.lines().enumerate() {
            let lower = line.to_lowercase();
            for phrase in &broad_phrases {
                if lower.contains(phrase) {
                    assert!(
                        line.contains("<!--") || line.contains("evidence"),
                        "{}:{} contains unmarked broad interoperability claim: {:?}",
                        doc.display(),
                        line_no + 1,
                        phrase
                    );
                }
            }
        }
    }
}
