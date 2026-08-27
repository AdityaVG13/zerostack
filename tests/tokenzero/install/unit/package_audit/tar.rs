use super::fixtures::*;
use super::*;

/// Run `package_audit` on a single tar archive built from `entries`, returning
/// the full report and the issues array.
fn run_tar_audit(entries: &[TarTestEntry<'_>]) -> (serde_json::Value, Vec<serde_json::Value>) {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    write_test_tar_entries(&artifact, entries);
    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    (report, issues)
}

/// Helper: run audit on a tar archive built from plain names (all type '0').
fn run_tar_audit_from_names(names: &[&str]) -> (serde_json::Value, Vec<serde_json::Value>) {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    write_test_tar(&artifact, names);
    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    (report, issues)
}

/// Assert the audit report indicates rejection.
fn assert_audit_rejected(report: &serde_json::Value) {
    assert_eq!(report["ok"], false);
}

/// Assert at least one issue matches all key/value pairs in `fields`.
fn assert_issue(issues: &[serde_json::Value], fields: &[(&str, &str)]) {
    assert!(
        issues
            .iter()
            .any(|issue| fields.iter().all(|(k, v)| issue[*k] == *v)),
        "expected issue matching {fields:?}\n  in: {issues:#?}"
    );
}

/// Assert no issue has the given `code`.
fn assert_no_issue(issues: &[serde_json::Value], code: &str) {
    assert!(
        !issues.iter().any(|issue| issue["code"] == code),
        "unexpected issue with code={code} in {issues:#?}"
    );
}

/// Assert at least one issue matches `code`+`member` and its `detail` contains `s`.
fn assert_issue_detail(issues: &[serde_json::Value], code: &str, member: &str, s: &str) {
    assert!(
        issues.iter().any(|issue| {
            issue["code"] == code
                && issue["member"] == member
                && issue["detail"].as_str().is_some_and(|d| d.contains(s))
        }),
        "expected {code} for {member} with detail containing '{s}'\n  in: {issues:#?}"
    );
}

/// Assert the `fields` array inside the matching issue contains every expected field name.
fn assert_issue_fields(
    issues: &[serde_json::Value],
    code: &str,
    member: &str,
    expected: &[&str],
    report: &serde_json::Value,
) {
    let issue = issues
        .iter()
        .find(|i| i["code"] == code && i["member"] == member)
        .unwrap_or_else(|| panic!("missing {code} issue for {member}: {report:#}"));
    let fields = issue["fields"].as_array().unwrap();
    for field in expected {
        assert!(
            fields.iter().any(|v| v == field),
            "missing {field} field in {issue:#}"
        );
    }
}

/// Assert the issue's JSON serialization does NOT contain `secret`.
fn assert_issue_no_secret(
    issues: &[serde_json::Value],
    code: &str,
    member: &str,
    secret: &str,
    report: &serde_json::Value,
) {
    let issue = issues
        .iter()
        .find(|i| i["code"] == code && i["member"] == member)
        .unwrap_or_else(|| panic!("missing {code} issue for {member}: {report:#}"));
    let serialized = serde_json::to_string(issue).unwrap();
    assert!(
        !serialized.contains(secret),
        "issue must not expose '{secret}': {issue:#}"
    );
}

/// Assert no issue matches `code`+`member`.
fn assert_no_issue_code_member(issues: &[serde_json::Value], code: &str, member: &str) {
    assert!(
        !issues
            .iter()
            .any(|issue| issue["code"] == code && issue["member"] == member),
        "unexpected {code} issue for {member} in {issues:#?}"
    );
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn package_audit_rejects_tar_archive_dev_target_launcher_payload() {
    let member = "tokenzero-v0.1.1/bin/tokenzero";
    let payload = b"#!/bin/sh\nexec target/release/tokenzero \"$@\"\n";
    let (report, issues) = run_tar_audit(&[TarTestEntry::new(member, b'0', payload)]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[("code", "dev_runtime_launcher"), ("member", member)],
    );
}

#[test]
fn package_audit_fails_closed_on_archive_link_target_control_characters() {
    let member = "tokenzero-v0.1.1/bin/tokenzero";
    let link_target = "bin/tokenzero\rshim";
    let (report, issues) =
        run_tar_audit(&[TarTestEntry::new(member, b'2', b"").with_link_target(link_target)]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_link_target_uninspectable"),
            ("member", member),
            ("link_target", link_target),
            ("reason", "control_character"),
        ],
    );
}

#[test]
fn package_audit_rejects_private_gzip_tar_members_in_process() {
    let dir = tempdir().unwrap();
    let tar_path = dir.path().join("release.tar");
    let artifact = dir.path().join("release.tar.gz");
    write_test_tar(
        &tar_path,
        &[
            "tokenzero-v0.1.1/._LICENSE",
            "tokenzero-v0.1.1/.tokenzero/config.json",
        ],
    );
    fs::write(&artifact, gzip_bytes(&fs::read(&tar_path).unwrap())).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(&issues, &[("code", "appledouble_metadata")]);
    assert_issue(&issues, &[("code", "private_tool_state_member")]);
}

#[test]
fn package_audit_rejects_concatenated_gzip_tar_members() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar.gz");
    let visible_fragment = test_tar_entry_bytes("tokenzero-v0.1.1/LICENSE", b"MIT");
    let mut hidden_fragment =
        test_tar_entry_bytes("tokenzero-v0.1.1/.tokenzero/config.json", b"{}");
    hidden_fragment.extend_from_slice(&[0u8; 1024]);
    let mut bytes = gzip_bytes(&visible_fragment);
    bytes.extend_from_slice(&gzip_bytes(&hidden_fragment));
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_member"),
            ("member", "tokenzero-v0.1.1/.tokenzero/config.json"),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_tar_missing_end_marker() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    fs::write(
        &artifact,
        test_tar_entry_bytes("tokenzero-v0.1.1/LICENSE", b"MIT"),
    )
    .unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_metadata_malformed"
            && issue["detail"]
                .as_str()
                .is_some_and(|d| d.contains("end-of-archive marker"))
    }));
}

