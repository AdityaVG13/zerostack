use super::super::*;
use std::io::Write;
use std::path::Path;

pub(crate) fn find_zip_eocd(bytes: &[u8]) -> Option<usize> {
    zip_eocd_candidates(bytes).into_iter().next()
}
pub(crate) struct ZipTestEntry<'a> {
    name: &'a str,
    data: &'a [u8],
    method: u16,
    version_made_by: u16,
    external_attrs: u32,
    local_name: Option<&'a str>,
    flags: u16,
    data_descriptor: bool,
    data_descriptor_signature: bool,
    force_zip64: bool,
    local_extra: Vec<u8>,
    central_extra: Vec<u8>,
    comment: Vec<u8>,
}

impl<'a> ZipTestEntry<'a> {
    pub(crate) fn file(name: &'a str, data: &'a [u8]) -> Self {
        Self {
            name,
            data,
            method: 0,
            version_made_by: (3 << 8) | 20,
            external_attrs: 0o100644 << 16,
            local_name: None,
            flags: 0,
            data_descriptor: false,
            data_descriptor_signature: true,
            force_zip64: false,
            local_extra: Vec::new(),
            central_extra: Vec::new(),
            comment: Vec::new(),
        }
    }

    pub(crate) fn symlink(name: &'a str, target: &'a [u8]) -> Self {
        Self {
            name,
            data: target,
            method: 0,
            version_made_by: (3 << 8) | 20,
            external_attrs: 0o120777 << 16,
            local_name: None,
            flags: 0,
            data_descriptor: false,
            data_descriptor_signature: true,
            force_zip64: false,
            local_extra: Vec::new(),
            central_extra: Vec::new(),
            comment: Vec::new(),
        }
    }

    pub(crate) fn with_method(mut self, method: u16) -> Self {
        self.method = method;
        self
    }

    pub(crate) fn with_flags(mut self, flags: u16) -> Self {
        self.flags |= flags;
        self
    }

    pub(crate) fn with_local_name(mut self, local_name: &'a str) -> Self {
        self.local_name = Some(local_name);
        self
    }

    pub(crate) fn with_data_descriptor(mut self) -> Self {
        self.flags |= ZIP_FLAG_DATA_DESCRIPTOR;
        self.data_descriptor = true;
        self
    }

    pub(crate) fn with_unsigned_data_descriptor(mut self) -> Self {
        self.flags |= ZIP_FLAG_DATA_DESCRIPTOR;
        self.data_descriptor = true;
        self.data_descriptor_signature = false;
        self
    }

    pub(crate) fn with_zip64_extra_fields(mut self) -> Self {
        self.force_zip64 = true;
        self
    }

    pub(crate) fn with_central_unicode_path(mut self, unicode_name: &str) -> Self {
        self.central_extra = zip_unicode_path_extra_bytes(self.name.as_bytes(), unicode_name);
        self
    }

    pub(crate) fn with_local_unicode_path(mut self, unicode_name: &str) -> Self {
        self.local_extra = zip_unicode_path_extra_bytes(self.local_name().as_bytes(), unicode_name);
        self
    }

    pub(crate) fn with_central_extra(mut self, extra: Vec<u8>) -> Self {
        self.central_extra = extra;
        self
    }

    pub(crate) fn with_local_extra(mut self, extra: Vec<u8>) -> Self {
        self.local_extra = extra;
        self
    }

    pub(crate) fn with_comment(mut self, comment: &[u8]) -> Self {
        self.comment = comment.to_vec();
        self
    }

    pub(crate) fn local_name(&self) -> &'a str {
        self.local_name.unwrap_or(self.name)
    }
}

