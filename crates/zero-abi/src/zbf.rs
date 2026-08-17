//! Canonical strict bounded Zero Binary Format (ZBF-1) objects and profiles.
//!
//! ZBF uses a fixed 192-byte big-endian header. The header binds every object to
//! one assembly manifest, one durable profile, one producer contract, and one
//! payload digest. Container payloads are recursively length-delimited and are
//! decoded under fixed byte, child-count, and depth bounds before allocation.

use std::{error::Error, fmt};

use crate::{ArtifactOwner, Sha256Digest, canonical_json, sha256};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const ZBF_MAGIC: [u8; 8] = *b"ZEROZBF1";
pub const ZBF_SCHEMA_MAJOR: u16 = 1;
pub const ZBF_SCHEMA_MINOR: u16 = 0;
pub const ZBF_HEADER_LEN: usize = 192;
pub const ZBF_CONTRACT_VERSION: u16 = 1;
pub const ZBF_CONTAINER_FLAG: u8 = 0x01;
pub const ZBF_MAX_OBJECT_BYTES: u64 = 16 * 1024 * 1024;
pub const ZBF_MAX_CHILDREN: u32 = 1024;
pub const ZBF_MAX_DEPTH: u16 = 16;
const ZBF_PROFILE_DOMAIN: &[u8] = b"zerostack.zbf_profile\0";

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableProfileId {
    PortableStrict,
    ApfsStrict,
    Ext4XfsStrict,
    NtfsStrict,
}

impl DurableProfileId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PortableStrict => "portable_strict",
            Self::ApfsStrict => "apfs_strict",
            Self::Ext4XfsStrict => "ext4_xfs_strict",
            Self::NtfsStrict => "ntfs_strict",
        }
    }

    const fn filesystem(self) -> &'static str {
        match self {
            Self::PortableStrict => "portable",
            Self::ApfsStrict => "apfs",
            Self::Ext4XfsStrict => "ext4_or_xfs",
            Self::NtfsStrict => "ntfs",
        }
    }
}

/// Closed, non-weakenable publication and decode profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DurableProfile {
    id: DurableProfileId,
}

impl DurableProfile {
    pub const fn new(id: DurableProfileId) -> Self {
        Self { id }
    }

    pub const fn portable_strict() -> Self {
        Self::new(DurableProfileId::PortableStrict)
    }

    pub const fn id(self) -> DurableProfileId {
        self.id
    }

    pub const fn profile_version(self) -> u16 {
        1
    }

    pub const fn max_object_bytes(self) -> u64 {
        ZBF_MAX_OBJECT_BYTES
    }

    pub const fn max_payload_bytes(self) -> u64 {
        ZBF_MAX_OBJECT_BYTES - ZBF_HEADER_LEN as u64
    }

    pub const fn max_children(self) -> u32 {
        ZBF_MAX_CHILDREN
    }

    pub const fn max_depth(self) -> u16 {
        ZBF_MAX_DEPTH
    }

    /// Canonical profile bytes. Profile names preregister platform identities;
    /// they do not claim native evidence for that filesystem.
    pub fn canonical_bytes(self) -> Vec<u8> {
        canonical_json(&json!({
            "profile_id": self.id.as_str(),
            "profile_version": self.profile_version(),
            "filesystem": self.id.filesystem(),
            "zbf_schema_major": ZBF_SCHEMA_MAJOR,
            "zbf_schema_minor": ZBF_SCHEMA_MINOR,
            "max_object_bytes": self.max_object_bytes(),
            "max_payload_bytes": self.max_payload_bytes(),
            "max_children": self.max_children(),
            "max_depth": self.max_depth(),
            "hash": "sha256",
            "atomic_publish": true,
            "sync_file_before_publish": true,
            "sync_parent_directory_after_publish": true,
            "unknown_required_versions": "reject"
        }))
        .into_bytes()
    }

    pub fn digest(self) -> Sha256Digest {
        let bytes = self.canonical_bytes();
        let mut bound = Vec::with_capacity(ZBF_PROFILE_DOMAIN.len() + bytes.len());
        bound.extend_from_slice(ZBF_PROFILE_DOMAIN);
        bound.extend_from_slice(&bytes);
        Sha256Digest::from_bytes(sha256(&bound))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u16)]