#[test]
fn package_audit_fails_closed_on_tar_trailing_data_after_end_marker() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    write_test_tar(&artifact, &["tokenzero-v0.1.1/LICENSE"]);
    let mut bytes = fs::read(&artifact).unwrap();
    bytes.extend_from_slice(&test_tar_entry_bytes(
        "tokenzero-v0.1.1/.tokenzero/config.json",
        b"{}",
    ));
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_trailing_data"
            && issue["detail"]
                .as_str()
                .is_some_and(|d| d.contains("end-of-archive marker"))
    }));
}

#[test]
fn package_audit_fails_closed_on_tar_private_owner_metadata() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let member = "tokenzero-v0.1.1/LICENSE";
    let mut header = test_tar_header(member, b'0', 0, None);
    write_tar_octal(&mut header[108..116], 501);
    write_tar_octal(&mut header[116..124], 20);
    header[265..271].copy_from_slice(b"aditya");
    header[297..302].copy_from_slice(b"staff");
    write_test_tar_checksum(&mut header);
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(&[0u8; 1024]);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue_fields(
        &issues,
        "archive_private_owner_metadata",
        member,
        &["uid", "gid", "uname", "gname"],
        &report,
    );
}

#[test]
fn package_audit_fails_closed_on_tar_special_member_types() {
    let char_device = "tokenzero-v0.1.1/dev/null";
    let fifo = "tokenzero-v0.1.1/run/install.fifo";
    let sparse_launcher = "tokenzero-v0.1.1/bin/tokenzero";
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new(char_device, b'3', b""),
        TarTestEntry::new(fifo, b'6', b""),
        TarTestEntry::new(sparse_launcher, b'S', b"target/release/tokenzero"),
    ]);
    assert_audit_rejected(&report);
    for (member, reason) in [
        (char_device, "character_device"),
        (fifo, "fifo"),
        (sparse_launcher, "sparse_file"),
    ] {
        assert_issue(
            &issues,
            &[
                ("code", "archive_unsupported_member_type"),
                ("member", member),
                ("reason", reason),
            ],
        );
    }
}

