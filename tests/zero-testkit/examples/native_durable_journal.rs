//! Native Z6 durable-journal receipt generator.
//!
//! The parent runs the complete in-process fault matrix and two child-process
//! crash/reopen cases. It emits a canonical receipt only on a native APFS,
//! ext4, or XFS filesystem. Unsupported filesystems fail instead of being
//! relabeled as durable evidence.

use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use zero_abi::{DigestV1, canonical_json, sha256};
use zero_gate::{
    DURABLE_PUBLICATION_SCHEMA_VERSION_V1, NativeDurabilityCheckV1, NativeDurabilityReceiptV1,
    NativeDurabilityResultV1, NativePlatformV1,
};
use zero_store::{
    DurableProfileIdV1, DurableProfileV1, FaultPlanV1, JournalBindingV1, JournalBoundaryV1,
    JournalFailureCodeV1, JournalPathsV1, JournalStateV1, RecoveryOutcomeV1,
    commit_journal_with_fault_v1, initialize_published_root_v1, prepare_journal_v1,
    read_continuation_cartridge_v1, read_journal_record_v1, read_published_root_v1,
    recover_journal_v1,
};
use zero_testkit::journal_fault_matrix::run_journal_fault_matrix_v1;

const RECEIPT_MARKER: &str = "ZEROSTACK_Z6_NATIVE_RECEIPT=";
const SOURCE_INPUTS: [(&str, &[u8]); 10] = [
    (
        "conformance/models/durable-journal-v2.json",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../conformance/models/durable-journal-v2.json"
        )),
    ),
    (
        "crates/zero-gate/Cargo.toml",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-gate/Cargo.toml"
        )),
    ),
    (
        "crates/zero-gate/src/durable_publication.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-gate/src/durable_publication.rs"
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
        "crates/zero-store/src/durable_journal.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-store/src/durable_journal.rs"
        )),
    ),
    (
        "crates/zero-store/src/lib.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zero-store/src/lib.rs"
        )),
    ),
    (
        "tests/zero-testkit/Cargo.toml",
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")),
    ),
    (
        "tests/zero-testkit/examples/native_durable_journal.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/native_durable_journal.rs"
        )),
    ),
    (
        "tests/zero-testkit/src/journal_fault_matrix.rs",
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/journal_fault_matrix.rs"
        )),
    ),
    (
        "tests/zero-testkit/src/lib.rs",
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")),
    ),
];

fn required_env(name: &'static str) -> Result<String, Box<dyn Error>> {
    let value =
        env::var(name).map_err(|_| format!("missing required environment variable {name}"))?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(format!("invalid required environment variable {name}").into());
    }
    Ok(value)
}

fn digest(byte: u8) -> DigestV1 {
    DigestV1::from_bytes([byte; 32])
}

fn paths(directory: &Path) -> Result<JournalPathsV1, Box<dyn Error>> {
    Ok(JournalPathsV1::new(
        directory.join("root.json"),
        directory.join("journal.json"),
        directory.join("cartridge.json"),
        directory.join("owner-death.json"),
        directory.join("recovery.json"),
    )?)
}

fn binding(profile: DurableProfileIdV1) -> JournalBindingV1 {
    JournalBindingV1::new(
        digest(1),
        digest(2),
        profile,
        digest(3),
        digest(4),
        digest(5),
    )
}

