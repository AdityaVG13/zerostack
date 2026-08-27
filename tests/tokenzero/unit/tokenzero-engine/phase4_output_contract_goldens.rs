//! Phase 4 golden capture: measure / project / expand / account.
//!
//! Tiers are labeled and never papered over:
//! - **Tier 1 byte** (`Tier1Raw`): expand returns the stored payload SHA-256-equal.
//! - **Tier 2 canonical** (`Tier2Canonical`): project capsule vs passthrough after
//!   handle-stable canonical JSON (ZeroHandle is content-addressed).
//! - **Tier 3 logical** (`Tier3Logical`): measure/account 4-tuple; estimator
//!   counts are logical (same disclosed heuristic), never claimed byte-exact BPE.

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokenzero_core::{
    Accounting, BYTES_ESTIMATOR_ID, LEXICAL_ESTIMATOR_ID, Mode, TokenizerIdPreflightError,
    ToolResponse, UNLABELED_ESTIMATE_TOKENIZER_PREFIX, count_tokens_tokenizer_id,
    preflight_tokenizer_id,
};
use tokenzero_kernel::{
    AccountMass, ZeroTokenEngine, account_mass, estimator_tokenizer_id, tiktoken_tokenizer_id,
};
use tokenzero_pulse::PulseEvent;
use tokenzero_test_support::{GauntletIdentityPair, GauntletOracle, ScenarioAgreement, scenario};
use zero_abi::{
    CompressionRequest, ExpandOptions, ProjectionRequest, TokenAccounting, TokenEngine,
};
use zerostack_test_support::{TempWorkspace, test_invocation};

const TIER1_EXPAND: &str =
    include_str!("../../fixtures/tier1/curated-corpus/expand-hello-world.golden");
const TIER2_PASSTHROUGH: &str =
    include_str!("../../fixtures/tier2/curated-corpus/project-passthrough.golden");
const TIER2_CAPSULE: &str =
    include_str!("../../fixtures/tier2/curated-corpus/project-capsule.golden");
const TIER3_MEASURE: &str =
    include_str!("../../fixtures/tier3/curated-corpus/measure-account-lexical.golden");
const TIER3_LLAMA: &str =
    include_str!("../../fixtures/tier3/curated-corpus/measure-account-sentencepiece-approx.golden");
const TIER1_SUMS: &str = include_str!("../../fixtures/tier1/curated-corpus/checksums.sha256");
const TIER2_SUMS: &str = include_str!("../../fixtures/tier2/curated-corpus/checksums.sha256");
const TIER3_SUMS: &str = include_str!("../../fixtures/tier3/curated-corpus/checksums.sha256");
const TIER1_MANIFEST: &str = include_str!("../../fixtures/tier1/curated-corpus/manifest.v1.json");
const TIER2_MANIFEST: &str = include_str!("../../fixtures/tier2/curated-corpus/manifest.v1.json");
const TIER3_MANIFEST: &str = include_str!("../../fixtures/tier3/curated-corpus/manifest.v1.json");

const REPRO: &str =
    "cargo test -p tokenzero-engine --test phase4_output_contract_goldens -- --test-threads=1";

const PAYLOAD: &[u8] = b"hello world";