#[test]
fn package_audit_rejects_gnu_longlink_sensitive_member() {
    let long_member = format!(
        "tokenzero-v0.1.1/{}/{}/{}/.env",
        "a".repeat(90),
        "b".repeat(90),
        "c".repeat(90)
    );
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("././@LongLink", b'L', format!("{long_member}\0").as_bytes()),
        TarTestEntry::new("payload.txt", b'0', b""),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[("code", "sensitive_member_name"), ("member", &long_member)],
    );
}

#[test]
fn package_audit_rejects_pax_path_private_member() {
    let pax_member = "tokenzero-v0.1.1/.tokenzero/config.json";
    let pax_payload = pax_record("path", pax_member);
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./PaxHeaders.0/config.json", b'x', &pax_payload),
        TarTestEntry::new("config.json", b'0', b""),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_member"),
            ("member", pax_member),
        ],
    );
}

#[test]
fn package_audit_accepts_empty_pax_path_delete_with_header_name() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("pax-empty-path");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let artifact = dir.join("release.tar");
    let pax_payload = pax_record("path", "");
    write_test_tar_entries(
        &artifact,
        &[
            TarTestEntry::new("./PaxHeaders.0/LICENSE", b'x', &pax_payload),
            TarTestEntry::new("tokenzero-v0.1.1/LICENSE", b'0', b"MIT"),
        ],
    );
    assert_eq!(package_audit(&dir, &[artifact])["ok"], true);
}

#[test]
fn package_audit_accepts_empty_pax_linkpath_delete_with_header_target() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("pax-empty-linkpath");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let artifact = dir.join("release.tar");
    let member = "tokenzero-v0.1.1/bin/tokenzero-link";
    let pax_payload = pax_record("linkpath", "");
    write_test_tar_entries(
        &artifact,
        &[
            TarTestEntry::new("./PaxHeaders.0/tokenzero-link", b'x', &pax_payload),
            TarTestEntry::new(member, b'2', b"").with_link_target("bin/tokenzero"),
        ],
    );
    assert_eq!(package_audit(&dir, &[artifact])["ok"], true);
}

#[test]
fn package_audit_empty_pax_path_suppresses_global_pax_path_for_member() {
    let dir = tempdir().unwrap();
    let inner = dir.path().join("inner.tar");
    let artifact = dir.path().join("release.tar");
    let global_path = "tokenzero-v0.1.1/artifacts/inner.tar";
    let nested_member = "tokenzero-v0.1.1/.tokenzero/config.json";
    write_test_tar(&inner, &[nested_member]);
    let inner_bytes = fs::read(&inner).unwrap();
    write_test_tar_entries(
        &artifact,
        &[
            TarTestEntry::new("./GlobalHead.0", b'g', &pax_record("path", global_path)),
            TarTestEntry::new("./PaxHeaders.0/payload.bin", b'x', &pax_record("path", "")),
            TarTestEntry::new("payload.bin", b'0', &inner_bytes),
        ],
    );
    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_global_pax_override_present"),
            ("field", "path"),
        ],
    );
    assert_no_issue_code_member(&issues, "private_tool_state_member", nested_member);
}

#[test]
fn package_audit_empty_pax_linkpath_suppresses_global_pax_linkpath_for_member() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let member = "tokenzero-v0.1.1/bin/tokenzero-link";
    let global_link_target = "../.env";
    write_test_tar_entries(
        &artifact,
        &[
            TarTestEntry::new(
                "./GlobalHead.0",
                b'g',
                &pax_record("linkpath", global_link_target),
            ),
            TarTestEntry::new(
                "./PaxHeaders.0/tokenzero-link",
                b'x',
                &pax_record("linkpath", ""),
            ),
            TarTestEntry::new(member, b'2', b"").with_link_target("bin/tokenzero"),
        ],
    );
    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_global_pax_override_present"),
            ("field", "linkpath"),
        ],
    );
    assert!(
        !issues.iter().any(|issue| {
            issue["code"] == "archive_link_target_escape"
                && issue["member"] == member
                && issue["link_target"] == global_link_target
        }),
        "global PAX linkpath should be deleted for the symlink member: {report:#}"
    );
}