fn source_tree_digest() -> DigestV1 {
    let mut bytes = b"zerostack.z6.source_tree.v1\0".to_vec();
    for (path, content) in SOURCE_INPUTS {
        bytes.extend_from_slice(&(path.len() as u32).to_be_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(&(content.len() as u64).to_be_bytes());
        bytes.extend_from_slice(content);
    }
    DigestV1::from_bytes(sha256(&bytes))
}

fn detect_filesystem(path: &Path) -> Result<String, Box<dyn Error>> {
    let output = if env::consts::OS == "linux" {
        Command::new("findmnt")
            .args(["-n", "-o", "FSTYPE", "--target"])
            .arg(path)
            .output()?
    } else if env::consts::OS == "macos" {
        let df = Command::new("df").arg("-P").arg(path).output()?;
        if !df.status.success() {
            return Err(format!(
                "filesystem mount lookup failed: {}",
                String::from_utf8_lossy(&df.stderr)
            )
            .into());
        }
        let text = String::from_utf8(df.stdout)?;
        let device = text
            .lines()
            .last()
            .and_then(|line| line.split_whitespace().next())
            .ok_or("df did not report a filesystem device")?;
        Command::new("/usr/sbin/diskutil")
            .arg("info")
            .arg(device)
            .output()?
    } else {
        return Err("Windows/NTFS evidence is NOT_RUN for this carrier".into());
    };
    if !output.status.success() {
        return Err(format!(
            "filesystem detection failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let text = String::from_utf8(output.stdout)?;
    if env::consts::OS == "linux" {
        return Ok(text.trim().to_ascii_lowercase());
    }
    let personality = text
        .lines()
        .find_map(|line| line.split_once("File System Personality:"))
        .map(|(_, value)| value.trim().to_ascii_lowercase())
        .ok_or("diskutil did not report File System Personality")?;
    if personality.contains("apfs") {
        Ok("apfs".into())
    } else {
        Ok(personality)
    }
}

fn profile(
    platform: &str,
    filesystem: &str,
) -> Result<(DurableProfileIdV1, NativePlatformV1), Box<dyn Error>> {
    match (platform, filesystem) {
        ("macos", "apfs") => Ok((DurableProfileIdV1::ApfsStrict, NativePlatformV1::Macos)),
        ("linux", "ext4" | "xfs") => {
            Ok((DurableProfileIdV1::Ext4XfsStrict, NativePlatformV1::Linux))
        }
        ("windows", "ntfs") => Err("Windows/NTFS evidence is NOT_RUN for this carrier".into()),
        _ => Err(format!(
            "filesystem {filesystem} on {platform} is not an eligible native durability profile"
        )
        .into()),
    }
}

fn fault_child(directory: &Path, boundary: JournalBoundaryV1) -> Result<(), Box<dyn Error>> {
    let expected_platform = required_env("ZEROSTACK_EXPECTED_PLATFORM")?;
    let filesystem = required_env("ZEROSTACK_EXPECTED_FILESYSTEM")?;
    let _ = profile(&expected_platform, &filesystem)?;
    let paths = paths(directory)?;
    let cartridge = read_continuation_cartridge_v1(&paths)?;
    let mut fault = FaultPlanV1::crash_at(boundary);
    let error = commit_journal_with_fault_v1(&paths, &cartridge, &mut fault).unwrap_err();
    if error.code != JournalFailureCodeV1::InjectedCrash || error.boundary != Some(boundary) {
        return Err("fault child did not stop at its preregistered boundary".into());
    }
    eprintln!("intentional native crash boundary: {boundary:?}");
    std::process::abort()
}

fn crash_case(
    root: &Path,
    name: &str,
    boundary: JournalBoundaryV1,
    expected: RecoveryOutcomeV1,
    profile: DurableProfileIdV1,
) -> Result<(), Box<dyn Error>> {
    let directory = root.join(name);
    fs::create_dir_all(&directory)?;
    let paths = paths(&directory)?;
    let binding = binding(profile);
    initialize_published_root_v1(&paths, binding.old_root)?;
    prepare_journal_v1(&paths, binding.clone())?;
    let status = Command::new(env::current_exe()?)
        .arg("--fault-child")
        .arg(&directory)
        .arg(match boundary {
            JournalBoundaryV1::RootPublishBeforeWrite => "before-root",
            JournalBoundaryV1::CommitBeforeWrite => "after-root",
            _ => return Err("unsupported native child boundary".into()),
        })
        .status()?;
    if status.success() {
        return Err("fault child unexpectedly exited successfully".into());
    }
    #[cfg(unix)]
    if status.code().is_some() {
        return Err("fault child exited normally instead of aborting at the boundary".into());
    }
    let recovery = recover_journal_v1(&paths, &binding)?;
    if recovery.outcome != expected {
        return Err(format!(
            "recovery outcome mismatch: expected {expected:?}, observed {:?}",
            recovery.outcome
        )
        .into());
    }
    let journal = read_journal_record_v1(&paths)?;
    let published = read_published_root_v1(&paths)?;
    let correspondence = match expected {
        RecoveryOutcomeV1::OldRootAborted => {
            journal.state == JournalStateV1::Aborted && published.root_digest == binding.old_root
        }
        RecoveryOutcomeV1::NewRootCommitted => {
            journal.state == JournalStateV1::Committed && published.root_digest == binding.new_root
        }
        _ => false,
    };
    if !correspondence {
        return Err("reopened journal and root do not correspond".into());
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    if env::args().nth(1).as_deref() == Some("--fault-child") {
        let directory = PathBuf::from(env::args().nth(2).ok_or("missing child directory")?);
        let boundary = match env::args().nth(3).as_deref() {
            Some("before-root") => JournalBoundaryV1::RootPublishBeforeWrite,
            Some("after-root") => JournalBoundaryV1::CommitBeforeWrite,
            _ => return Err("missing or invalid child boundary".into()),
        };
        return fault_child(&directory, boundary);
    }

    let platform = required_env("ZEROSTACK_EXPECTED_PLATFORM")?;
    if platform != env::consts::OS {
        return Err(format!(
            "native platform skew: expected {platform}, observed {}",
            env::consts::OS
        )
        .into());
    }
    let source_head = required_env("ZEROSTACK_SOURCE_HEAD")?;
    if source_head.len() != 40
        || !source_head
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("source head must be 40 lowercase hexadecimal characters".into());
    }
    let expected_source_tree = required_env("ZEROSTACK_SOURCE_TREE_DIGEST")?;
    let actual_source_tree = source_tree_digest();
    if expected_source_tree != actual_source_tree.to_hex() {
        return Err(format!(
            "source tree digest mismatch: expected {expected_source_tree}, observed {}",
            actual_source_tree.to_hex()
        )
        .into());
    }
    let exact_command = required_env("ZEROSTACK_EXACT_COMMAND")?;
    let run_id = required_env("ZEROSTACK_NATIVE_RUN_ID")?;
    let execution_authority = required_env("ZEROSTACK_EXECUTION_AUTHORITY")?;
    let scratch_root = env::temp_dir().join(format!(
        "zerostack-z6-native-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    ));
    fs::create_dir_all(&scratch_root)?;
    let observed_filesystem = detect_filesystem(&scratch_root)?;
    let expected_filesystem = required_env("ZEROSTACK_EXPECTED_FILESYSTEM")?;
    if observed_filesystem != expected_filesystem {
        return Err(format!(
            "native filesystem skew: expected {expected_filesystem}, observed {observed_filesystem}"
        )
        .into());
    }
    let (profile_id, native_platform) = profile(&platform, &observed_filesystem)?;

    let matrix = run_journal_fault_matrix_v1();
    if !matrix.all_passed || matrix.failed != 0 || matrix.boundaries_exercised != 40 {
        return Err("native fault matrix did not close every case".into());
    }
    crash_case(
        &scratch_root,
        "old-root-reopen",
        JournalBoundaryV1::RootPublishBeforeWrite,
        RecoveryOutcomeV1::OldRootAborted,
        profile_id,
    )?;
    crash_case(
        &scratch_root,
        "new-root-reopen",
        JournalBoundaryV1::CommitBeforeWrite,
        RecoveryOutcomeV1::NewRootCommitted,
        profile_id,
    )?;

    let executable = env::current_exe()?;
    let artifact_digest = DigestV1::from_bytes(sha256(&fs::read(executable)?));
    let receipt = NativeDurabilityReceiptV1 {
        schema_version: DURABLE_PUBLICATION_SCHEMA_VERSION_V1,
        durable_profile_id: profile_id,
        durable_profile_digest: DurableProfileV1::new(profile_id).digest(),
        platform: native_platform,
        filesystem: observed_filesystem,
        source_repository_head: source_head,
        source_tree_digest: actual_source_tree,
        artifact_digest,
        exact_command_digest: DigestV1::from_bytes(sha256(exact_command.as_bytes())),
        execution_authority_digest: DigestV1::from_bytes(sha256(
            format!("{execution_authority}\0{run_id}").as_bytes(),
        )),
        native_run_id: run_id,
        checks: vec![
            NativeDurabilityCheckV1::FileSync,
            NativeDurabilityCheckV1::AtomicReplace,
            NativeDurabilityCheckV1::DirectorySync,
            NativeDurabilityCheckV1::KillReopen,
        ],
        result: NativeDurabilityResultV1::PassedNative,
    };
    println!(
        "{RECEIPT_MARKER}{}",
        canonical_json(&serde_json::to_value(receipt)?)
    );
    Ok(())
}
