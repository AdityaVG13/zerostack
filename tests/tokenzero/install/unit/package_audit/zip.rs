use super::fixtures::*;
use super::*;

/// Assert the standard symlink escape issues for `../.env` targeting `symlink_member`.
fn assert_symlink_escape_issues(issues: &[serde_json::Value], symlink_member: &str) {
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_link_target_escape"
            && issue["member"] == symlink_member
            && issue["link_kind"] == "symlink"
            && issue["reason"] == "parent_directory"
    }));
    assert!(issues.iter().any(|issue| {
        issue["code"] == "sensitive_link_target"
            && issue["member"] == symlink_member
            && issue["link_target"] == "../.env"
    }));
}

/// Build a zip with a `../.env` symlink using the given encoding variant,
/// run `package_audit`, and assert the standard escape issues.
fn symlink_escape_encoding_variant<F>(build_entry: F)
where
    F: FnOnce(ZipTestEntry<'_>) -> ZipTestEntry<'_>,
{
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let symlink_member = "tokenzero-v0.1.1/bin/tokenzero";
    let base = ZipTestEntry::symlink(symlink_member, b"../.env");
    write_test_zip(&artifact, &[build_entry(base)]);

    let report = package_audit(dir.path(), &[artifact]);
    assert_eq!(report["ok"], false);
    assert_symlink_escape_issues(report["issues"].as_array().unwrap(), symlink_member);
}

/// Write a zip with a data-descriptor symlink, tamper bytes in-place, and return
/// the audit report. Keeps the data-descriptor CRC/size tests DRY.
fn tamper_zip_data_descriptor<F>(tamper: F) -> serde_json::Value
where
    F: FnOnce(&mut Vec<u8>, &std::path::Path),
{
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let symlink_member = "tokenzero-v0.1.1/bin/tokenzero-link";
    let target = b"bin/tokenzero";
    write_test_zip(
        &artifact,
        &[ZipTestEntry::symlink(symlink_member, target).with_data_descriptor()],
    );

    let mut bytes = fs::read(&artifact).unwrap();
    tamper(&mut bytes, &artifact);
    fs::write(&artifact, &bytes).unwrap();

    package_audit(dir.path(), &[artifact])
}

/// Run `package_audit` on a single zip archive built from `entries`, returning
/// the full report and the issues array.
fn run_zip_audit(entries: &[ZipTestEntry<'_>]) -> (serde_json::Value, Vec<serde_json::Value>) {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(&artifact, entries);
    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    (report, issues)
}

/// Assert at least one issue matches all key/value pairs in `fields`.
fn assert_issue(issues: &[serde_json::Value], fields: &[(&str, &str)]) {
    assert!(
        issues
            .iter()
            .any(|issue| fields.iter().all(|(k, v)| issue[*k] == *v)),
        "expected issue matching {fields:?} in {issues:#?}"
    );
}

/// Assert the report was rejected.
fn assert_audit_rejected(report: &serde_json::Value) {
    assert_eq!(report["ok"], false);
}

/// Assert at least one `archive_member_listing_failed` issue has a detail containing `s`.
fn assert_listing_failure(issues: &[serde_json::Value], detail_contains: &str) {
    assert!(
        issues.iter().any(|issue| {
            issue["code"] == "archive_member_listing_failed"
                && issue["detail"]
                    .as_str()
                    .is_some_and(|d| d.contains(detail_contains))
        }),
        "expected archive_member_listing_failed with detail containing '{detail_contains}' in {issues:#?}"
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
        "expected {code} for {member} with detail containing '{s}' in {issues:#?}"
    );
}

/// Read a zip file, locate the EOCD and central directory offset, and return
/// (bytes, eocd_offset, central_directory_offset) for byte-level tampering.
fn read_zip_with_offsets(path: &std::path::Path) -> (Vec<u8>, usize, usize) {
    let bytes = fs::read(path).unwrap();
    let eocd = find_zip_eocd(&bytes).unwrap();
    let cd = zip_u32_at(&bytes, eocd + 16).unwrap() as usize;
    (bytes, eocd, cd)
}

#[test]
fn package_audit_rejects_zip_symlink_target_escape() {
    let symlink_member = "tokenzero-v0.1.1/bin/tokenzero";
    let (report, issues) = run_zip_audit(&[ZipTestEntry::symlink(symlink_member, b"../.env")]);
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
}

#[test]
fn package_audit_fails_closed_on_unreadable_zip_symlink_target() {
    let symlink_member = "tokenzero-v0.1.1/config-link";
    let (report, issues) =
        run_zip_audit(&[ZipTestEntry::symlink(symlink_member, b"not-deflated").with_method(8)]);
    assert_audit_rejected(&report);
    assert_issue_detail(
        &issues,
        "zip_symlink_target_unreadable",
        symlink_member,
        "deflate",
    );
}

#[test]
fn symlink_escape_detects_deflated_encoding() {
    let symlink_member = "tokenzero-v0.1.1/bin/tokenzero";
    let compressed_target = deflate_bytes(b"../.env");
    let (report, issues) =
        run_zip_audit(&[ZipTestEntry::symlink(symlink_member, &compressed_target).with_method(8)]);
    assert_audit_rejected(&report);
    assert_symlink_escape_issues(&issues, symlink_member);
}

#[test]
fn symlink_escape_detects_data_descriptor_encoding() {
    symlink_escape_encoding_variant(|entry| entry.with_data_descriptor());
}

#[test]
fn symlink_escape_detects_unsigned_data_descriptor_encoding() {
    let symlink_member = "tokenzero-v0.1.1/bin/tokenzero";
    let (report, issues) = run_zip_audit(&[
        ZipTestEntry::symlink(symlink_member, b"../.env").with_unsigned_data_descriptor()
    ]);
    assert_audit_rejected(&report);
    assert_symlink_escape_issues(&issues, symlink_member);
    assert!(
        !issues
            .iter()
            .any(|issue| issue["code"] == "zip_data_descriptor_mismatch")
    );
}

#[test]
fn data_descriptor_crc_mismatch_is_rejected() {
    let symlink_member = "tokenzero-v0.1.1/bin/tokenzero-link";
    let report = tamper_zip_data_descriptor(|bytes, _path| {
        let local_header = zip_local_header(bytes, 0)
            .unwrap_or_else(|error| panic!("{}", zip_payload_error_detail(error)));
        let descriptor_crc_offset = local_header.data_start + b"bin/tokenzero".len() + 4;
        let wrong_crc = zip_crc32(b"bin/tokenzero") ^ u32::MAX;
        set_zip_u32_at(bytes, descriptor_crc_offset, wrong_crc);
    });
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue_detail(
        &issues,
        "zip_data_descriptor_mismatch",
        symlink_member,
        "CRC",
    );
}

#[test]
fn data_descriptor_local_size_mismatch_is_rejected() {
    let symlink_member = "tokenzero-v0.1.1/bin/tokenzero-link";
    let report = tamper_zip_data_descriptor(|bytes, _path| {
        let wrong_size = u32::try_from(b"bin/tokenzero".len() + 1).unwrap();
        set_zip_u32_at(bytes, 18, wrong_size);
        set_zip_u32_at(bytes, 22, wrong_size);
    });
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "zip_local_header_metadata_mismatch"),
            ("member", symlink_member),
            ("field", "data_descriptor_sizes"),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_zip64_data_descriptor_size_mismatch() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let symlink_member = "tokenzero-v0.1.1/config-link";
    let target = b"config.json";

    write_test_zip(
        &artifact,
        &[ZipTestEntry::symlink(symlink_member, target)
            .with_data_descriptor()
            .with_zip64_extra_fields()],
    );

    let (mut bytes, _, cd) = read_zip_with_offsets(&artifact);
    let name_len = zip_u16_at(&bytes, cd + 28).unwrap() as usize;
    let zip64_extra_offset = cd + 46 + name_len;
    assert_eq!(
        zip_u16_at(&bytes, zip64_extra_offset).unwrap(),
        ZIP64_EXTENDED_INFORMATION_EXTRA
    );
    set_zip_u64_at(
        &mut bytes,
        zip64_extra_offset + 4,
        u32::MAX as u64 + 1 + target.len() as u64,
    );
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_data_descriptor_mismatch"
            && issue["member"] == symlink_member
            && issue["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("zip64 descriptor"))
    }));
}

