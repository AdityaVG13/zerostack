use super::super::*;
use std::io::Write;
use std::path::Path;

pub(crate) struct TarTestEntry<'a> {
    name: &'a str,
    typeflag: u8,
    data: &'a [u8],
    link_target: Option<&'a str>,
}

impl<'a> TarTestEntry<'a> {
    pub(crate) fn new(name: &'a str, typeflag: u8, data: &'a [u8]) -> Self {
        Self {
            name,
            typeflag,
            data,
            link_target: None,
        }
    }

    pub(crate) fn with_link_target(mut self, link_target: &'a str) -> Self {
        self.link_target = Some(link_target);
        self
    }
}

pub(crate) fn write_test_tar(path: &Path, names: &[&str]) {
    let entries: Vec<_> = names
        .iter()
        .map(|name| TarTestEntry::new(name, b'0', b""))
        .collect();
    write_test_tar_entries(path, &entries);
}

pub(crate) fn write_test_tar_entries(path: &Path, entries: &[TarTestEntry<'_>]) {
    let mut file = fs::File::create(path).unwrap();
    for entry in entries {
        file.write_all(&test_tar_entry_bytes_with_type(
            entry.name,
            entry.typeflag,
            entry.data,
            entry.link_target,
        ))
        .unwrap();
    }
    file.write_all(&[0u8; 1024]).unwrap();
}

pub(crate) fn test_tar_entry_bytes(name: &str, data: &[u8]) -> Vec<u8> {
    test_tar_entry_bytes_with_type(name, b'0', data, None)
}

pub(crate) fn test_tar_entry_bytes_with_type(
    name: &str,
    typeflag: u8,
    data: &[u8],
    link_target: Option<&str>,
) -> Vec<u8> {
    let header = test_tar_header(name, typeflag, data.len() as u64, link_target);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(data);
    let padding = (512 - (data.len() % 512)) % 512;
    if padding > 0 {
        bytes.extend_from_slice(&vec![0u8; padding]);
    }
    bytes
}

pub(crate) fn test_tar_header(
    name: &str,
    typeflag: u8,
    size: u64,
    link_target: Option<&str>,
) -> [u8; 512] {
    let mut header = [0u8; 512];
    let name_bytes = name.as_bytes();
    assert!(name_bytes.len() < 100);
    header[..name_bytes.len()].copy_from_slice(name_bytes);
    if let Some(link_target) = link_target {
        let link_bytes = link_target.as_bytes();
        assert!(link_bytes.len() < 100);
        header[157..157 + link_bytes.len()].copy_from_slice(link_bytes);
    }
    write_tar_octal(&mut header[100..108], 0o644);
    write_tar_octal(&mut header[108..116], 0);
    write_tar_octal(&mut header[116..124], 0);
    write_tar_octal(&mut header[124..136], size);
    write_tar_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = typeflag;
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    write_test_tar_checksum(&mut header);
    header
}

pub(crate) fn write_test_tar_checksum(header: &mut [u8; 512]) {
    write_test_tar_checksum_bytes(header);
}

pub(crate) fn write_test_tar_checksum_bytes(header: &mut [u8]) {
    assert_eq!(header.len(), 512);
    header[148..156].fill(b' ');
    let checksum: u32 = header.iter().map(|byte| *byte as u32).sum();
    let checksum_text = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(checksum_text.as_bytes());
}

pub(crate) fn pax_record(key: &str, value: &str) -> Vec<u8> {
    let body = format!("{key}={value}\n");
    let mut length = body.len() + 2;
    loop {
        let record = format!("{length} {body}");
        if record.len() == length {
            return record.into_bytes();
        }
        length = record.len();
    }
}

pub(crate) fn write_tar_octal(field: &mut [u8], value: u64) {
    field.fill(0);
    let text = format!("{value:0width$o}", width = field.len() - 1);
    field[..text.len()].copy_from_slice(text.as_bytes());
}

pub(crate) fn write_tar_base256(field: &mut [u8], value: u128) {
    field.fill(0);
    let mut remaining = value;
    for byte in field.iter_mut().rev() {
        *byte = remaining as u8;
        remaining >>= 8;
    }
    assert_eq!(remaining, 0, "test tar base-256 value does not fit");
    field[0] |= 0x80;
}
