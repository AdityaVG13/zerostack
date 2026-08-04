//! Native Z4 adapter/parent-counter correspondence receipt generator.
//!
//! This is a test-only portable filesystem workload. It does not implement a
//! production OS counter. Native execution must be preregistered externally.

use std::{
    env,
    error::Error,
    fs::{self, OpenOptions},
    io::{self, Write},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use zero_abi::{canonical_json, sha256, DigestV1};
use zero_ledger::{
    CausalCounterUnitV1, CounterCorrespondenceReceiptV1, CounterEvidenceModeV1,
    ParentCounterIdentityV1, ParentCounterWindowV1,
};

const WORKLOAD_BYTES: usize = 4_096;
const RECEIPT_MARKER: &str = "ZEROSTACK_Z4_NATIVE_RECEIPT=";
const SOURCE_INPUTS: [(&str, &[u8]); 8] = [
    (
        "conformance/models/causal-work-v3.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/models/causal-work-v3.json"
        )),
    ),
    (
        "crates/zero-ledger/src/causal_work.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-ledger/src/causal_work.rs"
        )),
    ),
    (
        "crates/zero-ledger/src/lib.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-ledger/src/lib.rs"
        )),
    ),
    (
        "crates/zero-ledger/tests/fixtures/token-ledger-v2-archive.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-ledger/tests/fixtures/token-ledger-v2-archive.json"
        )),
    ),
    (
        "crates/zero-testkit/Cargo.toml",
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")),
    ),
    (
        "crates/zero-testkit/examples/native_counter_correspondence.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/native_counter_correspondence.rs"
        )),
    ),
    (
        "crates/zero-testkit/src/ledger_conservation.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/ledger_conservation.rs"
        )),
    ),
    (
        "crates/zero-testkit/src/lib.rs",
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")),
    ),
];

fn now_unix_ns() -> Result<u128, Box<dyn Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())
}

fn digest_domain(domain: &[u8], bytes: &[u8]) -> DigestV1 {
    let mut input = domain.to_vec();
    input.push(0);
    input.extend_from_slice(bytes);
    DigestV1::from_bytes(sha256(&input))
}

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    let value =
        env::var(name).map_err(|_| format!("missing required environment variable {name}"))?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(format!("invalid required environment variable {name}").into());
    }
    Ok(value)
}

fn source_tree_digest() -> Result<DigestV1, Box<dyn Error>> {
    let mut bytes = b"zerostack.z4.source_tree.v1\0".to_vec();
    for (path, content) in SOURCE_INPUTS {
        let path_bytes = path.as_bytes();
        bytes.extend_from_slice(&u32::try_from(path_bytes.len())?.to_be_bytes());
        bytes.extend_from_slice(path_bytes);
        bytes.extend_from_slice(&u64::try_from(content.len())?.to_be_bytes());
        bytes.extend_from_slice(content);
    }
    Ok(DigestV1::from_bytes(sha256(&bytes)))
}

