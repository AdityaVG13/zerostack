use super::fixtures::*;
use super::*;

#[test]
fn package_audit_rejects_external_runtime() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("package.txt");
    fs::write(&artifact, format!("{} tokenzero", ["uv", " run"].concat())).unwrap();
    let report = package_audit(dir.path(), &[artifact]);
    assert_eq!(report["ok"], false);
}

#[test]
fn package_audit_rejects_dev_target_launcher() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join(".tokenzero/bin/tokenzero.cmd");
    fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    fs::write(
        &artifact,
        "@echo off\r\n\"C:\\repo\\target\\release\\tokenzero.exe\" %*\r\n",
    )
    .unwrap();

    let report = package_audit(dir.path(), &[artifact]);

    assert_eq!(report["ok"], false);
    assert!(
        report["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["code"] == "dev_runtime_launcher")
    );
}

#[test]
fn package_audit_rejects_private_archive_members() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    write_test_tar(
        &artifact,
        &[
            "tokenzero-v0.1.1/._LICENSE",
            "tokenzero-v0.1.1/.tokenzero/config.json",
            "tokenzero-v0.1.1/.env",
            "tokenzero-v0.1.1/src/lib.rs",
        ],
    );

    let report = package_audit(dir.path(), &[artifact]);
    let issues = report["issues"].as_array().unwrap();

    assert_eq!(report["ok"], false);
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "appledouble_metadata")
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "private_tool_state_member")
    );
    assert!(
        issues
            .iter()
            .any(|issue| issue["code"] == "sensitive_member_name")
    );
}

#[test]
fn package_audit_rejects_local_generated_archive_members() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let members = [
        "tokenzero-v0.1.1/crash.dmp",
        "tokenzero-v0.1.1/prompt-transcript.md",
        "tokenzero-v0.1.1/chat-export.json",
        "tokenzero-v0.1.1/debug-report.txt",
        "tokenzero-v0.1.1/screenshot.png",
    ];
    write_test_tar(&artifact, &members);

    let report = package_audit(dir.path(), &[artifact]);
    let issues = report["issues"].as_array().unwrap();

    assert_eq!(report["ok"], false);
    for member in members {
        assert!(
            issues.iter().any(|issue| {
                issue["code"] == "local_generated_member" && issue["member"] == member
            }),
            "missing local_generated_member issue for {member}"
        );
    }
}

#[test]
fn package_audit_fails_closed_on_archive_member_control_characters() {
    let dir = tempdir().unwrap();
    let tar_artifact = dir.path().join("release.tar");
    let zip_artifact = dir.path().join("release.zip");
    let tar_member = "tokenzero-v0.1.1/bin/tokenzero\nshim";
    let zip_member = "tokenzero-v0.1.1/bin/tokenzero\0shim";

    write_test_tar(&tar_artifact, &[tar_member]);
    write_test_zip(&zip_artifact, &[ZipTestEntry::file(zip_member, b"")]);

    let report = package_audit(dir.path(), &[tar_artifact, zip_artifact]);
    let issues = report["issues"].as_array().unwrap();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_name_uninspectable"
            && issue["member"] == tar_member
            && issue["reason"] == "control_character"
    }));
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_name_uninspectable"
            && issue["member"] == zip_member
            && issue["reason"] == "nul_byte"
    }));
}

#[test]
fn package_audit_fails_closed_on_non_utf8_tar_member_name() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");

    let mut header = test_tar_header("tokenzero-v0.1.1/LICENSE", b'0', 0, None);
    header[20] = 0xff;
    write_test_tar_checksum(&mut header);
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(&[0u8; 1024]);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues = report["issues"].as_array().unwrap();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_name_uninspectable" && issue["reason"] == "invalid_utf8"
    }));
}

#[test]
fn package_audit_fails_closed_on_non_utf8_tar_link_target() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.tar");
    let member = "tokenzero-v0.1.1/bin/tokenzero";

    let mut header = test_tar_header(member, b'2', 0, Some("bin/tokenzero"));
    header[160] = 0xff;
    write_test_tar_checksum(&mut header);
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(&[0u8; 1024]);
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues = report["issues"].as_array().unwrap();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_link_target_uninspectable"
            && issue["member"] == member
            && issue["reason"] == "invalid_utf8"
    }));
}

#[test]
fn package_audit_fails_closed_on_non_utf8_zip_member_name() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/bin/tokenzero";
    write_test_zip(&artifact, &[ZipTestEntry::file(member, b"")]);

    let mut bytes = fs::read(&artifact).unwrap();
    let invalid_name_index = member.find("bin").unwrap();
    bytes[30 + invalid_name_index] = 0xff;
    let eocd_offset = find_zip_eocd(&bytes).unwrap();
    let central_directory_offset = zip_u32_at(&bytes, eocd_offset + 16).unwrap() as usize;
    bytes[central_directory_offset + 46 + invalid_name_index] = 0xff;
    fs::write(&artifact, bytes).unwrap();

    let report = package_audit(dir.path(), &[artifact]);
    let issues = report["issues"].as_array().unwrap();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_member_name_uninspectable" && issue["reason"] == "invalid_utf8"
    }));
}

#[test]
fn package_audit_fails_closed_on_non_utf8_zip_symlink_target() {
    let dir = tempdir().unwrap();
    let artifact = dir.path().join("release.zip");
    let member = "tokenzero-v0.1.1/bin/tokenzero";
    let target = b"bin/\xfftokenzero";
    write_test_zip(&artifact, &[ZipTestEntry::symlink(member, target)]);

    let report = package_audit(dir.path(), &[artifact]);
    let issues = report["issues"].as_array().unwrap();

    assert_eq!(report["ok"], false);
    assert!(issues.iter().any(|issue| {
        issue["code"] == "archive_link_target_uninspectable"
            && issue["member"] == member
            && issue["reason"] == "invalid_utf8"
    }));
}