pub(crate) fn write_test_zip(path: &Path, entries: &[ZipTestEntry<'_>]) {
    assert!(entries.len() <= u16::MAX as usize);
    let mut file_bytes = Vec::new();
    let mut central_directory = Vec::new();

    for entry in entries {
        let local_header_offset = file_bytes.len() as u32;
        write_zip_local_header(&mut file_bytes, entry);
        write_zip_central_header(&mut central_directory, entry, local_header_offset);
    }

    let central_directory_offset = file_bytes.len() as u32;
    let central_directory_size = central_directory.len() as u32;
    file_bytes.extend_from_slice(&central_directory);
    push_zip_u32(&mut file_bytes, 0x0605_4b50);
    push_zip_u16(&mut file_bytes, 0);
    push_zip_u16(&mut file_bytes, 0);
    push_zip_u16(&mut file_bytes, entries.len() as u16);
    push_zip_u16(&mut file_bytes, entries.len() as u16);
    push_zip_u32(&mut file_bytes, central_directory_size);
    push_zip_u32(&mut file_bytes, central_directory_offset);
    push_zip_u16(&mut file_bytes, 0);

    fs::write(path, file_bytes).unwrap();
}

pub(crate) fn append_zip64_eocd(bytes: &mut Vec<u8>) {
    let eocd_offset = find_zip_eocd(bytes).unwrap();
    let entry_count = zip_u16_at(bytes, eocd_offset + 10).unwrap() as u64;
    let central_directory_size = zip_u32_at(bytes, eocd_offset + 12).unwrap() as u64;
    let central_directory_offset = zip_u32_at(bytes, eocd_offset + 16).unwrap() as u64;
    let eocd = bytes.split_off(eocd_offset);
    let zip64_eocd_offset = bytes.len() as u64;

    push_zip_u32(bytes, ZIP64_EOCD_RECORD_SIGNATURE);
    push_zip_u64(bytes, 44);
    push_zip_u16(bytes, 45);
    push_zip_u16(bytes, 45);
    push_zip_u32(bytes, 0);
    push_zip_u32(bytes, 0);
    push_zip_u64(bytes, entry_count);
    push_zip_u64(bytes, entry_count);
    push_zip_u64(bytes, central_directory_size);
    push_zip_u64(bytes, central_directory_offset);

    push_zip_u32(bytes, ZIP64_EOCD_LOCATOR_SIGNATURE);
    push_zip_u32(bytes, 0);
    push_zip_u64(bytes, zip64_eocd_offset);
    push_zip_u32(bytes, 1);

    bytes.extend_from_slice(&eocd);
}

pub(crate) fn write_zip_local_header(out: &mut Vec<u8>, entry: &ZipTestEntry<'_>) {
    let local_extra = zip_local_extra(entry);
    let compressed_size = zip_test_entry_compressed_size(entry);
    let uncompressed_size = zip_test_entry_uncompressed_size(entry);
    push_zip_u32(out, 0x0403_4b50);
    push_zip_u16(out, 20);
    push_zip_u16(out, entry.flags);
    push_zip_u16(out, entry.method);
    push_zip_u16(out, 0);
    push_zip_u16(out, 0);
    push_zip_u32(out, zip_test_entry_crc32(entry));
    let local_compressed_size = if entry.data_descriptor {
        0
    } else if entry.force_zip64 {
        u32::MAX
    } else {
        zip_test_u32(compressed_size, "compressed size")
    };
    let local_uncompressed_size = if entry.data_descriptor {
        0
    } else if entry.force_zip64 {
        u32::MAX
    } else {
        zip_test_u32(uncompressed_size, "uncompressed size")
    };
    push_zip_u32(out, local_compressed_size);
    push_zip_u32(out, local_uncompressed_size);
    push_zip_u16(out, entry.local_name().len() as u16);
    push_zip_u16(out, local_extra.len() as u16);
    out.extend_from_slice(entry.local_name().as_bytes());
    out.extend_from_slice(&local_extra);
    out.extend_from_slice(entry.data);
    if entry.data_descriptor {
        if entry.data_descriptor_signature {
            push_zip_u32(out, ZIP_DATA_DESCRIPTOR_SIGNATURE);
        }
        push_zip_u32(out, zip_test_entry_crc32(entry));
        if entry.force_zip64 {
            push_zip_u64(out, compressed_size as u64);
            push_zip_u64(out, uncompressed_size as u64);
        } else {
            push_zip_u32(out, zip_test_u32(compressed_size, "compressed size"));
            push_zip_u32(out, zip_test_u32(uncompressed_size, "uncompressed size"));
        }
    }
}

pub(crate) fn write_zip_central_header(
    out: &mut Vec<u8>,
    entry: &ZipTestEntry<'_>,
    local_header_offset: u32,
) {
    let central_extra = zip_central_extra(entry, local_header_offset);
    let compressed_size = zip_test_entry_compressed_size(entry);
    let uncompressed_size = zip_test_entry_uncompressed_size(entry);
    push_zip_u32(out, 0x0201_4b50);
    push_zip_u16(out, entry.version_made_by);
    push_zip_u16(out, 20);
    push_zip_u16(out, entry.flags);
    push_zip_u16(out, entry.method);
    push_zip_u16(out, 0);
    push_zip_u16(out, 0);
    push_zip_u32(out, zip_test_entry_crc32(entry));
    if entry.force_zip64 {
        push_zip_u32(out, u32::MAX);
        push_zip_u32(out, u32::MAX);
    } else {
        push_zip_u32(out, zip_test_u32(compressed_size, "compressed size"));
        push_zip_u32(out, zip_test_u32(uncompressed_size, "uncompressed size"));
    }
    push_zip_u16(out, entry.name.len() as u16);
    push_zip_u16(out, central_extra.len() as u16);
    push_zip_u16(out, entry.comment.len() as u16);
    push_zip_u16(out, 0);
    push_zip_u16(out, 0);
    push_zip_u32(out, entry.external_attrs);
    if entry.force_zip64 {
        push_zip_u32(out, u32::MAX);
    } else {
        push_zip_u32(out, local_header_offset);
    }
    out.extend_from_slice(entry.name.as_bytes());
    out.extend_from_slice(&central_extra);
    out.extend_from_slice(&entry.comment);
}

pub(crate) fn zip_test_entry_crc32(entry: &ZipTestEntry<'_>) -> u32 {
    if entry.method == 8
        && let Ok(decompressed) = deflate_decompress_bytes(entry.data)
    {
        return zip_crc32(&decompressed);
    }
    zip_crc32(entry.data)
}

pub(crate) fn zip_test_entry_compressed_size(entry: &ZipTestEntry<'_>) -> usize {
    entry.data.len()
}

pub(crate) fn zip_test_entry_uncompressed_size(entry: &ZipTestEntry<'_>) -> usize {
    if entry.method == 8
        && let Ok(decompressed) = deflate_decompress_bytes(entry.data)
    {
        return decompressed.len();
    }
    entry.data.len()
}

pub(crate) fn zip_test_u32(value: usize, field: &str) -> u32 {
    u32::try_from(value).unwrap_or_else(|_| panic!("test zip {field} does not fit in u32"))
}

pub(crate) fn zip_local_extra(entry: &ZipTestEntry<'_>) -> Vec<u8> {
    let mut extra = entry.local_extra.clone();
    if entry.force_zip64 {
        extra.extend_from_slice(&zip64_extended_info_extra_bytes(&[
            zip_test_entry_uncompressed_size(entry) as u64,
            zip_test_entry_compressed_size(entry) as u64,
        ]));
    }
    extra
}

pub(crate) fn zip_central_extra(entry: &ZipTestEntry<'_>, local_header_offset: u32) -> Vec<u8> {
    let mut extra = entry.central_extra.clone();
    if entry.force_zip64 {
        extra.extend_from_slice(&zip64_extended_info_extra_bytes(&[
            zip_test_entry_uncompressed_size(entry) as u64,
            zip_test_entry_compressed_size(entry) as u64,
            local_header_offset as u64,
        ]));
    }
    extra
}

pub(crate) fn zip64_extended_info_extra_bytes(fields: &[u64]) -> Vec<u8> {
    let mut payload = Vec::new();
    for field in fields {
        push_zip_u64(&mut payload, *field);
    }
    zip_extra_field_bytes(ZIP64_EXTENDED_INFORMATION_EXTRA, &payload)
}

pub(crate) fn zip_extra_field_bytes(tag: u16, payload: &[u8]) -> Vec<u8> {
    let mut extra = Vec::new();
    push_zip_u16(&mut extra, tag);
    push_zip_u16(&mut extra, payload.len() as u16);
    extra.extend_from_slice(payload);
    extra
}

pub(crate) fn push_zip_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_zip_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn push_zip_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

pub(crate) fn set_zip_u16_at(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn set_zip_u32_at(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn set_zip_u64_at(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub(crate) fn set_test_zip_entry_uncompressed_sizes(bytes: &mut [u8], sizes: &[u32]) {
    let eocd_offset = find_zip_eocd(bytes).unwrap();
    let mut central_offset = zip_u32_at(bytes, eocd_offset + 16).unwrap() as usize;
    for size in sizes {
        let local_header_offset = zip_u32_at(bytes, central_offset + 42).unwrap() as usize;
        set_zip_u32_at(bytes, local_header_offset + 22, *size);
        set_zip_u32_at(bytes, central_offset + 24, *size);
        let name_len = zip_u16_at(bytes, central_offset + 28).unwrap() as usize;
        let extra_len = zip_u16_at(bytes, central_offset + 30).unwrap() as usize;
        let comment_len = zip_u16_at(bytes, central_offset + 32).unwrap() as usize;
        central_offset += 46 + name_len + extra_len + comment_len;
    }
}

pub(crate) fn zip_unicode_path_extra_bytes(header_name: &[u8], unicode_name: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.push(1);
    push_zip_u32(&mut payload, zip_crc32(header_name));
    payload.extend_from_slice(unicode_name.as_bytes());

    let mut extra = Vec::new();
    push_zip_u16(&mut extra, 0x7075);
    push_zip_u16(&mut extra, payload.len() as u16);
    extra.extend_from_slice(&payload);
    extra
}

pub(crate) fn gzip_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

pub(crate) fn deflate_bytes(bytes: &[u8]) -> Vec<u8> {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