#[test]
fn package_audit_fails_closed_on_zip_stored_size_mismatch() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/LICENSE";
    write_test_zip(&artifact, &[ZipTestEntry::file(member, b"MIT")]);

    let (mut bytes, _, cd) = read_zip_with_offsets(&artifact);
    set_zip_u32_at(&mut bytes, 22, 4);
    set_zip_u32_at(&mut bytes, cd + 24, 4);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[("code", "zip_entry_size_mismatch"), ("member", member)],
    );
}

#[test]
fn package_audit_fails_closed_on_zip_symlink_payload_size_mismatch() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let symlink_member = "tokenzero-v0.1.1/config-link";
    let target = b"config.json";
    write_test_zip(&artifact, &[ZipTestEntry::symlink(symlink_member, target)]);

    let (mut bytes, _, cd) = read_zip_with_offsets(&artifact);
    set_zip_u32_at(&mut bytes, 22, target.len() as u32 + 1);
    set_zip_u32_at(&mut bytes, cd + 24, target.len() as u32 + 1);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue_detail(
        &issues,
        "zip_symlink_target_unreadable",
        symlink_member,
        "uncompressed size mismatch",
    );
}

#[test]
fn package_audit_fails_closed_on_zip_payload_overlap_with_central_directory() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/LICENSE";
    write_test_zip(&artifact, &[ZipTestEntry::file(member, b"MIT")]);

    let local_header = zip_local_header(&fs::read(&artifact).unwrap(), 0)
        .unwrap_or_else(|error| panic!("{}", zip_payload_error_detail(error)));
    let (mut bytes, _, cd) = read_zip_with_offsets(&artifact);
    let overlapping_size = cd - local_header.data_start + 1;
    set_zip_u32_at(&mut bytes, 18, overlapping_size as u32);
    set_zip_u32_at(&mut bytes, 22, overlapping_size as u32);
    set_zip_u32_at(&mut bytes, cd + 20, overlapping_size as u32);
    set_zip_u32_at(&mut bytes, cd + 24, overlapping_size as u32);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "zip_local_record_overlap"),
            ("member", member),
            ("field", "central_directory"),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_overlapping_zip_local_records() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let first = "tokenzero-v0.1.1/LICENSE";
    let second = "tokenzero-v0.1.1/NOTICE";
    write_test_zip(
        &artifact,
        &[
            ZipTestEntry::file(first, b"first"),
            ZipTestEntry::file(second, b"second"),
        ],
    );

    let first_header = zip_local_header(&fs::read(&artifact).unwrap(), 0)
        .unwrap_or_else(|error| panic!("{}", zip_payload_error_detail(error)));
    let (mut bytes, _, cd) = read_zip_with_offsets(&artifact);
    let overlapping_size = first_header.data_start + b"first".len() - first_header.data_start + 1;
    set_zip_u32_at(&mut bytes, 18, overlapping_size as u32);
    set_zip_u32_at(&mut bytes, 22, overlapping_size as u32);
    set_zip_u32_at(&mut bytes, cd + 20, overlapping_size as u32);
    set_zip_u32_at(&mut bytes, cd + 24, overlapping_size as u32);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "zip_local_record_overlap"),
            ("member", first),
            ("field", "local_record"),
            ("next_member", second),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_missing_zip_data_descriptor_before_central_directory() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/bin/tokenzero-link";
    let target = b"bin/tokenzero";
    write_test_zip(
        &artifact,
        &[ZipTestEntry::symlink(member, target).with_data_descriptor()],
    );

    let local_header = zip_local_header(&fs::read(&artifact).unwrap(), 0)
        .unwrap_or_else(|error| panic!("{}", zip_payload_error_detail(error)));
    let (mut bytes, _, cd) = read_zip_with_offsets(&artifact);
    let descriptor_start = local_header.data_start + target.len();
    assert_eq!(cd - descriptor_start, 16);
    bytes.drain(descriptor_start..cd);
    let new_eocd = find_zip_eocd(&bytes).unwrap();
    set_zip_u32_at(&mut bytes, new_eocd + 16, descriptor_start as u32);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_data_descriptor_mismatch"
            && issue["member"] == member
            && issue["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("before the central directory"))
    }));
}

