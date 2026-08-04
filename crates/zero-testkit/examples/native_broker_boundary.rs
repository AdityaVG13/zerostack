//! Native Z5 aggregate broker-boundary receipt generator.
//! The parent launches this binary in worker mode and publishes only after G8/G9.

use std::{
    env,
    error::Error,
    fs,
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use zero_abi::{canonical_json, raw_worker::EffectClass, sha256, DigestV1};
use zero_gate::{
    prepare, AttributionClass, ControllerInstruction, ControllerPlan, ExecutionBinding,
    ExecutionSurface, FinalReceipt, GuardEvidence, PeerOwner, PerformanceAdmission, PrepareRequest,
    ResourceUsage, SemanticAuthority, SnapEvidence, SourceHead, StagedEffect, TransactionClosure,
    WorkerEnvelope, TWO_PHASE_SCHEMA_VERSION,
};

const RECEIPT_MARKER: &str = "ZEROSTACK_Z5_NATIVE_RECEIPT=";
const SOURCE_INPUTS: [(&str, &[u8]); 16] = [
    (
        "crates/zero-gate/Cargo.toml",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-gate/Cargo.toml"
        )),
    ),
    (
        "crates/zero-gate/src/lib.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-gate/src/lib.rs"
        )),
    ),
    (
        "crates/zero-gate/src/two_phase.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-gate/src/two_phase.rs"
        )),
    ),
    (
        "crates/zero-testkit/Cargo.toml",
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")),
    ),
    (
        "crates/zero-testkit/examples/native_broker_boundary.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/native_broker_boundary.rs"
        )),
    ),
    (
        "crates/zero-testkit/src/aggregate_broker_gate.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/aggregate_broker_gate.rs"
        )),
    ),
    (
        "crates/zero-testkit/src/lib.rs",
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")),
    ),
    (
        "crates/zero-testkit/conformance/two-phase-gate/v1/index.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/conformance/two-phase-gate/v1/index.json"
        )),
    ),
    (
        "crates/zero-testkit/conformance/two-phase-gate/v1/schema.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/conformance/two-phase-gate/v1/schema.json"
        )),
    ),
    (
        "crates/zero-testkit/conformance/two-phase-gate/v1/vectors.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/conformance/two-phase-gate/v1/vectors.json"
        )),
    ),
    (
        "crates/zero-testkit/conformance/two-phase-gate/v1/runners/python/verify_v1.py",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/conformance/two-phase-gate/v1/runners/python/verify_v1.py"
        )),
    ),
    (
        "conformance/models/two-phase-gate-v1.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/models/two-phase-gate-v1.json"
        )),
    ),
    (
        "conformance/models/two-phase-gate-rust-correspondence-v1.md",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/models/two-phase-gate-rust-correspondence-v1.md"
        )),
    ),
    (
        "conformance/schemas/two-phase-gate-v1.schema.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/schemas/two-phase-gate-v1.schema.json"
        )),
    ),
    (
        "conformance/src/racc.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/src/racc.rs"
        )),
    ),
    (
        "conformance/tests/two_phase_gate.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/tests/two_phase_gate.rs"
        )),
    ),
];

fn now_unix_ns() -> Result<u128, Box<dyn Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    let value =
        env::var(name).map_err(|_| format!("missing required environment variable {name}"))?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(format!("invalid required environment variable {name}").into());
    }
    Ok(value)
}

fn parse_digest(value: &str) -> Result<[u8; 32], Box<dyn Error>> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("digest must be 64 lowercase hexadecimal characters".into());
    }
    let mut digest = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        digest[index] = u8::from_str_radix(std::str::from_utf8(pair)?, 16)?;
    }
    Ok(digest)
}

fn source_tree_digest() -> DigestV1 {
    let mut bytes = b"zerostack.z5.source_tree.v1\0".to_vec();
    for (path, content) in SOURCE_INPUTS {
        bytes.extend_from_slice(&(path.len() as u32).to_be_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(&(content.len() as u64).to_be_bytes());
        bytes.extend_from_slice(content);
    }
    DigestV1::from_bytes(sha256(&bytes))
}

fn worker() -> Result<(), Box<dyn Error>> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    if input != b"native broker request" {
        return Err("worker input mismatch".into());
    }
    std::io::stdout().write_all(b"brokered result")?;
    Ok(())
}

