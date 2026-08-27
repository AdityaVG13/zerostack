
use super::*;

#[cfg(unix)]
#[test]
fn file_name_to_str_rejects_non_utf8_names() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let name = OsString::from_vec(vec![0xff]);
    let err = file_name_to_str(&name, "blob scan")
        .unwrap_err()
        .to_string();
    assert!(err.contains("blob scan: non-UTF-8 file name rejected"));
}