#[test]
fn package_audit_fails_closed_on_invalid_tar_size() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let mut header = test_tar_header("tokenzero-v0.1.1/LICENSE", b'0', 0, None);
    header[124..136].copy_from_slice(b"not-octal\0\0\0");
    write_test_tar_checksum(&mut header);
    fs::write(&artifact, header).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_member_size_malformed"),
            ("member", "tokenzero-v0.1.1/LICENSE"),
        ],
    );
}

#[test]
fn package_audit_reads_bounded_tar_base256_size() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let member = "tokenzero-v0.1.1/.env";
    let payload = b"license";
    let mut bytes = test_tar_entry_bytes(member, payload);
    write_tar_base256(&mut bytes[124..136], payload.len() as u128);
    write_test_tar_checksum_bytes(&mut bytes[0..512]);
    bytes.extend_from_slice(&[0u8; 1024]);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[("code", "sensitive_member_name"), ("member", member)],
    );
    assert_no_issue(&issues, "archive_member_size_malformed");
}

#[test]
fn package_audit_fails_closed_on_negative_tar_base256_size() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let mut header = test_tar_header("tokenzero-v0.1.1/LICENSE", b'0', 0, None);
    header[124..136].fill(0xff);
    write_test_tar_checksum(&mut header);
    fs::write(&artifact, header).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue_detail(
        &issues,
        "archive_member_size_malformed",
        "tokenzero-v0.1.1/LICENSE",
        "negative base-256",
    );
}

#[test]
fn package_audit_fails_closed_on_oversized_tar_base256_size() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let mut header = test_tar_header("tokenzero-v0.1.1/LICENSE", b'0', 0, None);
    header[124..136].fill(0);
    header[124] = 0x81;
    write_test_tar_checksum(&mut header);
    fs::write(&artifact, header).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue_detail(
        &issues,
        "archive_member_size_malformed",
        "tokenzero-v0.1.1/LICENSE",
        "too large",
    );
}

#[test]
fn package_audit_fails_closed_on_invalid_tar_checksum() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let mut header = test_tar_header("tokenzero-v0.1.1/LICENSE", b'0', 0, None);
    header[148..156].copy_from_slice(b"000000\0 ");
    fs::write(&artifact, header).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue_detail(
        &issues,
        "archive_member_metadata_malformed",
        "tokenzero-v0.1.1/LICENSE",
        "checksum",
    );
}