#[test]
fn package_audit_recurses_into_nested_archives() {
    let dir = tempdir().unwrap();
    let inner = dir.path().join("inner.tar");
    let artifact = dir.path().join("release.zip");
    let nested_member = "tokenzero-v0.1.1/.tokenzero/config.json";
    let outer_member = "tokenzero-v0.1.1/artifacts/inner.tar";

    write_test_tar(&inner, &[nested_member]);
    let inner_bytes = fs::read(&inner).unwrap();
    write_test_zip(&artifact, &[ZipTestEntry::file(outer_member, &inner_bytes)]);

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "private_tool_state_member"
            && issue["path"]
                .as_str()
                .is_some_and(|path| path.contains("release.zip!") && path.contains(outer_member))
            && issue["member"] == nested_member
    }));
}

#[test]
fn package_audit_recurses_into_deflated_nested_zip_archives() {
    let dir = tempdir().unwrap();
    let inner = dir.path().join("inner.tar");
    let artifact = dir.path().join("release.zip");
    let nested_member = "tokenzero-v0.1.1/.tokenzero/config.json";
    let outer_member = "tokenzero-v0.1.1/artifacts/inner.tar";

    write_test_tar(&inner, &[nested_member]);
    let compressed_inner = deflate_bytes(&fs::read(&inner).unwrap());
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(outer_member, &compressed_inner).with_method(8)],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "private_tool_state_member"
            && issue["path"]
                .as_str()
                .is_some_and(|path| path.contains("release.zip!") && path.contains(outer_member))
            && issue["member"] == nested_member
    }));
}

#[test]
fn package_audit_fails_closed_on_nested_zip_archive_crc_mismatch() {
    let dir = tempdir().unwrap();
    let inner = dir.path().join("inner.tar");
    let artifact = dir.path().join("release.zip");
    let outer_member = "tokenzero-v0.1.1/artifacts/inner.tar";

    write_test_tar(&inner, &["tokenzero-v0.1.1/LICENSE"]);
    let inner_bytes = fs::read(&inner).unwrap();
    write_test_zip(&artifact, &[ZipTestEntry::file(outer_member, &inner_bytes)]);

    let mut bytes = fs::read(&artifact).unwrap();
    let eocd_offset = find_zip_eocd(&bytes).unwrap();
    let central_directory_offset = zip_u32_at(&bytes, eocd_offset + 16).unwrap() as usize;
    let wrong_crc = zip_crc32(&inner_bytes) ^ u32::MAX;
    set_zip_u32_at(&mut bytes, 14, wrong_crc);
    set_zip_u32_at(&mut bytes, central_directory_offset + 16, wrong_crc);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "nested_archive_member_unreadable"
            && issue["member"] == outer_member
            && issue["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("CRC mismatch"))
    }));
}

#[test]
fn package_audit_fails_closed_on_zip_local_header_name_mismatch() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let central_member = "tokenzero-v0.1.1/config.json";
    let local_member = "tokenzero-v0.1.1/.tokenzero/config.json";

    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(central_member, b"{}").with_local_name(local_member)],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_local_header_name_mismatch"
            && issue["member"] == central_member
            && issue["local_member"] == local_member
    }));
    assert!(issues.iter().any(|issue| {
        issue["code"] == "private_tool_state_member" && issue["member"] == local_member
    }));
}