fn capsule_source() -> String {
    (0..80)
        .map(|i| format!("tok{i:02}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn framing_overhead_source() -> String {
    "a".repeat(200)
}

fn stamp() {
    GauntletIdentityPair::new(GauntletOracle::Spec).assert_distinct();
}

fn engine() -> (TempWorkspace, ZeroTokenEngine, zero_abi::EngineInvocation) {
    let ws = TempWorkspace::new("tz-phase4-golden").expect("workspace");
    let invocation = test_invocation(ws.root(), "phase4-golden", "cell-1");
    let engine = ZeroTokenEngine::open_unbound(ws.store(), None);
    (ws, engine, invocation)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn parse_json(s: &str) -> Value {
    serde_json::from_str(s.trim()).expect("golden JSON")
}

fn canonical_project(
    visible: &str,
    source: &[u8],
    visible_source_bytes: u64,
    exact: Option<&str>,
    accounting: &TokenAccounting,
) -> Value {
    json!({
        "accounting": {
            "billed": accounting.billed,
            "cached": accounting.cached,
            "certified": accounting.certified,
            "tokenizer": accounting.tokenizer,
            "visible": accounting.visible,
        },
        "exact": exact,
        "passthrough": exact.is_none() && visible.as_bytes() == source,
        "tier": "Tier2Canonical",
        "visible": visible,
        "visible_source_bytes": visible_source_bytes,
    })
}

fn canonical_account(accounting: &TokenAccounting, mass: AccountMass) -> Value {
    json!({
        "account": {
            "raw": mass.raw,
            "recovered": mass.recovered,
            "spent": mass.spent,
            "visible": mass.visible,
        },
        "certified": accounting.certified,
        "equivalence_predicate": "logical_token_mass",
        "tier": "Tier3Logical",
        "tokenizer": accounting.tokenizer,
        "token_accounting": {
            "billed": accounting.billed,
            "cached": accounting.cached,
            "certified": accounting.certified,
            "tokenizer": accounting.tokenizer,
            "visible": accounting.visible,
        },
    })
}

fn pulse_accepts_estimator(id: &str) {
    preflight_tokenizer_id(id).unwrap_or_else(|err| panic!("{id} failed honesty preflight: {err}"));
    PulseEvent::tool_call("measure", "auto", 1, 1, 0, 0, 0, None)
        .with_tokenizer_id(id)
        .unwrap_or_else(|_| panic!("{id} must satisfy Pulse estimator:<slug> grammar"));
}

#[test]
fn kernel_and_pulse_refuse_unlabeled_estimate_alias() {
    stamp();
    assert_eq!(UNLABELED_ESTIMATE_TOKENIZER_PREFIX, "estimate:");
    assert_eq!(
        preflight_tokenizer_id("estimate:tokenzero-lexical"),
        Err(TokenizerIdPreflightError::UnlabeledEstimateAlias)
    );
    let unlabeled = PulseEvent::tool_call("measure", "auto", 1, 1, 0, 0, 0, None)
        .with_tokenizer_id("estimate:tokenzero-lexical")
        .expect_err("Pulse must refuse estimate:");
    assert!(unlabeled.contains("estimate:"));
    let (_ws, engine, invocation) = engine();
    let measured = engine.measure(&invocation, PAYLOAD).expect("measure");
    preflight_tokenizer_id(&measured.tokenizer).expect("kernel emit must pass preflight");
    assert!(
        !measured
            .tokenizer
            .starts_with(UNLABELED_ESTIMATE_TOKENIZER_PREFIX),
        "kernel must not emit unlabeled estimate: ids, got {}",
        measured.tokenizer
    );
    assert!(measured.tokenizer.starts_with("estimator:"));
    pulse_accepts_estimator(&measured.tokenizer);
}

#[test]
fn tier1_expand_is_byte_exact_against_checked_in_golden() {
    stamp();
    let (_ws, engine, invocation) = engine();
    assert_eq!(TIER1_EXPAND.as_bytes(), PAYLOAD, "golden payload drift");
    let projected = engine
        .project(
            &invocation,
            ProjectionRequest {
                bytes: PAYLOAD.to_vec(),
                visible_byte_limit: 32,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .expect("project under limit is passthrough; store still used by compress");
    // Passthrough has no handle. Compress always stores an exact handle.
    let compressed = engine
        .compress(
            &invocation,
            zero_abi::CompressionRequest {
                bytes: PAYLOAD.to_vec(),
                max_tokens: 64,
                mode: "passthrough".into(),
                label: None,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .expect("compress");
    let expanded = engine
        .expand(&invocation, &compressed.exact, ExpandOptions::default())
        .expect("expand");
    assert_eq!(
        expanded.as_slice(),
        TIER1_EXPAND.as_bytes(),
        "Tier1Raw: expand must be SHA-256-equal to the checked-in payload"
    );
    assert_eq!(sha256_hex(&expanded), sha256_hex(TIER1_EXPAND.as_bytes()));
    assert!(projected.exact.is_none(), "tiny payload is passthrough");
    let _ = REPRO;
}

#[test]
fn tier2_project_passthrough_matches_canonical_golden() {
    stamp();
    let (_ws, engine, invocation) = engine();
    let result = engine
        .project(
            &invocation,
            ProjectionRequest {
                bytes: PAYLOAD.to_vec(),
                visible_byte_limit: 1024,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .expect("passthrough project");
    assert!(result.exact.is_none());
    assert_eq!(result.visible.as_bytes(), PAYLOAD);
    let got = canonical_project(
        &result.visible,
        PAYLOAD,
        result.visible_source_bytes,
        None,
        &result.accounting,
    );
    assert_eq!(got, parse_json(TIER2_PASSTHROUGH));
    assert!(!result.accounting.certified);
    assert_eq!(result.accounting.tokenizer, LEXICAL_ESTIMATOR_ID);
}

#[test]
fn tier2_project_capsule_matches_canonical_golden() {
    stamp();
    let (_ws, engine, invocation) = engine();
    assert!(capsule_source().len() > 80);
    let result = engine
        .project(
            &invocation,
            ProjectionRequest {
                bytes: capsule_source().as_bytes().to_vec(),
                visible_byte_limit: 120,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .expect("capsule project");
    let handle = result
        .exact
        .as_ref()
        .expect("over-limit project stores exact");
    let got = canonical_project(
        &result.visible,
        capsule_source().as_bytes(),
        result.visible_source_bytes,
        Some(handle.as_str()),
        &result.accounting,
    );
    assert_eq!(got, parse_json(TIER2_CAPSULE));
    assert!(result.visible.contains("exact: "));
    assert_ne!(
        result.visible.as_str(),
        capsule_source(),
        "capsule is not passthrough"
    );
    assert!(
        result.accounting.visible < result.accounting.billed,
        "honest capsule must save vs raw payload (visible={} billed={})",
        result.accounting.visible,
        result.accounting.billed
    );
    let expanded = engine
        .expand(&invocation, handle, ExpandOptions::default())
        .expect("expand capsule handle");
    assert_eq!(
        expanded,
        capsule_source().as_bytes(),
        "Tier1 recoverability of capsule"
    );
}

#[test]
fn tier3_measure_account_lexical_is_logical_not_byte_exact_bpe() {
    stamp();
    let (_ws, engine, invocation) = engine();
    let measured = engine.measure(&invocation, PAYLOAD).expect("measure");
    let mass = account_mass(&measured);
    assert_eq!(mass.raw, measured.billed);
    assert_eq!(mass.visible, measured.visible);
    assert_eq!(mass.spent, measured.visible);
    assert_eq!(mass.recovered, measured.cached);
    assert_eq!(mass.recovered, 0, "measure does not invent recovered mass");
    assert!(
        !measured.certified,
        "lexical gauge is an estimator, not exact"
    );
    assert_eq!(measured.tokenizer, LEXICAL_ESTIMATOR_ID);
    pulse_accepts_estimator(&measured.tokenizer);
    let got = canonical_account(&measured, mass);
    assert_eq!(got, parse_json(TIER3_MEASURE));
}

#[test]
fn tier3_sentencepiece_estimate_is_labelled_estimator_never_certified() {
    stamp();
    let ws = TempWorkspace::new("tz-phase4-llama").expect("workspace");
    let invocation = test_invocation(ws.root(), "phase4-llama", "cell-1");
    let engine = ZeroTokenEngine::open_unbound(ws.store(), Some("llama-3.1-8b-instruct".into()));
    let measured = engine.measure(&invocation, PAYLOAD).expect("measure");
    assert!(!measured.certified);
    assert_eq!(
        measured.tokenizer,
        estimator_tokenizer_id(Some("llama-3.1-8b-instruct"))
    );
    assert!(measured.tokenizer.starts_with("estimator:"));
    assert_ne!(measured.tokenizer, "llama-3.1-8b-instruct");
    pulse_accepts_estimator(&measured.tokenizer);
    let got = canonical_account(&measured, account_mass(&measured));
    assert_eq!(got, parse_json(TIER3_LLAMA));
}

#[test]
fn tiktoken_bpe_is_certified_but_not_pulse_exact_identity() {
    stamp();
    let ws = TempWorkspace::new("tz-phase4-tiktoken").expect("workspace");
    let invocation = test_invocation(ws.root(), "phase4-tiktoken", "cell-1");
    let engine = ZeroTokenEngine::open_unbound(ws.store(), Some("gpt-4o".into()));
    let measured = engine.measure(&invocation, PAYLOAD).expect("measure");
    assert!(
        measured.certified,
        "bundled tiktoken BPE is exact for the encoding"
    );
    assert_eq!(measured.tokenizer, tiktoken_tokenizer_id("gpt-4o"));
    assert_eq!(measured.tokenizer, "tiktoken:o200k_base");
    assert_ne!(
        measured.tokenizer, "gpt-4o",
        "bare model ids are unlabeled exact identities"
    );
    let recorded = PulseEvent::tool_call("measure", "auto", 1, 1, 0, 0, 0, None)
        .with_tokenizer_id(&measured.tokenizer)
        .expect("kernel tiktoken: must be Pulse-legal after Pass 10 grammar");
    assert_eq!(recorded.tokenizer_id, "tiktoken:o200k_base");
    assert!(
        !measured.tokenizer.contains('@'),
        "tiktoken:encoding is not Pulse provider/model@hex; do not smuggle it as ExactTokenizerIdentity"
    );
}

#[test]
fn non_utf8_measure_uses_byte_estimator() {
    stamp();
    let (_ws, engine, invocation) = engine();
    let measured = engine.measure(&invocation, &[0xff, 0xfe]).expect("measure");
    assert!(!measured.certified);
    assert_eq!(measured.tokenizer, BYTES_ESTIMATOR_ID);
    assert_eq!(measured.billed, 2);
    pulse_accepts_estimator(&measured.tokenizer);
}

#[test]
fn checked_in_goldens_match_checksums_and_tier_labels() {
    stamp();
    let files = [
        (
            "expand-hello-world.golden",
            TIER1_EXPAND.as_bytes(),
            TIER1_SUMS,
        ),
        (
            "project-passthrough.golden",
            TIER2_PASSTHROUGH.as_bytes(),
            TIER2_SUMS,
        ),
        (
            "project-capsule.golden",
            TIER2_CAPSULE.as_bytes(),
            TIER2_SUMS,
        ),
        (
            "measure-account-lexical.golden",
            TIER3_MEASURE.as_bytes(),
            TIER3_SUMS,
        ),
        (
            "measure-account-sentencepiece-approx.golden",
            TIER3_LLAMA.as_bytes(),
            TIER3_SUMS,
        ),
    ];
    for (name, bytes, sums) in files {
        let expected = sums
            .lines()
            .find_map(|line| {
                let (hash, file) = line.split_once("  ")?;
                (file == name).then_some(hash)
            })
            .unwrap_or_else(|| panic!("checksums missing {name}"));
        assert_eq!(sha256_hex(bytes), expected, "{name}");
    }
    for (tier, manifest) in [
        ("Tier1Raw", TIER1_MANIFEST),
        ("Tier2Canonical", TIER2_MANIFEST),
        ("Tier3Logical", TIER3_MANIFEST),
    ] {
        let v = parse_json(manifest);
        assert_eq!(v["tier"], tier);
        assert_eq!(v["schema"], "gauntlet.golden.manifest.v1");
        let ids: Vec<&str> = v["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|e| e["fixture_id"].as_str().expect("id"))
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(
            ids, sorted,
            "{tier} manifest entries must be sorted by fixture_id"
        );
        for entry in v["entries"].as_array().expect("entries") {
            assert_eq!(entry["tier"], tier, "never paper over the tier");
        }
    }
}

#[test]
fn account_mass_framing_overhead_passthroughs_never_worse() {
    stamp();
    let pair = GauntletIdentityPair::new(GauntletOracle::Spec);
    let (_ws, engine, invocation) = engine();
    let source = framing_overhead_source();
    let result = engine
        .project(
            &invocation,
            ProjectionRequest {
                bytes: source.as_bytes().to_vec(),
                visible_byte_limit: 120,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .expect("project");
    let mass = account_mass(&result.accounting);
    match scenario(
        "project-framing-overhead-never-worse",
        pair,
        || Ok(mass),
        || {
            if mass.spent > mass.raw {
                Err("never-worse: capsule spent exceeds raw payload")
            } else {
                Ok(())
            }
        },
    ) {
        ScenarioAgreement::BothOk(_) => {}
        ScenarioAgreement::BothErr { .. } => {
            panic!("subject Ok / spec Err is the hard-fail shape; both-error is not this case")
        }
    }
    assert_eq!(mass.raw, 1);
    assert_eq!(mass.spent, 1);
    assert!(
        result.exact.is_none(),
        "worse-than-raw capsule must passthrough"
    );
    assert_eq!(result.visible, source);
    assert_eq!(result.accounting.tokenizer, LEXICAL_ESTIMATOR_ID);
    assert!(
        result.accounting.tokenizer.starts_with("estimator:"),
        "unlabeled estimate is not a tokenizer id"
    );
    assert_eq!(mass.recovered, 0);
}

#[test]
fn compress_tiny_exact_is_never_worse_than_raw() {
    stamp();
    let pair = GauntletIdentityPair::new(GauntletOracle::Spec);
    let (_ws, engine, invocation) = engine();
    let source = "hi";
    let result = engine
        .compress(
            &invocation,
            CompressionRequest {
                bytes: source.as_bytes().to_vec(),
                max_tokens: 64,
                mode: "exact".into(),
                label: None,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .expect("compress");
    let mass = account_mass(&result.accounting);
    match scenario(
        "compress-tiny-exact-never-worse",
        pair,
        || Ok(mass),
        || {
            if mass.spent > mass.raw {
                Err("never-worse: compress spent exceeds raw payload")
            } else {
                Ok(())
            }
        },
    ) {
        ScenarioAgreement::BothOk(_) => {}
        ScenarioAgreement::BothErr { .. } => {
            panic!("subject Ok / spec Err is the hard-fail shape; both-error is not this case")
        }
    }
    assert!(mass.spent <= mass.raw);
    assert_eq!(result.omitted_tokens, 0);
    assert_eq!(result.visible.trim_end(), source);
    assert!(result.accounting.tokenizer.starts_with("estimator:"));
}

#[test]
fn compress_tight_budget_does_not_save_by_clamping_a_worse_wrapper() {
    stamp();
    let (_ws, engine, invocation) = engine();
    let source = "hi";
    let result = engine
        .compress(
            &invocation,
            CompressionRequest {
                bytes: source.as_bytes().to_vec(),
                max_tokens: 2,
                mode: "exact".into(),
                label: None,
                media_type: "text/plain; charset=utf-8".into(),
            },
        )
        .expect("compress");
    let mass = account_mass(&result.accounting);
    assert!(mass.spent <= mass.raw);
    assert_eq!(
        result.omitted_tokens, 0,
        "clamping an exact stub that costs more than raw must not report omitted savings"
    );
    assert_eq!(result.visible.trim_end(), source);
    let expanded = engine
        .expand(&invocation, &result.exact, ExpandOptions::default())
        .expect("expand handle after compress persist");
    assert_eq!(
        expanded,
        source.as_bytes(),
        "expand must return original bytes after compress store"
    );
}

#[test]
fn mcp_toolresponse_accounting_json_has_kernel_measure_tokenizer_id() {
    stamp();
    let response = ToolResponse::ok(
        "read",
        Mode::Auto,
        "hello".into(),
        Vec::new(),
        Accounting::measured(10, 4, 6, 4, 0, Some(0)),
    );
    let json = serde_json::to_value(&response).expect("serialize ToolResponse");
    let accounting = &json["accounting"];
    let id = accounting["tokenizer_id"]
        .as_str()
        .expect("MCP Accounting JSON must emit tokenizer_id");
    preflight_tokenizer_id(id).expect("tokenizer_id must pass honesty preflight");
    assert_eq!(id, count_tokens_tokenizer_id());
    assert!(
        id.starts_with("estimator:"),
        "MCP accounting must be kernel measure estimator or tiktoken, got {id}"
    );
    assert!(
        !id.contains('@'),
        "MCP accounting must not invent ExactTokenizerIdentity: {id}"
    );
    assert_eq!(accounting["certified"], false);
    assert_eq!(accounting["spent_tokens"], 10);
    assert_eq!(accounting["recovered_tokens"], 6);
    let mut unlabeled = Accounting::measured(2, 2, 0, 2, 0, None);
    unlabeled.tokenizer_id = "estimate:tokenzero-lexical".into();
    unlabeled.certified = true;
    let stamped = ToolResponse::ok("read", Mode::Auto, "x".into(), Vec::new(), unlabeled);
    let stamped_json = serde_json::to_value(&stamped).expect("serialize stamped");
    assert_eq!(
        stamped_json["accounting"]["tokenizer_id"],
        LEXICAL_ESTIMATOR_ID
    );
    assert_eq!(stamped_json["accounting"]["certified"], false);
}