fn main() -> Result<(), Box<dyn Error>> {
    let started_at = now_unix_ns()?;
    let platform = env::consts::OS;
    let expected_platform = required_env("ZEROSTACK_EXPECTED_PLATFORM")?;
    if platform != expected_platform {
        return Err(format!(
            "native platform skew: expected {expected_platform}, observed {platform}"
        )
        .into());
    }
    if !matches!(platform, "macos" | "linux" | "windows") {
        return Err(format!("unsupported native profile {platform}").into());
    }
    let source_head = required_env("ZEROSTACK_SOURCE_HEAD")?;
    if source_head.len() != 40 || !source_head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("source head must be exactly 40 hexadecimal characters".into());
    }
    let expected_source_tree_digest = required_env("ZEROSTACK_SOURCE_TREE_DIGEST")?;
    let actual_source_tree_digest = source_tree_digest()?;
    if actual_source_tree_digest.to_hex() != expected_source_tree_digest {
        return Err(format!(
            "source tree digest mismatch: expected {expected_source_tree_digest}, observed {}",
            actual_source_tree_digest.to_hex()
        )
        .into());
    }

    let evidence_request = required_env("ZEROSTACK_EVIDENCE_MODE")?;
    let (evidence_mode, native_evidence, execution_authority, result_status) =
        match evidence_request.as_str() {
            "rch-verification" => (
                CounterEvidenceModeV1::RchCompilation,
                false,
                json!({
                    "provider": "rch",
                    "source_tree_digest": actual_source_tree_digest,
                    "native_evidence": false
                }),
                "passed_rch_verification",
            ),
            "github-actions-native" => {
                if env::var("GITHUB_ACTIONS").as_deref() != Ok("true")
                    || env::var("CI").as_deref() != Ok("true")
                    || env::var("GITHUB_EVENT_NAME").as_deref() != Ok("workflow_dispatch")
                    || env::var("GITHUB_REF").as_deref() != Ok("refs/heads/main")
                    || env::var("GITHUB_REPOSITORY").as_deref() != Ok("AdityaVG13/zerostack")
                {
                    return Err(
                        "native evidence requires the preregistered manual GitHub workflow on main"
                            .into(),
                    );
                }
                let checked_out_head = Command::new("git").args(["rev-parse", "HEAD"]).output()?;
                if !checked_out_head.status.success()
                    || String::from_utf8(checked_out_head.stdout)?.trim() != source_head
                {
                    return Err("GitHub source head does not match checked-out Git HEAD".into());
                }
                let run_id = required_env("GITHUB_RUN_ID")?;
                let run_attempt = required_env("GITHUB_RUN_ATTEMPT")?;
                let runner_name = required_env("RUNNER_NAME")?;
                (
                    CounterEvidenceModeV1::Native,
                    true,
                    json!({
                        "provider": "github-actions",
                        "repository": "AdityaVG13/zerostack",
                        "event": "workflow_dispatch",
                        "git_ref": "refs/heads/main",
                        "source_tree_digest": actual_source_tree_digest,
                        "run_id": run_id,
                        "run_attempt": run_attempt,
                        "runner_name": runner_name,
                        "runner_os": env::var("RUNNER_OS").unwrap_or_default(),
                        "runner_arch": env::var("RUNNER_ARCH").unwrap_or_default(),
                        "native_evidence": true
                    }),
                    "passed_native",
                )
            }
            "dsr-native" => {
                let run_id = required_env("ZEROSTACK_DSR_RUN_ID")?;
                (
                    CounterEvidenceModeV1::Native,
                    true,
                    json!({
                        "provider": "doodlestein_self_releaser",
                        "version": "0.1.2",
                        "run_id": run_id,
                        "source_tree_digest": actual_source_tree_digest,
                        "external_run_receipt_required": true,
                        "native_evidence": true
                    }),
                    "passed_native",
                )
            }
            _ => return Err("unsupported evidence mode".into()),
        };

    let assembly_manifest_digest = required_env("ZEROSTACK_ASSEMBLY_MANIFEST_DIGEST")?;
    if assembly_manifest_digest.len() != 64
        || !assembly_manifest_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("assembly manifest digest must be exactly 64 hexadecimal characters".into());
    }
    let exact_command = required_env("ZEROSTACK_EXACT_COMMAND")?;

    let workload = (0..WORKLOAD_BYTES)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let workload_digest = digest_domain(b"zerostack.z4.native_workload.v1", &workload);
    let path = env::temp_dir().join(format!(
        "zerostack-z4-counter-{}-{}",
        std::process::id(),
        started_at
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let start = file.metadata()?.len();
    let mut adapter_observed_delta = 0_u64;
    while adapter_observed_delta < WORKLOAD_BYTES as u64 {
        let offset = usize::try_from(adapter_observed_delta)?;
        let written = file.write(&workload[offset..])?;
        if written == 0 {
            return Err(
                io::Error::new(io::ErrorKind::WriteZero, "adapter wrote zero bytes").into(),
            );
        }
        adapter_observed_delta = adapter_observed_delta
            .checked_add(u64::try_from(written)?)
            .ok_or("adapter counter overflow")?;
    }
    file.sync_all()?;
    let end = file.metadata()?.len();

    let executable = env::current_exe()?;
    let adapter_binary_digest = DigestV1::from_bytes(sha256(&fs::read(executable)?));
    let boundary_digest = digest_domain(
        b"zerostack.z4.counter_boundary.v1",
        b"std.fs.file_length.bytes:start_before_write:end_after_sync_all",
    );
    let platform_profile_digest = digest_domain(
        b"zerostack.z4.platform_profile.v1",
        canonical_json(&json!({
            "os": platform,
            "arch": env::consts::ARCH,
            "family": env::consts::FAMILY,
            "counter": "std.fs.file_length.bytes"
        }))
        .as_bytes(),
    );
    let identity = ParentCounterIdentityV1 {
        counter_id: "std.fs.file_length.bytes".into(),
        unit: CausalCounterUnitV1::Bytes,
        boundary_digest,
        adapter_digest: adapter_binary_digest,
        platform_profile_digest,
    };
    let correspondence = CounterCorrespondenceReceiptV1::new(
        platform.into(),
        evidence_mode,
        identity.clone(),
        ParentCounterWindowV1 {
            identity,
            start,
            end,
        },
        adapter_observed_delta,
        adapter_binary_digest,
    )?;
    if correspondence.is_native_evidence() != native_evidence {
        return Err("evidence mode was not preserved".into());
    }

    let rustc = Command::new("rustc").arg("-vV").output()?;
    if !rustc.status.success() {
        return Err("rustc -vV failed".into());
    }
    let rustc_identity = String::from_utf8(rustc.stdout)?;
    let completed_at = now_unix_ns()?;
    let receipt = json!({
        "schema_version": 1,
        "bead_key": "Z4",
        "claim_or_freeze_ids": ["Z4"],
        "assembly_manifest_digest": assembly_manifest_digest,
        "source_repository_heads": {"ZeroStack": source_head},
        "model_or_spec_version": "zerostack.causal_work.native_counter.v1",
        "toolchain_identities": [{"tool": "rustc", "verbose_version": rustc_identity}],
        "exact_commands": [exact_command],
        "input_fixture_hashes": {"native_workload": workload_digest},
        "output_artifact_hashes": {"adapter_binary": adapter_binary_digest},
        "mutants_run": [],
        "platform_profile": {
            "os": platform,
            "arch": env::consts::ARCH,
            "family": env::consts::FAMILY,
            "native_evidence": native_evidence
        },
        "execution_authority": execution_authority,
        "correspondence": correspondence,
        "result": {
            "status": result_status,
            "parent_delta": end.checked_sub(start).ok_or("parent counter regressed")?,
            "adapter_delta": adapter_observed_delta,
            "workload_bytes": WORKLOAD_BYTES
        },
        "failure_code": null,
        "residual_assumptions": [
            "temporary-file metadata length represents the parent byte counter",
            "successful std::io::Write return values represent the adapter byte counter",
            "this test-only workload does not establish production engine counter semantics"
        ],
        "started_at": format!("unix_ns:{started_at}"),
        "completed_at": format!("unix_ns:{completed_at}")
    });
    println!("{RECEIPT_MARKER}{}", canonical_json(&receipt));
    Ok(())
}
