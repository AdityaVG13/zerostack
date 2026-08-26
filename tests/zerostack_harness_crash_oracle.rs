//! Crash-boundary recovery: committed-or-not-committed, no partial dest.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use zero_abi::DigestV1;
use zero_store::{
    DurableProfileIdV1, JournalBindingV1, JournalFailureCodeV1, JournalPathsV1, SharedCas,
    atomic_write_file, initialize_published_root_v1, initialize_published_root_with_fault_v1,
    prepare_journal_v1, read_published_root_v1, recover_journal_v1, recover_journal_with_fault_v1,
};
use zerostack_harness::crash_boundary::{CrashBoundary, arm_crash_boundary};
use zerostack_harness::eprocess::{EProcess, MonitoredInvariant};
use zerostack_harness::fault_vfs::DEFAULT_FAULT_SEED;

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "zs-crash-{label}-{}-{nanos}-{DEFAULT_FAULT_SEED:x}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("scratch");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn digest(tag: u8) -> DigestV1 {
    let mut bytes = [0u8; 32];
    bytes[0] = tag;
    bytes[31] = 0x5a;
    DigestV1::from_bytes(bytes)
}

fn paths(dir: &Path) -> JournalPathsV1 {
    JournalPathsV1::new(
        dir.join("root.json"),
        dir.join("journal.json"),
        dir.join("cartridge.json"),
        dir.join("owner_death.json"),
        dir.join("recovery.json"),
    )
    .expect("distinct journal paths")
}

fn binding() -> JournalBindingV1 {
    JournalBindingV1::new(
        digest(1),
        digest(2),
        DurableProfileIdV1::PortableStrict,
        digest(3),
        digest(4),
        digest(5),
    )
}

fn leftover_temps(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.contains(".tmp") || name.contains("journal-tmp"))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn mid_write_kill_leaves_no_partial_dest() {
    let scratch = Scratch::new("after-tmp");
    let paths = paths(&scratch.0);
    let mut spec = FaultSpec::power_cut(
        "after-tmp-before-rename",
        CrashBoundary::AfterTmpWriteBeforeRename,
    );
    let mut fault = spec.arm();
    let err = initialize_published_root_with_fault_v1(&paths, digest(3), &mut fault)
        .expect_err("injected crash");
    assert_eq!(err.code, JournalFailureCodeV1::InjectedCrash);
    spec.record_trigger(paths.root_record().to_string_lossy().as_ref(), 0);

    assert!(
        !paths.root_record().exists(),
        "dest must not be visible before rename"
    );
    assert!(
        leftover_temps(&scratch.0).is_empty(),
        "failed publish must unlink its temp"
    );

    initialize_published_root_v1(&paths, digest(3)).expect("retry after crash");
    let root = read_published_root_v1(&paths).expect("complete root");
    assert_eq!(root.root_digest, digest(3));
}

#[test]
fn leftover_temp_is_not_the_visible_file() {
    let scratch = Scratch::new("leftover");
    let dest = scratch.0.join("artifact");
    atomic_write_file(&dest, b"complete-one").expect("first write");
    let leftover = scratch
        .0
        .join(format!(".artifact.tmp-leftover-{}", std::process::id()));
    fs::write(&leftover, b"torn-partial").expect("plant leftover");
    atomic_write_file(&dest, b"complete-two").expect("second write");
    assert_eq!(fs::read(&dest).expect("dest"), b"complete-two");
    assert!(leftover.exists(), "unrelated leftover stays a sibling");
    assert_ne!(
        fs::read(&leftover).expect("leftover"),
        fs::read(&dest).unwrap()
    );
}