#[test]
fn package_audit_fails_closed_on_truncated_tar_payload() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let mut bytes = test_tar_header("tokenzero-v0.1.1/LICENSE", b'0', 16, None).to_vec();
    bytes.extend_from_slice(b"partial");
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_member_payload_truncated"),
            ("member", "tokenzero-v0.1.1/LICENSE"),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_malformed_pax_path() {
    let hidden_member = "tokenzero-v0.1.1/.tokenzero/config.json";
    let malformed_pax = format!("999 path={hidden_member}\n");
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./PaxHeaders.0/config.json", b'x', malformed_pax.as_bytes()),
        TarTestEntry::new("config.json", b'0', b""),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_member_metadata_malformed"),
            ("member", "./PaxHeaders.0/config.json"),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_duplicate_pax_overrides() {
    let dir = tempdir().unwrap();
    let path_artifact = dir.path().join("duplicate-path.tar");
    let linkpath_artifact = dir.path().join("duplicate-linkpath.tar");
    let hidden_member = "tokenzero-v0.1.1/.tokenzero/config.json";
    let safe_member = "tokenzero-v0.1.1/config.json";
    let hidden_link_target = "../.env";
    let safe_link_target = "config.json";

    let mut duplicate_path = pax_record("path", hidden_member);
    duplicate_path.extend_from_slice(&pax_record("path", safe_member));
    write_test_tar_entries(
        &path_artifact,
        &[
            TarTestEntry::new("./PaxHeaders.0/config.json", b'x', &duplicate_path),
            TarTestEntry::new("config.json", b'0', b"{}"),
        ],
    );

    let mut duplicate_linkpath = pax_record("linkpath", hidden_link_target);
    duplicate_linkpath.extend_from_slice(&pax_record("linkpath", safe_link_target));
    write_test_tar_entries(
        &linkpath_artifact,
        &[
            TarTestEntry::new("./PaxHeaders.0/config-link", b'x', &duplicate_linkpath),
            TarTestEntry::new("config-link", b'2', b"").with_link_target(safe_link_target),
        ],
    );

    let report = package_audit(dir.path(), &[path_artifact, linkpath_artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    for (member, field) in [
        ("./PaxHeaders.0/config.json", "path"),
        ("./PaxHeaders.0/config-link", "linkpath"),
    ] {
        assert_issue_detail(&issues, "archive_member_metadata_malformed", member, field);
    }
}

#[test]
fn package_audit_fails_closed_on_pax_private_metadata_fields() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let mut pax_payload = pax_record("uname", "builder");
    pax_payload.extend_from_slice(&pax_record("comment", "/tmp/example/release"));
    write_test_tar_entries(
        &artifact,
        &[
            TarTestEntry::new("./PaxHeaders.0/LICENSE", b'x', &pax_payload),
            TarTestEntry::new("tokenzero-v0.1.1/LICENSE", b'0', b"MIT"),
        ],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue_fields(
        &issues,
        "archive_pax_metadata_present",
        "./PaxHeaders.0/LICENSE",
        &["uname", "comment"],
        &report,
    );
    assert_issue_no_secret(
        &issues,
        "archive_pax_metadata_present",
        "./PaxHeaders.0/LICENSE",
        "builder",
        &report,
    );
    assert_issue_no_secret(
        &issues,
        "archive_pax_metadata_present",
        "./PaxHeaders.0/LICENSE",
        "/tmp/example",
        &report,
    );
}

#[test]
fn package_audit_fails_closed_on_global_pax_metadata_fields() {
    let pax_payload = pax_record("SCHILY.xattr.com.apple.quarantine", "local-machine");
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./GlobalHead.0", b'g', &pax_payload),
        TarTestEntry::new("tokenzero-v0.1.1/LICENSE", b'0', b"MIT"),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_pax_metadata_present"),
            ("member", "./GlobalHead.0"),
        ],
    );
    assert!(
        issues.iter().any(|issue| {
            issue["code"] == "archive_pax_metadata_present"
                && issue["fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|f| f == "SCHILY.xattr.*")
        }),
        "expected SCHILY.xattr.* field in {issues:#?}"
    );
}

#[test]
fn package_audit_fails_closed_on_benign_global_pax_path_override() {
    let global_path = "tokenzero-v0.1.1/LICENSE";
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./GlobalHead.0", b'g', &pax_record("path", global_path)),
        TarTestEntry::new("LICENSE", b'0', b"MIT"),
    ]);
    assert_audit_rejected(&report);
    assert_issue_no_secret(
        &issues,
        "archive_global_pax_override_present",
        "./GlobalHead.0",
        global_path,
        &report,
    );
}

#[test]
fn package_audit_fails_closed_on_benign_global_pax_linkpath_override() {
    let member = "tokenzero-v0.1.1/bin/tokenzero-link";
    let global_link_target = "bin/tokenzero";
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new(
            "./GlobalHead.0",
            b'g',
            &pax_record("linkpath", global_link_target),
        ),
        TarTestEntry::new(member, b'2', b"").with_link_target(global_link_target),
    ]);
    assert_audit_rejected(&report);
    assert_issue_no_secret(
        &issues,
        "archive_global_pax_override_present",
        "./GlobalHead.0",
        global_link_target,
        &report,
    );
}

#[test]
fn package_audit_applies_global_pax_path_to_nested_archive_payload() {
    let dir = tempdir().unwrap();
    let inner = dir.path().join("inner.tar");
    let artifact = dir.path().join("release.tar");
    let global_path = "tokenzero-v0.1.1/artifacts/inner.tar";
    let nested_member = "tokenzero-v0.1.1/.tokenzero/config.json";
    write_test_tar(&inner, &[nested_member]);
    let inner_bytes = fs::read(&inner).unwrap();
    write_test_tar_entries(
        &artifact,
        &[
            TarTestEntry::new("./GlobalHead.0", b'g', &pax_record("path", global_path)),
            TarTestEntry::new("payload.bin", b'0', &inner_bytes),
        ],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "private_tool_state_member"
            && issue["path"]
                .as_str()
                .is_some_and(|p| p.contains("release.tar!") && p.contains(global_path))
            && issue["member"] == nested_member
    }));
}