#[test]
fn unicode_path_extra_rejects_private_member_in_central_and_local() {
    let visible_member = "tokenzero-v0.1.1/config.json";
    let unicode_member = "tokenzero-v0.1.1/.tokenzero/config.json";

    for (label, build_entry) in [
        (
            "central",
            ZipTestEntry::file(visible_member, b"{}").with_central_unicode_path(unicode_member),
        ),
        (
            "local",
            ZipTestEntry::file(visible_member, b"{}").with_local_unicode_path(unicode_member),
        ),
    ] {
        let dir = tempdir().unwrap();
        let artifact = dir.path().join("release.zip");
        write_test_zip(&artifact, &[build_entry]);

        let report = package_audit(dir.path(), &[artifact]);
        let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

        assert_eq!(report["ok"], false, "{label} unicode path must reject");
        assert!(
            issues.iter().any(|issue| {
                issue["code"] == "private_tool_state_member" && issue["member"] == unicode_member
            }),
            "{label}: missing private_tool_state_member for {unicode_member}"
        );
    }
}

#[test]
fn package_audit_fails_closed_on_conflicting_zip_unicode_path_extra_fields() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let visible_member = "tokenzero-v0.1.1/config.json";
    let central_unicode_member = "tokenzero-v0.1.1/config-central.json";
    let local_unicode_member = "tokenzero-v0.1.1/config-local.json";

    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(visible_member, b"{}")
            .with_central_unicode_path(central_unicode_member)
            .with_local_unicode_path(local_unicode_member)],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_local_header_metadata_mismatch"
            && issue["member"] == visible_member
            && issue["field"] == "unicode_path"
            && issue["central"] == central_unicode_member
            && issue["local"] == local_unicode_member
    }));
}

#[test]
fn package_audit_fails_closed_on_malformed_zip_unicode_path_extra() {
    let visible_member = "tokenzero-v0.1.1/config.json";
    let malformed_unicode_path = vec![0x75, 0x70, 0x05, 0x00, 1, 0, 0, 0, 0];
    let (report, issues) = run_zip_audit(&[
        ZipTestEntry::file(visible_member, b"{}").with_central_extra(malformed_unicode_path)
    ]);
    assert_audit_rejected(&report);
    assert_listing_failure(&issues, "unicode path extra field");
}

#[test]
fn package_audit_recurses_into_zip_unicode_path_extra_nested_archive() {
    let dir = tempdir().unwrap();
    let inner = dir.path().join("inner.tar");
    let artifact = dir.path().join("release.zip");
    let visible_member = "tokenzero-v0.1.1/artifacts/payload.bin";
    let unicode_member = "tokenzero-v0.1.1/artifacts/inner.tar";
    let nested_member = "tokenzero-v0.1.1/.tokenzero/config.json";

    write_test_tar(&inner, &[nested_member]);
    let inner_bytes = fs::read(&inner).unwrap();
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(visible_member, &inner_bytes)
            .with_central_unicode_path(unicode_member)],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "private_tool_state_member"
            && issue["path"]
                .as_str()
                .is_some_and(|path| path.contains("release.zip!") && path.contains(unicode_member))
            && issue["member"] == nested_member
    }));
}

#[test]
fn package_audit_rejects_zip_unicode_path_extra_dotdir_directory() {
    let visible_member = "tokenzero-v0.1.1/metadata";
    let unicode_member = "tokenzero-v0.1.1/.idea/";
    let (report, issues) = run_zip_audit(&[
        ZipTestEntry::file(visible_member, b"").with_central_unicode_path(unicode_member)
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "non_public_dotdir_member"),
            ("member", unicode_member),
        ],
    );
}

#[test]
fn package_audit_fails_closed_on_split_zip_archive() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
    );

    let (mut bytes, eocd, _) = read_zip_with_offsets(&artifact);
    set_zip_u16_at(&mut bytes, eocd + 4, 1);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    assert_audit_rejected(&report);
    assert_listing_failure(report["issues"].as_array().unwrap(), "multi-disk");
}

#[test]
fn package_audit_fails_closed_on_duplicate_zip_eocd_candidates() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(
            "tokenzero-v0.1.1/.tokenzero/config.json",
            b"{}",
        )],
    );

    let mut bytes = fs::read(&artifact).unwrap();
    let original_eocd_offset = find_zip_eocd(&bytes).unwrap();
    set_zip_u16_at(&mut bytes, original_eocd_offset + 20, 22);
    push_zip_u32(&mut bytes, 0x0605_4b50);
    push_zip_u16(&mut bytes, 0);
    push_zip_u16(&mut bytes, 0);
    push_zip_u16(&mut bytes, 0);
    push_zip_u16(&mut bytes, 0);
    push_zip_u32(&mut bytes, 0);
    push_zip_u32(&mut bytes, 0);
    push_zip_u16(&mut bytes, 0);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_listing_failed"
            && issue["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("plausible end-of-central-directory"))
    }));
}