#[serde(rename_all = "snake_case")]
pub enum ZbfArtifactKind {
    AssemblyManifest = 1,
    FsPack = 2,
    GraphPack = 3,
    TokenPack = 4,
    Plan = 5,
    Receipt = 6,
    Witness = 7,
    Effect = 8,
    Snapshot = 9,
}

impl ZbfArtifactKind {
    fn from_u16(value: u16) -> Option<Self> {
        Some(match value {
            1 => Self::AssemblyManifest,
            2 => Self::FsPack,
            3 => Self::GraphPack,
            4 => Self::TokenPack,
            5 => Self::Plan,
            6 => Self::Receipt,
            7 => Self::Witness,
            8 => Self::Effect,
            9 => Self::Snapshot,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ZbfHeader {
    pub schema_major: u16,
    pub schema_minor: u16,
    pub kind: ZbfArtifactKind,
    pub owner: ArtifactOwner,
    pub flags: u8,
    pub payload_len: u64,
    pub assembly_manifest_digest: Sha256Digest,
    pub durable_profile_digest: Sha256Digest,
    pub source_root_digest: Sha256Digest,
    pub producer_contract_digest: Sha256Digest,
    pub payload_digest: Sha256Digest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZbfPayload {
    Bytes(Vec<u8>),
    Children(Vec<ZbfObject>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZbfObject {
    pub header: ZbfHeader,
    pub payload: ZbfPayload,
}

impl ZbfObject {
    #[allow(clippy::too_many_arguments)]
    pub fn new_leaf(
        kind: ZbfArtifactKind,
        owner: ArtifactOwner,
        assembly_manifest_digest: Sha256Digest,
        profile: DurableProfile,
        source_root_digest: Sha256Digest,
        producer_contract_digest: Sha256Digest,
        payload: Vec<u8>,
    ) -> Result<Self, ZbfError> {
        let payload_len = bounded_payload_len(payload.len(), profile)?;
        Ok(Self {
            header: ZbfHeader {
                schema_major: ZBF_SCHEMA_MAJOR,
                schema_minor: ZBF_SCHEMA_MINOR,
                kind,
                owner,
                flags: 0,
                payload_len,
                assembly_manifest_digest,
                durable_profile_digest: profile.digest(),
                source_root_digest,
                producer_contract_digest,
                payload_digest: Sha256Digest::from_bytes(sha256(&payload)),
            },
            payload: ZbfPayload::Bytes(payload),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_container(
        kind: ZbfArtifactKind,
        owner: ArtifactOwner,
        assembly_manifest_digest: Sha256Digest,
        profile: DurableProfile,
        source_root_digest: Sha256Digest,
        producer_contract_digest: Sha256Digest,
        children: Vec<ZbfObject>,
    ) -> Result<Self, ZbfError> {
        let payload = encode_children(&children, profile, 0)?;
        let payload_len = bounded_payload_len(payload.len(), profile)?;
        Ok(Self {
            header: ZbfHeader {
                schema_major: ZBF_SCHEMA_MAJOR,
                schema_minor: ZBF_SCHEMA_MINOR,
                kind,
                owner,
                flags: ZBF_CONTAINER_FLAG,
                payload_len,
                assembly_manifest_digest,
                durable_profile_digest: profile.digest(),
                source_root_digest,
                producer_contract_digest,
                payload_digest: Sha256Digest::from_bytes(sha256(&payload)),
            },
            payload: ZbfPayload::Children(children),
        })
    }

    pub fn to_bytes(&self, profile: DurableProfile) -> Result<Vec<u8>, ZbfError> {
        self.to_bytes_at(profile, 0)
    }

    fn to_bytes_at(&self, profile: DurableProfile, depth: u16) -> Result<Vec<u8>, ZbfError> {
        require_depth(depth, profile)?;
        validate_header_contract(&self.header, profile)?;
        let payload = match &self.payload {
            ZbfPayload::Bytes(bytes) => {
                if self.header.flags != 0 {
                    return Err(ZbfError::HeaderPayloadMismatch);
                }
                bounded_payload_len(bytes.len(), profile)?;
                bytes.clone()
            }
            ZbfPayload::Children(children) => {
                if self.header.flags != ZBF_CONTAINER_FLAG {
                    return Err(ZbfError::HeaderPayloadMismatch);
                }
                encode_children(children, profile, depth)?
            }
        };
        let payload_len = bounded_payload_len(payload.len(), profile)?;
        let payload_digest = Sha256Digest::from_bytes(sha256(&payload));
        if self.header.payload_len != payload_len || self.header.payload_digest != payload_digest {
            return Err(ZbfError::HeaderPayloadMismatch);
        }
        let total_len = ZBF_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(ZbfError::LengthOverflow)?;
        if u64::try_from(total_len).map_err(|_| ZbfError::LengthOverflow)?
            > profile.max_object_bytes()
        {
            return Err(ZbfError::ObjectTooLarge {
                actual: total_len as u64,
                maximum: profile.max_object_bytes(),
            });
        }
        let mut out = Vec::with_capacity(total_len);
        encode_header(&self.header, &mut out);
        out.extend_from_slice(&payload);
        Ok(out)
    }

    pub fn from_bytes(
        bytes: &[u8],
        expected_assembly_manifest_digest: Sha256Digest,
        profile: DurableProfile,
    ) -> Result<Self, ZbfError> {
        decode_object(bytes, expected_assembly_manifest_digest, profile, 0)
    }

    pub fn identity(&self, profile: DurableProfile) -> Result<Sha256Digest, ZbfError> {
        Ok(Sha256Digest::from_bytes(sha256(&self.to_bytes(profile)?)))
    }
}

fn bounded_payload_len(actual: usize, profile: DurableProfile) -> Result<u64, ZbfError> {
    let actual = u64::try_from(actual).map_err(|_| ZbfError::LengthOverflow)?;
    if actual > profile.max_payload_bytes() {
        return Err(ZbfError::PayloadTooLarge {
            actual,
            maximum: profile.max_payload_bytes(),
        });
    }
    Ok(actual)
}

fn require_depth(depth: u16, profile: DurableProfile) -> Result<(), ZbfError> {
    if depth > profile.max_depth() {
        Err(ZbfError::DepthExceeded {
            actual: depth,
            maximum: profile.max_depth(),
        })
    } else {
        Ok(())
    }
}

fn encode_children(
    children: &[ZbfObject],
    profile: DurableProfile,
    parent_depth: u16,
) -> Result<Vec<u8>, ZbfError> {
    let count = u32::try_from(children.len()).map_err(|_| ZbfError::TooManyChildren {
        actual: u32::MAX,
        maximum: profile.max_children(),
    })?;
    if count > profile.max_children() {
        return Err(ZbfError::TooManyChildren {
            actual: count,
            maximum: profile.max_children(),
        });
    }
    let child_depth = parent_depth
        .checked_add(1)
        .ok_or(ZbfError::DepthExceeded {
            actual: u16::MAX,
            maximum: profile.max_depth(),
        })?;
    require_depth(child_depth, profile)?;
    let mut payload = Vec::new();
    payload.extend_from_slice(&count.to_be_bytes());
    for child in children {
        let bytes = child.to_bytes_at(profile, child_depth)?;
        let len = u64::try_from(bytes.len()).map_err(|_| ZbfError::LengthOverflow)?;
        let next_len = payload
            .len()
            .checked_add(8)
            .and_then(|value| value.checked_add(bytes.len()))
            .ok_or(ZbfError::LengthOverflow)?;
        bounded_payload_len(next_len, profile)?;
        payload.extend_from_slice(&len.to_be_bytes());
        payload.extend_from_slice(&bytes);
    }
    Ok(payload)
}

fn decode_object(
    bytes: &[u8],
    expected_assembly_manifest_digest: Sha256Digest,
    profile: DurableProfile,
    depth: u16,
) -> Result<ZbfObject, ZbfError> {
    require_depth(depth, profile)?;
    let actual_len = u64::try_from(bytes.len()).map_err(|_| ZbfError::LengthOverflow)?;
    if actual_len > profile.max_object_bytes() {
        return Err(ZbfError::ObjectTooLarge {
            actual: actual_len,
            maximum: profile.max_object_bytes(),
        });
    }
    if bytes.len() < ZBF_HEADER_LEN {
        return Err(ZbfError::UnexpectedEof);
    }
    let header = decode_header(&bytes[..ZBF_HEADER_LEN])?;
    validate_header_contract(&header, profile)?;
    if header.assembly_manifest_digest != expected_assembly_manifest_digest {
        return Err(ZbfError::AssemblyMismatch {
            expected: expected_assembly_manifest_digest,
            actual: header.assembly_manifest_digest,
        });
    }
    let payload_len =
        usize::try_from(header.payload_len).map_err(|_| ZbfError::LengthOverflow)?;
    if header.payload_len > profile.max_payload_bytes() {
        return Err(ZbfError::PayloadTooLarge {
            actual: header.payload_len,
            maximum: profile.max_payload_bytes(),
        });
    }
    let expected_len = ZBF_HEADER_LEN
        .checked_add(payload_len)
        .ok_or(ZbfError::LengthOverflow)?;
    if bytes.len() < expected_len {
        return Err(ZbfError::UnexpectedEof);
    }
    if bytes.len() > expected_len {
        return Err(ZbfError::TrailingBytes);
    }
    let payload = &bytes[ZBF_HEADER_LEN..expected_len];
    let actual_digest = Sha256Digest::from_bytes(sha256(payload));
    if actual_digest != header.payload_digest {
        return Err(ZbfError::DigestMismatch {
            expected: header.payload_digest,
            actual: actual_digest,
        });
    }
    let payload = if header.flags == ZBF_CONTAINER_FLAG {
        ZbfPayload::Children(decode_children(
            payload,
            expected_assembly_manifest_digest,
            profile,
            depth,
        )?)
    } else {
        ZbfPayload::Bytes(payload.to_vec())
    };
    Ok(ZbfObject { header, payload })
}

fn decode_children(
    payload: &[u8],
    expected_assembly_manifest_digest: Sha256Digest,
    profile: DurableProfile,
    parent_depth: u16,
) -> Result<Vec<ZbfObject>, ZbfError> {
    let mut cursor = Cursor::new(payload);
    let count = cursor.u32()?;
    if count > profile.max_children() {
        return Err(ZbfError::TooManyChildren {
            actual: count,
            maximum: profile.max_children(),
        });
    }
    let child_depth = parent_depth
        .checked_add(1)
        .ok_or(ZbfError::DepthExceeded {
            actual: u16::MAX,
            maximum: profile.max_depth(),
        })?;
    require_depth(child_depth, profile)?;
    let capacity = usize::try_from(count).map_err(|_| ZbfError::LengthOverflow)?;
    let mut children = Vec::with_capacity(capacity);
    for _ in 0..count {
        let child_len = usize::try_from(cursor.u64()?).map_err(|_| ZbfError::LengthOverflow)?;
        if child_len < ZBF_HEADER_LEN {
            return Err(ZbfError::ContainerMalformed);
        }
        let child_bytes = cursor.take(child_len)?;
        children.push(decode_object(
            child_bytes,
            expected_assembly_manifest_digest,
            profile,
            child_depth,
        )?);
    }
    if cursor.remaining() != 0 {
        return Err(ZbfError::TrailingBytes);
    }
    Ok(children)
}

fn validate_header_contract(
    header: &ZbfHeader,
    profile: DurableProfile,
) -> Result<(), ZbfError> {
    if header.schema_major != ZBF_SCHEMA_MAJOR || header.schema_minor != ZBF_SCHEMA_MINOR {
        return Err(ZbfError::UnsupportedVersion {
            major: header.schema_major,
            minor: header.schema_minor,
        });
    }
    if header.flags & !ZBF_CONTAINER_FLAG != 0 {
        return Err(ZbfError::UnknownFlags(header.flags));
    }
    let expected = profile.digest();
    if header.durable_profile_digest != expected {
        return Err(ZbfError::ProfileMismatch {
            expected,
            actual: header.durable_profile_digest,
        });
    }
    Ok(())
}

fn encode_header(header: &ZbfHeader, out: &mut Vec<u8>) {
    out.extend_from_slice(&ZBF_MAGIC);
    out.extend_from_slice(&header.schema_major.to_be_bytes());
    out.extend_from_slice(&header.schema_minor.to_be_bytes());
    out.extend_from_slice(&(header.kind as u16).to_be_bytes());
    out.push(owner_to_u8(header.owner));
    out.push(header.flags);
    out.extend_from_slice(&header.payload_len.to_be_bytes());
    for digest in [
        header.assembly_manifest_digest,
        header.durable_profile_digest,
        header.source_root_digest,
        header.producer_contract_digest,
        header.payload_digest,
    ] {
        out.extend_from_slice(digest.as_bytes());
    }
    out.extend_from_slice(&[0_u8; 8]);
    debug_assert_eq!(out.len(), ZBF_HEADER_LEN);
}

fn decode_header(bytes: &[u8]) -> Result<ZbfHeader, ZbfError> {
    let mut cursor = Cursor::new(bytes);
    if cursor.take(8)? != ZBF_MAGIC {
        return Err(ZbfError::BadMagic);
    }
    let schema_major = cursor.u16()?;
    let schema_minor = cursor.u16()?;
    let raw_kind = cursor.u16()?;
    let kind = ZbfArtifactKind::from_u16(raw_kind).ok_or(ZbfError::InvalidKind(raw_kind))?;
    let raw_owner = cursor.u8()?;
    let owner = owner_from_u8(raw_owner).ok_or(ZbfError::InvalidOwner(raw_owner))?;
    let flags = cursor.u8()?;
    let payload_len = cursor.u64()?;
    let assembly_manifest_digest = cursor.digest()?;
    let durable_profile_digest = cursor.digest()?;
    let source_root_digest = cursor.digest()?;
    let producer_contract_digest = cursor.digest()?;
    let payload_digest = cursor.digest()?;
    if cursor.take(8)?.iter().any(|byte| *byte != 0) {
        return Err(ZbfError::ReservedNonZero);
    }
    if cursor.remaining() != 0 {
        return Err(ZbfError::TrailingBytes);
    }
    Ok(ZbfHeader {
        schema_major,
        schema_minor,
        kind,
        owner,
        flags,
        payload_len,
        assembly_manifest_digest,
        durable_profile_digest,
        source_root_digest,
        producer_contract_digest,
        payload_digest,
    })
}

fn owner_to_u8(owner: ArtifactOwner) -> u8 {
    match owner {
        ArtifactOwner::ZeroStack => 0,
        ArtifactOwner::FsZero => 1,
        ArtifactOwner::GraphZero => 2,
        ArtifactOwner::TokenZero => 3,
        ArtifactOwner::PiZeroStack => 4,
    }
}

fn owner_from_u8(value: u8) -> Option<ArtifactOwner> {
    Some(match value {
        0 => ArtifactOwner::ZeroStack,
        1 => ArtifactOwner::FsZero,
        2 => ArtifactOwner::GraphZero,
        3 => ArtifactOwner::TokenZero,
        4 => ArtifactOwner::PiZeroStack,
        _ => return None,
    })
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.offset)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], ZbfError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ZbfError::LengthOverflow)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(ZbfError::UnexpectedEof)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, ZbfError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ZbfError> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, ZbfError> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn u64(&mut self) -> Result<u64, ZbfError> {
        let bytes = self.take(8)?;
        Ok(u64::from_be_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]))
    }

    fn digest(&mut self) -> Result<Sha256Digest, ZbfError> {
        let bytes = self.take(32)?;
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(bytes);
        Ok(Sha256Digest::from_bytes(digest))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ZbfFailureCode {
    UnexpectedEof,
    BadMagic,
    UnsupportedVersion,
    InvalidKind,
    InvalidOwner,
    UnknownFlags,
    ReservedNonZero,
    LengthOverflow,
    ObjectTooLarge,
    PayloadTooLarge,
    TooManyChildren,
    DepthExceeded,
    TrailingBytes,
    DigestMismatch,
    AssemblyMismatch,
    ProfileMismatch,
    HeaderPayloadMismatch,
    ContainerMalformed,
    StoreFailure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ZbfError {
    UnexpectedEof,
    BadMagic,
    UnsupportedVersion {
        major: u16,
        minor: u16,
    },
    InvalidKind(u16),
    InvalidOwner(u8),
    UnknownFlags(u8),
    ReservedNonZero,
    LengthOverflow,
    ObjectTooLarge {
        actual: u64,
        maximum: u64,
    },
    PayloadTooLarge {
        actual: u64,
        maximum: u64,
    },
    TooManyChildren {
        actual: u32,
        maximum: u32,
    },
    DepthExceeded {
        actual: u16,
        maximum: u16,
    },
    TrailingBytes,
    DigestMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    AssemblyMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    ProfileMismatch {
        expected: Sha256Digest,
        actual: Sha256Digest,
    },
    HeaderPayloadMismatch,
    ContainerMalformed,
    Cas {
        class: &'static str,
        message: String,
    },
}

impl ZbfError {
    pub const fn code(&self) -> ZbfFailureCode {
        match self {
            Self::UnexpectedEof => ZbfFailureCode::UnexpectedEof,
            Self::BadMagic => ZbfFailureCode::BadMagic,
            Self::UnsupportedVersion { .. } => ZbfFailureCode::UnsupportedVersion,
            Self::InvalidKind(_) => ZbfFailureCode::InvalidKind,
            Self::InvalidOwner(_) => ZbfFailureCode::InvalidOwner,
            Self::UnknownFlags(_) => ZbfFailureCode::UnknownFlags,
            Self::ReservedNonZero => ZbfFailureCode::ReservedNonZero,
            Self::LengthOverflow => ZbfFailureCode::LengthOverflow,
            Self::ObjectTooLarge { .. } => ZbfFailureCode::ObjectTooLarge,
            Self::PayloadTooLarge { .. } => ZbfFailureCode::PayloadTooLarge,
            Self::TooManyChildren { .. } => ZbfFailureCode::TooManyChildren,
            Self::DepthExceeded { .. } => ZbfFailureCode::DepthExceeded,
            Self::TrailingBytes => ZbfFailureCode::TrailingBytes,
            Self::DigestMismatch { .. } => ZbfFailureCode::DigestMismatch,
            Self::AssemblyMismatch { .. } => ZbfFailureCode::AssemblyMismatch,
            Self::ProfileMismatch { .. } => ZbfFailureCode::ProfileMismatch,
            Self::HeaderPayloadMismatch => ZbfFailureCode::HeaderPayloadMismatch,
            Self::ContainerMalformed => ZbfFailureCode::ContainerMalformed,
            Self::Cas { .. } => ZbfFailureCode::StoreFailure,
        }
    }
}

impl fmt::Display for ZbfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ZBF failure: {:?}", self.code())
    }
}

impl Error for ZbfError {}

pub fn zbf_contract_manifest() -> serde_json::Value {
    json!({
        "contract": "zerostack.zbf",
        "contract_version": ZBF_CONTRACT_VERSION,
        "schema_major": ZBF_SCHEMA_MAJOR,
        "schema_minor": ZBF_SCHEMA_MINOR,
        "byte_order": "big_endian",
        "header_bytes": ZBF_HEADER_LEN,
        "header_fields": [
            "magic", "schema_major", "schema_minor", "artifact_kind", "owner", "flags",
            "payload_len", "assembly_manifest_digest", "durable_profile_digest",
            "source_root_digest", "producer_contract_digest", "payload_digest", "reserved"
        ],
        "container_payload": "u32 child_count; repeated u64 child_len + complete_zbf_object",
        "profile_domain": "zerostack.zbf_profile\u{0}",
        "required_profiles": ["portable_strict", "apfs_strict", "ext4_xfs_strict", "ntfs_strict"],
        "bounds": {
            "max_object_bytes": ZBF_MAX_OBJECT_BYTES,
            "max_children": ZBF_MAX_CHILDREN,
            "max_depth": ZBF_MAX_DEPTH
        },
        "unknown_required_versions": "reject",
        "unknown_flags": "reject",
        "reserved_bytes": "must_be_zero"
    })
}

pub fn zbf_contract_digest() -> Sha256Digest {
    Sha256Digest::from_bytes(sha256(
        canonical_json(&zbf_contract_manifest()).as_bytes(),
    ))
}