#[test]
fn mid_journal_recover_is_fail_closed_or_consistent() {
    let scratch = Scratch::new("recover");
    let paths = paths(&scratch.0);
    let binding = binding();
    initialize_published_root_v1(&paths, binding.old_root).expect("root");
    prepare_journal_v1(&paths, binding.clone()).expect("prepare");

    let mut fault = arm_crash_boundary(CrashBoundary::MidJournalRecover);
    let first = recover_journal_with_fault_v1(&paths, &binding, &mut fault);
    match first {
        Err(error) => assert_eq!(error.code, JournalFailureCodeV1::InjectedCrash),
        Ok(receipt) => {
            assert!(receipt.journal_root_correspondence);
        }
    }

    let recovered = recover_journal_v1(&paths, &binding).expect("second recover");
    assert!(recovered.journal_root_correspondence);

    // Independent on-disk verification: journal and root must form a valid pair.
    let journal_bytes = fs::read(paths.journal_record()).expect("journal bytes");
    let root_bytes = fs::read(paths.root_record()).expect("root bytes");
    let journal: serde_json::Value = serde_json::from_slice(&journal_bytes).expect("journal json");
    let root: serde_json::Value = serde_json::from_slice(&root_bytes).expect("root json");
    let root_digest = root
        .get("root_digest")
        .and_then(|v| v.as_str())
        .expect("root_digest");
    let old_hex = format!("{}", binding.old_root);
    let new_hex = format!("{}", binding.new_root);
    // Root must be exactly one of the two allowed digests.
    assert!(
        root_digest == old_hex || root_digest == new_hex,
        "recovered root must be old or new, got {root_digest}"
    );
    // Journal binding must match expected.
    let j_old = journal
        .get("binding")
        .and_then(|b| b.get("old_root"))
        .and_then(|v| v.as_str())
        .expect("journal old_root");
    let j_new = journal
        .get("binding")
        .and_then(|b| b.get("new_root"))
        .and_then(|v| v.as_str())
        .expect("journal new_root");
    assert_eq!(j_old, old_hex, "journal old_root must match binding");
    assert_eq!(j_new, new_hex, "journal new_root must match binding");
    // No mixed state: root and journal state must agree.
    let state = journal
        .get("state")
        .and_then(|v| v.as_str())
        .expect("journal state");
    if root_digest == old_hex {
        assert!(
            state == "prepared" || state == "aborted",
            "old root must pair with prepared/aborted, got state {state}"
        );
    } else {
        assert_eq!(
            state, "committed",
            "new root must pair with committed, got {state}"
        );
    }
    // Also verify typed read agrees with raw bytes.
    let typed_root = read_published_root_v1(&paths).expect("typed root after recover");
    assert!(
        typed_root.root_digest == binding.old_root || typed_root.root_digest == binding.new_root,
        "typed root must be consistent prefix"
    );
}

#[test]
fn cas_digest_match_feeds_hardware_eprocess() {
    let scratch = Scratch::new("cas");
    let cas = SharedCas::open(&scratch.0);
    let bytes = b"crash-oracle-cas";
    // Independent digest: SHA-256 of the fixed vector, not the CAS's own function.
    let expected_hex = "7294a7ba17f322652d25968a91c6c60470aa7bfd8702bdb73cfd53d91de7cd4b";
    let hash = cas.put(bytes).expect("put");
    assert_eq!(
        hash, expected_hex,
        "CAS must return independently known SHA-256"
    );
    let got = cas.get_verified(&hash).expect("get");
    assert_eq!(got, bytes, "verified read must return original bytes");
    // Persistence: reopen and verify again.
    let cas2 = SharedCas::open(&scratch.0);
    let got2 = cas2.get_verified(&hash).expect("reopen get");
    assert_eq!(got2, bytes);
    // Monitoring path accepts the verified match.
    let holds = got == bytes && hash == expected_hex;
    let mut proc = EProcess::new(MonitoredInvariant::CasDigestMatch);
    proc.update(!holds);
    assert!(holds);
    assert!(!proc.rejected());

    // Corrupted object must not verify; monitoring must reject mismatch.
    let tampered_path = cas.object_path(&hash);
    fs::write(&tampered_path, b"corrupted").expect("tamper");
    let corrupted = cas.get_verified(&hash);
    assert!(
        corrupted.is_err(),
        "tampered CAS object must fail verification"
    );
    let mut proc2 = EProcess::new(MonitoredInvariant::CasDigestMatch);
    // Simulate monitor seeing mismatch (hash mismatch or read error).
    proc2.update(true);
    assert!(proc2.rejected(), "monitor must reject mismatched digest");
}