#[test]
fn package_audit_fails_closed_on_zip_central_directory_inside_eocd_comment() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
    );

    let bytes = fs::read(&artifact).unwrap();
    let original_eocd_offset = find_zip_eocd(&bytes).unwrap();
    let original_directory_offset = zip_u32_at(&bytes, original_eocd_offset + 16).unwrap() as usize;
    let central_directory = bytes[original_directory_offset..original_eocd_offset].to_vec();
    let mut reordered = bytes[..original_directory_offset].to_vec();
    let new_eocd_offset = reordered.len();
    push_zip_u32(&mut reordered, 0x0605_4b50);
    push_zip_u16(&mut reordered, 0);
    push_zip_u16(&mut reordered, 0);
    push_zip_u16(&mut reordered, 1);
    push_zip_u16(&mut reordered, 1);
    push_zip_u32(&mut reordered, central_directory.len() as u32);
    push_zip_u32(&mut reordered, (new_eocd_offset + 22) as u32);
    push_zip_u16(&mut reordered, central_directory.len() as u16);
    reordered.extend_from_slice(&central_directory);
    fs::write(&artifact, reordered).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_listing_failed"
            && issue["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("central directory overlaps or follows"))
    }));
}

#[test]
fn package_audit_fails_closed_on_zip64_entry_field_sentinel() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
    );

    let (mut bytes, _, cd) = read_zip_with_offsets(&artifact);
    set_zip_u32_at(&mut bytes, cd + 42, u32::MAX);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    assert_audit_rejected(&report);
    assert_listing_failure(
        report["issues"].as_array().unwrap(),
        "zip64 extended information",
    );
}

#[test]
fn package_audit_reads_zip64_entry_extra_fields() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT").with_zip64_extra_fields()],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["archives_checked"], 1);
    assert!(
        !issues
            .iter()
            .any(|issue| issue["code"] == "archive_member_listing_failed"),
        "{report:#}"
    );
}

#[test]
fn package_audit_recurses_into_zip64_nested_archive() {
    let dir = tempdir().unwrap();
    let inner = dir.path().join("inner.tar");
    let artifact = dir.path().join("release.zip");
    let outer_member = "tokenzero-v0.1.1/artifacts/inner.tar";
    let nested_member = "tokenzero-v0.1.1/.tokenzero/config.json";

    write_test_tar(&inner, &[nested_member]);
    let inner_bytes = fs::read(&inner).unwrap();
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(outer_member, &inner_bytes).with_zip64_extra_fields()],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "private_tool_state_member"
            && issue["path"]
                .as_str()
                .is_some_and(|path| path.contains("release.zip!") && path.contains(outer_member))
            && issue["member"] == nested_member
    }));
}

#[test]
fn package_audit_fails_closed_on_duplicate_zip64_extra_field() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let mut duplicate_zip64 = zip64_extended_info_extra_bytes(&[3, 3, 0]);
    duplicate_zip64.extend_from_slice(&zip64_extended_info_extra_bytes(&[3, 3, 0]));

    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")
            .with_central_extra(duplicate_zip64)],
    );

    let mut bytes = fs::read(&artifact).unwrap();
    let eocd_offset = find_zip_eocd(&bytes).unwrap();
    let central_directory_offset = zip_u32_at(&bytes, eocd_offset + 16).unwrap() as usize;
    set_zip_u32_at(&mut bytes, central_directory_offset + 20, u32::MAX);
    set_zip_u32_at(&mut bytes, central_directory_offset + 24, u32::MAX);
    set_zip_u32_at(&mut bytes, central_directory_offset + 42, u32::MAX);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_listing_failed"
            && issue["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("duplicated"))
    }));
}

#[test]
fn package_audit_fails_closed_on_duplicate_unhandled_zip_extra_fields() {
    let dir = tempdir().unwrap();
    let central_artifact = dir.path().join("central-extra-duplicate.zip");
    let local_artifact = dir.path().join("local-extra-duplicate.zip");
    let mut duplicate_extra = zip_extra_field_bytes(0x5455, &[1, 0, 0, 0, 0]);
    duplicate_extra.extend_from_slice(&zip_extra_field_bytes(0x5455, &[1, 0, 0, 0, 0]));

    write_test_zip(
        &central_artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")
            .with_central_extra(duplicate_extra.clone())],
    );
    write_test_zip(
        &local_artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/NOTICE", b"MIT").with_local_extra(duplicate_extra)],
    );

    let report = package_audit(dir.path(), &[central_artifact, local_artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    let duplicate_extra_issues = issues
        .iter()
        .filter(|issue| {
            issue["code"] == "archive_member_listing_failed"
                && issue["detail"].as_str().is_some_and(|detail| {
                    detail.contains("0x5455") && detail.contains("duplicated")
                })
        })
        .count();
    assert_eq!(
        duplicate_extra_issues, 2,
        "expected both central and local duplicate extra fields to fail closed: {report:#}"
    );
}

#[test]
fn package_audit_fails_closed_on_zip64_surplus_sentinel_fields() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/LICENSE";
    let zip64_with_surplus = zip64_extended_info_extra_bytes(&[3, 3, 0, 42]);

    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(member, b"MIT").with_central_extra(zip64_with_surplus)],
    );

    let mut bytes = fs::read(&artifact).unwrap();
    let eocd_offset = find_zip_eocd(&bytes).unwrap();
    let central_directory_offset = zip_u32_at(&bytes, eocd_offset + 16).unwrap() as usize;
    set_zip_u32_at(&mut bytes, central_directory_offset + 20, u32::MAX);
    set_zip_u32_at(&mut bytes, central_directory_offset + 24, u32::MAX);
    set_zip_u32_at(&mut bytes, central_directory_offset + 42, u32::MAX);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_listing_failed"
            && issue["detail"].as_str().is_some_and(|detail| {
                detail.contains("zip64") && detail.contains("unclaimed bytes")
            })
    }));
}