#[test]
fn package_audit_applies_global_pax_path_to_duplicate_detection() {
    let global_path = "tokenzero-v0.1.1/LICENSE";
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./GlobalHead.0", b'g', &pax_record("path", global_path)),
        TarTestEntry::new("first.txt", b'0', b"first"),
        TarTestEntry::new("second.txt", b'0', b"second"),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "tar_duplicate_member_name"),
            ("member", global_path),
        ],
    );
}

#[test]
fn package_audit_applies_global_pax_linkpath_to_following_links() {
    let member = "tokenzero-v0.1.1/bin/tokenzero-link";
    let global_link_target = "../.env";
    let header_link_target = "bin/tokenzero";
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new(
            "./GlobalHead.0",
            b'g',
            &pax_record("linkpath", global_link_target),
        ),
        TarTestEntry::new(member, b'2', b"").with_link_target(header_link_target),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_link_target_escape"),
            ("member", member),
            ("link_target", global_link_target),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_duplicate_tar_member_names() {
    let dir = tempdir().unwrap();
    let tar_artifact = dir.path().join("release.tar");
    let gzip_artifact = dir.path().join("release.tar.gz");
    let member = "tokenzero-v0.1.1/LICENSE";
    write_test_tar_entries(
        &tar_artifact,
        &[
            TarTestEntry::new(member, b'0', b"first"),
            TarTestEntry::new(member, b'0', b"second"),
        ],
    );
    fs::write(
        &gzip_artifact,
        gzip_bytes(&fs::read(&tar_artifact).unwrap()),
    )
    .unwrap();

    let report = package_audit(dir.path(), &[tar_artifact.clone(), gzip_artifact.clone()]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    for artifact in [tar_artifact, gzip_artifact] {
        assert_issue(
            &issues,
            &[
                ("code", "tar_duplicate_member_name"),
                ("path", &artifact.display().to_string()),
                ("member", member),
            ],
        );
    }
}

#[test]
fn package_audit_rejects_archive_member_path_escape() {
    let parent_member = "tokenzero-v0.1.1/../.env";
    let absolute_member = "/tmp/tokenzero/LICENSE";
    let windows_member = "C:/Users/example/.ssh/id_ed25519";
    let (report, issues) =
        run_tar_audit_from_names(&[parent_member, absolute_member, windows_member]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_member_path_escape"),
            ("member", parent_member),
            ("reason", "parent_directory"),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "archive_member_path_escape"),
            ("member", absolute_member),
            ("reason", "absolute_path"),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "archive_member_path_escape"),
            ("member", windows_member),
            ("reason", "windows_drive_path"),
        ],
    );
}

#[test]
fn package_audit_rejects_tar_link_target_escape() {
    let symlink_member = "tokenzero-v0.1.1/bin/tokenzero";
    let hardlink_member = "tokenzero-v0.1.1/cache/recovery-cache.json";
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new(symlink_member, b'2', b"").with_link_target("../.env"),
        TarTestEntry::new(hardlink_member, b'1', b"")
            .with_link_target("/home/example/.tokenzero/recovery-cache.json"),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "archive_link_target_escape"),
            ("member", symlink_member),
            ("link_kind", "symlink"),
            ("reason", "parent_directory"),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "sensitive_link_target"),
            ("member", symlink_member),
            ("link_target", "../.env"),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "archive_link_target_escape"),
            ("member", hardlink_member),
            ("link_kind", "hardlink"),
            ("reason", "absolute_path"),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_link_target"),
            ("member", hardlink_member),
            (
                "link_target",
                "/home/example/.tokenzero/recovery-cache.json",
            ),
        ],
    );
}

#[test]
fn package_audit_rejects_private_dotdir_directory_leaf_members() {
    let dir = tempdir().unwrap();
    let tar_artifact = dir.path().join("release.tar");
    let zip_artifact = dir.path().join("release.zip");
    let tar_private_dir = "tokenzero-v0.1.1/.tokenzero";
    let zip_private_dir = "tokenzero-v0.1.1/.cursor/";
    write_test_tar_entries(
        &tar_artifact,
        &[TarTestEntry::new(tar_private_dir, b'5', b"")],
    );
    write_test_zip(&zip_artifact, &[ZipTestEntry::file(zip_private_dir, b"")]);

    let report = package_audit(dir.path(), &[tar_artifact, zip_artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_member"),
            ("member", tar_private_dir),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_member"),
            ("member", zip_private_dir),
        ],
    );
}