fn request(source_head: String, assembly: [u8; 32], source_tree: [u8; 32]) -> PrepareRequest {
    let plan = ControllerPlan {
        instructions: vec![
            ControllerInstruction::Dispatch {
                owner: PeerOwner::ZeroStack,
            },
            ControllerInstruction::Verify,
            ControllerInstruction::StageEffect,
            ControllerInstruction::BufferVisible,
            ControllerInstruction::CloseTransaction,
        ],
    };
    PrepareRequest {
        binding: ExecutionBinding {
            schema_version: TWO_PHASE_SCHEMA_VERSION,
            assembly_manifest_digest: assembly,
            source_tree_digest: source_tree,
            source_repository_heads: vec![SourceHead {
                repository: "ZeroStack".into(),
                head: source_head,
            }],
            image_digest: [3; 32],
            plan_digest: plan.digest(),
            comparison_identity_digest: [4; 32],
            predecessor_receipt_head: [5; 32],
        },
        surface: ExecutionSurface::Mcp,
        effect_class: EffectClass::ReversibleMutation,
        plan,
        envelope: WorkerEnvelope {
            fuel: 100,
            deadline_ms: 1_000,
            io_bytes: 1_024,
            output_bytes: 128,
            memory_bytes: 16 * 1_024 * 1_024,
            processes: 1,
            risk_units: 10,
            worker_steps: 4,
        },
        evidence: GuardEvidence {
            canonical_object_digest: [6; 32],
            decoded_object_digest: [6; 32],
            owner_coherent: true,
            producer_coherent: true,
            schema_coherent: true,
            source_root_coherent: true,
            semantic_authority: SemanticAuthority::OwnerScoped,
            attribution_class: AttributionClass::Fixed,
            snap: SnapEvidence::NotClaimed,
            safety_shield_digest: [7; 32],
            approval_grant_digest: None,
            irreversible_pre_action_evidence_digest: None,
            performance: PerformanceAdmission::ExactNeutral,
        },
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    if env::args().nth(1).as_deref() == Some("--worker") {
        return worker();
    }
    let started_at = now_unix_ns()?;
    let expected_platform = required_env("ZEROSTACK_EXPECTED_PLATFORM")?;
    if env::consts::OS != expected_platform {
        return Err("native platform does not match preregistration".into());
    }
    if !matches!(env::consts::OS, "macos" | "linux" | "windows") {
        return Err("unsupported native profile".into());
    }
    let evidence_mode = required_env("ZEROSTACK_EVIDENCE_MODE")?;
    let (native_evidence, result_status) = match evidence_mode.as_str() {
        "dsr-native" => (true, "passed_native"),
        "rch-verification" => (false, "passed_rch_verification"),
        _ => return Err("unsupported execution authority".into()),
    };
    let source_head = required_env("ZEROSTACK_SOURCE_HEAD")?;
    if source_head.len() != 40
        || !source_head
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("source head is not a canonical Git SHA-1".into());
    }
    let expected_source = required_env("ZEROSTACK_SOURCE_TREE_DIGEST")?;
    let actual_source = source_tree_digest();
    if actual_source.to_hex() != expected_source {
        return Err(format!(
            "source tree digest mismatch: expected {expected_source}, observed {}",
            actual_source.to_hex()
        )
        .into());
    }
    let assembly = required_env("ZEROSTACK_ASSEMBLY_MANIFEST_DIGEST")?;
    let exact_command = required_env("ZEROSTACK_EXACT_COMMAND")?;
    let run_id = required_env("ZEROSTACK_DSR_RUN_ID")?;

    let permit = prepare(request(
        source_head.clone(),
        parse_digest(&assembly)?,
        parse_digest(&expected_source)?,
    ))?;
    let mut execution = permit.start();
    let executable = env::current_exe()?;
    let timer = Instant::now();
    let mut child = Command::new(&executable)
        .arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing child stdin")?
        .write_all(b"native broker request")?;
    let child_output = child.wait_with_output()?;
    if !child_output.status.success() {
        return Err(format!(
            "native worker failed: {}",
            String::from_utf8_lossy(&child_output.stderr)
        )
        .into());
    }
    let elapsed_ms = u64::try_from(timer.elapsed().as_millis())?.max(1);
    execution.dispatch(
        PeerOwner::ZeroStack,
        ResourceUsage {
            fuel: 20,
            elapsed_ms,
            io_bytes: u64::try_from(child_output.stdout.len())?,
            memory_bytes: 1024,
            processes: 1,
            risk_units: 1,
            worker_steps: 1,
        },
    )?;
    execution.record_verification(sha256(&child_output.stdout))?;
    execution.stage_effect(StagedEffect {
        effect_digest: [9; 32],
        effect_class: EffectClass::ReversibleMutation,
        approval_grant_digest: None,
        pre_action_evidence_digest: None,
    })?;
    execution.buffer_visible(&child_output.stdout)?;
    let ready = execution.close_transaction(TransactionClosure::commit([10; 32], true))?;
    let FinalReceipt::Commit(receipt) = ready.finalize()? else {
        return Err("unexpected fallback receipt".into());
    };
    let record = receipt.record().clone();
    let trace = receipt.trace().clone();
    trace.verify_complete()?;
    let published = receipt.publish();
    if published.visible_bytes != child_output.stdout || published.approved_effects.len() != 1 {
        return Err("publication mismatch".into());
    }

    let adapter_binary_digest = DigestV1::from_bytes(sha256(&fs::read(&executable)?));
    let rustc = Command::new("rustc").arg("-vV").output()?;
    if !rustc.status.success() {
        return Err("rustc -vV failed".into());
    }
    let completed_at = now_unix_ns()?;
    let output_digest = DigestV1::from_bytes(sha256(&published.visible_bytes));
    let receipt = json!({
        "schema_version": 1,
        "bead_key": "zerostack-3btw",
        "claim_or_freeze_ids": ["Z5", "zerostack-racc-frontier-86qk.20"],
        "assembly_manifest_digest": assembly,
        "source_repository_heads": {"ZeroStack": source_head},
        "model_or_spec_version": "zerostack.two-phase-gate.v1",
        "toolchain_identities": [{"tool":"rustc","verbose_version":String::from_utf8(rustc.stdout)?}],
        "exact_commands": [exact_command],
        "input_fixture_hashes": {"native_broker_request": DigestV1::from_bytes(sha256(b"native broker request"))},
        "output_artifact_hashes": {"adapter_binary":adapter_binary_digest,"published_bytes":output_digest},
        "mutants_run": [],
        "platform_profile": {"os":env::consts::OS,"arch":env::consts::ARCH,"family":env::consts::FAMILY,"native_evidence":native_evidence},
        "execution_authority": {"provider":if native_evidence { "doodlestein_self_releaser" } else { "rch" },"run_id":run_id,"source_tree_digest":actual_source,"external_run_receipt_required":native_evidence,"native_evidence":native_evidence},
        "broker_boundary": {"kind":"native_child_process","child_exit_success":true,"publication_after_finalize":true,"guard_count":trace.events().len()},
        "permit_and_receipt": {"receipt":record,"trace":trace,"published_receipt_head":published.receipt_head,"successor_root":published.successor_root},
        "result": {"status":result_status,"visible_bytes":published.visible_bytes.len(),"approved_effects":published.approved_effects.len()},
        "failure_code": null,
        "residual_assumptions": ["this test-only child process is not a production engine worker","operating-system hard resource enforcement is outside the Z5 Rust kernel"],
        "started_at": format!("unix_ns:{started_at}"),
        "completed_at": format!("unix_ns:{completed_at}")
    });
    println!("{RECEIPT_MARKER}{}", canonical_json(&receipt));
    Ok(())
}