#[test]
fn package_audit_fails_closed_on_zip64_directory_offset_sentinel() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
    );

    let (mut bytes, eocd, _) = read_zip_with_offsets(&artifact);
    set_zip_u32_at(&mut bytes, eocd + 16, u32::MAX);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    assert_audit_rejected(&report);
    assert_listing_failure(report["issues"].as_array().unwrap(), "zip64");
}

#[test]
fn package_audit_fails_closed_on_zip64_locator_offset_overflow() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
    );

    let mut bytes = fs::read(&artifact).unwrap();
    append_zip64_eocd(&mut bytes);
    let eocd_offset = find_zip_eocd(&bytes).unwrap();
    let locator_offset = eocd_offset - 20;
    set_zip_u16_at(&mut bytes, eocd_offset + 8, u16::MAX);
    set_zip_u16_at(&mut bytes, eocd_offset + 10, u16::MAX);
    set_zip_u32_at(&mut bytes, eocd_offset + 12, u32::MAX);
    set_zip_u32_at(&mut bytes, eocd_offset + 16, u32::MAX);
    set_zip_u64_at(&mut bytes, locator_offset + 8, u64::MAX);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_listing_failed"
            && issue["detail"].as_str().is_some_and(|detail| {
                detail.contains("zip64 end-of-central-directory")
                    && (detail.contains("overflowed")
                        || detail.contains("too large")
                        || detail.contains("outside"))
            })
    }));
}

#[test]
fn package_audit_reads_zip64_end_of_central_directory() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
    );

    let mut bytes = fs::read(&artifact).unwrap();
    append_zip64_eocd(&mut bytes);
    let eocd_offset = find_zip_eocd(&bytes).unwrap();
    set_zip_u16_at(&mut bytes, eocd_offset + 8, u16::MAX);
    set_zip_u16_at(&mut bytes, eocd_offset + 10, u16::MAX);
    set_zip_u32_at(&mut bytes, eocd_offset + 12, u32::MAX);
    set_zip_u32_at(&mut bytes, eocd_offset + 16, u32::MAX);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["archives_checked"], 1);
    assert!(
        !issues
            .iter()
            .any(|issue| issue["code"] == "archive_member_listing_failed"),
        "{report:#}"
    );
}

#[test]
fn package_audit_fails_closed_on_encrypted_zip_entry_flag() {
    let member = "tokenzero-v0.1.1/LICENSE";
    let (report, issues) =
        run_zip_audit(&[ZipTestEntry::file(member, b"MIT").with_flags(ZIP_FLAG_ENCRYPTED)]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[("code", "zip_entry_uninspectable"), ("member", member)],
    );
    assert!(issues.iter().any(|issue| {
        issue["flags"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|f| f == "encrypted")
    }));
}

#[test]
fn package_audit_fails_closed_on_unsupported_zip_executable_payload() {
    let member = "tokenzero-v0.1.1/bin/tokenzero";
    let (report, issues) = run_zip_audit(&[ZipTestEntry::file(member, b"opaque").with_method(12)]);
    assert_audit_rejected(&report);
    let issue = issues
        .iter()
        .find(|i| i["code"] == "zip_regular_file_uninspectable" && i["member"] == member)
        .unwrap();
    assert_eq!(issue["compression_method"], 12);
}

#[test]
fn package_audit_fails_closed_on_unsupported_zip_native_addon_payload() {
    let member = "tokenzero-v0.1.1/node_modules/addon/build/Release/addon.node";
    let (report, issues) = run_zip_audit(&[ZipTestEntry::file(member, b"opaque").with_method(12)]);
    assert_audit_rejected(&report);
    let issue = issues
        .iter()
        .find(|i| i["code"] == "zip_regular_file_uninspectable" && i["member"] == member)
        .unwrap();
    assert_eq!(issue["compression_method"], 12);
}

#[test]
fn package_audit_fails_closed_on_zip_aggregate_payload_budget() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let first = "tokenzero-v0.1.1/share/nested-one.zip";
    let second = "tokenzero-v0.1.1/share/nested-two.zip";
    let payload = deflate_bytes(b"tiny");
    write_test_zip(
        &artifact,
        &[
            ZipTestEntry::file(first, &payload).with_method(8),
            ZipTestEntry::file(second, &payload).with_method(8),
        ],
    );

    let mut bytes = fs::read(&artifact).unwrap();
    let advertised_size = u32::try_from(MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES / 2 + 1).unwrap();
    set_test_zip_entry_uncompressed_sizes(&mut bytes, &[advertised_size, advertised_size]);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_total_payload_size_exceeded"
            && issue["member"] == second
            && issue["limit"].as_u64() == Some(MAX_ZIP_TOTAL_UNCOMPRESSED_BYTES as u64)
    }));
}