#[test]
fn package_audit_rejects_private_dotdir_link_target_leaf() {
    let symlink_member = "tokenzero-v0.1.1/config-link";
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new(symlink_member, b'2', b"").with_link_target(".tokenzero")
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_link_target"),
            ("member", symlink_member),
            ("link_target", ".tokenzero"),
        ],
    );
}

#[test]
fn package_audit_rejects_pax_global_private_metadata() {
    let global_path = "tokenzero-v0.1.1/.tokenzero/config.json";
    let global_linkpath = "../.env";
    let mut pax_payload = pax_record("path", global_path);
    pax_payload.extend_from_slice(&pax_record("linkpath", global_linkpath));
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./GlobalHead.0", b'g', &pax_payload),
        TarTestEntry::new("tokenzero-v0.1.1/config.json", b'0', b"{}"),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_member"),
            ("member", global_path),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "archive_link_target_escape"),
            ("member", "./GlobalHead.0"),
            ("link_target", global_linkpath),
            ("reason", "parent_directory"),
        ],
    );
}

#[test]
fn package_audit_rejects_pax_and_gnu_link_targets() {
    let pax_member = "tokenzero-v0.1.1/config";
    let pax_target = "tokenzero-v0.1.1/.tokenzero/config.json";
    let pax_payload = pax_record("linkpath", pax_target);
    let gnu_member = "tokenzero-v0.1.1/ssh-key";
    let gnu_target = format!("../{}/id_ed25519", "private".repeat(20));
    let gnu_target_payload = format!("{gnu_target}\0");
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./PaxHeaders.0/config", b'x', &pax_payload),
        TarTestEntry::new(pax_member, b'2', b""),
        TarTestEntry::new("././@LongLink", b'K', gnu_target_payload.as_bytes()),
        TarTestEntry::new(gnu_member, b'2', b""),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_link_target"),
            ("member", pax_member),
            ("link_target", pax_target),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "archive_link_target_escape"),
            ("member", gnu_member),
            ("link_target", &gnu_target),
            ("reason", "parent_directory"),
        ],
    );
    assert_issue(
        &issues,
        &[
            ("code", "sensitive_link_target"),
            ("member", gnu_member),
            ("link_target", &gnu_target),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_conflicting_tar_name_overrides() {
    let safe_long_member = "tokenzero-v0.1.1/config.json";
    let private_pax_member = "tokenzero-v0.1.1/.tokenzero/config.json";
    let pax_payload = pax_record("path", private_pax_member);
    let long_payload = format!("{safe_long_member}\0");
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./PaxHeaders.0/config.json", b'x', &pax_payload),
        TarTestEntry::new("././@LongLink", b'L', long_payload.as_bytes()),
        TarTestEntry::new("config.json", b'0', b""),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_member"),
            ("member", private_pax_member),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_conflicting_tar_link_overrides() {
    let symlink_member = "tokenzero-v0.1.1/config-link";
    let private_pax_target = "tokenzero-v0.1.1/.tokenzero/config.json";
    let pax_payload = pax_record("linkpath", private_pax_target);
    let long_payload = "tokenzero-v0.1.1/config.json\0".to_string();
    let (report, issues) = run_tar_audit(&[
        TarTestEntry::new("./PaxHeaders.0/config-link", b'x', &pax_payload),
        TarTestEntry::new("././@LongLink", b'K', long_payload.as_bytes()),
        TarTestEntry::new(symlink_member, b'2', b"").with_link_target("config.json"),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "private_tool_state_link_target"),
            ("member", symlink_member),
            ("link_target", private_pax_target),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_tar_directory_payload() {
    let member = "tokenzero-v0.1.1/docs/";
    let (report, issues) = run_tar_audit(&[TarTestEntry::new(member, b'5', b"hidden")]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "tar_directory_payload_present"),
            ("member", member),
        ],
    );
}