#[test]
fn package_audit_fails_closed_on_oversized_top_level_archives() {
    let dir = tempdir().unwrap();
    let artifacts: Vec<PathBuf> = [
        "release.zip",
        "release.tar",
        "release.tar.gz",
        "release.tgz",
        "tokenzero.crate",
    ]
    .iter()
    .map(|name| dir.path().join(name))
    .collect();

    for artifact in &artifacts {
        fs::File::create(artifact)
            .unwrap()
            .set_len(MAX_TOP_LEVEL_ARCHIVE_BYTES + 1)
            .unwrap();
    }

    let report = package_audit(dir.path(), &artifacts);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(
        !issues
            .iter()
            .any(|issue| issue["code"] == "archive_member_listing_failed"),
        "{report:#}"
    );
    for artifact in &artifacts {
        let artifact_path = artifact.display().to_string();
        assert!(
            issues.iter().any(|issue| {
                issue["code"] == "archive_file_too_large"
                    && issue["path"] == artifact_path
                    && issue["limit"].as_u64() == Some(MAX_TOP_LEVEL_ARCHIVE_BYTES)
            }),
            "missing archive_file_too_large for {artifact_path}: {report:#}"
        );
    }
}

#[test]
fn package_audit_fails_closed_on_zip_directory_payload() {
    let member = "tokenzero-v0.1.1/docs/";
    let (report, issues) = run_zip_audit(&[ZipTestEntry::file(member, b"hidden")]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "zip_directory_payload_present"),
            ("member", member),
        ],
    );
}

#[test]
fn package_audit_accepts_deflated_zip_executable_payload() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/bin/tokenzero";
    let payload = deflate_bytes(b"#!/bin/sh\nexec tokenzero-runtime \"$@\"\n");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(member, &payload).with_method(8)],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["archives_checked"], 1);
    assert!(
        !issues
            .iter()
            .any(|issue| issue["code"] == "zip_regular_file_uninspectable"),
        "{report:#}"
    );
}

#[test]
fn package_audit_fails_closed_on_zip_entry_comment() {
    let member = "tokenzero-v0.1.1/LICENSE";
    let (report, issues) =
        run_zip_audit(&[ZipTestEntry::file(member, b"MIT").with_comment(b"/tmp/example/release")]);
    assert_audit_rejected(&report);
    let issue = issues
        .iter()
        .find(|i| i["code"] == "zip_entry_comment_present" && i["member"] == member)
        .unwrap();
    assert_eq!(issue["comment_bytes"], 20);
}

#[test]
fn package_audit_fails_closed_on_zip_archive_comment() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
    );

    let archive_comment = b"/tmp/example/release";
    let mut bytes = fs::read(&artifact).unwrap();
    let eocd_offset = find_zip_eocd(&bytes).unwrap();
    set_zip_u16_at(&mut bytes, eocd_offset + 20, archive_comment.len() as u16);
    bytes.extend_from_slice(archive_comment);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_archive_comment_present"
            && issue["comment_bytes"] == archive_comment.len()
    }));
}

#[test]
fn package_audit_fails_closed_on_zip_extra_fields() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let central_member = "tokenzero-v0.1.1/LICENSE";
    let local_member = "tokenzero-v0.1.1/NOTICE";
    let central_extra = zip_extra_field_bytes(0x5455, b"\x01\x00\x00\x00\x00");
    let local_extra = zip_extra_field_bytes(0x7875, b"\x01\x01\xed\x01\x14");

    write_test_zip(
        &artifact,
        &[
            ZipTestEntry::file(central_member, b"MIT").with_central_extra(central_extra),
            ZipTestEntry::file(local_member, b"notice").with_local_extra(local_extra),
        ],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_extra_field_present"
            && issue["member"] == central_member
            && issue["field_location"] == "central"
            && issue["tag"] == "0x5455"
            && issue["size"] == 5
    }));
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_extra_field_present"
            && issue["member"] == local_member
            && issue["field_location"] == "local"
            && issue["tag"] == "0x7875"
            && issue["size"] == 5
    }));
}

#[test]
fn package_audit_fails_closed_on_unneeded_zip64_extra_field() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/LICENSE";
    let zip64_without_sentinel = zip64_extended_info_extra_bytes(&[3, 3, 0]);

    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(member, b"MIT").with_central_extra(zip64_without_sentinel)],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_extra_field_present"
            && issue["member"] == member
            && issue["field_location"] == "central"
            && issue["tag"] == "0x0001"
    }));
}

#[test]
fn package_audit_fails_closed_on_zip_leading_unclaimed_bytes() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/LICENSE";
    write_test_zip(&artifact, &[ZipTestEntry::file(member, b"MIT")]);

    let preamble = b"#!/bin/sh\nexec /tmp/hidden\n";
    let (mut bytes, eocd, cd) = read_zip_with_offsets(&artifact);
    bytes.splice(0..0, preamble.iter().copied());
    let new_cd = cd + preamble.len();
    let new_eocd = eocd + preamble.len();
    set_zip_u32_at(&mut bytes, new_eocd + 16, new_cd as u32);
    set_zip_u32_at(&mut bytes, new_cd + 42, preamble.len() as u32);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_unclaimed_local_bytes"
            && issue["start"] == 0
            && issue["end"] == preamble.len()
    }));
}

#[test]
fn package_audit_fails_closed_on_zip_gap_before_central_directory() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
    );

    let gap = b"raw_traces/local_only";
    let (mut bytes, eocd, cd) = read_zip_with_offsets(&artifact);
    bytes.splice(cd..cd, gap.iter().copied());
    let new_cd = cd + gap.len();
    let new_eocd = eocd + gap.len();
    set_zip_u32_at(&mut bytes, new_eocd + 16, new_cd as u32);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "zip_unclaimed_local_bytes"
            && issue["start"] == cd
            && issue["end"] == new_cd
    }));
}

#[test]
fn package_audit_fails_closed_on_zip_local_header_method_mismatch() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/LICENSE";
    write_test_zip(&artifact, &[ZipTestEntry::file(member, b"MIT")]);

    let mut bytes = fs::read(&artifact).unwrap();
    set_zip_u16_at(&mut bytes, 8, 8);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[
            ("code", "zip_local_header_metadata_mismatch"),
            ("member", member),
            ("field", "compression_method"),
        ],
    );
    let issue = issues
        .iter()
        .find(|i| i["code"] == "zip_local_header_metadata_mismatch" && i["member"] == member)
        .unwrap();
    assert_eq!(issue["central"], 0);
    assert_eq!(issue["local"], 8);
}

#[test]
fn package_audit_fails_closed_on_zip_central_directory_count_mismatch() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    write_test_zip(
        &artifact,
        &[
            ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT"),
            ZipTestEntry::file("tokenzero-v0.1.1/.tokenzero/config.json", b"{}"),
        ],
    );

    let mut bytes = fs::read(&artifact).unwrap();
    let eocd_offset = find_zip_eocd(&bytes).unwrap();
    set_zip_u16_at(&mut bytes, eocd_offset + 8, 1);
    set_zip_u16_at(&mut bytes, eocd_offset + 10, 1);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_listing_failed"
            && issue["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("unparsed bytes"))
    }));
}

#[test]
fn package_audit_fails_closed_on_duplicate_zip_member_names() {
    let member = "tokenzero-v0.1.1/LICENSE";
    let (report, issues) = run_zip_audit(&[
        ZipTestEntry::file(member, b"first"),
        ZipTestEntry::file(member, b"second"),
    ]);
    assert_audit_rejected(&report);
    assert_issue(
        &issues,
        &[("code", "zip_duplicate_member_name"), ("member", member)],
    );
}

#[test]
fn package_audit_malformed_zip_corpus_has_stable_listing_failures() {
    struct MalformedZipCase {
        name: &'static str,
        build: fn(&Path),
        detail_contains: &'static str,
    }

    fn write_missing_eocd(path: &Path) {
        write_test_zip(
            path,
            &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
        );
        let mut bytes = fs::read(path).unwrap();
        let eocd_offset = find_zip_eocd(&bytes).unwrap();
        bytes.truncate(eocd_offset);
        fs::write(path, bytes).unwrap();
    }

    fn write_invalid_central_signature(path: &Path) {
        write_test_zip(
            path,
            &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
        );
        let mut bytes = fs::read(path).unwrap();
        let eocd_offset = find_zip_eocd(&bytes).unwrap();
        let central_directory_offset = zip_u32_at(&bytes, eocd_offset + 16).unwrap() as usize;
        set_zip_u32_at(&mut bytes, central_directory_offset, 0);
        fs::write(path, bytes).unwrap();
    }

    fn write_invalid_local_signature(path: &Path) {
        write_test_zip(
            path,
            &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")],
        );
        let mut bytes = fs::read(path).unwrap();
        set_zip_u32_at(&mut bytes, 0, 0);
        fs::write(path, bytes).unwrap();
    }

    fn write_truncated_extra_field(path: &Path) {
        write_test_zip(
            path,
            &[ZipTestEntry::file("tokenzero-v0.1.1/LICENSE", b"MIT")
                .with_central_extra(vec![0x55, 0x54, 0x01])],
        );
    }

    let dir = tempdir().unwrap();
    let cases = [
        MalformedZipCase {
            name: "missing-eocd.zip",
            build: write_missing_eocd,
            detail_contains: "end-of-central-directory record was not found",
        },
        MalformedZipCase {
            name: "invalid-central-signature.zip",
            build: write_invalid_central_signature,
            detail_contains: "central directory entry has an invalid signature",
        },
        MalformedZipCase {
            name: "invalid-local-signature.zip",
            build: write_invalid_local_signature,
            detail_contains: "local header has an invalid signature",
        },
        MalformedZipCase {
            name: "truncated-extra-field.zip",
            build: write_truncated_extra_field,
            detail_contains: "extra field header is truncated",
        },
    ];

    for case in cases {
        let artifact = dir.path().join(case.name);
        (case.build)(&artifact);

        let report = package_audit(dir.path(), &[artifact]);
        let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

        assert_eq!(report["ok"], false, "case {}: {report:#}", case.name);
        assert!(
            issues.iter().any(|issue| {
                issue["code"] == "archive_member_listing_failed"
                    && issue["detail"]
                        .as_str()
                        .is_some_and(|detail| detail.contains(case.detail_contains))
            }),
            "case {} missing stable listing failure {:#}",
            case.name,
            report
        );
    }
}

#[test]
fn package_audit_rejects_zip_archive_external_runtime_payload() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/bin/tokenzero.cmd";
    let payload = b"@echo off\r\nuv run tokenzero %*\r\n";
    let compressed_payload = deflate_bytes(payload);

    write_test_zip(
        &artifact,
        &[ZipTestEntry::file(member, &compressed_payload).with_method(8)],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues: Vec<serde_json::Value> = report["issues"].as_array().unwrap().clone();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "external_runtime_dependency" && issue["member"] == member
    }));
}
